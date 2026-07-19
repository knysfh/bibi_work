import { chromium, expect, test } from '@playwright/test';
import path from 'node:path';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';
const OTLP_FIXTURE_URL = process.env.BIWORK_OTLP_FIXTURE_URL;

type OtlpTraceSummary = {
  trace_id: string;
  service_names: string[];
  span_names: string[];
  rust_binary_match: boolean;
};

type ElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

async function ferrisKeyToken(): Promise<string | null> {
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

async function api<T>(token: string, apiPath: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    ...init,
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', ...init?.headers },
  });
  if (!response.ok) throw new Error(`${init?.method ?? 'GET'} ${apiPath}: ${response.status} ${await response.text()}`);
  const body = (await response.json()) as T | { data: T };
  return body && typeof body === 'object' && 'data' in body ? body.data : body;
}

test('runs a custom biwork_cli agent through the authenticated desktop ACP worker', async () => {
  test.setTimeout(120_000);
  const browser = await chromium.connectOverCDP(CDP_URL);
  const page = browser
    .contexts()
    .flatMap((context) => context.pages())
    .find((candidate) => candidate.url().includes('5173'));
  if (!page) throw new Error('running BiWork renderer was not found');
  const token =
    (await page.evaluate(() => (window.electronAPI as ElectronApi).getAuthAccessToken())) ?? (await ferrisKeyToken());
  test.skip(!token, 'the running desktop must have a real FerrisKey session');
  await page.evaluate((accessToken) => (window.electronAPI as ElectronApi).setAuthAccessToken(accessToken), token);
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page).toHaveURL(/#\/guid$/, { timeout: 20_000 });

  const fixture = path.resolve('tests/fixtures/acp-echo-agent.mjs');
  const agent = await api<{ id: string }>(token!, '/api/agents/custom', {
    method: 'POST',
    body: JSON.stringify({
      name: `Live ACP Echo ${Date.now()}`,
      command: 'node',
      args: [fixture],
      env: [],
      advanced: { description: 'Real desktop ACP execution fixture' },
    }),
  });
  let conversationId: string | null = null;
  try {
    const conversation = await api<{ id: string }>(token!, '/api/conversations', {
      method: 'POST',
      body: JSON.stringify({
        name: 'Live desktop ACP verification',
        type: 'acp',
        assistant: { id: agent.id, name: 'Live ACP Echo', agent_type: 'acp', agent_source: 'custom' },
      }),
    });
    conversationId = conversation.id;
    const sent = await api<{ turn_id: string }>(token!, `/api/conversations/${conversation.id}/messages`, {
      method: 'POST',
      body: JSON.stringify({ content: 'hello from live desktop ACP', loading_id: `desktop-acp-${Date.now()}` }),
    });
    await expect
      .poll(
        async () => {
          const runs = await api<Array<{ id: string; status: string }>>(
            token!,
            `/api/v1/runs?tenant_id=${encodeURIComponent((await api<{ tenant_id: string }>(token!, '/api/me')).tenant_id)}&conversation_id=${conversation.id}`
          );
          return runs.find((run) => run.id === sent.turn_id)?.status;
        },
        { timeout: 60_000 }
      )
      .toBe('completed');
    const messages = await api<Array<{ role: string; content: unknown }>>(
      token!,
      `/api/conversations/${conversation.id}/messages`
    );
    expect(JSON.stringify(messages)).toContain('echo:hello from live desktop ACP');

    if (OTLP_FIXTURE_URL) {
      await expect
        .poll(
          async () => {
            const response = await fetch(`${OTLP_FIXTURE_URL}/summary`);
            if (!response.ok) return null;
            const body = (await response.json()) as { traces?: OtlpTraceSummary[] };
            return Boolean(
              body.traces?.find(
                (trace) =>
                  trace.rust_binary_match &&
                  trace.service_names.includes('bibi-work-desktop') &&
                  trace.span_names.includes('acp.run') &&
                  trace.span_names.includes('acp.prompt')
              )
            );
          },
          { timeout: 30_000 }
        )
        .toBe(true);
    }

    await page.goto(`${page.url().split('#')[0]}#/conversation/${conversation.id}`);
    await expect(page.getByText('echo:hello from live desktop ACP')).toBeVisible({ timeout: 20_000 });
    await page.screenshot({ path: '../artifacts/playwright/2026-07-12/desktop-acp-live.png', fullPage: true });
  } finally {
    if (conversationId) {
      await api(token!, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    }
    await api(token!, `/api/agents/custom/${agent.id}`, { method: 'DELETE' }).catch(() => undefined);
    await browser.close();
  }
});
