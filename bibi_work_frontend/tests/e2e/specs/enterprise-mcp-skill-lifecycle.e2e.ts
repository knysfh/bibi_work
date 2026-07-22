import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import path from 'node:path';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const FERRISKEY_PASSWORD = process.env.BIWORK_FERRISKEY_PASSWORD;
const FERRISKEY_BASE_URL = process.env.BIWORK_FERRISKEY_BASE_URL ?? 'http://localhost:3333';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';
const INTERNAL_TOKEN = process.env.BIBI_WORK_INTERNAL_TOKEN ?? 'local-internal-token';
const STREAMABLE_MCP_URL = process.env.BIWORK_REAL_STREAMABLE_MCP_URL ?? 'http://127.0.0.1:3103/mcp';
const REMOTE_SKILL_URL =
  process.env.BIWORK_REAL_SKILL_URL ?? 'https://github.com/anthropics/skills/tree/main/skills/mcp-builder';
const SCREENSHOT_DATE = new Date().toISOString().slice(0, 10);
const SCREENSHOT_PATH = path.resolve(
  process.cwd(),
  `../artifacts/playwright/${SCREENSHOT_DATE}/23-streamable-mcp-remote-skill.png`
);
const SKILL_SCREENSHOT_PATH = path.resolve(
  process.cwd(),
  `../artifacts/playwright/${SCREENSHOT_DATE}/24-remote-skill-import.png`
);
const MCP_CONVERSATION_SCREENSHOT_PATH = path.resolve(
  process.cwd(),
  `../artifacts/playwright/${SCREENSHOT_DATE}/27-google-maps-conversation-enabled.png`
);
const MCP_DISABLED_SCREENSHOT_PATH = path.resolve(
  process.cwd(),
  `../artifacts/playwright/${SCREENSHOT_DATE}/28-google-maps-conversation-disabled.png`
);
const AGENT_MCP_SCREENSHOT_PATH = path.resolve(
  process.cwd(),
  `../artifacts/playwright/${SCREENSHOT_DATE}/29-agent-mcp-tool-permissions.png`
);

type DesktopElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
};

