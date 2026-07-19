import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import path from 'node:path';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';
const SCREENSHOT_DIR = path.resolve('../artifacts/playwright/2026-07-12');

type ElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

async function api<T>(token: string, apiPath: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    ...init,
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init?.headers },
  });
  if (!response.ok) throw new Error(`${init?.method ?? 'GET'} ${apiPath}: ${response.status} ${await response.text()}`);
  const body = (await response.json()) as T | { data: T };
  return body && typeof body === 'object' && 'data' in body ? body.data : body;
}

async function passwordGrant(): Promise<string | null> {
  const password = process.env.BIWORK_FERRISKEY_PASSWORD;
  if (!password) return null;
  const response = await fetch('http://localhost:3333/realms/bibi-work/protocol/openid-connect/token', {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'password',
      client_id: 'bibi-work-backend',
      username: process.env.BIWORK_FERRISKEY_USERNAME ?? 'alon',
      password,
    }),
  });
  if (!response.ok) throw new Error(`FerrisKey password grant failed: ${response.status}`);
  return ((await response.json()) as { access_token?: string }).access_token ?? null;
}

async function connectAuthenticatedDesktop(): Promise<{
  browser: Browser;
  page: Page;
  token: string;
  tenantId: string;
}> {
  const browser = await chromium.connectOverCDP(CDP_URL);
  const page = browser
    .contexts()
    .flatMap((context) => context.pages())
    .find((candidate) => candidate.url().includes('5173'));
  if (!page) throw new Error('running BiWork renderer was not found');
  const token =
    (await page.evaluate(() => (window.electronAPI as ElectronApi).getAuthAccessToken())) ?? (await passwordGrant());
  if (!token) throw new Error('real FerrisKey token is unavailable');
  await page.evaluate((value) => (window.electronAPI as ElectronApi).setAuthAccessToken(value), token);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page).toHaveURL(/#\/guid$/, { timeout: 20_000 });
  await page
    .getByText('Conversation not found or has been deleted')
    .waitFor({ state: 'hidden', timeout: 10_000 })
    .catch(() => undefined);
  const me = await api<{ tenant_id: string }>(token, '/api/me');
  return { browser, page, token, tenantId: me.tenant_id };
}

async function createAgent(token: string): Promise<{ id: string }> {
  return api(token, '/api/agents/custom', {
    method: 'POST',
    body: JSON.stringify({
      name: `Live ACP Governance ${Date.now()}`,
      command: 'node',
      args: [path.resolve('tests/fixtures/acp-echo-agent.mjs')],
      env: [],
      advanced: { description: 'ACP permission and cancellation fixture' },
    }),
  });
}

async function createConversation(token: string, agentId: string, title: string): Promise<{ id: string }> {
  return api(token, '/api/conversations', {
    method: 'POST',
    body: JSON.stringify({
      name: title,
      type: 'acp',
      assistant: { id: agentId, name: title, agent_type: 'acp', agent_source: 'custom' },
    }),
  });
}

test('routes ACP permission through the Rust approval UI and returns the selected option', async () => {
  test.setTimeout(120_000);
  const { browser, page, token, tenantId } = await connectAuthenticatedDesktop();
  const agent = await createAgent(token);
  let conversationId: string | null = null;
  try {
    const conversation = await createConversation(token, agent.id, 'Live ACP permission governance');
    conversationId = conversation.id;
    await page.goto(`${page.url().split('#')[0]}#/conversation/${conversation.id}`);
    const sent = await api<{ turn_id: string }>(token, `/api/conversations/${conversation.id}/messages`, {
      method: 'POST',
      body: JSON.stringify({ content: 'request-permission', loading_id: `acp-permission-${Date.now()}` }),
    });
    const card = page.getByTestId('message-permission-card');
    await expect(card).toBeVisible({ timeout: 30_000 });
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, 'desktop-acp-permission-requested.png'), fullPage: true });
    await page.getByTestId('message-permission-option-proceed_once').click();
    await page.getByTestId('message-permission-confirm').click();
    await expect(card).toBeHidden({ timeout: 30_000 });
    await expect
      .poll(
        async () => {
          const runs = await api<Array<{ id: string; status: string }>>(
            token,
            `/api/v1/runs?tenant_id=${tenantId}&conversation_id=${conversation.id}`
          );
          return runs.find((run) => run.id === sent.turn_id)?.status;
        },
        { timeout: 30_000 }
      )
      .toBe('completed');
    await expect(page.getByText('permission:allow-fixture')).toBeVisible({ timeout: 20_000 });
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, 'desktop-acp-permission-approved.png'), fullPage: true });
  } finally {
    if (conversationId)
      await api(token, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    await api(token, `/api/agents/custom/${agent.id}`, { method: 'DELETE' }).catch(() => undefined);
    await browser.close();
  }
});

