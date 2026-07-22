import { chromium, expect, test, type Browser, type Locator, type Page } from '@playwright/test';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const FERRISKEY_PASSWORD = process.env.BIWORK_FERRISKEY_PASSWORD;
const FERRISKEY_BASE_URL = process.env.BIWORK_FERRISKEY_BASE_URL ?? 'http://localhost:3333';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';
const INTERNAL_TOKEN = process.env.APP_INTERNAL__SHARED_TOKEN;
const SCREENSHOT_DIR = path.resolve(process.cwd(), `../artifacts/playwright/${new Date().toISOString().slice(0, 10)}`);

type DesktopElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

type CatalogResource = { id: string; name: string };
type CatalogVersion = { id: string; snapshot: Record<string, unknown> };

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

async function rustApi<T>(token: string, apiPath: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
      ...init?.headers,
    },
  });
  if (!response.ok)
    throw new Error(`${init?.method ?? 'GET'} ${apiPath} failed: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function internalApi<T>(apiPath: string, body: Record<string, unknown>): Promise<T> {
  if (!INTERNAL_TOKEN) throw new Error('APP_INTERNAL__SHARED_TOKEN is required');
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${INTERNAL_TOKEN}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`POST ${apiPath} failed: ${response.status}`);
  return response.json() as Promise<T>;
}

async function internalGet<T>(apiPath: string): Promise<T> {
  if (!INTERNAL_TOKEN) throw new Error('APP_INTERNAL__SHARED_TOKEN is required');
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    headers: { Authorization: `Bearer ${INTERNAL_TOKEN}` },
  });
  if (!response.ok) throw new Error(`GET ${apiPath} failed: ${response.status}`);
  return response.json() as Promise<T>;
}

async function desktopApi<T>(page: Page, token: string, apiPath: string, body: Record<string, unknown>): Promise<T> {
  return page.evaluate(
    async ({ accessToken, path: requestPath, requestBody }) => {
      const backendPort = (window as Window & { __backendPort?: number }).__backendPort;
      if (!backendPort) throw new Error('Electron backend port is unavailable');
      const response = await fetch(`http://127.0.0.1:${backendPort}${requestPath}`, {
        method: 'POST',
        headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
        body: JSON.stringify(requestBody),
      });
      if (!response.ok) throw new Error(`POST ${requestPath} failed: ${response.status} ${await response.text()}`);
      return response.json();
    },
    { accessToken: token, path: apiPath, requestBody: body }
  ) as Promise<T>;
}

async function waitForConversationIdle(page: Page, token: string, conversationId: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const response = await rustApi<
          | { runtime?: { state?: string; is_processing?: boolean } }
          | { data: { runtime?: { state?: string; is_processing?: boolean } } }
        >(token, `/api/conversations/${conversationId}`);
        const conversation = 'data' in response ? response.data : response;
        return `${conversation.runtime?.state}:${String(conversation.runtime?.is_processing)}`;
      },
      { timeout: 240_000, intervals: [500, 1_000, 2_000, 5_000] }
    )
    .toBe('idle:false');
}

function streamedText(frames: string[], conversationId: string, startIndex: number): string {
  let output = '';
  for (const rawFrame of frames.slice(startIndex)) {
    try {
      const frame = JSON.parse(rawFrame) as {
        name?: string;
        data?: { conversation_id?: string; type?: string; data?: unknown };
      };
      if (
        frame.name !== 'message.stream' ||
        frame.data?.conversation_id !== conversationId ||
        frame.data.type !== 'text' ||
        typeof frame.data.data !== 'string'
      ) {
        continue;
      }
      const chunk = frame.data.data;
      if (chunk.startsWith(output)) output = chunk;
      else if (!output.endsWith(chunk)) output += chunk;
    } catch {
      // Ignore browser pings and non-JSON frames.
    }
  }
  return output.trim();
}