type CatalogResource = { id: string; name: string; status: string };
type CatalogVersion = {
  id: string;
  version_label: string;
  snapshot: Record<string, unknown>;
  status: string;
  source_uri?: string | null;
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
  if (!response.ok)
    throw new Error(`${init?.method ?? 'GET'} ${apiPath} failed: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function internalApi<T>(apiPath: string, body: Record<string, unknown>): Promise<T> {
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${INTERNAL_TOKEN}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`POST ${apiPath} failed: ${response.status} ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function desktopApi<T>(page: Page, token: string, apiPath: string, method = 'GET', body?: unknown): Promise<T> {
  return page.evaluate(
    async ({ accessToken, path: requestPath, requestMethod, requestBody }) => {
      const backendPort = (window as Window & { __backendPort?: number }).__backendPort;
      if (!backendPort) throw new Error('Electron backend port is unavailable');
      const response = await fetch(`http://127.0.0.1:${backendPort}${requestPath}`, {
        method: requestMethod,
        headers: { Authorization: `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
        ...(requestBody === undefined ? {} : { body: JSON.stringify(requestBody) }),
      });
      if (!response.ok)
        throw new Error(`${requestMethod} ${requestPath} failed: ${response.status} ${await response.text()}`);
      return response.json();
    },
    { accessToken: token, path: apiPath, requestMethod: method, requestBody: body }
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
  await expect(page.getByTestId('sendbox-input')).toBeEnabled({ timeout: 20_000 });
}

async function mcpToolCallIds(token: string, tenantId: string, conversationId: string): Promise<string[]> {
  const events = await rustApi<Array<{ type?: string; payload?: Record<string, unknown> }>>(
    token,
    `/api/v1/conversations/${conversationId}/events?${new URLSearchParams({ tenant_id: tenantId })}`
  );
  return [
    ...new Set(
      events.flatMap((event) => {
        const toolName = typeof event.payload?.tool_name === 'string' ? event.payload.tool_name : '';
        const toolCallId = typeof event.payload?.tool_call_id === 'string' ? event.payload.tool_call_id : '';
        return toolName.includes('maps_geocode') && toolCallId ? [toolCallId] : [];
      })
    ),
  ];
}

test('real streamable MCP and remote Skill complete governed lifecycle', async () => {
  test.setTimeout(600_000);
  let browser: Browser | null = null;
  let page: Page | null = null;
  let token: string | null = null;
  let tenantId: string | null = null;
  let mcpServerId: string | null = null;
  const lifecycleAgentVersionIds: string[] = [];
  let runtimeId: string | null = null;
  let conversationId: string | null = null;
  let skillId: string | null = null;
  let skillVersionId: string | null = null;
  const mcpName = `Google Maps streamable MCP ${Date.now()}`;

  try {
    browser = await chromium.connectOverCDP(CDP_URL);
    page =
      browser
        .contexts()
        .flatMap((context) => context.pages())
        .find((candidate) => {
          const url = candidate.url();
          return url.startsWith('http://localhost:5173') || url.includes('/out/renderer/index.html');
        }) ?? null;
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
    await page.evaluate(() => {
      window.location.hash = '#/settings/tools';
    });
    await expect(page.getByRole('heading', { name: 'Tools' })).toBeVisible({ timeout: 20_000 });

    const me = await rustApi<{
      tenant_id: string;
      user: { id: string };
      device: { id: string };
      session: { id: string };
      roles: string[];
    }>(token, '/api/v1/me');
    tenantId = me.tenant_id;

    await desktopApi(page, token, '/api/skills/mcp-builder', 'DELETE').catch(() => undefined);
    const created = await desktopApi<{ data?: { id: string }; id?: string }>(page, token, '/api/mcp/servers', 'POST', {
      name: mcpName,
      description: 'Real Google Maps streamable HTTP MCP lifecycle verification',
      transport: { type: 'streamable_http', url: STREAMABLE_MCP_URL },
      original_json: JSON.stringify({ type: 'streamable_http', url: STREAMABLE_MCP_URL }),
    });
    mcpServerId = created.data?.id ?? created.id ?? null;
    expect(mcpServerId).toBeTruthy();

    await page.reload();
    await expect(page.getByRole('heading', { name: 'Tools' })).toBeVisible({ timeout: 20_000 });
    const card = page.locator('.arco-collapse').filter({ hasText: mcpName }).last();
    await expect(card).toBeVisible();
    await card.getByTitle('Check MCP Availability').click();
    await expect
      .poll(
        async () => {
          const response = await desktopApi<
            | Array<{ id: string; last_test_status?: string; tools?: Array<{ name?: string }> }>
            | {
                data: Array<{ id: string; last_test_status?: string; tools?: Array<{ name?: string }> }>;
              }
          >(page!, token!, '/api/mcp/servers');
          const servers = 'data' in response ? response.data : response;
          const server = servers.find((candidate) => candidate.id === mcpServerId);
          return `${server?.last_test_status}:${server?.tools?.some((tool) => tool.name === 'maps_geocode')}`;
        },
        { timeout: 90_000, intervals: [500, 1_000, 2_000, 5_000] }
      )
      .toBe('connected:true');

    const mcpTools = await rustApi<CatalogResource[]>(
      token,
      `/api/v1/mcp-servers/${mcpServerId}/tools?tenant_id=${encodeURIComponent(tenantId)}`
    );
    const geocodeTool = mcpTools.find((tool) => tool.name === 'maps_geocode');
    expect(geocodeTool).toBeTruthy();

    const imported = await desktopApi<{ data?: { skill_names?: string[]; failed?: unknown[] } }>(
      page,
      token,
      '/api/skills/import',
      'POST',
      { skill_path: REMOTE_SKILL_URL }
    );
    expect(imported.data?.failed).toEqual([]);

    const skills = await rustApi<CatalogResource[]>(
      token,
      `/api/v1/skills?${new URLSearchParams({ tenant_id: tenantId, status: 'active', limit: '100' })}`
    );
    const skill = skills.find((candidate) => candidate.name === 'mcp-builder');
    expect(skill).toBeTruthy();
    skillId = skill!.id;
    const skillVersions = await rustApi<CatalogVersion[]>(
      token,
      `/api/v1/skills/${skillId}/versions?${new URLSearchParams({ tenant_id: tenantId, status: 'published' })}`
    );
    const skillVersion = skillVersions[0];
    expect(skillVersion).toBeTruthy();
    skillVersionId = skillVersion.id;

    const agents = await rustApi<CatalogResource[]>(token, `/api/v1/agents?tenant_id=${encodeURIComponent(tenantId)}`);
    const agent = agents.find((candidate) => candidate.name === 'LLM provider smoke agent');
    expect(agent).toBeTruthy();
    const publishedVersions = await rustApi<CatalogVersion[]>(
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
        .map((candidate) => candidate.snapshot.model_profile_id)
        .find(
          (candidate): candidate is string => typeof candidate === 'string' && activeModelProfileIds.has(candidate)
        ) ?? activeModelProfiles[0]?.id;
    expect(modelProfileId).toBeTruthy();
    const lifecycleVersion = await rustApi<CatalogVersion>(token, `/api/v1/agents/${agent!.id}/versions`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        version_label: `mcp-skill-lifecycle-${Date.now()}`,
        snapshot: {
          system_prompt:
            'For a Beijing Railway Station coordinate request, call the available MCP tool whose name ends with maps_geocode exactly once with address exactly "Beijing Railway Station, Beijing, China". Never call any tool again after the first tool result. Immediately report the latitude and longitude from that first result and include MCP_ENABLED_RESULT. If no such tool is available, do not guess coordinates and reply exactly MCP_DISABLED_NO_TOOL.',
          model_profile_id: modelProfileId,
        },
      }),
    });
    lifecycleAgentVersionIds.push(lifecycleVersion.id);
    await rustApi(token, `/api/v1/agent-versions/${lifecycleVersion.id}/bindings`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        skill_version_ids: [],
        tool_version_ids: [],
        sql_tool_version_ids: [],
        mcp_tool_ids: [],
      }),
    });
    const publishedMcpCapabilities = await desktopApi<{
      data?: {
        changed: boolean;
        runtime_id: string;
        browser_enabled: boolean;
        selected_mcp_tool_ids: string[];
      };
      changed?: boolean;
      runtime_id?: string;
      browser_enabled?: boolean;
      selected_mcp_tool_ids?: string[];
    }>(page, token, `/api/agents/${agent!.id}/mcp-capabilities`, 'PUT', {
      mcp_tool_ids: [geocodeTool!.id],
      browser_enabled: false,
    });
    const publishedMcpData = publishedMcpCapabilities.data ?? publishedMcpCapabilities;
    expect(publishedMcpData.changed).toBe(true);
    expect(publishedMcpData.selected_mcp_tool_ids).toEqual([geocodeTool!.id]);
    runtimeId = publishedMcpData.runtime_id ?? null;
    expect(runtimeId).toBeTruthy();
    const capabilities = await desktopApi<{
      data?: { runtime_id: string; selected_mcp_tool_ids: string[] };
      runtime_id?: string;
      selected_mcp_tool_ids?: string[];
    }>(page, token, `/api/agents/${runtimeId}/mcp-capabilities`);
    const capabilityData = capabilities.data ?? capabilities;
    expect(capabilityData.runtime_id).toBe(runtimeId);
    expect(capabilityData.selected_mcp_tool_ids).toEqual([geocodeTool!.id]);

    await page.evaluate((agentId) => {
      window.location.hash = `#/settings/agent/${agentId}/repair`;
    }, runtimeId);
    const agentMcpPanel = page.getByTestId('agent-mcp-capabilities-panel');
    await expect(agentMcpPanel).toBeVisible({ timeout: 20_000 });
    await expect(agentMcpPanel).toContainText('Local browser');
    await expect(
      agentMcpPanel.getByTestId('agent-browser-capability').locator('input[type="checkbox"]')
    ).not.toBeChecked();
    await expect(page.getByTestId(`agent-mcp-tool-${geocodeTool!.id}`).locator('input[type="checkbox"]')).toBeChecked();
    expect(await agentMcpPanel.evaluate((element) => element.scrollWidth > element.clientWidth + 1)).toBe(false);
    await page.screenshot({ path: AGENT_MCP_SCREENSHOT_PATH, fullPage: true });

    const geocode = await internalApi<{ result?: { content?: unknown[] }; mcp_tool_id?: string }>(
      '/internal/mcp-tools:call',
      {
        tenant_id: tenantId,
        actor: {
          user_id: me.user.id,
          device_id: me.device.id,
          session_id: me.session.id,
          roles: me.roles,
        },
        conversation_id: null,
        run_id: null,
        mcp_server_id: mcpServerId,
        mcp_tool_id: geocodeTool!.id,
        tool_name: 'forged_read_name_must_not_control_risk',
        arguments: { address: 'Beijing Railway Station, Beijing, China' },
      }
    );
    expect(geocode.mcp_tool_id).toBe(geocodeTool!.id);
    expect(geocode.result?.content?.length).toBeGreaterThan(0);
    expect(JSON.stringify(geocode.result)).toMatch(/116(?:\.\d+)?/);
    expect(JSON.stringify(geocode.result)).toMatch(/39(?:\.\d+)?/);

    const conversation = await rustApi<{ data?: { id?: string }; id?: string }>(token, '/api/conversations', {
      method: 'POST',
      body: JSON.stringify({
        name: `Google Maps MCP conversation ${Date.now()}`,
        type: 'acp',
        assistant: { id: agent!.id },
        extra: {
          selected_mcp_server_ids: [mcpServerId],
          mcp_server_ids: [mcpServerId],
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
    const conversationMcpMenu = page.getByTestId('conversation-mcp-menu');
    await expect(conversationMcpMenu).toContainText(/·\s*1$/);
    await conversationMcpMenu.hover();
    await expect(
      page.getByTestId(`conversation-mcp-server-${mcpServerId}`).locator('input[type="checkbox"]')
    ).toBeChecked();
    await page.keyboard.press('Escape');

    const enabledPrompt = '帮我查询北京站的经纬度';
    const sendbox = page.getByTestId('sendbox-input');
    const sendButton = page.getByTestId('sendbox-send-btn');
    await sendbox.fill(enabledPrompt);
    await expect(sendButton).toBeEnabled({ timeout: 20_000 });
    await sendButton.click();
    const enabledReply = page.getByTestId('message-text-content').filter({ hasText: 'MCP_ENABLED_RESULT' }).last();
    await expect(enabledReply).toContainText(/116(?:\.\d+)?/, { timeout: 180_000 });
    await expect(enabledReply).toContainText(/39(?:\.\d+)?/);
    await waitForConversationIdle(page, token, conversationId!);
    const enabledToolGroup = page.locator('.tool-group-summary').last();
    const enabledToolSummary = enabledToolGroup.getByText(/^View Steps · 1$/);
    await expect(enabledToolSummary).toBeVisible({ timeout: 30_000 });
    await enabledToolSummary.click();
    await expect(enabledToolGroup.locator('.tool-group-summary__body')).toContainText('maps_geocode', {
      timeout: 30_000,
    });
    const enabledCallIds = await mcpToolCallIds(token, tenantId, conversationId!);
    expect(enabledCallIds).toHaveLength(1);
    await page.screenshot({ path: MCP_CONVERSATION_SCREENSHOT_PATH, fullPage: true });

    const removedMcpCapabilities = await desktopApi<{
      data?: {
        changed: boolean;
        runtime_id: string;
        browser_enabled: boolean;
        selected_mcp_tool_ids: string[];
        previous_version_revoked: boolean;
      };
      changed?: boolean;
      runtime_id?: string;
      browser_enabled?: boolean;
      selected_mcp_tool_ids?: string[];
      previous_version_revoked?: boolean;
    }>(page, token, `/api/agents/${runtimeId}/mcp-capabilities`, 'PUT', {
      mcp_tool_ids: [],
      browser_enabled: false,
    });
    const removedMcpData = removedMcpCapabilities.data ?? removedMcpCapabilities;
    expect(removedMcpData.changed).toBe(true);
    expect(removedMcpData.selected_mcp_tool_ids).toEqual([]);
    expect(removedMcpData.runtime_id).toBe(runtimeId);
    expect(removedMcpData.previous_version_revoked).toBe(false);
    await sendbox.fill('再次帮我查询北京站的经纬度；工具已从 Agent 权限中移除时不要使用记忆或猜测。');
    await expect(sendButton).toBeEnabled({ timeout: 20_000 });
    await sendButton.click();
    await expect(
      page.getByTestId('message-text-content').filter({ hasText: 'MCP_DISABLED_NO_TOOL' }).last()
    ).toBeVisible({
      timeout: 180_000,
    });
    await waitForConversationIdle(page, token, conversationId!);
    const disabledCallIds = await mcpToolCallIds(token, tenantId, conversationId!);
    expect(disabledCallIds).toEqual(enabledCallIds);
    await page.screenshot({ path: MCP_DISABLED_SCREENSHOT_PATH, fullPage: true });

    await page.evaluate(() => {
      window.location.hash = '#/settings/tools';
    });
    await expect(page.getByRole('heading', { name: 'Tools' })).toBeVisible({ timeout: 20_000 });
    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true });
    await page.evaluate(() => {
      window.location.hash = '#/settings/skills';
    });
    await expect(page.getByRole('heading', { name: 'Skills' })).toBeVisible();
    await expect(page.getByText('mcp-builder', { exact: true })).toBeVisible({ timeout: 20_000 });
    await page.screenshot({ path: SKILL_SCREENSHOT_PATH, fullPage: true });

    await rustApi(token, `/api/v1/skill-versions/${skillVersionId}/disable`, {
      method: 'POST',
      body: JSON.stringify({ tenant_id: tenantId }),
    });
    await desktopApi(page, token, '/api/skills/mcp-builder', 'DELETE');
    skillId = null;
    skillVersionId = null;
  } finally {
    if (token && conversationId) {
      await rustApi(token, `/api/conversations/${conversationId}`, { method: 'DELETE' }).catch(() => undefined);
    }
    if (token && tenantId) {
      await Promise.all(
        Array.from(new Set(lifecycleAgentVersionIds), (agentVersionId) =>
          rustApi(token!, `/api/v1/agent-versions/${agentVersionId}/disable`, {
            method: 'POST',
            body: JSON.stringify({ tenant_id: tenantId }),
          }).catch(() => undefined)
        )
      );
    }
    if (page && token && skillId) {
      await desktopApi(page, token, '/api/skills/mcp-builder', 'DELETE').catch(() => undefined);
    }
    if (page && token && mcpServerId) {
      await desktopApi(page, token, `/api/mcp/servers/${mcpServerId}`, 'DELETE').catch(() => undefined);
    }
    await browser?.close().catch(() => undefined);
  }
});