test('cancels a running device-local ACP request without emitting a completed answer', async () => {
  test.setTimeout(120_000);
  const { browser, page, token, tenantId } = await connectAuthenticatedDesktop();
  const agent = await createAgent(token);
  let conversationId: string | null = null;
  try {
    const conversation = await createConversation(token, agent.id, 'Live ACP cancellation governance');
    conversationId = conversation.id;
    await page.goto(`${page.url().split('#')[0]}#/conversation/${conversation.id}`);
    const sent = await api<{ turn_id: string }>(token, `/api/conversations/${conversation.id}/messages`, {
      method: 'POST',
      body: JSON.stringify({ content: 'wait-for-cancel', loading_id: `acp-cancel-${Date.now()}` }),
    });
    await expect(page.locator('.sendbox-stop-button')).toBeVisible({ timeout: 20_000 });
    await page.locator('.sendbox-stop-button').click();
    await expect
      .poll(
        async () => {
          const runs = await api<Array<{ id: string; status: string }>>(
            token,
            `/api/v1/runs?tenant_id=${tenantId}&conversation_id=${conversation.id}`
          );
          return runs.find((run) => run.id === sent.turn_id)?.status;
        },
        { timeout: 20_000 }
      )
      .toBe('cancelled');
    await expect(page.getByText('permission:allow-fixture')).toHaveCount(0);
    await page
      .getByText('Conversation not found or has been deleted')
      .waitFor({ state: 'hidden', timeout: 10_000 })
      .catch(() => undefined);
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, 'desktop-acp-cancelled.png'), fullPage: true });
  } finally {
    if (conversationId)
      await api(token, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    await api(token, `/api/agents/custom/${agent.id}`, { method: 'DELETE' }).catch(() => undefined);
    await browser.close();
  }
});

test('returns the governed reject option to the ACP agent without failing the whole turn', async () => {
  test.setTimeout(120_000);
  const { browser, page, token, tenantId } = await connectAuthenticatedDesktop();
  const agent = await createAgent(token);
  let conversationId: string | null = null;
  try {
    const conversation = await createConversation(token, agent.id, 'Live ACP permission rejection');
    conversationId = conversation.id;
    await page.goto(`${page.url().split('#')[0]}#/conversation/${conversation.id}`);
    const sent = await api<{ turn_id: string }>(token, `/api/conversations/${conversation.id}/messages`, {
      method: 'POST',
      body: JSON.stringify({ content: 'request-permission', loading_id: `acp-reject-${Date.now()}` }),
    });
    const card = page.getByTestId('message-permission-card');
    await expect(card).toBeVisible({ timeout: 30_000 });
    await page.getByTestId('message-permission-option-cancel').click();
    await page.getByTestId('message-permission-confirm').click();
    await expect(card).toBeHidden({ timeout: 30_000 });
    await expect
      .poll(
        async () => {
          const runs = await api<Array<{ id: string; status: string }>>(
            token,
            `/api/v1/runs?tenant_id=${tenantId}&conversation_id=${conversation.id}`
          );
          return runs.find((run) => run.id === sent.turn_id)?.status;
        },
        { timeout: 30_000 }
      )
      .toBe('completed');
    const rejectedAnswer = page.getByText('permission:reject-fixture', { exact: true });
    await expect(rejectedAnswer).toHaveCount(1, { timeout: 20_000 });
    await expect(rejectedAnswer).toBeVisible();
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, 'desktop-acp-permission-rejected.png'), fullPage: true });
  } finally {
    if (conversationId)
      await api(token, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    await api(token, `/api/agents/custom/${agent.id}`, { method: 'DELETE' }).catch(() => undefined);
    await browser.close();
  }
});