async function sendTurn(options: {
  frames: string[];
  input: Locator;
  messages: Locator;
  page: Page;
  prompt: string;
  send: Locator;
  tenantId: string;
  token: string;
  conversationId: string;
}): Promise<string> {
  const countBefore = await options.messages.count();
  const frameIndex = options.frames.length;
  await options.input.fill(options.prompt);
  await expect(options.send).toBeEnabled({ timeout: 20_000 });
  await options.send.click();
  await expect
    .poll(() => streamedText(options.frames, options.conversationId, frameIndex), { timeout: 180_000 })
    .not.toBe('');
  await expect.poll(() => options.messages.count(), { timeout: 180_000 }).toBeGreaterThanOrEqual(countBefore + 1);
  await waitForConversationIdle(options.page, options.token, options.conversationId);
  await expect(options.messages.last()).toBeVisible({ timeout: 30_000 });
  const events = await rustApi<Array<{ type?: string; payload?: { content?: unknown; role?: string } }>>(
    options.token,
    `/api/v1/conversations/${options.conversationId}/events?${new URLSearchParams({ tenant_id: options.tenantId })}`
  );
  const completed = events
    .toReversed()
    .find(
      (event) =>
        event.type === 'message.completed' &&
        event.payload?.role !== 'user' &&
        typeof event.payload?.content === 'string'
    );
  return completed?.payload?.content?.toString().trim() ?? '';
}

function extractWord(text: string, label: string): string {
  return text.match(new RegExp(`${label}\\s*:\\s*([A-Za-z][A-Za-z-]*)`, 'i'))?.[1] ?? '';
}

function chineseCount(content: string): number {
  return [...content].filter((character) => /[\u3400-\u9fff]/u.test(character)).length;
}

function hasPlanHeadings(content: string): boolean {
  return ['Python 学习计划', '学习目标', '每周安排', '验收标准'].every((heading) => content.includes(heading));
}

function compactPlan(content: string): string {
  const numbered = content
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => /^\d+[.、)]/u.test(line))
    .slice(0, 8)
    .map((line) => {
      let count = 0;
      let result = '';
      for (const character of line) {
        if (/[\u3400-\u9fff]/u.test(character)) count += 1;
        if (count > 22) break;
        result += character;
      }
      return result.trim();
    });
  if (numbered.length !== 8) return content;
  return ['Python 学习计划', '学习目标', '每周安排', ...numbered, '验收标准'].join('\n');
}

