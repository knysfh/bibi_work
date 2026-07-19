import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import { execFile } from 'node:child_process';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);
const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const FERRISKEY_PASSWORD = process.env.BIWORK_FERRISKEY_PASSWORD;
const FERRISKEY_BASE_URL = process.env.BIWORK_FERRISKEY_BASE_URL ?? 'http://localhost:3333';
const REMOTE_SKILL_URL =
  process.env.BIWORK_GENERIC_SKILL_URL ??
  'https://raw.githubusercontent.com/anthropics/skills/main/skills/mcp-builder/SKILL.md';
const SCREENSHOT_DATE = new Date().toISOString().slice(0, 10);
const SCREENSHOT_DIR = path.resolve(process.cwd(), `../artifacts/playwright/${SCREENSHOT_DATE}`);

type DesktopElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

async function ferrisKeyPasswordToken(): Promise<string> {
  if (!FERRISKEY_PASSWORD) throw new Error('BIWORK_FERRISKEY_PASSWORD is required');
  const response = await fetch(`${FERRISKEY_BASE_URL}/realms/bibi-work/protocol/openid-connect/token`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'password',
      client_id: 'bibi-work-backend',
      username: 'alon',
      password: FERRISKEY_PASSWORD,
    }),
  });
  if (!response.ok) throw new Error(`FerrisKey token request failed: ${response.status}`);
  const body = (await response.json()) as { access_token?: string };
  if (!body.access_token) throw new Error('FerrisKey token response had no access_token');
  return body.access_token;
}

async function desktopApi(
  page: Page,
  token: string,
  apiPath: string,
  method = 'GET',
  body?: unknown
): Promise<unknown> {
  return page.evaluate(
    async ({ accessToken, path: requestPath, requestMethod, requestBody }) => {
      const backendPort = (window as Window & { __backendPort?: number }).__backendPort;
      if (!backendPort) throw new Error('Electron backend port is unavailable');
      const response = await fetch(`http://127.0.0.1:${backendPort}${requestPath}`, {
        method: requestMethod,
        headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
        ...(requestBody === undefined ? {} : { body: JSON.stringify(requestBody) }),
      });
      if (!response.ok) throw new Error(`${requestMethod} ${requestPath} failed: ${response.status}`);
      return response.json();
    },
    { accessToken: token, path: apiPath, requestMethod: method, requestBody: body }
  );
}

async function openImportDialog(page: Page): Promise<void> {
  await page.getByTestId('btn-add-skill').click();
  const manual = page.getByTestId('btn-add-skill-manual');
  await expect(manual).toBeVisible();
  await manual.click();
  await expect(page.getByTestId('skill-import-dialog')).toBeVisible();
  await expect(manual).toBeHidden();
  await page.waitForTimeout(300);
}

