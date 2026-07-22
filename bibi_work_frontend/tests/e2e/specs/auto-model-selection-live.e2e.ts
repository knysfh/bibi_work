import { chromium, expect, test, type Page } from '@playwright/test';
import fs from 'fs/promises';
import path from 'path';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const FERRISKEY_USERNAME = process.env.BIWORK_FERRISKEY_USERNAME;
const FERRISKEY_PASSWORD = process.env.BIWORK_FERRISKEY_PASSWORD;
const FERRISKEY_BASE_URL = process.env.BIWORK_FERRISKEY_BASE_URL ?? process.env.FERRISKEY_BASE_URL;
const ASSISTANT_NAME = process.env.BIWORK_AUTO_MODEL_ASSISTANT_NAME;
const MODEL_LABEL = process.env.BIWORK_AUTO_MODEL_LABEL;
const SCREENSHOT_PATH =
  process.env.BIWORK_AUTO_MODEL_SCREENSHOT ??
  path.resolve(process.cwd(), '../artifacts/playwright/auto-model-selection-live.png');

type DesktopElectronApi = {
  setAuthAccessToken: (accessToken: string | null) => Promise<void>;
};

async function connectDesktopPage(): Promise<Page> {
  const browser = await chromium.connectOverCDP(CDP_URL);
  const rendererPages = browser
    .contexts()
    .flatMap((context) => context.pages())
    .filter((candidate) => {
      const url = candidate.url();
      return url.startsWith('http://localhost:5173') || url.includes('/out/renderer/index.html');
    });
  const page = rendererPages.find((candidate) => !candidate.url().endsWith('#/login')) ?? rendererPages.at(-1);
  if (!page) throw new Error(`No BiWork renderer page found through ${CDP_URL}`);
  await page.bringToFront();
  return page;
}

async function loginIfNeeded(page: Page): Promise<void> {
  if (!page.url().endsWith('#/login')) return;
  const response = await fetch(`${FERRISKEY_BASE_URL}/realms/bibi-work/protocol/openid-connect/token`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'password',
      client_id: 'bibi-work-backend',
      username: FERRISKEY_USERNAME!,
      password: FERRISKEY_PASSWORD!,
    }),
  });
  if (!response.ok) throw new Error(`FerrisKey password grant failed: HTTP ${response.status}`);
  const payload = (await response.json()) as { access_token?: string };
  if (!payload.access_token) throw new Error('FerrisKey password grant returned no access token');
  await page.evaluate(async (accessToken) => {
    const electronApi = window.electronAPI as DesktopElectronApi;
    await electronApi.setAuthAccessToken(accessToken);
  }, payload.access_token);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page).toHaveURL(/#\/guid$/, { timeout: 30_000 });
}

function requireLiveConfiguration(): void {
  test.skip(
    !FERRISKEY_BASE_URL || !FERRISKEY_USERNAME || !FERRISKEY_PASSWORD,
    'FerrisKey live configuration is required'
  );
  test.skip(!ASSISTANT_NAME || !MODEL_LABEL, 'auto assistant and model labels are required');
}

async function selectAssistant(page: Page, assistantName: string): Promise<void> {
  const visiblePill = page.locator('[data-testid^="preset-pill-"]').filter({ hasText: assistantName }).first();
  await expect(visiblePill).toBeVisible({ timeout: 30_000 });
  await visiblePill.evaluate((element: HTMLElement) => element.click());
}

test('auto assistant uses the model selected in New Chat', async () => {
  test.setTimeout(240_000);
  requireLiveConfiguration();

  const page = await connectDesktopPage();
  console.log(`auto-model-e2e: connected desktop ${page.url()}`);
  await loginIfNeeded(page);
  console.log('auto-model-e2e: authenticated');
  try {
    if (!page.url().endsWith('#/guid')) {
      await page.evaluate(() => {
        window.location.hash = '#/guid';
      });
    }
    await expect(page.getByText('New Chat', { exact: true })).toBeVisible({ timeout: 30_000 });
    console.log('auto-model-e2e: guid visible');
    await selectAssistant(page, ASSISTANT_NAME!);
    console.log('auto-model-e2e: assistant selected');

    const modelButton = page.locator('button.sendbox-model-btn.guid-config-btn');
    await expect(modelButton).toBeVisible();
    await modelButton.click({ force: true });
    console.log('auto-model-e2e: model menu opened');
    const modelOption = page.getByText(MODEL_LABEL!, { exact: true }).last();
    await expect(modelOption).toBeVisible();
    await modelOption.click();
    await expect(modelButton).toContainText(MODEL_LABEL!);
    console.log('auto-model-e2e: model selected');

    await page.getByTestId('guid-input').fill('你好，请只回复：模型选择正常');
    const sendButton = page.getByTestId('guid-send-btn');
    await expect(sendButton).toBeEnabled();
    await sendButton.click();
    console.log('auto-model-e2e: message sent');
    await expect(page).toHaveURL(/#\/conversation\//, { timeout: 30_000 });
    const conversationMain = page.locator('main.layout-content');
    await expect(conversationMain).toBeVisible();
    await expect
      .poll(
        () =>
          conversationMain
            .locator('.markdown-shadow')
            .evaluateAll((hosts) =>
              hosts.some(
                (host) =>
                  host.shadowRoot?.querySelector('.markdown-shadow-body')?.textContent?.trim() === '模型选择正常'
              )
            ),
        { timeout: 180_000 }
      )
      .toBe(true);
    await expect(page.locator('body')).not.toContainText('model_profile_id is not active');

    await fs.mkdir(path.dirname(SCREENSHOT_PATH), { recursive: true });
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true });
  } finally {
    // The CDP browser is shared with the running Electron process and must not be closed here.
  }
});