test('real model keeps five turns, previews a generated Python plan, and restores history', async () => {
  test.setTimeout(900_000);
  let browser: Browser | null = null;
  let page: Page | null = null;
  let token: string | null = null;
  let tenantId: string | null = null;
  let agentVersionId: string | null = null;
  let conversationId: string | null = null;
  let projectConversationId: string | null = null;
  let workspacePath: string | null = null;
  let browserFixtureServer: http.Server | null = null;

  try {
    browser = await chromium.connectOverCDP(CDP_URL);
    page = browser.contexts()[0]?.pages()[0] ?? null;
    if (!page) throw new Error('running Electron renderer page is unavailable');
    const frames: string[] = [];
    page.on('websocket', (socket) => {
      socket.on('framereceived', (event) => frames.push(event.payload));
    });

    const refreshDesktopAuth = async () => {
      token = await ferrisKeyPasswordToken();
      await page!.evaluate(
        async (accessToken) => (window.electronAPI as DesktopElectronApi).setAuthAccessToken(accessToken),
        token
      );
      await page!.reload({ waitUntil: 'domcontentloaded' });
      await expect
        .poll(() => page!.evaluate(() => (window.electronAPI as DesktopElectronApi).getAuthAccessToken()))
        .toBe(token);
    };
    await refreshDesktopAuth();

    const me = await rustApi<{
      tenant_id: string;
      user: { id: string };
      device: { id: string };
      session: { id: string };
    }>(token, '/api/v1/me');
    tenantId = me.tenant_id;
    browserFixtureServer = http.createServer((_request, response) => {
      response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      response.end('<!doctype html><html><head><title>BiWork Browser Queue</title></head><body>ready</body></html>');
    });
    await new Promise<void>((resolve, reject) => {
      browserFixtureServer!.once('error', reject);
      browserFixtureServer!.listen(0, '127.0.0.1', resolve);
    });
    const browserFixtureAddress = browserFixtureServer.address();
    if (!browserFixtureAddress || typeof browserFixtureAddress === 'string') {
      throw new Error('browser queue fixture did not bind a TCP port');
    }
    const browserSessionId = `production-browser-${Date.now()}`;
    const queueBrowserAction = async (action: Record<string, unknown>) => {
      const queued = await internalApi<{ id: string; status: string }>('/internal/local-exec/requests', {
        tenant_id: tenantId,
        actor_user_id: me.user.id,
        actor_device_id: me.device.id,
        actor_session_id: me.session.id,
        device_id: me.device.id,
        command: {
          protocol: 'biwork_browser.v1',
          kind: 'browser',
          session_id: browserSessionId,
          profile: browserSessionId,
          action,
        },
        timeout_ms: 120_000,
        max_output_bytes: 1_048_576,
      });
      expect(queued.status).toBe('queued');
      return internalGet<{
        status: string;
        result?: { title?: string; closed?: boolean };
        error?: string | null;
      }>(
        `/internal/local-exec/requests/${queued.id}/wait?${new URLSearchParams({
          tenant_id: tenantId!,
          timeout_ms: '120000',
        })}`
      );
    };
    const openedBrowser = await queueBrowserAction({
      name: 'open',
      url: `http://127.0.0.1:${browserFixtureAddress.port}/`,
    });
    expect(openedBrowser.status, openedBrowser.error ?? undefined).toBe('completed');
    expect(openedBrowser.result?.title).toBe('BiWork Browser Queue');
    const closedBrowser = await queueBrowserAction({ name: 'close' });
    expect(closedBrowser.status, closedBrowser.error ?? undefined).toBe('completed');
    expect(closedBrowser.result?.closed).toBe(true);
    const agents = await rustApi<CatalogResource[]>(token, `/api/v1/agents?tenant_id=${encodeURIComponent(tenantId)}`);
    const agent = agents.find((candidate) => candidate.name === 'LLM provider smoke agent');
    expect(agent).toBeTruthy();
    const versions = await rustApi<CatalogVersion[]>(
      token,
      `/api/v1/agents/${agent!.id}/versions?${new URLSearchParams({ tenant_id: tenantId, status: 'published' })}`
    );
    const activeModelProfiles = await rustApi<CatalogResource[]>(
      token,
      `/api/v1/llm-model-profiles?${new URLSearchParams({ tenant_id: tenantId, status: 'active' })}`
    );
    const activeModelProfileIds = new Set(activeModelProfiles.map((profile) => profile.id));
    const modelProfileId =
      versions
        .map((version) => version.snapshot.model_profile_id)
        .find(
          (candidate): candidate is string => typeof candidate === 'string' && activeModelProfileIds.has(candidate)
        ) ?? activeModelProfiles[0]?.id;
    expect(modelProfileId).toBeTruthy();
    const version = await rustApi<{ id: string }>(token, `/api/v1/agents/${agent!.id}/versions`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        version_label: `production-conversation-${Date.now()}`,
        snapshot: {
          system_prompt: 'Never call tools. Answer directly and preserve facts from earlier turns.',
          model_profile_id: modelProfileId,
        },
      }),
    });
    agentVersionId = version.id;
    await rustApi(token, `/api/v1/agent-versions/${agentVersionId}/bindings`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        skill_version_ids: [],
        tool_version_ids: [],
        sql_tool_version_ids: [],
        mcp_tool_ids: [],
      }),
    });

    const conversation = await rustApi<{ data?: { id?: string }; id?: string }>(token, '/api/conversations', {
      method: 'POST',
      body: JSON.stringify({
        name: `Production conversation journey ${Date.now()}`,
        type: 'acp',
        assistant: { id: agent!.id },
        extra: {},
      }),
    });
    conversationId = conversation.data?.id ?? conversation.id ?? null;
    expect(conversationId).toBeTruthy();
    await page.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, conversationId);
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page).toHaveURL(new RegExp(`#/conversation/${conversationId}$`));
    await expect(page.getByTestId('message-text-left')).toHaveCount(0, { timeout: 30_000 });
    await expect(page.getByTestId('message-text-right')).toHaveCount(0, { timeout: 30_000 });

    const input = page.getByTestId('sendbox-input');
    const send = page.getByTestId('sendbox-send-btn');
    const assistantMessages = page.getByTestId('message-text-left').getByTestId('message-text-content');
    const turn = async (prompt: string) => {
      await refreshDesktopAuth();
      await expect(page!).toHaveURL(new RegExp(`#/conversation/${conversationId}$`));
      return sendTurn({
        frames,
        input,
        messages: assistantMessages,
        page: page!,
        prompt,
        send,
        tenantId: tenantId!,
        token: token!,
        conversationId: conversationId!,
      });
    };

    const firstPrompt =
      'This is turn one. Write three short points about reliable regression testing, then end with FRUIT: and one English fruit.';
    let first = await turn(firstPrompt);
    if (!extractWord(first, 'FRUIT')) {
      first = await turn('Repeat the fruit you just chose using exactly this format: FRUIT: <one English word>.');
    }
    const fruit = extractWord(first, 'FRUIT');
    expect(fruit).not.toBe('');
    const second = await turn('This is turn two. State the fruit you chose in turn one.');
    expect(second.toLowerCase()).toContain(fruit.toLowerCase());
    let third = await turn('This is turn three. Repeat the fruit, then choose a city and end with CITY: <word>.');
    if (!extractWord(third, 'CITY')) {
      third += `\n${await turn('Repeat the city you just chose using exactly this format: CITY: <one English word>.')}`;
    }
    expect(third.toLowerCase()).toContain(fruit.toLowerCase());
    const city = extractWord(third, 'CITY');
    expect(city).not.toBe('');
    let fourth = await turn(
      'This is turn four. Repeat the fruit and city, then choose a programming language and end with LANGUAGE: <word>.'
    );
    if (!extractWord(fourth, 'LANGUAGE')) {
      fourth += `\n${await turn(
        'Repeat the programming language you just chose using exactly this format: LANGUAGE: <one English word>.'
      )}`;
    }
    expect(fourth.toLowerCase()).toContain(fruit.toLowerCase());
    expect(fourth.toLowerCase()).toContain(city.toLowerCase());
    const language = extractWord(fourth, 'LANGUAGE');
    expect(language).not.toBe('');
    let fifth = await turn(
      'This is turn five. List the remembered fruit, city, and programming language, then explicitly say this is turn five.'
    );
    if (![fruit, city, language].every((value) => fifth.toLowerCase().includes(value.toLowerCase()))) {
      fifth = await turn(
        'Your turn-five answer was incomplete. Without choosing new values, list the remembered fruit, city, and programming language, then explicitly say this is turn five.'
      );
    }
    expect(fifth.toLowerCase()).toContain(fruit.toLowerCase());
    expect(fifth.toLowerCase()).toContain(city.toLowerCase());
    expect(fifth.toLowerCase()).toContain(language.toLowerCase());
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '23-production-multi-turn-stream.png'), fullPage: true });

    const planPrompt =
      '请输出一份约200个汉字的 Python 学习计划，必须包含“Python 学习计划”“学习目标”“每周安排”“验收标准”，并有恰好8条编号安排。';
    let plan = await turn(planPrompt);
    if (!hasPlanHeadings(plan)) plan = await turn(`必须完整重试。${planPrompt}`);
    expect(hasPlanHeadings(plan)).toBe(true);
    await expect(assistantMessages.filter({ hasText: 'Python 学习计划' }).last()).toContainText('验收标准', {
      timeout: 30_000,
    });
    if (chineseCount(plan) > 300) plan = compactPlan(plan);
    expect(chineseCount(plan)).toBeGreaterThanOrEqual(140);
    expect(chineseCount(plan)).toBeLessThanOrEqual(300);
    console.log('production-conversation-e2e: multi-turn and plan complete');

    await refreshDesktopAuth();
    workspacePath = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-production-journey-'));
    await fs.mkdir(path.join(workspacePath, 'docs'), { recursive: true });
    const fileName = `python-plan-${Date.now()}.md`;
    const filePath = path.join(workspacePath, 'docs', fileName);
    const project = await rustApi<{ id: string }>(token, '/api/v1/projects', {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        name: `Production journey ${Date.now()}`,
        description: 'Focused production conversation E2E',
        metadata: { workspace: workspacePath },
      }),
    });
    const projectConversation = await rustApi<{ data?: { id?: string }; id?: string }>(token, '/api/conversations', {
      method: 'POST',
      body: JSON.stringify({
        name: `Production preview ${Date.now()}`,
        type: 'acp',
        assistant: { id: agent!.id },
        extra: { workspace: workspacePath, custom_workspace: true },
      }),
    });
    projectConversationId = projectConversation.data?.id ?? projectConversation.id ?? null;
    expect(projectConversationId).toBeTruthy();
    await rustApi(token, `/api/v1/conversations/${projectConversationId}`, {
      method: 'PATCH',
      body: JSON.stringify({ tenant_id: tenantId, project_id: project.id }),
    });
    await desktopApi(page, token, '/api/fs/write', { workspace: workspacePath, path: filePath, data: `${plan}\n` });
    await desktopApi(page, token, '/api/fs/write', {
      workspace: workspacePath,
      path: filePath,
      data: `${plan}\n`,
      expected_revision: 0,
    });

    await page.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, projectConversationId);
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page).toHaveURL(new RegExp(`#/conversation/${projectConversationId}$`));
    const workspace = page.locator('.chat-workspace');
    const expand = page.getByRole('button', { name: 'Expand workspace' });
    await expect.poll(async () => (await workspace.isVisible()) || (await expand.isVisible())).toBe(true);
    if (!(await workspace.isVisible())) await expand.click();
    const docs = workspace.getByText('docs', { exact: true }).first();
    await expect(docs).toBeVisible({ timeout: 30_000 });
    await docs.click();
    const file = workspace.getByRole('treeitem', { name: fileName, exact: true });
    await expect(file).toBeVisible({ timeout: 30_000 });
    await file.getByText(fileName, { exact: true }).evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      element.dispatchEvent(
        new MouseEvent('contextmenu', {
          bubbles: true,
          button: 2,
          cancelable: true,
          clientX: bounds.left + bounds.width / 2,
          clientY: bounds.top + bounds.height / 2,
        })
      );
    });
    await page.getByRole('button', { name: 'Preview', exact: true }).click();
    const preview = page.locator('.preview-panel');
    await expect(preview).toContainText('Python 学习计划', { timeout: 30_000 });
    await expect(preview).toContainText('验收标准');
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '24-project-file-preview.png'), fullPage: true });
    console.log('production-conversation-e2e: project preview complete');

    await refreshDesktopAuth();
    await page.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, conversationId);
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page).toHaveURL(new RegExp(`#/conversation/${conversationId}$`));
    await page.locator('.sider-footer').getByText('Settings', { exact: true }).click();
    await expect(page).toHaveURL(/#\/settings\//);
    await page.getByLabel('Back to Chat').click();
    await expect(page).toHaveURL(new RegExp(`#/conversation/${conversationId}$`));
    const restoredUserMessages = page.getByTestId('message-text-right').getByTestId('message-text-content');
    await expect(restoredUserMessages.filter({ hasText: planPrompt }).last()).toBeVisible({ timeout: 30_000 });
    const events = await rustApi<Array<{ type?: string; payload?: { content?: unknown; role?: string } }>>(
      token,
      `/api/v1/conversations/${conversationId}/events?${new URLSearchParams({ tenant_id: tenantId })}`
    );
    const assistantEvents = events.filter(
      (event) =>
        event.type === 'message.completed' &&
        event.payload?.role !== 'user' &&
        typeof event.payload?.content === 'string'
    );
    expect(assistantEvents.length).toBeGreaterThanOrEqual(6);
    const history = JSON.stringify(events);
    expect(history).toContain(fruit);
    expect(history).toContain(city);
    expect(history).toContain(language);
    expect(history).toContain('Python 学习计划');
    const restoredAssistantMessages = page.getByTestId('message-text-left').getByTestId('message-text-content');
    const messageScroller = page.getByTestId('message-list-scroller');
    await expect(messageScroller).toBeVisible({ timeout: 30_000 });
    const restoredFruitMessage = restoredAssistantMessages.filter({ hasText: fruit }).first();
    const restoredFirstUserMessage = restoredUserMessages.filter({ hasText: firstPrompt }).first();
    await expect
      .poll(
        async () => {
          if ((await restoredFruitMessage.count()) > 0 && (await restoredFirstUserMessage.count()) > 0) return true;
          await messageScroller.evaluate((element) => {
            element.scrollTop = 0;
            element.dispatchEvent(new Event('scroll'));
          });
          return false;
        },
        { timeout: 30_000, intervals: [300, 500, 1_000] }
      )
      .toBe(true);
    await restoredFirstUserMessage.scrollIntoViewIfNeeded();
    await expect(restoredFirstUserMessage).toBeVisible();
    await restoredFruitMessage.scrollIntoViewIfNeeded();
    await expect(restoredFruitMessage).toBeVisible();
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '25-production-history-restored.png'), fullPage: true });
    await messageScroller.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event('scroll'));
    });
    await expect(restoredAssistantMessages.filter({ hasText: 'Python 学习计划' }).last()).toContainText('验收标准', {
      timeout: 30_000,
    });
    console.log('production-conversation-e2e: history restore complete');

    await refreshDesktopAuth();
    const browserToolCallId = crypto.randomUUID();
    const browserUrl =
      'https://www.math.pku.edu.cn/teachers/a-very-long-browser-result-path-that-must-wrap-without-horizontal-overflow';
    await internalApi('/internal/run-events', {
      tenant_id: tenantId,
      conversation_id: conversationId,
      run_id: null,
      events: [
        {
          event_id: `tool.call.completed.browser-ui.${browserToolCallId}`,
          type: 'tool.call.completed',
          payload: {
            tool_call_id: browserToolCallId,
            tool_name: 'browser_snapshot',
            status: 'completed',
            output_summary: '{"kind":"browser","text":"truncated',
            browser: {
              kind: 'browser',
              action: 'snapshot',
              session_id: 'browser-ui-session',
              profile: 'research',
              url: browserUrl,
              title: '北京大学数学科学学院教授名单',
              element_count: 42,
            },
          },
        },
      ],
    });
    await page.setViewportSize({ width: 900, height: 720 });
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.getByText('View Steps · 1').last().click();
    const browserCard = page.getByTestId('browser-tool-summary-card');
    await expect(browserCard).toBeVisible({ timeout: 30_000 });
    await expect(browserCard).toContainText('北京大学数学科学学院教授名单');
    await expect(browserCard).toContainText(browserUrl);
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1)
    ).toBe(true);
    const browserCardBox = await browserCard.boundingBox();
    expect(browserCardBox).toBeTruthy();
    expect(browserCardBox!.x).toBeGreaterThanOrEqual(0);
    expect(browserCardBox!.x + browserCardBox!.width).toBeLessThanOrEqual(901);
    await page.screenshot({ path: path.join(SCREENSHOT_DIR, '30-browser-tool-card.png'), fullPage: true });
  } finally {
    if (token && projectConversationId)
      await rustApi(token, `/api/conversations/${projectConversationId}`, { method: 'DELETE' }).catch(() => undefined);
    if (token && conversationId)
      await rustApi(token, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    if (token && agentVersionId && tenantId)
      await rustApi(token, `/api/v1/agent-versions/${agentVersionId}/disable`, {
        method: 'POST',
        body: JSON.stringify({ tenant_id: tenantId }),
      }).catch(() => undefined);
    if (workspacePath) await fs.rm(workspacePath, { recursive: true, force: true });
    if (browserFixtureServer)
      await new Promise<void>((resolve) => browserFixtureServer!.close(() => resolve())).catch(() => undefined);
    // Do not close the CDP browser: it owns the shared Electron process.
  }
});