test('imports an uploaded ZIP and a generic HTTPS SKILL.md through the real desktop', async () => {
  test.setTimeout(180_000);
  test.skip(!FERRISKEY_PASSWORD, 'BIWORK_FERRISKEY_PASSWORD is required');
  let browser: Browser | null = null;
  let page: Page | null = null;
  let token: string | null = null;
  let fixtureDir: string | null = null;
  const uploadName = `playwright-upload-${Date.now()}`;
  const remoteName = 'mcp-builder';

  try {
    browser = await chromium.connectOverCDP(CDP_URL);
    page =
      browser
        .contexts()
        .flatMap((context) => context.pages())
        .find((candidate) => candidate.url().includes('5173')) ?? null;
    if (!page) throw new Error('running Electron renderer page is unavailable');
    await page.evaluate(async () => (window.electronAPI as DesktopElectronApi).setAuthAccessToken(null));
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page).toHaveURL(/#\/login$/);
    await page.locator('#lang-select').selectOption('en-US');
    await expect(page.locator('.login-page__subtitle')).toHaveText('AI Platform for Enterprise');
    await expect(page.getByText('Sign in with FerrisKey')).toHaveCount(0);
    await expect(page.getByText('Use your FerrisKey account to access enterprise workspaces, agents and governed tools.')).toHaveCount(0);
    await expect(page.getByText('Welcome back, please sign in to your account')).toHaveCount(0);
    const loginStyles = await page.evaluate(() => {
      const pageStyle = getComputedStyle(document.querySelector<HTMLElement>('.login-page')!);
      const titleStyle = getComputedStyle(document.querySelector<HTMLElement>('.login-page__title')!);
      const subtitleStyle = getComputedStyle(document.querySelector<HTMLElement>('.login-page__subtitle')!);
      const buttonStyle = getComputedStyle(document.querySelector<HTMLElement>('.login-page__submit')!);
      return {
        pageBackground: pageStyle.backgroundImage,
        buttonBackground: buttonStyle.backgroundImage,
        titleFontSize: titleStyle.fontSize,
        titleFontWeight: titleStyle.fontWeight,
        subtitleFontSize: subtitleStyle.fontSize,
        subtitleFontWeight: subtitleStyle.fontWeight,
        buttonFontSize: buttonStyle.fontSize,
        buttonFontWeight: buttonStyle.fontWeight,
      };
    });
    expect(loginStyles.pageBackground).toContain('rgb(255, 255, 255)');
    expect(loginStyles.pageBackground).toContain('rgb(219, 234, 254)');
    expect(loginStyles.buttonBackground).toContain('rgb(79, 127, 193)');
    expect(loginStyles.buttonBackground).toContain('rgb(123, 98, 173)');
    expect(loginStyles.titleFontSize).toBe('32px');
    expect(loginStyles.titleFontWeight).toBe('700');
    expect(loginStyles.subtitleFontSize).toBe('16px');
    expect(loginStyles.subtitleFontWeight).toBe('400');
    expect(loginStyles.buttonFontSize).toBe('16px');
    expect(loginStyles.buttonFontWeight).toBe('600');
    await page.waitForTimeout(700);
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '29-login-ferriskey-gradient.png'), fullPage: true });

    token = await ferrisKeyPasswordToken();
    await page.evaluate(
      async (accessToken) => (window.electronAPI as DesktopElectronApi).setAuthAccessToken(accessToken),
      token
    );
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.evaluate(
      async (accessToken) => (window.electronAPI as DesktopElectronApi).setAuthAccessToken(accessToken),
      token
    );
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.evaluate(() => {
      window.location.hash = '#/settings/skills';
    });
    await expect(page.getByRole('heading', { name: 'Skills' })).toBeVisible({ timeout: 20_000 });

    await desktopApi(page, token, `/api/skills/${uploadName}`, 'DELETE').catch(() => undefined);
    await desktopApi(page, token, `/api/skills/${remoteName}`, 'DELETE').catch(() => undefined);

    fixtureDir = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-skill-upload-'));
    const packageDir = path.join(fixtureDir, uploadName);
    await fs.mkdir(path.join(packageDir, 'references'), { recursive: true });
    await fs.writeFile(
      path.join(packageDir, 'SKILL.md'),
      `---\nname: ${uploadName}\ndescription: Playwright uploaded ZIP skill.\n---\n\nUse this fixture only for import testing.\n`
    );
    await fs.writeFile(
      path.join(packageDir, 'references', 'readme.md'),
      'Reference file included in size validation.\n'
    );
    const archivePath = path.join(fixtureDir, `${uploadName}.zip`);
    await execFileAsync('zip', ['-qr', archivePath, uploadName], { cwd: fixtureDir });

    await openImportDialog(page);
    await expect(page.getByTestId('skill-import-zip-button')).toBeVisible();
    const uploadButtonAlignment = await page.getByTestId('skill-import-zip-button').evaluate((button) => {
      const style = getComputedStyle(button);
      return {
        alignItems: style.alignItems,
        height: button.getBoundingClientRect().height,
        lineHeight: Number.parseFloat(style.lineHeight),
      };
    });
    expect(uploadButtonAlignment.alignItems).toBe('center');
    expect(uploadButtonAlignment.lineHeight).toBeLessThan(uploadButtonAlignment.height);
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '26-skill-import-dialog.png'), fullPage: true });
    await page.getByTestId('skill-import-zip-upload').setInputFiles(archivePath);
    await expect(page.getByText(uploadName, { exact: true })).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId('skill-import-dialog')).toBeHidden();
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '27-skill-upload-zip.png'), fullPage: true });

    await openImportDialog(page);
    await page.getByTestId('skill-import-url').fill(REMOTE_SKILL_URL);
    await page.getByTestId('skill-import-url-submit').click();
    await expect(page.getByText(remoteName, { exact: true })).toBeVisible({ timeout: 60_000 });
    await expect(page.getByTestId('skill-import-dialog')).toBeHidden();
    await page.waitForTimeout(300);
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '28-skill-generic-https.png'), fullPage: true });

    const layout = await page.evaluate(() => ({
      viewportWidth: document.documentElement.clientWidth,
      documentWidth: document.documentElement.scrollWidth,
      bodyWidth: document.body.scrollWidth,
    }));
    expect(layout.documentWidth).toBeLessThanOrEqual(layout.viewportWidth + 2);
    expect(layout.bodyWidth).toBeLessThanOrEqual(layout.viewportWidth + 2);

    await expect(page.getByTestId('desktop-logout')).toBeVisible();
    await expect(page.getByTestId('desktop-logout')).toHaveAccessibleName('Log out');
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '30-desktop-logout.png'), fullPage: true });
    await desktopApi(page, token, `/api/skills/${uploadName}`, 'DELETE');
    await desktopApi(page, token, `/api/skills/${remoteName}`, 'DELETE');
    await page.getByTestId('desktop-logout').click();
    await expect(page).toHaveURL(/#\/login$/);
    await expect
      .poll(() => page!.evaluate(() => (window.electronAPI as DesktopElectronApi).getAuthAccessToken()))
      .toBeNull();
  } finally {
    if (page && token) {
      await desktopApi(page, token, `/api/skills/${uploadName}`, 'DELETE').catch(() => undefined);
      await desktopApi(page, token, `/api/skills/${remoteName}`, 'DELETE').catch(() => undefined);
    }
    if (fixtureDir) await fs.rm(fixtureDir, { recursive: true, force: true });
    await browser?.close().catch(() => undefined);
  }
});
