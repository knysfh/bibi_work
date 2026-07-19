import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import path from 'node:path';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';
const SCREENSHOT_PATH = path.resolve('../artifacts/playwright/2026-07-12/desktop-team-run-completed.png');

type ElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

type TeamRunDetail = {
  team_run: { id: string; status: string };
  members: Array<{ role: string; status: string; run_id?: string }>;
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
  await page.goto(`${page.url().split('#')[0]}#/guid`);
  await expect(page).toHaveURL(/#\/guid$/, { timeout: 20_000 });
  const me = await api<{ tenant_id: string }>(token, '/api/me');
  return { browser, page, token, tenantId: me.tenant_id };
}

test('runs a real two-member Team through Rust and renders both terminal members', async () => {
  test.setTimeout(240_000);
  test.skip(!process.env.BIWORK_FERRISKEY_PASSWORD, 'BIWORK_FERRISKEY_PASSWORD is required');
  const { browser, page, token, tenantId } = await connectAuthenticatedDesktop();
  let teamId: string | null = null;
  try {
    const assistants = await api<Array<{ id: string; name: string }>>(token, '/api/assistants');
    const smokeAgent = assistants.find((assistant) => assistant.name === 'LLM provider smoke agent');
    if (!smokeAgent) throw new Error('LLM provider smoke agent is unavailable');

    const team = await api<{ id: string; assistants: Array<{ slot_id: string; role: string }> }>(token, '/api/teams', {
      method: 'POST',
      body: JSON.stringify({
        name: `Live Team Run ${Date.now()}`,
        workspace: '',
        workspace_mode: 'shared',
        assistants: [
          {
            assistant_id: smokeAgent.id,
            assistant_name: 'Leader',
            role: 'leader',
            model: 'minimax-m2.5',
          },
          {
            assistant_id: smokeAgent.id,
            assistant_name: 'Reviewer',
            role: 'teammate',
            model: 'minimax-m2.5',
          },
        ],
      }),
    });
    teamId = team.id;
    const leader = team.assistants.find((assistant) => assistant.role === 'leader');
    const reviewer = team.assistants.find((assistant) => assistant.role === 'teammate');
    expect(leader?.slot_id).toBeTruthy();
    expect(reviewer?.slot_id).toBeTruthy();
    await page.goto(`${page.url().split('#')[0]}#/team/${team.id}`);
    await expect(page.getByText('Leader', { exact: true }).first()).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText('Reviewer', { exact: true }).first()).toBeVisible({ timeout: 30_000 });

    const prompt = 'Reply exactly TEAM_MULTI_OK and do not call tools.';
    const messageResponse = page.waitForResponse(
      (response) => response.url().endsWith(`/api/teams/${team.id}/messages`) && response.request().method() === 'POST'
    );
    const input = page.locator('textarea').first();
    await input.fill(prompt);
    await input.press('Enter');
    const response = await messageResponse;
    expect(response.ok()).toBe(true);
    const responseBody = (await response.json()) as {
      data?: { team_run_id: string };
      team_run_id?: string;
    };
    const teamRunId = responseBody.data?.team_run_id ?? responseBody.team_run_id;
    expect(teamRunId).toBeTruthy();

    await expect
      .poll(
        async () => {
          const detail = await api<TeamRunDetail>(
            token,
            `/api/v1/agent-teams/${team.id}/runs/${teamRunId}?tenant_id=${tenantId}`
          );
          return {
            status: detail.team_run.status,
            memberStatuses: detail.members.map((member) => member.status),
            childRunCount: detail.members.filter((member) => member.run_id).length,
          };
        },
        { timeout: 180_000, intervals: [1_000, 2_000, 5_000] }
      )
      .toEqual({ status: 'completed', memberStatuses: ['completed', 'completed'], childRunCount: 2 });

    await expect(page.getByText('Completed', { exact: true })).toHaveCount(2, { timeout: 30_000 });
    await expect(
      page.getByTestId(`team-chat-slot-${leader!.slot_id}`).getByText('TEAM_MULTI_OK', { exact: true })
    ).toHaveCount(1, { timeout: 30_000 });
    await expect(
      page.getByTestId(`team-chat-slot-${reviewer!.slot_id}`).getByText('TEAM_MULTI_OK', { exact: true })
    ).toHaveCount(1, { timeout: 30_000 });
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true });
  } finally {
    if (teamId) await api(token, `/api/teams/${teamId}`, { method: 'DELETE' }).catch(() => undefined);
    await browser.close();
  }
});
