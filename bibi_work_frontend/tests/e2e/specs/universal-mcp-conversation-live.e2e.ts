import { chromium, expect, test, type Browser, type Page } from '@playwright/test';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const FERRISKEY_PASSWORD = process.env.BIWORK_FERRISKEY_PASSWORD;
const FERRISKEY_BASE_URL = process.env.BIWORK_FERRISKEY_BASE_URL ?? 'http://localhost:3333';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';

type DesktopElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

type CatalogResource = {
  id: string;
  name: string;
  status?: string;
  snapshot?: Record<string, unknown>;
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

async function rustApi<T>(token: string, apiPath: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
      ...init?.headers,
    },
  });
  if (!response.ok) throw new Error(`${init?.method ?? 'GET'} ${apiPath} failed: ${response.status}`);
  return response.json() as Promise<T>;
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

test('Agent-bound universal MCP is discoverable and callable after conversation filtering', async () => {
  test.setTimeout(300_000);
  let browser: Browser | null = null;
  let page: Page | null = null;
  let token: string | null = null;
  let tenantId: string | null = null;
  let agentVersionId: string | null = null;
  let conversationId: string | null = null;

  try {
    browser = await chromium.connectOverCDP(CDP_URL);
    page = browser.contexts()[0]?.pages()[0] ?? null;
    if (!page) throw new Error('running Electron renderer page is unavailable');
    token = await ferrisKeyPasswordToken();
    await page.evaluate(
      async (accessToken) => (window.electronAPI as DesktopElectronApi).setAuthAccessToken(accessToken),
      token
    );
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect
      .poll(() => page!.evaluate(() => (window.electronAPI as DesktopElectronApi).getAuthAccessToken()))
      .toBe(token);

    const me = await rustApi<{ tenant_id: string }>(token, '/api/v1/me');
    tenantId = me.tenant_id;
    const servers = await rustApi<CatalogResource[]>(
      token,
      `/api/v1/mcp-servers?${new URLSearchParams({ tenant_id: tenantId, status: 'active', limit: '100' })}`
    );
    const universal = servers.find((server) => server.name === 'universal');
    test.skip(!universal, 'local universal MCP server is not configured');

    const tools = await rustApi<CatalogResource[]>(
      token,
      `/api/v1/mcp-servers/${universal!.id}/tools?tenant_id=${encodeURIComponent(tenantId)}`
    );
    const statusTool = tools.find((tool) => tool.name === 'get_connection_status');
    test.skip(!statusTool, 'universal status tool is unavailable');

    const agents = await rustApi<CatalogResource[]>(token, `/api/v1/agents?tenant_id=${encodeURIComponent(tenantId)}`);
    const agent = agents.find((candidate) => candidate.name === 'LLM provider smoke agent');
    expect(agent).toBeTruthy();
    const publishedVersions = await rustApi<Array<{ snapshot: Record<string, unknown> }>>(
      token,
      `/api/v1/agents/${agent!.id}/versions?${new URLSearchParams({ tenant_id: tenantId, status: 'published' })}`
    );
    const activeModelProfiles = await rustApi<CatalogResource[]>(
      token,
      `/api/v1/llm-model-profiles?${new URLSearchParams({ tenant_id: tenantId, status: 'active' })}`
    );
    const activeModelProfileIds = new Set(activeModelProfiles.map((profile) => profile.id));
    const modelProfileId =
      publishedVersions
        .map((version) => version.snapshot.model_profile_id)
        .find((value): value is string => typeof value === 'string' && activeModelProfileIds.has(value)) ??
      activeModelProfiles[0]?.id;
    expect(modelProfileId).toBeTruthy();

    const version = await rustApi<{ id: string }>(token, `/api/v1/agents/${agent!.id}/versions`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        version_label: `universal-mcp-live-${Date.now()}`,
        snapshot: {
          system_prompt:
            'When asked to verify universal MCP, call the available MCP tool whose name ends with get_connection_status exactly once. After the tool result, reply with UNIVERSAL_MCP_OK and a brief status summary.',
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
        mcp_tool_ids: [statusTool!.id],
      }),
    });

    const conversation = await rustApi<{ data?: { id?: string }; id?: string }>(token, '/api/conversations', {
      method: 'POST',
      body: JSON.stringify({
        name: `Universal MCP live ${Date.now()}`,
        type: 'acp',
        assistant: { id: agent!.id },
        extra: {
          selected_mcp_server_ids: [universal!.id],
          mcp_server_ids: [universal!.id],
        },
      }),
    });
    conversationId = conversation.data?.id ?? conversation.id ?? null;
    expect(conversationId).toBeTruthy();

    await page.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, conversationId);
    await page.reload({ waitUntil: 'domcontentloaded' });
    await expect(page).toHaveURL(new RegExp(`#/conversation/${conversationId}$`));
    await page.getByTestId('attach-folder-btn').click();
    const mcpMenu = page.getByTestId('conversation-mcp-menu');
    await expect(mcpMenu).toContainText(/·\s*1$/);
    await page.keyboard.press('Escape');

    await page.getByTestId('sendbox-input').fill('请验证 universal MCP 的数据库连接状态。');
    await page.getByTestId('sendbox-send-btn').click();
    await expect(page.getByTestId('message-text-content').filter({ hasText: 'UNIVERSAL_MCP_OK' }).last()).toBeVisible({
      timeout: 180_000,
    });
    await waitForConversationIdle(page, token, conversationId!);

    const events = await rustApi<Array<{ type?: string; payload?: Record<string, unknown> }>>(
      token,
      `/api/v1/conversations/${conversationId}/events?${new URLSearchParams({ tenant_id: tenantId })}`
    );
    expect(
      events.some(
        (event) =>
          event.type === 'tool.call.completed' &&
          typeof event.payload?.tool_name === 'string' &&
          event.payload.tool_name.includes('get_connection_status')
      )
    ).toBe(true);
  } finally {
    if (token && conversationId) {
      await rustApi(token, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    }
    if (token && tenantId && agentVersionId) {
      await rustApi(token, `/api/v1/agent-versions/${agentVersionId}/disable`, {
        method: 'POST',
        body: JSON.stringify({ tenant_id: tenantId }),
      }).catch(() => undefined);
    }
    await browser?.close().catch(() => undefined);
  }
});
