import { chromium, expect, test, type Browser, type Locator, type Page } from '@playwright/test';
import { createHash } from 'crypto';
import { createServer, type Server } from 'http';
import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import * as tar from 'tar';

const CDP_URL = process.env.BIWORK_LIVE_CDP_URL ?? 'http://127.0.0.1:9230';
const FERRISKEY_USERNAME = process.env.BIWORK_FERRISKEY_USERNAME ?? 'alon';
const FERRISKEY_PASSWORD = process.env.BIWORK_FERRISKEY_PASSWORD;
const FERRISKEY_ALICE_USERNAME = process.env.BIWORK_FERRISKEY_ALICE_USERNAME ?? 'alice';
const FERRISKEY_ALICE_PASSWORD = process.env.BIWORK_FERRISKEY_ALICE_PASSWORD ?? FERRISKEY_PASSWORD;
const FERRISKEY_BASE_URL = process.env.BIWORK_FERRISKEY_BASE_URL ?? 'http://localhost:3333';
const CHROME_EXECUTABLE = process.env.PLAYWRIGHT_CHROME_EXECUTABLE ?? '/usr/bin/google-chrome-stable';
const RUST_API_URL = process.env.BIWORK_RUST_API_URL ?? 'http://127.0.0.1:8361';
const INTERNAL_TOKEN = process.env.BIBI_WORK_INTERNAL_TOKEN ?? 'local-internal-token';
const SCREENSHOT_DATE = new Date().toISOString().slice(0, 10);
const SCREENSHOT_DIR = path.resolve(process.cwd(), `../artifacts/playwright/${SCREENSHOT_DATE}`);
const HUB_ONLY = process.env.BIWORK_LIVE_HUB_ONLY === '1';
const TARGET_REGRESSION_ONLY = process.env.BIWORK_PRODUCTION_REGRESSION_ONLY === '1';

type DesktopElectronApi = {
  getAuthAccessToken: () => Promise<string | null>;
  setAuthAccessToken: (token: string | null) => Promise<void>;
  startDesktopOidcLogin: () => Promise<{ authorizationUrl: string }>;
};

type CatalogResource = {
  id: string;
  name: string;
  status: string;
};

type CatalogVersion = {
  id: string;
  version_label: string;
  snapshot: Record<string, unknown>;
  status: string;
};

type HubFixture = {
  catalogManifest: Record<string, unknown>;
  displayName: string;
  integrity: string;
  name: string;
  tempDir: string;
};

type AliceCapabilityFixture = {
  agentId: string;
  agentVersionId: string;
  artifactPath: string;
  policyBindingIds: string[];
  tenantId: string;
};

type PolicyBinding = {
  id: string;
};

type ProductionProjectFixture = {
  fileName: string;
  projectName: string;
  workspacePath: string;
};

type LiveMcpFixture = {
  server: Server;
  setTools: (tools: Array<Record<string, unknown>>) => void;
  url: string;
};

async function createLiveMcpFixture(): Promise<LiveMcpFixture> {
  let tools: Array<Record<string, unknown>> = [
    {
      name: 'enterprise_live_mcp_health',
      description: 'Return deterministic MCP lifecycle health.',
      inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    },
  ];
  const server = createServer((request, response) => {
    let body = '';
    request.setEncoding('utf8');
    request.on('data', (chunk) => {
      body += chunk;
    });
    request.on('end', () => {
      const payload = JSON.parse(body || '{}') as { id?: unknown; method?: unknown };
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify({ jsonrpc: '2.0', id: payload.id, result: { tools } }));
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve());
  });
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('MCP fixture did not bind a TCP port');
  return {
    server,
    setTools: (nextTools) => {
      tools = nextTools;
    },
    url: `http://127.0.0.1:${address.port}`,
  };
}

async function rustApi<T>(accessToken: string, apiPath: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${RUST_API_URL}${apiPath}`, {
    ...init,
    headers: {
      Authorization: `Bearer ${accessToken}`,
      'Content-Type': 'application/json',
      ...init?.headers,
    },
  });
  if (!response.ok) {
    throw new Error(`${init?.method ?? 'GET'} ${apiPath} failed: ${response.status} ${await response.text()}`);
  }
  return response.json() as Promise<T>;
}

async function rustInternalApi<T>(apiPath: string, body: Record<string, unknown>): Promise<T> {
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

async function ferrisKeyPasswordToken(username: string, password: string): Promise<string> {
  const response = await fetch(`${FERRISKEY_BASE_URL}/realms/bibi-work/protocol/openid-connect/token`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'password',
      client_id: 'bibi-work-backend',
      username,
      password,
    }),
  });
  if (!response.ok) {
    throw new Error(`FerrisKey password grant failed for ${username}: ${response.status} ${await response.text()}`);
  }
  const payload = (await response.json()) as { access_token?: string };
  if (!payload.access_token) throw new Error(`FerrisKey password grant returned no access token for ${username}`);
  return payload.access_token;
}

async function desktopGatewayApi<T>(
  page: Page,
  accessToken: string,
  apiPath: string,
  method: string,
  body?: Record<string, unknown>
): Promise<T> {
  return page.evaluate(
    async ({ token, path: requestPath, requestMethod, requestBody }) => {
      const backendPort = (window as Window & { __backendPort?: number }).__backendPort;
      if (!backendPort) throw new Error('Electron backend port is unavailable');
      const response = await fetch(`http://127.0.0.1:${backendPort}${requestPath}`, {
        method: requestMethod,
        headers: {
          Authorization: `Bearer ${token}`,
          'Content-Type': 'application/json',
        },
        ...(requestBody ? { body: JSON.stringify(requestBody) } : {}),
      });
      if (!response.ok) {
        throw new Error(`${requestMethod} ${requestPath} failed: ${response.status} ${await response.text()}`);
      }
      return response.json();
    },
    { token: accessToken, path: apiPath, requestMethod: method, requestBody: body }
  ) as Promise<T>;
}

async function desktopGatewayRaw(
  page: Page,
  accessToken: string,
  apiPath: string,
  method: string,
  body?: Record<string, unknown>
): Promise<{ status: number; body: unknown }> {
  return page.evaluate(
    async ({ token, path: requestPath, requestMethod, requestBody }) => {
      const backendPort = (window as Window & { __backendPort?: number }).__backendPort;
      if (!backendPort) throw new Error('Electron backend port is unavailable');
      const response = await fetch(`http://127.0.0.1:${backendPort}${requestPath}`, {
        method: requestMethod,
        headers: {
          Authorization: `Bearer ${token}`,
          'Content-Type': 'application/json',
        },
        ...(requestBody ? { body: JSON.stringify(requestBody) } : {}),
      });
      const responseBody = (await response.json().catch(() => null)) as unknown;
      return { status: response.status, body: responseBody };
    },
    { token: accessToken, path: apiPath, requestMethod: method, requestBody: body }
  );
}

async function createHubFixture(): Promise<HubFixture> {
  const name = 'enterprise-live-e2e-extension-v2';
  const displayName = 'Enterprise Live E2E Extension V2';
  const tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-live-hub-'));
  const packageRoot = path.join(tempDir, 'package');
  const extensionRoot = path.join(packageRoot, name);
  const archivePath = path.join(tempDir, `${name}.tgz`);
  const adapterId = `${name}-acp`;
  const installedManifest = {
    name,
    displayName,
    version: '1.0.0',
    description: 'A governed local Hub installation fixture.',
    author: 'Bibi Work E2E',
    hubs: ['acpAdapters'],
    contributes: {
      acpAdapters: [
        {
          id: adapterId,
          name: 'Enterprise Live E2E Agent',
          description: 'A deterministic extension-contributed ACP adapter.',
          connectionType: 'cli',
          cliCommand: 'echo',
          defaultCliPath: 'echo',
          acpArgs: [],
          supportsStreaming: true,
        },
      ],
    },
  };
  await fs.mkdir(extensionRoot, { recursive: true });
  await fs.writeFile(
    path.join(extensionRoot, 'biwork-extension.json'),
    `${JSON.stringify(installedManifest, null, 2)}\n`
  );
  await tar.c({ gzip: true, file: archivePath, cwd: packageRoot }, [name]);
  const archive = await fs.readFile(archivePath);
  const integrity = `sha512-${createHash('sha512').update(archive).digest('base64')}`;
  const tarball = `data:application/gzip;base64,${archive.toString('base64')}`;
  return {
    catalogManifest: {
      ...installedManifest,
      dist: { tarball, integrity, unpackedSize: archive.byteLength },
      engines: { biwork: '*' },
    },
    displayName,
    integrity,
    name,
    tempDir,
  };
}

async function verifyLiveHubInstall(page: Page, accessToken: string, fixture: HubFixture): Promise<void> {
  if (!page.url().endsWith('#/settings/agent')) {
    await page.getByText('Settings', { exact: true }).click();
    await expect(page).toHaveURL(/#\/settings\/agent$/);
  }
  await expect(page.getByRole('heading', { name: 'Agents' })).toBeVisible();
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '06-settings-agent.png'), fullPage: true });
  await page.getByRole('button', { name: 'Install from Market' }).click();
  await expect(page.getByRole('heading', { name: 'Install from Market' })).toBeVisible();
  await expect(page.getByText('Please wait...', { exact: true })).toBeHidden({ timeout: 20_000 });
  const hubCard = page.getByTestId('agent-hub-card').filter({ hasText: fixture.displayName });
  await expect(hubCard).toBeVisible();
  const hubCardCount = await page.getByTestId('agent-hub-card').count();
  const hubModalBounds = await page.locator('.agent-hub-modal').boundingBox();
  expect(hubModalBounds).not.toBeNull();
  expect(hubModalBounds!.width).toBeLessThanOrEqual(Math.min(1000, Math.max(360, hubCardCount * 230 + 48)) + 2);
  const initialHubResponse = await desktopGatewayApi<
    | Array<{ name: string; status: string; bundled?: boolean; dist?: { tarball?: string }; installError?: string }>
    | {
        data: Array<{
          name: string;
          status: string;
          bundled?: boolean;
          dist?: { tarball?: string };
          installError?: string;
        }>;
      }
  >(page, accessToken, '/api/hub/extensions', 'GET');
  const initialHub = 'data' in initialHubResponse ? initialHubResponse.data : initialHubResponse;
  const initialFixture = initialHub.find((extension) => extension.name === fixture.name);
  expect(initialFixture).toMatchObject({ status: 'not_installed', bundled: false });
  expect(initialFixture?.dist?.tarball).toMatch(/^data:application\/gzip;base64,/);
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '10-agent-hub.png'), fullPage: true });
  await hubCard.getByRole('button', { name: 'Install', exact: true }).click();
  await expect
    .poll(
      async () => {
        const response = await desktopGatewayApi<
          | Array<{ name: string; status: string; installError?: string }>
          | { data: Array<{ name: string; status: string; installError?: string }> }
        >(page, accessToken, '/api/hub/extensions', 'GET');
        const extensions = 'data' in response ? response.data : response;
        const extension = extensions.find((item) => item.name === fixture.name);
        if (extension?.status === 'install_failed') {
          throw new Error(extension.installError || 'Hub install failed');
        }
        return extension?.status;
      },
      { timeout: 30_000 }
    )
    .toBe('installed');
  await expect(hubCard.getByRole('button', { name: 'Installed', exact: true })).toBeVisible();
  const hubExtensionsResponse = await desktopGatewayApi<
    Array<{ name: string; status: string }> | { data: Array<{ name: string; status: string }> }
  >(page, accessToken, '/api/hub/extensions', 'GET');
  const hubExtensions = 'data' in hubExtensionsResponse ? hubExtensionsResponse.data : hubExtensionsResponse;
  expect(hubExtensions.find((extension) => extension.name === fixture.name)?.status).toBe('installed');
  const installedAdapters = await desktopGatewayApi<unknown>(page, accessToken, '/api/extensions/acp-adapters', 'GET');
  expect(JSON.stringify(installedAdapters)).toContain(`${fixture.name}-acp`);
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '17-extension-installed.png'), fullPage: true });

  await desktopGatewayApi<unknown>(page, accessToken, '/api/hub/uninstall', 'POST', { name: fixture.name });
  await page.reload({ waitUntil: 'domcontentloaded' });
  await expect(page).toHaveURL(/#\/settings\/agent$/);
  await expect(page.getByRole('heading', { name: 'Agents' })).toBeVisible();
  await page.getByRole('button', { name: 'Install from Market' }).click();
  await expect(page.getByText('Please wait...', { exact: true })).toBeHidden({ timeout: 20_000 });
  const uninstalledHubCard = page.getByTestId('agent-hub-card').filter({ hasText: fixture.displayName });
  await expect(uninstalledHubCard.getByRole('button', { name: 'Install', exact: true })).toBeVisible({
    timeout: 20_000,
  });
  const uninstalledHubExtensionsResponse = await desktopGatewayApi<
    Array<{ name: string; status: string }> | { data: Array<{ name: string; status: string }> }
  >(page, accessToken, '/api/hub/extensions', 'GET');
  const uninstalledHubExtensions =
    'data' in uninstalledHubExtensionsResponse
      ? uninstalledHubExtensionsResponse.data
      : uninstalledHubExtensionsResponse;
  expect(uninstalledHubExtensions.find((extension) => extension.name === fixture.name)?.status).toBe('not_installed');
  const uninstalledAdapters = await desktopGatewayApi<unknown>(
    page,
    accessToken,
    '/api/extensions/acp-adapters',
    'GET'
  );
  expect(JSON.stringify(uninstalledAdapters)).not.toContain(`${fixture.name}-acp`);
  await page.screenshot({ path: path.join(SCREENSHOT_DIR, '18-extension-uninstalled.png'), fullPage: true });
  await page.keyboard.press('Escape');
  await expect(page.getByRole('heading', { name: 'Install from Market' })).toBeHidden();
}

async function createApprovalSmokeVersion(accessToken: string): Promise<{
  agentId: string;
  agentVersionId: string;
  modelProfileId: string;
  tenantId: string;
}> {
  const me = await rustApi<{ tenant_id: string } | { data: { tenant_id: string } }>(accessToken, '/api/me');
  const tenantId = 'data' in me ? me.data.tenant_id : me.tenant_id;
  const query = new URLSearchParams({ tenant_id: tenantId, limit: '100' });

  const tools = await rustApi<CatalogResource[]>(accessToken, `/api/v1/tools?${query}`);
  let tool = tools.find((candidate) => candidate.name === 'enterprise_approval_smoke_health');
  if (!tool) {
    tool = await rustApi<CatalogResource>(accessToken, '/api/v1/tools', {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        name: 'enterprise_approval_smoke_health',
        description: 'Return the local Python agent health status after explicit approval.',
        tool_type: 'third_party',
        schema: {},
        metadata: { purpose: 'live_e2e' },
      }),
    });
  }

  const toolVersions = await rustApi<CatalogVersion[]>(
    accessToken,
    `/api/v1/tools/${tool.id}/versions?${new URLSearchParams({ tenant_id: tenantId, status: 'published' })}`
  );
  let toolVersion = toolVersions.find(
    (candidate) =>
      candidate.snapshot.risk_level === 'high' &&
      candidate.snapshot.requires_approval === true &&
      typeof candidate.snapshot.executor === 'object'
  );
  if (!toolVersion) {
    toolVersion = await rustApi<CatalogVersion>(accessToken, `/api/v1/tools/${tool.id}/versions`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        version_label: 'live-e2e-v1',
        snapshot: {
          description: 'Return the local Python agent health status.',
          input_schema: { type: 'object', properties: {}, additionalProperties: false },
          executor: { type: 'http', url: 'http://127.0.0.1:8371/health', method: 'GET' },
          risk_level: 'high',
          requires_approval: true,
        },
      }),
    });
  }

  const agents = await rustApi<CatalogResource[]>(accessToken, `/api/v1/agents?${query}`);
  const agent = agents.find((candidate) => candidate.name === 'LLM provider smoke agent');
  if (!agent) throw new Error('LLM provider smoke agent is missing');

  const publishedVersions = await rustApi<CatalogVersion[]>(
    accessToken,
    `/api/v1/agents/${agent.id}/versions?${new URLSearchParams({ tenant_id: tenantId, status: 'published' })}`
  );
  const modelProfileId = publishedVersions
    .map((candidate) => candidate.snapshot.model_profile_id)
    .find((candidate): candidate is string => typeof candidate === 'string');
  if (!modelProfileId) throw new Error('LLM provider smoke agent has no published model profile');
  await Promise.all(
    publishedVersions
      .filter((candidate) => candidate.version_label.startsWith('approval-live-'))
      .map((staleVersion) =>
        rustApi<unknown>(accessToken, `/api/v1/agent-versions/${staleVersion.id}/disable`, {
          method: 'POST',
          body: JSON.stringify({ tenant_id: tenantId }),
        })
      )
  );

  const agentVersion = await rustApi<CatalogVersion>(accessToken, `/api/v1/agents/${agent.id}/versions`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: tenantId,
      version_label: `approval-live-${Date.now()}`,
      snapshot: {
        system_prompt:
          'For an approval smoke request, call enterprise_approval_smoke_health exactly once, then report its status. Do not answer before calling the tool.',
        model_profile_id: modelProfileId,
      },
    }),
  });
  await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${agentVersion.id}/bindings`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: tenantId,
      skill_version_ids: [],
      tool_version_ids: [toolVersion.id],
      sql_tool_version_ids: [],
      mcp_tool_ids: [],
    }),
  });
  return { agentId: agent.id, agentVersionId: agentVersion.id, modelProfileId, tenantId };
}

async function verifyMemoryBatchAccessLogging(accessToken: string, tenantId: string): Promise<void> {
  type Me = { user: { id: string } } | { data: { user: { id: string } } };
  type Memory = { id: string; content: string };
  const me = await rustApi<Me>(accessToken, '/api/me');
  const userId = 'data' in me ? me.data.user.id : me.user.id;
  const marker = `memory-batch-e2e-${Date.now()}`;
  const created: Memory[] = [];

  try {
    created.push(
      ...(await Promise.all(
        ['first', 'second'].map((suffix) =>
          rustApi<Memory>(accessToken, '/api/v1/memories', {
            method: 'POST',
            body: JSON.stringify({
              tenant_id: tenantId,
              user_id: userId,
              layer: 'semantic',
              content: `${marker}-${suffix}`,
              confidence: 0.9,
              status: 'approved',
              visibility: 'private',
              sensitivity: 'normal',
            }),
          })
        )
      ))
    );

    const memories = await rustApi<Memory[]>(
      accessToken,
      `/api/v1/memories?${new URLSearchParams({
        tenant_id: tenantId,
        user_id: userId,
        status: 'approved',
        query: marker,
        limit: '10',
      })}`
    );
    expect(memories.map((memory) => memory.id).toSorted()).toEqual(created.map((memory) => memory.id).toSorted());
  } finally {
    await Promise.all(
      created.map((memory) =>
        rustApi<Memory>(accessToken, `/api/v1/memories/${memory.id}/archive`, { method: 'POST' }).catch(
          (): undefined => undefined
        )
      )
    );
  }
}

async function createCronSmokeVersion(
  accessToken: string,
  fixture: { agentId: string; modelProfileId: string; tenantId: string }
): Promise<string> {
  const publishedVersions = await rustApi<CatalogVersion[]>(
    accessToken,
    `/api/v1/agents/${fixture.agentId}/versions?${new URLSearchParams({
      tenant_id: fixture.tenantId,
      status: 'published',
      limit: '100',
    })}`
  );
  await Promise.all(
    publishedVersions
      .filter((candidate) => candidate.version_label.startsWith('cron-live-'))
      .map((staleVersion) =>
        rustApi<unknown>(accessToken, `/api/v1/agent-versions/${staleVersion.id}/disable`, {
          method: 'POST',
          body: JSON.stringify({ tenant_id: fixture.tenantId }),
        })
      )
  );

  const agentVersion = await rustApi<CatalogVersion>(accessToken, `/api/v1/agents/${fixture.agentId}/versions`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: fixture.tenantId,
      version_label: `cron-live-${Date.now()}`,
      snapshot: {
        system_prompt: 'For the Cron live smoke request, reply exactly ok. Do not call tools.',
        model_profile_id: fixture.modelProfileId,
      },
    }),
  });
  await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${agentVersion.id}/bindings`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: fixture.tenantId,
      skill_version_ids: [],
      tool_version_ids: [],
      sql_tool_version_ids: [],
      mcp_tool_ids: [],
    }),
  });
  return agentVersion.id;
}

async function createProductionRegressionVersion(
  accessToken: string,
  fixture: { agentId: string; modelProfileId: string; tenantId: string }
): Promise<string> {
  const publishedVersions = await rustApi<CatalogVersion[]>(
    accessToken,
    `/api/v1/agents/${fixture.agentId}/versions?${new URLSearchParams({
      tenant_id: fixture.tenantId,
      status: 'published',
      limit: '100',
    })}`
  );
  await Promise.all(
    publishedVersions
      .filter((candidate) => candidate.version_label.startsWith('production-regression-live-'))
      .map((staleVersion) =>
        rustApi<unknown>(accessToken, `/api/v1/agent-versions/${staleVersion.id}/disable`, {
          method: 'POST',
          body: JSON.stringify({ tenant_id: fixture.tenantId }),
        })
      )
  );

  const agentVersion = await rustApi<CatalogVersion>(accessToken, `/api/v1/agents/${fixture.agentId}/versions`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: fixture.tenantId,
      version_label: `production-regression-live-${Date.now()}`,
      snapshot: {
        system_prompt:
          'You are the BiWork production regression assistant. Never call tools. Follow the user request directly, preserve conversational context, and reproduce user-provided verification tokens exactly.',
        model_profile_id: fixture.modelProfileId,
      },
    }),
  });
  await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${agentVersion.id}/bindings`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: fixture.tenantId,
      skill_version_ids: [],
      tool_version_ids: [],
      sql_tool_version_ids: [],
      mcp_tool_ids: [],
    }),
  });
  return agentVersion.id;
}

async function ensureProductionProjectFixture(): Promise<ProductionProjectFixture> {
  const projectName = 'BiWork Production Preview E2E';
  const fileName = `production-preview-${Date.now()}.md`;
  const workspacePath = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-production-preview-'));
  await fs.mkdir(path.join(workspacePath, 'docs'), { recursive: true });
  return { fileName, projectName, workspacePath };
}

function extractLabeledWord(text: string, label: string): string {
  const labeledMatch = text.match(new RegExp(`${label}\\s*:\\s*([A-Za-z][A-Za-z-]*)`, 'i'));
  if (labeledMatch?.[1]) return labeledMatch[1];
  const finalWordMatch = text.match(/([A-Za-z][A-Za-z-]*)[^A-Za-z-]*$/);
  if (!finalWordMatch?.[1]) throw new Error(`Model reply did not end with a word for ${label}`);
  return finalWordMatch[1];
}

function literalPattern(value: string): RegExp {
  return new RegExp(value.replaceAll(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i');
}

function hasRequiredPlanHeadings(content: string): boolean {
  return ['Python 学习计划', '学习目标', '每周安排', '验收标准'].every((heading) => content.includes(heading));
}

function compactGeneratedPlan(content: string): string {
  const numberedLines = content
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => /^\d+[.、)]/u.test(line))
    .slice(0, 8)
    .map((line) => {
      let chineseCount = 0;
      let compacted = '';
      for (const character of line) {
        if (/[\u3400-\u9fff]/u.test(character)) chineseCount += 1;
        if (chineseCount > 22) break;
        compacted += character;
      }
      return compacted.trim();
    });
  if (numberedLines.length !== 8) return content;
  return ['Python 学习计划', '学习目标', '每周安排', ...numberedLines, '验收标准'].join('\n');
}

function latestStreamedText(frames: string[], conversationId: string, startIndex: number): string {
  let latest = '';
  for (const rawFrame of frames.slice(startIndex)) {
    try {
      const frame = JSON.parse(rawFrame) as {
        name?: string;
        data?: { conversation_id?: string; type?: string; data?: unknown };
      };
      if (
        frame.name === 'message.stream' &&
        frame.data?.conversation_id === conversationId &&
        frame.data.type === 'text' &&
        typeof frame.data.data === 'string' &&
        frame.data.data.trim()
      ) {
        const chunk = frame.data.data;
        if (chunk.startsWith(latest)) latest = chunk;
        else if (!latest.endsWith(chunk)) latest += chunk;
      }
    } catch {
      // Ignore browser pings and non-JSON frames.
    }
  }
  return latest.trim();
}

async function waitForConversationIdle(page: Page, accessToken: string, conversationId: string): Promise<void> {
  await expect
    .poll(
      async () => {
        const response = await rustApi<
          | { runtime?: { state?: string; is_processing?: boolean } }
          | {
              data: { runtime?: { state?: string; is_processing?: boolean } };
            }
        >(accessToken, `/api/conversations/${conversationId}`);
        const conversation = 'data' in response ? response.data : response;
        return `${conversation.runtime?.state}:${String(conversation.runtime?.is_processing)}`;
      },
      { timeout: 240_000, intervals: [500, 1_000, 2_000, 5_000] }
    )
    .toBe('idle:false');
  await expect(page.getByTestId('sendbox-input')).toBeEnabled({ timeout: 20_000 });
}

async function sendPromptAndCaptureStream(options: {
  accessToken: string;
  conversationId: string;
  frames: string[];
  input: Locator;
  messages: Locator;
  page: Page;
  prompt: string;
  send: Locator;
}): Promise<string> {
  const messageCountBefore = await options.messages.count();
  const frameIndex = options.frames.length;
  await options.input.fill(options.prompt);
  await expect(options.send).toBeEnabled({ timeout: 20_000 });
  await options.send.click();
  await expect
    .poll(() => options.messages.count(), { timeout: 180_000 })
    .toBeGreaterThanOrEqual(messageCountBefore + 1);
  await waitForConversationIdle(options.page, options.accessToken, options.conversationId);
  return latestStreamedText(options.frames, options.conversationId, frameIndex);
}

async function collectVisibleStreamingSamples(
  page: Page,
  accessToken: string,
  conversationId: string,
  userPrompt: string,
  tenantId: string,
  websocketFrames: string[]
): Promise<string[]> {
  const samples: string[] = [];
  const deadline = Date.now() + 240_000;
  let activityObserved = false;
  let idleObservedAt: number | null = null;
  let processedFrameCount = 0;
  while (Date.now() < deadline) {
    for (; processedFrameCount < websocketFrames.length; processedFrameCount += 1) {
      try {
        const frame = JSON.parse(websocketFrames[processedFrameCount]) as {
          name?: string;
          data?: { conversation_id?: string; type?: string; data?: unknown };
        };
        if (
          frame.name === 'message.stream' &&
          frame.data?.conversation_id === conversationId &&
          frame.data.type === 'text' &&
          typeof frame.data.data === 'string'
        ) {
          const normalized = frame.data.data.trim();
          if (normalized && normalized !== userPrompt && samples.at(-1) !== normalized) samples.push(normalized);
          activityObserved = true;
        }
      } catch {
        // Ignore non-JSON frames such as browser-level pings.
      }
    }

    // eslint-disable-next-line no-await-in-loop -- sequential polling must preserve runtime observation order
    const response = await rustApi<
      | { runtime?: { state?: string; is_processing?: boolean } }
      | {
          data: { runtime?: { state?: string; is_processing?: boolean } };
        }
    >(accessToken, `/api/conversations/${conversationId}`);
    const conversation = 'data' in response ? response.data : response;
    const idle = conversation.runtime?.state === 'idle' && conversation.runtime?.is_processing === false;
    if (!idle) activityObserved = true;
    if (activityObserved && idle) idleObservedAt ??= Date.now();
    if (!idle) idleObservedAt = null;
    if (samples.length > 0 && idleObservedAt && Date.now() - idleObservedAt >= 1_500) return [...new Set(samples)];
    // eslint-disable-next-line no-await-in-loop -- bounded polling delay is intentionally sequential
    await page.waitForTimeout(75);
  }
  const storedEvents = await rustApi<Array<{ type: string; payload?: { content?: unknown } }>>(
    accessToken,
    `/api/v1/conversations/${conversationId}/events?${new URLSearchParams({ tenant_id: tenantId })}`
  ).catch(() => []);
  const storedEventSummary = storedEvents.map((event) => ({
    type: event.type,
    contentLength: typeof event.payload?.content === 'string' ? event.payload.content.length : undefined,
  }));
  throw new Error(
    `Timed out while sampling streamed output for ${conversationId}; storedEvents=${JSON.stringify(storedEventSummary)}`
  );
}

async function ensureLiveToolVersion(
  accessToken: string,
  tenantId: string,
  definition: {
    description: string;
    name: string;
    snapshot: Record<string, unknown>;
    toolType: string;
    versionLabel: string;
  }
): Promise<{ toolId: string; toolVersionId: string }> {
  const query = new URLSearchParams({ tenant_id: tenantId, limit: '100' });
  const tools = await rustApi<CatalogResource[]>(accessToken, `/api/v1/tools?${query}`);
  let tool = tools.find((candidate) => candidate.name === definition.name);
  if (!tool) {
    tool = await rustApi<CatalogResource>(accessToken, '/api/v1/tools', {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        name: definition.name,
        description: definition.description,
        tool_type: definition.toolType,
        schema: {},
        metadata: { purpose: 'live_e2e' },
      }),
    });
  }
  const versions = await rustApi<CatalogVersion[]>(
    accessToken,
    `/api/v1/tools/${tool.id}/versions?${new URLSearchParams({
      tenant_id: tenantId,
      status: 'published',
      limit: '100',
    })}`
  );
  let version = versions.find((candidate) => candidate.version_label === definition.versionLabel);
  if (!version) {
    version = await rustApi<CatalogVersion>(accessToken, `/api/v1/tools/${tool.id}/versions`, {
      method: 'POST',
      body: JSON.stringify({
        tenant_id: tenantId,
        version_label: definition.versionLabel,
        snapshot: definition.snapshot,
      }),
    });
  }
  return { toolId: tool.id, toolVersionId: version.id };
}

async function createAliceCapabilitySmokeVersion(
  adminAccessToken: string,
  aliceUserId: string
): Promise<AliceCapabilityFixture> {
  const artifactPath = `/artifacts/alice-capability-smoke-${Date.now()}.txt`;
  const me = await rustApi<{ tenant_id: string }>(adminAccessToken, '/api/me');
  const tenantId = me.tenant_id;
  const readTool = await ensureLiveToolVersion(adminAccessToken, tenantId, {
    name: 'read_file',
    description: 'Read a governed virtual file path.',
    toolType: 'builtin',
    versionLabel: 'live-e2e-v1',
    snapshot: {
      description: 'Read a governed virtual file path.',
      input_schema: {
        type: 'object',
        properties: { path: { type: 'string' } },
        required: ['path'],
        additionalProperties: false,
      },
      risk_level: 'low',
      requires_approval: false,
    },
  });
  const writeTool = await ensureLiveToolVersion(adminAccessToken, tenantId, {
    name: 'write_file',
    description: 'Write a governed virtual file path with optimistic revision control.',
    toolType: 'builtin',
    versionLabel: 'live-e2e-v1',
    snapshot: {
      description: 'Write a governed virtual file path with optimistic revision control.',
      input_schema: {
        type: 'object',
        properties: {
          path: { type: 'string' },
          content: { type: 'string' },
          expected_revision: { type: 'integer' },
          reason: { type: 'string' },
        },
        required: ['path', 'content', 'expected_revision'],
        additionalProperties: false,
      },
      risk_level: 'low',
      requires_approval: false,
    },
  });
  const lowRiskTool = await ensureLiveToolVersion(adminAccessToken, tenantId, {
    name: 'enterprise_low_risk_smoke_health',
    description: 'Return the local Python agent health status without approval.',
    toolType: 'third_party',
    versionLabel: 'live-e2e-v1',
    snapshot: {
      description: 'Return the local Python agent health status without approval.',
      input_schema: { type: 'object', properties: {}, additionalProperties: false },
      executor: { type: 'http', url: 'http://127.0.0.1:8371/health', method: 'GET' },
      risk_level: 'low',
      requires_approval: false,
    },
  });

  const query = new URLSearchParams({ tenant_id: tenantId, limit: '100' });
  const agents = await rustApi<CatalogResource[]>(adminAccessToken, `/api/v1/agents?${query}`);
  const agent = agents.find((candidate) => candidate.name === 'LLM provider smoke agent');
  if (!agent) throw new Error('LLM provider smoke agent is missing');
  const publishedVersions = await rustApi<CatalogVersion[]>(
    adminAccessToken,
    `/api/v1/agents/${agent.id}/versions?${new URLSearchParams({
      tenant_id: tenantId,
      status: 'published',
      limit: '100',
    })}`
  );
  const modelProfileId = publishedVersions
    .map((candidate) => candidate.snapshot.model_profile_id)
    .find((candidate): candidate is string => typeof candidate === 'string');
  if (!modelProfileId) throw new Error('LLM provider smoke agent has no published model profile');
  await Promise.all(
    publishedVersions
      .filter((candidate) => candidate.version_label.startsWith('alice-capability-live-'))
      .map((staleVersion) =>
        rustApi<unknown>(adminAccessToken, `/api/v1/agent-versions/${staleVersion.id}/disable`, {
          method: 'POST',
          body: JSON.stringify({ tenant_id: tenantId }),
        })
      )
  );

  const agentVersion = await rustApi<CatalogVersion>(adminAccessToken, `/api/v1/agents/${agent.id}/versions`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: tenantId,
      version_label: `alice-capability-live-${Date.now()}`,
      snapshot: {
        system_prompt: `For the Alice capability smoke request, call write_file once for ${artifactPath} with content 中文工具摘要-你好世界 and expected_revision 0, call read_file once for the same path, then call enterprise_low_risk_smoke_health once. Finish with exactly: alice smoke ok. Do not skip a tool.`,
        model_profile_id: modelProfileId,
      },
    }),
  });
  await rustApi<unknown>(adminAccessToken, `/api/v1/agent-versions/${agentVersion.id}/bindings`, {
    method: 'POST',
    body: JSON.stringify({
      tenant_id: tenantId,
      skill_version_ids: [],
      tool_version_ids: [readTool.toolVersionId, writeTool.toolVersionId, lowRiskTool.toolVersionId],
      sql_tool_version_ids: [],
      mcp_tool_ids: [],
    }),
  });

  const bindingSpecs = [
    { resource_type: 'agent', resource_id: agent.id, action: 'run' },
    { resource_type: 'tool', resource_id: readTool.toolId, action: 'use' },
    { resource_type: 'tool', resource_id: writeTool.toolId, action: 'use' },
    { resource_type: 'tool', resource_id: lowRiskTool.toolId, action: 'use' },
    { resource_type: 'tool', resource_id: 'read_file', action: 'execute' },
    { resource_type: 'tool', resource_id: 'write_file', action: 'execute' },
    { resource_type: 'tool', resource_id: lowRiskTool.toolId, action: 'execute' },
  ];
  const bindingResults = await Promise.allSettled(
    bindingSpecs.map((binding) =>
      rustApi<PolicyBinding>(adminAccessToken, '/api/v1/policy-bindings', {
        method: 'POST',
        body: JSON.stringify({
          tenant_id: tenantId,
          ...binding,
          subject_type: 'user',
          subject_id: aliceUserId,
          effect: 'allow',
          risk_level: 'low',
          obligations: {},
          policy_version: 'live-e2e-v1',
        }),
      })
    )
  );
  const policyBindingIds = bindingResults.flatMap((result) => (result.status === 'fulfilled' ? [result.value.id] : []));
  const bindingFailure = bindingResults.find((result): result is PromiseRejectedResult => result.status === 'rejected');
  if (bindingFailure) {
    await Promise.all(
      policyBindingIds.map((bindingId) =>
        rustApi<unknown>(adminAccessToken, `/api/v1/policy-bindings/${bindingId}/disable`, {
          method: 'POST',
          body: JSON.stringify({ tenant_id: tenantId }),
        }).catch(() => undefined)
      )
    );
    await rustApi<unknown>(adminAccessToken, `/api/v1/agent-versions/${agentVersion.id}/disable`, {
      method: 'POST',
      body: JSON.stringify({ tenant_id: tenantId }),
    }).catch(() => undefined);
    throw bindingFailure.reason;
  }
  return { agentId: agent.id, agentVersionId: agentVersion.id, artifactPath, policyBindingIds, tenantId };
}

async function connectDesktopPage(): Promise<{ browser: Browser; page: Page }> {
  const browser = await chromium.connectOverCDP(CDP_URL);
  const pages = browser.contexts().flatMap((context) => context.pages());
  const page = pages.find((candidate) => candidate.url().startsWith('http://localhost:5173'));
  if (!page) {
    throw new Error(`No BiWork renderer page found through ${CDP_URL}`);
  }
  await page.bringToFront();
  return { browser, page };
}

async function launchFerrisKeyBrowser(): Promise<{ browser: Browser; page: Page }> {
  const browser = await chromium.launch({
    executablePath: CHROME_EXECUTABLE,
    headless: true,
    args: ['--disable-gpu', '--no-proxy-server', '--no-sandbox'],
  });
  const context = await browser.newContext();
  await context.route('https://fonts.googleapis.com/**', (route) => route.abort());
  await context.route('https://fonts.gstatic.com/**', (route) => route.abort());
  return { browser, page: await context.newPage() };
}

test('logs into the running Electron desktop through FerrisKey OIDC and renders enterprise navigation', async () => {
  test.setTimeout(900_000);
  test.skip(!FERRISKEY_PASSWORD, 'BIWORK_FERRISKEY_PASSWORD is required for the live enterprise smoke test');
  test.skip(
    !FERRISKEY_ALICE_PASSWORD,
    'BIWORK_FERRISKEY_ALICE_PASSWORD or BIWORK_FERRISKEY_PASSWORD is required for the Alice authorization smoke'
  );

  const { page: desktopPage } = await connectDesktopPage();
  let ferrisKeyBrowser: Browser | null = null;
  let accessToken: string | null = null;
  let approvalAgentVersionId: string | null = null;
  let approvalConversationId: string | null = null;
  let aliceAccessToken: string | null = null;
  let aliceBootstrapToken: string | null = null;
  let aliceCapabilityFixture: AliceCapabilityFixture | null = null;
  let aliceConversationId: string | null = null;
  let cronAgentVersionId: string | null = null;
  let cronConversationId: string | null = null;
  let cronJobId: string | null = null;
  let productionAgentVersionId: string | null = null;
  let productionConversationId: string | null = null;
  let projectConversationId: string | null = null;
  let projectWorkspacePath: string | null = null;
  let channelPairingCode: string | null = null;
  let channelPlatformUserId: string | null = null;
  let channelUserId: string | null = null;
  let hubFixture: HubFixture | null = null;
  let mcpFixture: LiveMcpFixture | null = null;
  let mcpServerId: string | null = null;
  let stdioMcpServerId: string | null = null;
  let teamId: string | null = null;
  let tenantId: string | null = null;
  const desktopConsoleMessages: string[] = [];
  const desktopPageErrors: string[] = [];
  const websocketFrames: string[] = [];
  desktopPage.on('console', (message) => desktopConsoleMessages.push(message.text()));
  desktopPage.on('pageerror', (error) => desktopPageErrors.push(error.message));
  desktopPage.on('websocket', (socket) => {
    socket.on('framereceived', (event) => {
      if (typeof event.payload === 'string') websocketFrames.push(event.payload);
    });
  });

  try {
    await desktopPage.evaluate(async () => {
      const electronApi = window.electronAPI as DesktopElectronApi;
      await electronApi.setAuthAccessToken(null);
    });
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(/#\/login$/);
    await expect(desktopPage.getByRole('button', { name: /FerrisKey/i })).toBeVisible({ timeout: 30_000 });
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '01-desktop-login.png'), fullPage: true });

    const { authorizationUrl } = await desktopPage.evaluate(async () => {
      const electronApi = window.electronAPI as DesktopElectronApi;
      return electronApi.startDesktopOidcLogin();
    });
    expect(authorizationUrl).toContain('/protocol/openid-connect/auth');
    expect(authorizationUrl).toContain('code_challenge_method=S256');

    const ferrisKey = await launchFerrisKeyBrowser();
    ferrisKeyBrowser = ferrisKey.browser;
    await ferrisKey.page.goto(authorizationUrl, { waitUntil: 'domcontentloaded' });
    await expect(ferrisKey.page.getByLabel('Username')).toBeVisible();
    await ferrisKey.page.getByLabel('Username').fill(FERRISKEY_USERNAME);
    await ferrisKey.page.getByLabel('Password').fill(FERRISKEY_PASSWORD!);
    await ferrisKey.page.screenshot({ path: path.join(SCREENSHOT_DIR, '02-ferriskey-login.png'), fullPage: true });
    await ferrisKey.page.getByRole('button', { name: /^Login$/ }).click();

    await expect(desktopPage).toHaveURL(/#\/guid$/, { timeout: 20_000 });
    await expect(desktopPage.getByText('New Chat', { exact: true })).toBeVisible();
    await expect(desktopPage.getByRole('textbox', { name: /Send a message/i })).toBeVisible();
    await expect(desktopPage.getByTestId('biwork-brand-logo')).toBeVisible();
    await expect(desktopPage.getByTestId('biwork-brand-logo').locator('img')).toHaveAttribute('alt', 'BiWork');
    await expect(desktopPage.getByText('BiWork', { exact: true }).first()).toBeVisible();
    await expect(desktopPage.getByText(/AionUi|AionHub/i)).toHaveCount(0);
    await expect
      .poll(() => desktopConsoleMessages.some((message) => message.includes('[ensureWs] CONNECTED')))
      .toBe(true);
    await expect.poll(() => desktopConsoleMessages.some((message) => message.includes('[WS:msg] auth.ok'))).toBe(true);
    accessToken = await desktopPage.evaluate(async () => {
      const electronApi = window.electronAPI as DesktopElectronApi;
      return electronApi.getAuthAccessToken();
    });
    expect(accessToken).toBeTruthy();
    const unsafeExternalUrl = await desktopGatewayRaw(desktopPage, accessToken!, '/api/shell/open-external', 'POST', {
      url: 'javascript:alert(document.domain)',
    });
    expect(unsafeExternalUrl.status).toBe(500);
    expect(unsafeExternalUrl.body).toMatchObject({
      success: false,
      code: 'LOCAL_RUNTIME_ERROR',
      error: 'external URL protocol is not allowed: javascript:',
    });
    hubFixture = await createHubFixture();
    await desktopGatewayApi<unknown>(desktopPage, accessToken!, '/api/hub/uninstall', 'POST', {
      name: hubFixture.name,
    }).catch(() => undefined);
    await rustApi<unknown>(accessToken!, '/api/extensions/sync', {
      method: 'POST',
      body: JSON.stringify({
        extensions: [
          {
            name: hubFixture.name,
            source: 'hub',
            version: '1.0.0',
            integrity: hubFixture.integrity,
            manifest: hubFixture.catalogManifest,
            risk_level: 'safe',
            enabled: false,
            installed: false,
            install_status: 'not_installed',
            contributions: [],
          },
        ],
      }),
    });
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '03-guid.png'), fullPage: true });
    if (HUB_ONLY) {
      await verifyLiveHubInstall(desktopPage, accessToken!, hubFixture);
      return;
    }

    const approvalFixture = await createApprovalSmokeVersion(accessToken!);
    approvalAgentVersionId = approvalFixture.agentVersionId;
    tenantId = approvalFixture.tenantId;
    await verifyMemoryBatchAccessLogging(accessToken!, tenantId);
    if (!TARGET_REGRESSION_ONLY) {
      await desktopPage.reload({ waitUntil: 'domcontentloaded' });
      await expect(desktopPage).toHaveURL(/#\/guid$/);
      const approvalAssistant = desktopPage.getByTestId(`preset-pill-${approvalFixture.agentId}`);
      await expect(approvalAssistant).toBeVisible({ timeout: 20_000 });
      await approvalAssistant.click();
      await desktopPage
        .getByTestId('guid-input')
        .fill('Approval smoke: call enterprise_approval_smoke_health exactly once, then report the returned status.');
      const approvalSendButton = desktopPage.getByTestId('guid-send-btn');
      await expect(approvalSendButton).toBeEnabled();
      await approvalSendButton.click();
      await expect(desktopPage).toHaveURL(/#\/conversation\//, { timeout: 20_000 });
      approvalConversationId = desktopPage.url().split('/conversation/')[1] ?? null;
      expect(approvalConversationId).toBeTruthy();
      const permissionCard = desktopPage.getByTestId('message-permission-card');
      await expect
        .poll(
          async () => {
            const approvals = await rustApi<Array<{ conversation_id?: string; status: string }>>(
              accessToken!,
              `/api/v1/approvals?${new URLSearchParams({
                tenant_id: tenantId!,
                status: 'pending',
                limit: '100',
              })}`
            );
            return approvals.some((approval) => approval.conversation_id === approvalConversationId);
          },
          { timeout: 180_000, intervals: [1_000, 2_000, 5_000] }
        )
        .toBe(true);
      const compatConfirmationsResponse = await rustApi<
        Array<{ id: string; call_id: string }> | { data: Array<{ id: string; call_id: string }> }
      >(accessToken!, `/api/conversations/${approvalConversationId}/confirmations`);
      const compatConfirmations =
        'data' in compatConfirmationsResponse ? compatConfirmationsResponse.data : compatConfirmationsResponse;
      expect(compatConfirmations).toHaveLength(1);
      let confirmationRecoveryStatus: number | null = null;
      const captureConfirmationRecovery = (response: import('@playwright/test').Response) => {
        if (
          response.request().method() === 'GET' &&
          response.url().endsWith(`/api/conversations/${approvalConversationId}/confirmations`)
        ) {
          confirmationRecoveryStatus = response.status();
        }
      };
      desktopPage.on('response', captureConfirmationRecovery);
      await desktopPage.reload({ waitUntil: 'domcontentloaded' });
      await expect(desktopPage).toHaveURL(new RegExp(`#/conversation/${approvalConversationId}$`));
      const recoveredDesktopToken = await desktopPage.evaluate(async () => {
        const electronApi = window.electronAPI as DesktopElectronApi;
        return electronApi.getAuthAccessToken();
      });
      expect(
        recoveredDesktopToken !== null && recoveredDesktopToken === accessToken,
        `desktop token disappeared after approval reload; recent console=${JSON.stringify(
          desktopConsoleMessages.slice(-40)
        )}; pageErrors=${JSON.stringify(desktopPageErrors)}`
      ).toBe(true);
      await expect(desktopPage.getByRole('button', { name: 'Continue with FerrisKey' })).toBeHidden({
        timeout: 20_000,
      });
      await expect.poll(() => confirmationRecoveryStatus, { timeout: 30_000 }).toBe(200);
      desktopPage.off('response', captureConfirmationRecovery);
      await expect(permissionCard).toBeVisible({ timeout: 30_000 });
      await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '12-approval-requested.png'), fullPage: true });
      await desktopPage.getByTestId('message-permission-option-proceed_once').click();
      const confirmationResponsePromise = desktopPage.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          response.url().includes(`/api/conversations/${approvalConversationId}/confirmations/`) &&
          response.url().endsWith('/confirm'),
        { timeout: 20_000 }
      );
      await desktopPage.getByTestId('message-permission-confirm').click();
      const confirmationResponse = await confirmationResponsePromise;
      if (!confirmationResponse.ok()) {
        throw new Error(
          `Approval confirmation failed (${confirmationResponse.status()}): ${await confirmationResponse.text()}`
        );
      }
      await expect(permissionCard).toBeHidden({ timeout: 90_000 });
      await expect
        .poll(
          async () => {
            const response = await rustApi<
              { runtime?: { state?: string } } | { data: { runtime?: { state?: string } } }
            >(accessToken!, `/api/conversations/${approvalConversationId}`);
            const conversation = 'data' in response ? response.data : response;
            return conversation.runtime?.state;
          },
          { timeout: 240_000, intervals: [1_000, 2_000, 5_000] }
        )
        .toBe('idle');
      await expect(desktopPage.getByTestId('message-text-content').last()).toContainText(/ok|status|health/i, {
        timeout: 20_000,
      });
      await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '11-conversation-stream.png'), fullPage: true });
      await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '13-approval-resumed.png'), fullPage: true });
      const pendingApprovals = await rustApi<Array<{ conversation_id?: string; status: string }>>(
        accessToken!,
        `/api/v1/approvals?${new URLSearchParams({ tenant_id: tenantId, status: 'pending', limit: '100' })}`
      );
      expect(pendingApprovals.filter((approval) => approval.conversation_id === approvalConversationId)).toEqual([]);
      const auditVerify = await rustApi<{ valid: boolean }>(
        accessToken!,
        `/api/v1/audit/hash-chain:verify?${new URLSearchParams({ tenant_id: tenantId, limit: '1000' })}`
      );
      expect(auditVerify.valid).toBe(true);
      cronAgentVersionId = await createCronSmokeVersion(accessToken!, approvalFixture);
      await desktopPage.getByText('Scheduled Tasks', { exact: true }).click();
      await expect(desktopPage).toHaveURL(/#\/scheduled$/);
      await desktopPage.reload({ waitUntil: 'domcontentloaded' });
      await expect(desktopPage).toHaveURL(/#\/scheduled$/);
      const scheduledReloadToken = await desktopPage.evaluate(async () => {
        const electronApi = window.electronAPI as DesktopElectronApi;
        return electronApi.getAuthAccessToken();
      });
      expect(
        scheduledReloadToken !== null && scheduledReloadToken === accessToken,
        `desktop token disappeared after scheduled-page reload; recent console=${JSON.stringify(
          desktopConsoleMessages.slice(-60)
        )}; pageErrors=${JSON.stringify(desktopPageErrors)}`
      ).toBe(true);
      await expect(desktopPage.getByRole('heading', { name: 'Scheduled Tasks' })).toBeVisible();
      const newTaskButton = desktopPage.getByRole('button', { name: 'New task' });
      await expect(newTaskButton).toBeVisible();
      await newTaskButton.click();
      await desktopPage.getByText('Create manually', { exact: true }).click();
      const cronDialog = desktopPage.getByRole('dialog');
      await expect(cronDialog).toContainText('Create Scheduled Task');
      const cronName = `BiWork live cron ${Date.now()}`;
      await desktopPage.getByPlaceholder('Enter task name').fill(cronName);
      await desktopPage.getByTestId('cron-assistant-select').click();
      await desktopPage.getByText('LLM provider smoke agent', { exact: true }).last().click();
      await desktopPage
        .getByPlaceholder('Instructions for the assistant to execute')
        .fill('Cron live smoke: reply exactly ok and do not call tools.');
      await desktopPage.getByRole('button', { name: 'Save' }).click();
      await expect(desktopPage.getByText(cronName, { exact: true })).toBeVisible({ timeout: 20_000 });
      await expect(cronDialog).toBeHidden({ timeout: 20_000 });
      await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '04-scheduled.png'), fullPage: true });
      await desktopPage.getByText(cronName, { exact: true }).click();
      await expect(desktopPage).toHaveURL(/#\/scheduled\/[^/]+$/);
      cronJobId = desktopPage.url().split('/scheduled/')[1] ?? null;
      expect(cronJobId).toBeTruthy();
      await desktopPage.getByRole('button', { name: 'Run now' }).click();
      await expect(desktopPage).toHaveURL(/#\/conversation\/[^/]+$/, { timeout: 30_000 });
      cronConversationId = desktopPage.url().split('/conversation/')[1] ?? null;
      expect(cronConversationId).toBeTruthy();
      await expect(
        desktopPage.getByText('Cron live smoke: reply exactly ok and do not call tools.', { exact: true })
      ).toBeVisible({ timeout: 90_000 });
      await expect
        .poll(
          async () => {
            const response = await rustApi<
              { runtime?: { state?: string } } | { data: { runtime?: { state?: string } } }
            >(accessToken!, `/api/conversations/${cronConversationId}`);
            const conversation = 'data' in response ? response.data : response;
            return conversation.runtime?.state;
          },
          { timeout: 180_000, intervals: [1_000, 2_000, 5_000] }
        )
        .toBe('idle');
      await desktopPage.reload({ waitUntil: 'domcontentloaded' });
      await expect(desktopPage).toHaveURL(new RegExp(`#/conversation/${cronConversationId}$`));
      const cronConversationMain = desktopPage.locator('main').last();
      const cronOkMessages = cronConversationMain
        .getByTestId('message-text-content')
        .filter({ hasText: /^\s*ok\s*$/i });
      await expect(cronOkMessages).toHaveCount(1, { timeout: 20_000 });
      await expect(cronConversationMain.getByText(/^Processing\.\.\./)).toBeHidden({ timeout: 20_000 });
      await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '14-cron-run-now.png'), fullPage: true });
    }

    productionAgentVersionId = await createProductionRegressionVersion(accessToken!, approvalFixture);
    const productionConversationResponse = await rustApi<{ data?: { id?: string }; id?: string }>(
      accessToken!,
      '/api/conversations',
      {
        method: 'POST',
        body: JSON.stringify({
          name: `BiWork production multi-turn ${Date.now()}`,
          type: 'acp',
          assistant: { id: approvalFixture.agentId },
          extra: {},
        }),
      }
    );
    productionConversationId = productionConversationResponse.data?.id ?? productionConversationResponse.id ?? null;
    expect(productionConversationId).toBeTruthy();
    await desktopPage.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, productionConversationId);
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(new RegExp(`#/conversation/${productionConversationId}$`));
    const firstTurnPrompt =
      `Write at least 12 numbered lines about reliable regression testing. ` +
      'The final line must be FRUIT: followed by one common English fruit word of your choice for later turns.';
    const productionInput = desktopPage.getByTestId('sendbox-input');
    const productionSend = desktopPage.getByTestId('sendbox-send-btn');
    await productionInput.fill(firstTurnPrompt);
    const firstTurnFrameIndex = websocketFrames.length;
    const streamSamplesPromise = collectVisibleStreamingSamples(
      desktopPage,
      accessToken!,
      productionConversationId!,
      firstTurnPrompt,
      approvalFixture.tenantId,
      websocketFrames
    );
    await expect(productionSend).toBeEnabled({ timeout: 20_000 });
    await productionSend.click();
    const productionAssistantMessages = desktopPage
      .getByTestId('message-text-left')
      .getByTestId('message-text-content');
    await expect.poll(() => productionAssistantMessages.count(), { timeout: 180_000 }).toBeGreaterThanOrEqual(1);
    await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
    const firstTurnReply = productionAssistantMessages.filter({ hasText: /regression/i }).last();
    await expect(firstTurnReply).toContainText(/regression/i);
    const streamSamples = await streamSamplesPromise;
    const firstTurnText = latestStreamedText(websocketFrames, productionConversationId!, firstTurnFrameIndex);
    const rememberedFruit = extractLabeledWord(firstTurnText, 'FRUIT');
    expect(
      streamSamples.length,
      `Expected incremental rendering, observed samples=${JSON.stringify(streamSamples)}`
    ).toBeGreaterThanOrEqual(1);

    const turnTwoMessageCountBefore = await productionAssistantMessages.count();
    await productionInput.fill('This is turn two. In one short sentence, state the fruit you chose in turn one.');
    await expect(productionSend).toBeEnabled({ timeout: 20_000 });
    await productionSend.click();
    await expect
      .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
      .toBeGreaterThanOrEqual(turnTwoMessageCountBefore + 1);
    await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
    const rememberedFruitPattern = literalPattern(rememberedFruit);
    const secondTurnReply = productionAssistantMessages.filter({ hasText: rememberedFruitPattern }).last();
    await expect(secondTurnReply).toContainText(rememberedFruitPattern);
    const turnThreeMessageCountBefore = await productionAssistantMessages.count();
    const turnThreeFrameIndex = websocketFrames.length;
    await productionInput.fill(
      'This is turn three. Repeat the remembered fruit, then choose a one-word city for later and write it as CITY: <word>.'
    );
    await expect(productionSend).toBeEnabled({ timeout: 20_000 });
    await productionSend.click();
    await expect
      .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
      .toBeGreaterThanOrEqual(turnThreeMessageCountBefore + 1);
    await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
    const thirdTurnText = latestStreamedText(websocketFrames, productionConversationId!, turnThreeFrameIndex);
    expect(thirdTurnText).not.toBe('');
    expect(thirdTurnText).toMatch(rememberedFruitPattern);
    const rememberedCity = extractLabeledWord(thirdTurnText, 'CITY');
    const rememberedCityPattern = literalPattern(rememberedCity);
    const thirdTurnReply = productionAssistantMessages.filter({ hasText: rememberedCityPattern }).last();
    await expect(thirdTurnReply).toContainText(rememberedFruitPattern);
    await expect(thirdTurnReply).toContainText(rememberedCityPattern);
    const turnFourMessageCountBefore = await productionAssistantMessages.count();
    const turnFourFrameIndex = websocketFrames.length;
    await productionInput.fill(
      'This is turn four. Repeat the remembered fruit and city, then choose a programming language and write it as LANGUAGE: <word>.'
    );
    await expect(productionSend).toBeEnabled({ timeout: 20_000 });
    await productionSend.click();
    await expect
      .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
      .toBeGreaterThanOrEqual(turnFourMessageCountBefore + 1);
    await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
    const fourthTurnText = latestStreamedText(websocketFrames, productionConversationId!, turnFourFrameIndex);
    expect(fourthTurnText).not.toBe('');
    expect(fourthTurnText).toMatch(rememberedFruitPattern);
    expect(fourthTurnText).toMatch(rememberedCityPattern);
    const rememberedLanguage = extractLabeledWord(fourthTurnText, 'LANGUAGE');
    const rememberedLanguagePattern = literalPattern(rememberedLanguage);
    const fourthTurnReply = productionAssistantMessages.filter({ hasText: rememberedLanguagePattern }).last();
    await expect(fourthTurnReply).toContainText(rememberedFruitPattern);
    await expect(fourthTurnReply).toContainText(rememberedCityPattern);
    await expect(fourthTurnReply).toContainText(rememberedLanguagePattern);
    const turnFiveMessageCountBefore = await productionAssistantMessages.count();
    await productionInput.fill(
      'This is the fifth turn. Without asking again, list the remembered fruit, city, and programming language, then confirm this is turn five.'
    );
    await expect(productionSend).toBeEnabled({ timeout: 20_000 });
    await productionSend.click();
    await expect
      .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
      .toBeGreaterThanOrEqual(turnFiveMessageCountBefore + 1);
    await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
    const fifthTurnReply = productionAssistantMessages.filter({ hasText: rememberedLanguagePattern }).last();
    await expect(fifthTurnReply).toContainText(rememberedFruitPattern);
    await expect(fifthTurnReply).toContainText(rememberedCityPattern);
    await expect(fifthTurnReply).toContainText(rememberedLanguagePattern);
    await expect(fifthTurnReply).toContainText(/(?:turn\s*(?:five|5)|fifth\s+turn)/i);
    await expect(desktopPage.getByText(/^Processing\.\.\./)).toBeHidden({ timeout: 20_000 });
    await desktopPage.screenshot({
      path: path.join(SCREENSHOT_DIR, '23-production-multi-turn-stream.png'),
      fullPage: true,
    });

    const planPrompt =
      '请写一篇约200字的中文 Python 学习计划。必须包含“Python 学习计划”“学习目标”“每周安排”“验收标准”四个标题，' +
      '并写出恰好8条编号学习安排，每条12至20个汉字，覆盖基础语法、数据结构、函数模块、文件处理、测试、项目和复盘。' +
      '正文控制在160至300个汉字。只输出计划正文，不要调用工具。';
    const capturePlan = (prompt: string) =>
      sendPromptAndCaptureStream({
        accessToken: accessToken!,
        conversationId: productionConversationId!,
        frames: websocketFrames,
        input: productionInput,
        messages: productionAssistantMessages,
        page: desktopPage,
        prompt,
        send: productionSend,
      });
    let planContent = await capturePlan(planPrompt);
    if (!hasRequiredPlanHeadings(planContent)) {
      planContent = await capturePlan(`上一次没有产生可用计划。${planPrompt}`);
    }
    if (!hasRequiredPlanHeadings(planContent)) {
      planContent = await capturePlan(`这是最后一次重试，必须输出完整正文。${planPrompt}`);
    }
    expect(hasRequiredPlanHeadings(planContent)).toBe(true);
    const planReply = productionAssistantMessages.filter({ hasText: 'Python 学习计划' }).last();
    await expect(planReply).toContainText('Python 学习计划', { timeout: 30_000 });
    await expect(planReply).toContainText('学习目标');
    await expect(planReply).toContainText('每周安排');
    await expect(planReply).toContainText('验收标准');
    expect(planContent).toContain('Python 学习计划');
    let chineseCharacterCount = [...planContent].filter((character) => /[\u3400-\u9fff]/u.test(character)).length;
    if (chineseCharacterCount < 160) {
      const messageCountBeforeRevision = await productionAssistantMessages.count();
      const revisionFrameIndex = websocketFrames.length;
      await productionInput.fill(
        `上一版主体只有 ${chineseCharacterCount} 个汉字。请只输出一个80至120个汉字的“补充安排”段落，` +
          '补充每日练习、周末项目和复盘方法，不要重复已有四个标题。'
      );
      await expect(productionSend).toBeEnabled({ timeout: 20_000 });
      await productionSend.click();
      await expect
        .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
        .toBeGreaterThanOrEqual(messageCountBeforeRevision + 1);
      await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
      const planAddendum = latestStreamedText(websocketFrames, productionConversationId!, revisionFrameIndex);
      expect(planAddendum).not.toBe('');
      planContent = `${planContent}\n\n${planAddendum}`;
      chineseCharacterCount = [...planContent].filter((character) => /[\u3400-\u9fff]/u.test(character)).length;
    }
    if (chineseCharacterCount < 160) {
      const messageCountBeforeProjectAddendum = await productionAssistantMessages.count();
      const projectAddendumFrameIndex = websocketFrames.length;
      await productionInput.fill(
        `当前学习计划合计 ${chineseCharacterCount} 个汉字，仍不足约200字。请只输出一个60至90个汉字的“项目实战”段落，` +
          '描述一个命令行小项目、测试方法和完成标准，不要重复前文。'
      );
      await expect(productionSend).toBeEnabled({ timeout: 20_000 });
      await productionSend.click();
      await expect
        .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
        .toBeGreaterThanOrEqual(messageCountBeforeProjectAddendum + 1);
      await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
      const projectAddendum = latestStreamedText(websocketFrames, productionConversationId!, projectAddendumFrameIndex);
      expect(projectAddendum).not.toBe('');
      planContent = `${planContent}\n\n${projectAddendum}`;
      chineseCharacterCount = [...planContent].filter((character) => /[\u3400-\u9fff]/u.test(character)).length;
    }
    if (chineseCharacterCount > 300) {
      const messageCountBeforeCondense = await productionAssistantMessages.count();
      const condenseFrameIndex = websocketFrames.length;
      await productionInput.fill(
        `上一版共 ${chineseCharacterCount} 个汉字，过长。请完整重写为160至260个汉字，保留“Python 学习计划”“学习目标”` +
          '“每周安排”“验收标准”四个标题和恰好8条编号安排，删除冗余解释，只输出完整新计划。'
      );
      await expect(productionSend).toBeEnabled({ timeout: 20_000 });
      await productionSend.click();
      await expect
        .poll(() => productionAssistantMessages.count(), { timeout: 180_000 })
        .toBeGreaterThanOrEqual(messageCountBeforeCondense + 1);
      await waitForConversationIdle(desktopPage, accessToken!, productionConversationId!);
      planContent = latestStreamedText(websocketFrames, productionConversationId!, condenseFrameIndex);
      const condensedReply = productionAssistantMessages.filter({ hasText: 'Python 学习计划' }).last();
      await expect(condensedReply).toContainText('学习目标');
      await expect(condensedReply).toContainText('每周安排');
      await expect(condensedReply).toContainText('验收标准');
      chineseCharacterCount = [...planContent].filter((character) => /[\u3400-\u9fff]/u.test(character)).length;
    }
    if (chineseCharacterCount > 300) {
      planContent = compactGeneratedPlan(planContent);
      chineseCharacterCount = [...planContent].filter((character) => /[\u3400-\u9fff]/u.test(character)).length;
    }
    expect(chineseCharacterCount).toBeGreaterThanOrEqual(160);
    expect(chineseCharacterCount).toBeLessThanOrEqual(300);
    const expectedProductionAssistantMessageCount = await productionAssistantMessages.count();
    expect(expectedProductionAssistantMessageCount).toBeGreaterThanOrEqual(6);

    const projectFixture = await ensureProductionProjectFixture();
    projectWorkspacePath = projectFixture.workspacePath;
    const projects = await rustApi<Array<{ id: string; metadata?: Record<string, unknown> }>>(
      accessToken!,
      `/api/v1/projects?tenant_id=${encodeURIComponent(tenantId!)}`
    );
    const existingProject = projects.find((project) => project.metadata?.workspace === projectFixture.workspacePath);
    const project =
      existingProject ??
      (await rustApi<{ id: string }>(accessToken!, '/api/v1/projects', {
        method: 'POST',
        body: JSON.stringify({
          tenant_id: tenantId,
          name: projectFixture.projectName,
          description: 'Production Electron workspace regression fixture',
          metadata: { workspace: projectFixture.workspacePath },
        }),
      }));
    const projectConversationResponse = await rustApi<{ data?: { id: string }; id?: string }>(
      accessToken!,
      '/api/conversations',
      {
        method: 'POST',
        body: JSON.stringify({
          name: 'BiWork production project preview',
          type: 'acp',
          assistant: { id: approvalFixture.agentId },
          extra: { workspace: projectFixture.workspacePath, custom_workspace: true },
        }),
      }
    );
    projectConversationId = projectConversationResponse.data?.id ?? projectConversationResponse.id ?? null;
    expect(projectConversationId).toBeTruthy();
    await rustApi<unknown>(accessToken!, `/api/v1/conversations/${projectConversationId}`, {
      method: 'PATCH',
      body: JSON.stringify({ tenant_id: tenantId, project_id: project.id }),
    });
    await desktopPage.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, projectConversationId);
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(new RegExp(`#/conversation/${projectConversationId}$`));
    await desktopGatewayApi<unknown>(desktopPage, accessToken!, '/api/fs/write', 'POST', {
      workspace: projectFixture.workspacePath,
      path: path.join(projectFixture.workspacePath, 'docs', projectFixture.fileName),
      data: `${planContent}\n`,
    });
    await desktopGatewayApi<unknown>(desktopPage, accessToken!, '/api/fs/write', 'POST', {
      workspace: projectFixture.workspacePath,
      path: path.join(projectFixture.workspacePath, 'docs', projectFixture.fileName),
      data: `${planContent}\n`,
      expected_revision: 0,
    });
    const previewFilePath = path.join(projectFixture.workspacePath, 'docs', projectFixture.fileName);
    const persistedPlanResponse = await desktopGatewayApi<string | { data?: string }>(
      desktopPage,
      accessToken!,
      '/api/fs/read',
      'POST',
      {
        workspace: projectFixture.workspacePath,
        path: previewFilePath,
      }
    );
    const persistedPlan =
      typeof persistedPlanResponse === 'string' ? persistedPlanResponse : (persistedPlanResponse.data ?? '');
    expect(persistedPlan).toContain('Python 学习计划');
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(new RegExp(`#/conversation/${projectConversationId}$`));
    const projectWorkspace = desktopPage.locator('.chat-workspace');
    const expandWorkspace = desktopPage.getByRole('button', { name: 'Expand workspace' });
    await expect
      .poll(async () => (await projectWorkspace.isVisible()) || (await expandWorkspace.isVisible()), {
        timeout: 30_000,
      })
      .toBe(true);
    if (!(await projectWorkspace.isVisible())) {
      await expandWorkspace.click();
    }
    await expect(projectWorkspace).toBeVisible({ timeout: 30_000 });
    const docsFolder = projectWorkspace.getByText('docs', { exact: true }).first();
    await expect(docsFolder).toBeVisible({ timeout: 30_000 });
    await docsFolder.click();
    const previewFile = projectWorkspace.getByRole('treeitem', { name: projectFixture.fileName, exact: true });
    await expect(previewFile).toBeVisible({ timeout: 30_000 });
    await previewFile.getByText(projectFixture.fileName, { exact: true }).evaluate((element) => {
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
    const previewAction = desktopPage.getByRole('button', { name: 'Preview', exact: true });
    await expect(previewAction).toBeVisible({ timeout: 10_000 });
    await previewAction.click();
    const previewPanel = desktopPage.locator('.preview-panel');
    await expect(previewPanel).toBeVisible({ timeout: 30_000 });
    await expect(previewPanel).toContainText('Python 学习计划');
    await expect(previewPanel).toContainText('学习目标');
    await expect(previewPanel).toContainText('每周安排');
    await expect(previewPanel).toContainText('验收标准');
    await previewPanel.evaluate(async (element) => {
      await Promise.all(element.getAnimations().map((animation) => animation.finished));
    });
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '24-project-file-preview.png'), fullPage: true });

    await desktopPage.evaluate((id) => {
      window.location.hash = `#/conversation/${id}`;
    }, productionConversationId);
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(new RegExp(`#/conversation/${productionConversationId}$`));
    const restoredHistory = desktopPage.locator('main').last();
    const restoredAssistantMessages = restoredHistory
      .getByTestId('message-text-left')
      .getByTestId('message-text-content');
    const restoredEvents = await rustApi<Array<{ payload?: { content?: unknown } }>>(
      accessToken!,
      `/api/v1/conversations/${productionConversationId}/events?${new URLSearchParams({ tenant_id: tenantId! })}`
    );
    const restoredEventText = JSON.stringify(restoredEvents);
    expect(restoredEventText).toContain(rememberedFruit);
    expect(restoredEventText).toContain(rememberedCity);
    expect(restoredEventText).toContain(rememberedLanguage);
    expect(restoredEventText).toContain('Python 学习计划');
    const messageScroller = desktopPage.getByTestId('message-list-scroller');
    await expect(messageScroller).toBeVisible({ timeout: 30_000 });
    await messageScroller.evaluate((element) => {
      element.scrollTop = 0;
      element.dispatchEvent(new Event('scroll'));
    });
    await expect(restoredAssistantMessages.filter({ hasText: /regression/i }).last()).toContainText(
      rememberedFruitPattern,
      { timeout: 30_000 }
    );
    await messageScroller.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
      element.dispatchEvent(new Event('scroll'));
    });
    await expect(restoredAssistantMessages.filter({ hasText: 'Python 学习计划' }).last()).toContainText('验收标准', {
      timeout: 30_000,
    });
    await desktopPage.screenshot({
      path: path.join(SCREENSHOT_DIR, '25-production-history-restored.png'),
      fullPage: true,
    });

    const teamName = `BiWork Team Smoke ${Date.now()}`;
    const teamResponse = await rustApi<{ data?: { id: string }; id?: string }>(accessToken!, '/api/teams', {
      method: 'POST',
      body: JSON.stringify({
        name: teamName,
        workspace: '',
        workspace_mode: 'shared',
        assistants: [
          {
            assistant_id: approvalFixture.agentId,
            assistant_name: 'Leader slot 0',
            role: 'leader',
            model: 'minimax-m2.5',
          },
        ],
      }),
    });
    teamId = teamResponse.data?.id ?? teamResponse.id ?? null;
    expect(teamId).toBeTruthy();
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await desktopPage.evaluate((id) => {
      window.location.hash = `#/team/${id}`;
    }, teamId);
    const teamEntry = desktopPage.getByText(teamName, { exact: true }).first();
    if (!(await teamEntry.isVisible().catch(() => false))) {
      await desktopPage.getByText('Teams', { exact: true }).click();
    }
    await expect(teamEntry).toBeVisible();
    await expect(desktopPage).toHaveURL(new RegExp(`#/team/${teamId}$`));
    await expect(desktopPage.locator('main').last()).toContainText('Leader slot 0', { timeout: 35_000 });
    await expect(desktopPage.getByRole('textbox', { name: /Send message to /i }).first()).toBeVisible({
      timeout: 35_000,
    });
    const teamSlot = desktopPage.locator('[data-testid^="team-chat-slot-"]').first();
    const teamInput = teamSlot.getByTestId('sendbox-input');
    const teamSend = teamSlot.getByTestId('sendbox-send-btn');
    const teamContextNonce = `team-context-${Date.now()}`;
    await teamInput.fill(
      `Remember this exact code for the next turn: ${teamContextNonce}. Reply exactly: team turn one complete ${teamContextNonce}`
    );
    await expect(teamSend).toBeEnabled({ timeout: 20_000 });
    const [teamTurnOneResponse] = await Promise.all([
      desktopPage.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname === `/api/teams/${teamId}/messages`,
        { timeout: 30_000 }
      ),
      teamSend.click(),
    ]);
    expect(teamTurnOneResponse.ok(), await teamTurnOneResponse.text()).toBeTruthy();
    await expect(teamSlot.getByTestId('message-text-content').last()).toContainText(
      `team turn one complete ${teamContextNonce}`,
      { timeout: 180_000 }
    );
    await expect(teamInput).toBeEnabled({ timeout: 20_000 });
    await expect(teamSlot.getByText(/^Processing\.\.\./)).toBeHidden({ timeout: 20_000 });
    await teamInput.fill(
      'What exact code did I ask you to remember in the previous turn? Reply exactly: team turn two complete; previous=<code>'
    );
    await expect(teamSend).toBeEnabled({ timeout: 20_000 });
    await teamSend.click();
    await expect(teamSlot.getByTestId('message-text-content').last()).toContainText(
      `team turn two complete; previous=${teamContextNonce}`,
      { timeout: 180_000 }
    );
    await teamInput.fill('processing-cleared-probe');
    await expect(teamSend).toBeEnabled({ timeout: 20_000 });
    await teamInput.fill('');
    await expect(teamSlot.getByText(/^Processing\.\.\./)).toBeHidden({ timeout: 20_000 });
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '26-team-multi-turn.png'), fullPage: true });

    if (TARGET_REGRESSION_ONLY) return;

    await verifyLiveHubInstall(desktopPage, accessToken!, hubFixture);

    await desktopPage.getByText('Model', { exact: true }).click();
    await expect(desktopPage).toHaveURL(/#\/settings\/model$/);
    await expect(desktopPage.getByRole('heading', { name: 'Model', exact: true })).toBeVisible();
    const rotationCard = desktopPage.getByTestId('credential-rotation-card');
    await expect(rotationCard).toBeVisible();
    await expect(rotationCard).toContainText('Worker disabled');
    await expect(rotationCard).toContainText('Gateway not configured');
    await desktopPage.screenshot({
      path: path.join(SCREENSHOT_DIR, '21-credential-rotation-health.png'),
      fullPage: true,
    });

    mcpFixture = await createLiveMcpFixture();
    const existingMcpResponse = await desktopGatewayApi<
      Array<{ id: string; name: string }> | { data: Array<{ id: string; name: string }> }
    >(desktopPage, accessToken!, '/api/mcp/servers', 'GET');
    const existingMcpServers = 'data' in existingMcpResponse ? existingMcpResponse.data : existingMcpResponse;
    await Promise.all(
      existingMcpServers
        .filter(
          (server) =>
            server.name.startsWith('Enterprise MCP lifecycle ') || server.name.startsWith('Enterprise stdio MCP ')
        )
        .map((server) => desktopGatewayApi(desktopPage, accessToken!, `/api/mcp/servers/${server.id}`, 'DELETE'))
    );
    const mcpName = `Enterprise MCP lifecycle ${Date.now()}`;
    const createdMcpResponse = await desktopGatewayApi<{
      data?: { id: string; name: string };
      id?: string;
      name?: string;
    }>(desktopPage, accessToken!, '/api/mcp/servers', 'POST', {
      name: mcpName,
      description: 'Live E2E MCP lifecycle fixture',
      transport: { type: 'http', url: mcpFixture.url },
      original_json: JSON.stringify({ type: 'http', url: mcpFixture.url }),
    });
    mcpServerId = createdMcpResponse.data?.id || createdMcpResponse.id || null;
    expect(mcpServerId).toBeTruthy();
    const stdioMcpName = `Enterprise stdio MCP ${Date.now()}`;
    const createdStdioMcpResponse = await desktopGatewayApi<{
      data?: { id: string };
      id?: string;
    }>(desktopPage, accessToken!, '/api/mcp/servers', 'POST', {
      name: stdioMcpName,
      description: 'Live E2E governed stdio MCP fixture',
      transport: {
        type: 'stdio',
        command: process.execPath,
        args: [path.resolve(process.cwd(), 'tests/fixtures/mcp/stdio-server.mjs')],
      },
      original_json: JSON.stringify({ type: 'stdio', command: process.execPath }),
    });
    stdioMcpServerId = createdStdioMcpResponse.data?.id || createdStdioMcpResponse.id || null;
    expect(stdioMcpServerId).toBeTruthy();

    await desktopPage.getByText('Tools', { exact: true }).click();
    await expect(desktopPage).toHaveURL(/#\/settings\/tools$/);
    await expect(desktopPage.getByRole('heading', { name: 'Tools' })).toBeVisible();
    const mcpCard = desktopPage.locator('.arco-collapse').filter({ hasText: mcpName }).last();
    await expect(mcpCard).toBeVisible();
    const mcpCheckButton = mcpCard.getByTitle('Check MCP Availability');
    await mcpCheckButton.click();
    await expect
      .poll(async () => {
        const response = await desktopGatewayApi<
          | Array<{ id: string; last_test_status?: string; tools?: unknown[] }>
          | { data: Array<{ id: string; last_test_status?: string; tools?: unknown[] }> }
        >(desktopPage, accessToken!, '/api/mcp/servers', 'GET');
        const servers = 'data' in response ? response.data : response;
        const server = servers.find((candidate) => candidate.id === mcpServerId);
        return `${server?.last_test_status}:${server?.tools?.length}`;
      })
      .toBe('connected:1');
    await expect(mcpCheckButton).not.toHaveClass(/arco-btn-loading/);
    const stdioMcpCard = desktopPage.locator('.arco-collapse').filter({ hasText: stdioMcpName }).last();
    await expect(stdioMcpCard).toBeVisible();
    await stdioMcpCard.getByTitle('Check MCP Availability').click();
    await expect
      .poll(async () => {
        const response = await desktopGatewayApi<
          | Array<{ id: string; last_test_status?: string; tools?: Array<{ name?: string }> }>
          | { data: Array<{ id: string; last_test_status?: string; tools?: Array<{ name?: string }> }> }
        >(desktopPage, accessToken!, '/api/mcp/servers', 'GET');
        const servers = 'data' in response ? response.data : response;
        const server = servers.find((candidate) => candidate.id === stdioMcpServerId);
        return `${server?.last_test_status}:${server?.tools?.[0]?.name}`;
      })
      .toBe('connected:stdio_fixture_health');
    await expect(stdioMcpCard.getByTitle('Check MCP Availability')).not.toHaveClass(/arco-btn-loading/);

    const me = await rustApi<{
      tenant_id: string;
      user: { id: string };
      device: { id: string };
      session: { id: string };
      roles: string[];
    }>(accessToken!, '/api/v1/me');
    const stdioTools = await rustApi<CatalogResource[]>(
      accessToken!,
      `/api/v1/mcp-servers/${stdioMcpServerId}/tools?tenant_id=${encodeURIComponent(me.tenant_id)}`
    );
    const stdioTool = stdioTools.find((tool) => tool.name === 'stdio_fixture_health');
    expect(stdioTool).toBeTruthy();
    const stdioCall = await rustInternalApi<{
      structuredContent?: { status?: string; arguments?: { queued?: boolean } };
      mcp_server_id?: string;
      mcp_tool_id?: string;
    }>('/internal/mcp-tools:call', {
      tenant_id: me.tenant_id,
      actor: {
        user_id: me.user.id,
        device_id: me.device.id,
        session_id: me.session.id,
        roles: me.roles,
      },
      conversation_id: null,
      run_id: null,
      mcp_server_id: stdioMcpServerId,
      mcp_tool_id: stdioTool!.id,
      tool_name: stdioTool!.name,
      arguments: { queued: true },
    });
    expect(stdioCall).toMatchObject({
      structuredContent: { status: 'ok', arguments: { queued: true } },
      mcp_server_id: stdioMcpServerId,
      mcp_tool_id: stdioTool!.id,
    });

    mcpFixture.setTools([]);
    await mcpCheckButton.click();
    await expect
      .poll(async () => {
        const response = await desktopGatewayApi<
          | Array<{ id: string; last_test_status?: string; tools?: unknown[] }>
          | { data: Array<{ id: string; last_test_status?: string; tools?: unknown[] }> }
        >(desktopPage, accessToken!, '/api/mcp/servers', 'GET');
        const servers = 'data' in response ? response.data : response;
        const server = servers.find((candidate) => candidate.id === mcpServerId);
        return `${server?.last_test_status}:${server?.tools?.length}`;
      })
      .toBe('connected:0');
    await expect(mcpCheckButton).not.toHaveClass(/arco-btn-loading/);
    await mcpCard.hover();
    await mcpCard.getByRole('switch', { name: `Disable ${mcpName}` }).click();
    await expect
      .poll(async () => {
        const response = await desktopGatewayApi<
          Array<{ id: string; enabled: boolean }> | { data: Array<{ id: string; enabled: boolean }> }
        >(desktopPage, accessToken!, '/api/mcp/servers', 'GET');
        const servers = 'data' in response ? response.data : response;
        return servers.find((candidate) => candidate.id === mcpServerId)?.enabled;
      })
      .toBe(false);
    await mcpCard.getByRole('switch', { name: `Enable ${mcpName}` }).click();
    await expect
      .poll(async () => {
        const response = await desktopGatewayApi<
          Array<{ id: string; enabled: boolean }> | { data: Array<{ id: string; enabled: boolean }> }
        >(desktopPage, accessToken!, '/api/mcp/servers', 'GET');
        const servers = 'data' in response ? response.data : response;
        return servers.find((candidate) => candidate.id === mcpServerId)?.enabled;
      })
      .toBe(true);
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '22-mcp-lifecycle-health.png'), fullPage: true });
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '07-settings-tools.png'), fullPage: true });

    await desktopPage.getByText('Skills', { exact: true }).click();
    await expect(desktopPage).toHaveURL(/#\/settings\/skills$/);
    await expect(desktopPage.getByRole('heading', { name: 'Skills' })).toBeVisible();
    await expect(desktopPage.getByText('Please wait...', { exact: true })).toBeHidden({ timeout: 20_000 });
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '08-settings-skills.png'), fullPage: true });

    await desktopPage.getByText('Remote', { exact: true }).click();
    await expect(desktopPage).toHaveURL(/#\/settings\/webui$/);
    await expect(desktopPage.getByRole('heading', { name: 'WebUI' })).toBeVisible();
    await desktopPage.getByRole('tab', { name: /^Channels/ }).click();
    await expect(desktopPage.getByRole('heading', { name: 'Channels' })).toBeVisible();
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '09-settings-channels.png'), fullPage: true });

    await rustApi<unknown>(accessToken!, '/api/channel/plugins/enable', {
      method: 'POST',
      body: JSON.stringify({ plugin_id: 'telegram', config: {} }),
    });
    channelPlatformUserId = `live-e2e-${Date.now()}`;
    channelPairingCode = `LIVE${Date.now().toString().slice(-8)}`;
    await rustApi<unknown>(accessToken!, '/api/channel/pairings/request', {
      method: 'POST',
      body: JSON.stringify({
        platform: 'telegram',
        platform_user_id: channelPlatformUserId,
        display_name: 'BiWork live channel user',
        code: channelPairingCode,
        ttl_seconds: 600,
      }),
    });
    await desktopPage.getByText('Telegram', { exact: true }).click();
    await expect(desktopPage.getByText('Pending Pairing Requests', { exact: true })).toBeVisible();
    await expect(desktopPage.getByText(channelPairingCode, { exact: true })).toBeVisible({ timeout: 20_000 });
    await desktopPage.screenshot({
      path: path.join(SCREENSHOT_DIR, '15-channel-pairing-requested.png'),
      fullPage: true,
    });
    await desktopPage.getByRole('button', { name: 'Approve', exact: true }).click();
    await expect(desktopPage.getByText(channelPairingCode, { exact: true })).toBeHidden({ timeout: 20_000 });
    await expect(desktopPage.getByText('Authorized Users', { exact: true })).toBeVisible();
    await expect(desktopPage.getByText('BiWork live channel user', { exact: true })).toBeVisible();
    const channelUsersResponse = await rustApi<
      Array<{ id: string; platform_user_id: string }> | { data: Array<{ id: string; platform_user_id: string }> }
    >(accessToken!, '/api/channel/users');
    const channelUsers = 'data' in channelUsersResponse ? channelUsersResponse.data : channelUsersResponse;
    channelUserId = channelUsers.find((user) => user.platform_user_id === channelPlatformUserId)?.id ?? null;
    expect(channelUserId).toBeTruthy();
    await desktopPage.screenshot({
      path: path.join(SCREENSHOT_DIR, '16-channel-user-authorized.png'),
      fullPage: true,
    });

    const connectedCountBeforeReload = desktopConsoleMessages.filter((message) =>
      message.includes('[ensureWs] CONNECTED')
    ).length;
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(/#\/settings\/webui$/);
    await expect(desktopPage.getByRole('heading', { name: 'WebUI' })).toBeVisible();
    await expect
      .poll(
        () =>
          desktopConsoleMessages.filter((message) => message.includes('[ensureWs] CONNECTED')).length >
          connectedCountBeforeReload
      )
      .toBe(true);
    expect(
      await desktopPage.evaluate(async () => {
        const electronApi = window.electronAPI as DesktopElectronApi;
        return Boolean(await electronApi.getAuthAccessToken());
      })
    ).toBe(true);

    aliceBootstrapToken = await ferrisKeyPasswordToken(FERRISKEY_ALICE_USERNAME, FERRISKEY_ALICE_PASSWORD!);
    const aliceMe = await rustApi<{ tenant_id: string; user: { id: string; username: string } }>(
      aliceBootstrapToken,
      '/api/me'
    );
    expect(aliceMe.user.username).toBe(FERRISKEY_ALICE_USERNAME);
    aliceCapabilityFixture = await createAliceCapabilitySmokeVersion(accessToken!, aliceMe.user.id);

    const aliceAssistantsResponse = await rustApi<
      Array<{ id: string; name: string }> | { data: Array<{ id: string; name: string }> }
    >(aliceBootstrapToken, '/api/assistants');
    const aliceAssistants = 'data' in aliceAssistantsResponse ? aliceAssistantsResponse.data : aliceAssistantsResponse;
    expect(aliceAssistants).toEqual([
      expect.objectContaining({ id: aliceCapabilityFixture.agentId, name: 'LLM provider smoke agent' }),
    ]);

    await ferrisKeyBrowser?.close();
    ferrisKeyBrowser = null;
    await desktopPage.evaluate(async () => {
      const electronApi = window.electronAPI as DesktopElectronApi;
      await electronApi.setAuthAccessToken(null);
    });
    await desktopPage.reload({ waitUntil: 'domcontentloaded' });
    await expect(desktopPage).toHaveURL(/#\/login$/);
    const aliceOidc = await desktopPage.evaluate(async () => {
      const electronApi = window.electronAPI as DesktopElectronApi;
      return electronApi.startDesktopOidcLogin();
    });
    const aliceFerrisKey = await launchFerrisKeyBrowser();
    ferrisKeyBrowser = aliceFerrisKey.browser;
    await aliceFerrisKey.page.goto(aliceOidc.authorizationUrl, { waitUntil: 'domcontentloaded' });
    await aliceFerrisKey.page.getByLabel('Username').fill(FERRISKEY_ALICE_USERNAME);
    await aliceFerrisKey.page.getByLabel('Password').fill(FERRISKEY_ALICE_PASSWORD!);
    await aliceFerrisKey.page.getByRole('button', { name: /^Login$/ }).click();

    await expect(desktopPage).toHaveURL(/#\/guid$/, { timeout: 20_000 });
    aliceAccessToken = await desktopPage.evaluate(async () => {
      const electronApi = window.electronAPI as DesktopElectronApi;
      return electronApi.getAuthAccessToken();
    });
    expect(aliceAccessToken).toBeTruthy();
    const aliceUiMe = await rustApi<{ user: { username: string } }>(aliceAccessToken!, '/api/me');
    expect(aliceUiMe.user.username).toBe(FERRISKEY_ALICE_USERNAME);
    const authorizedAssistant = desktopPage.getByTestId(`preset-pill-${aliceCapabilityFixture.agentId}`);
    await expect(authorizedAssistant).toBeVisible({ timeout: 20_000 });
    await expect(desktopPage.locator('[data-testid^="preset-pill-"]')).toHaveCount(1);
    await authorizedAssistant.click();
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '19-alice-authorized-guid.png'), fullPage: true });

    await desktopPage
      .getByTestId('guid-input')
      .fill(
        `Alice capability smoke: write 中文工具摘要-你好世界 to ${aliceCapabilityFixture.artifactPath}, read it back, call enterprise_low_risk_smoke_health, then reply exactly alice smoke ok.`
      );
    await desktopPage.getByTestId('guid-send-btn').click();
    await expect(desktopPage).toHaveURL(/#\/conversation\//, { timeout: 20_000 });
    aliceConversationId = desktopPage.url().split('/conversation/')[1] ?? null;
    expect(aliceConversationId).toBeTruthy();
    await expect(desktopPage.getByTestId('message-permission-card')).toBeHidden({ timeout: 20_000 });
    await expect(desktopPage.getByTestId('message-text-content').last()).toContainText(/^\s*alice smoke ok[.!]?\s*$/i, {
      timeout: 120_000,
    });
    await expect(desktopPage.getByTestId('message-permission-card')).toBeHidden();
    const aliceHistory = await rustApi<unknown>(
      aliceAccessToken!,
      `/api/conversations/${aliceConversationId}/messages?${new URLSearchParams({ limit: '200' })}`
    );
    const aliceHistoryText = JSON.stringify(aliceHistory);
    expect(aliceHistoryText).toContain('write_file');
    expect(aliceHistoryText).toContain('read_file');
    expect(aliceHistoryText).toContain('enterprise_low_risk_smoke_health');
    expect(aliceHistoryText).toContain(aliceCapabilityFixture.artifactPath);
    const aliceToolGroup = desktopPage.locator('.tool-group-summary').last();
    await expect(aliceToolGroup.getByText(/^View Steps · (?:[3-9]|\d{2,})$/)).toBeVisible();
    await aliceToolGroup.locator('.tool-group-summary__body').getByText('read_file', { exact: true }).last().click();
    const unicodeToolView = aliceToolGroup.getByTestId('tool-result-view').filter({ hasText: '中文工具摘要-你好世界' });
    await expect(unicodeToolView).toBeVisible();
    await expect(unicodeToolView).not.toContainText(/\\u[0-9a-f]{4}/i);
    await expect(aliceToolGroup.locator('.tool-detail-content')).toHaveCount(0);
    await aliceToolGroup.getByRole('button', { name: 'Technical details' }).last().click();
    const unicodeTechnicalDetail = aliceToolGroup
      .locator('.tool-detail-content')
      .filter({ hasText: '中文工具摘要-你好世界' });
    await expect(unicodeTechnicalDetail).toBeVisible();
    await expect(unicodeTechnicalDetail).not.toContainText(/\\u[0-9a-f]{4}/i);
    await desktopPage.screenshot({ path: path.join(SCREENSHOT_DIR, '20-alice-capability-run.png'), fullPage: true });

    expect(desktopPageErrors).toEqual([]);
  } finally {
    if (accessToken && teamId) {
      await rustApi<unknown>(accessToken, `/api/teams/${teamId}`, { method: 'DELETE' }).catch(() => undefined);
    }
    if (accessToken && projectConversationId) {
      await rustApi<unknown>(accessToken, `/api/conversations/${projectConversationId}`, {
        method: 'DELETE',
      }).catch(() => undefined);
    }
    if (accessToken && productionConversationId) {
      await rustApi<unknown>(accessToken, `/api/conversations/${productionConversationId}`, {
        method: 'DELETE',
      }).catch(() => undefined);
    }
    if (aliceAccessToken && aliceConversationId) {
      await rustApi<unknown>(aliceAccessToken, `/api/conversations/${aliceConversationId}`, {
        method: 'DELETE',
      }).catch(() => undefined);
    }
    if (accessToken && aliceCapabilityFixture) {
      await Promise.all(
        aliceCapabilityFixture.policyBindingIds.map((bindingId) =>
          rustApi<unknown>(accessToken, `/api/v1/policy-bindings/${bindingId}/disable`, {
            method: 'POST',
            body: JSON.stringify({ tenant_id: aliceCapabilityFixture!.tenantId }),
          }).catch(() => undefined)
        )
      );
      await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${aliceCapabilityFixture.agentVersionId}/disable`, {
        method: 'POST',
        body: JSON.stringify({ tenant_id: aliceCapabilityFixture.tenantId }),
      }).catch(() => undefined);
    }
    if (aliceAccessToken) {
      await rustApi<unknown>(aliceAccessToken, '/api/auth/logout', { method: 'POST' }).catch(() => undefined);
    }
    if (aliceBootstrapToken) {
      await rustApi<unknown>(aliceBootstrapToken, '/api/auth/logout', { method: 'POST' }).catch(() => undefined);
    }
    if (accessToken && cronJobId) {
      await rustApi<unknown>(accessToken, `/api/cron/jobs/${cronJobId}`, {
        method: 'DELETE',
      }).catch(() => undefined);
    }
    if (accessToken && cronConversationId) {
      await rustApi<unknown>(accessToken, `/api/conversations/${cronConversationId}`, {
        method: 'DELETE',
      }).catch(() => undefined);
    }
    if (accessToken && approvalConversationId) {
      await rustApi<unknown>(accessToken, `/api/conversations/${approvalConversationId}`, {
        method: 'DELETE',
      }).catch(() => undefined);
    }
    if (accessToken && channelUserId) {
      await rustApi<unknown>(accessToken, '/api/channel/users/revoke', {
        method: 'POST',
        body: JSON.stringify({ user_id: channelUserId }),
      }).catch(() => undefined);
    }
    if (accessToken && channelPairingCode) {
      await rustApi<unknown>(accessToken, '/api/channel/pairings/reject', {
        method: 'POST',
        body: JSON.stringify({ code: channelPairingCode }),
      }).catch(() => undefined);
    }
    if (accessToken) {
      await rustApi<unknown>(accessToken, '/api/channel/plugins/disable', {
        method: 'POST',
        body: JSON.stringify({ plugin_id: 'telegram' }),
      }).catch(() => undefined);
    }
    if (accessToken && hubFixture) {
      await desktopGatewayApi<unknown>(desktopPage, accessToken, '/api/hub/uninstall', 'POST', {
        name: hubFixture.name,
      }).catch(() => undefined);
    }
    if (accessToken && mcpServerId) {
      await desktopGatewayApi<unknown>(desktopPage, accessToken, `/api/mcp/servers/${mcpServerId}`, 'DELETE').catch(
        () => undefined
      );
    }
    if (accessToken && stdioMcpServerId) {
      await desktopGatewayApi<unknown>(
        desktopPage,
        accessToken,
        `/api/mcp/servers/${stdioMcpServerId}`,
        'DELETE'
      ).catch(() => undefined);
    }
    if (mcpFixture) {
      await new Promise<void>((resolve) => mcpFixture!.server.close(() => resolve()));
    }
    if (accessToken && approvalAgentVersionId && tenantId) {
      await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${approvalAgentVersionId}/disable`, {
        method: 'POST',
        body: JSON.stringify({ tenant_id: tenantId }),
      }).catch(() => undefined);
    }
    if (accessToken && cronAgentVersionId && tenantId) {
      await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${cronAgentVersionId}/disable`, {
        method: 'POST',
        body: JSON.stringify({ tenant_id: tenantId }),
      }).catch(() => undefined);
    }
    if (accessToken && productionAgentVersionId && tenantId) {
      await rustApi<unknown>(accessToken, `/api/v1/agent-versions/${productionAgentVersionId}/disable`, {
        method: 'POST',
        body: JSON.stringify({ tenant_id: tenantId }),
      }).catch(() => undefined);
    }
    if (hubFixture) {
      await fs.rm(hubFixture.tempDir, { recursive: true, force: true });
    }
    if (projectWorkspacePath) {
      await fs.rm(projectWorkspacePath, { recursive: true, force: true });
    }
    if (accessToken) {
      await desktopPage
        .evaluate(async (token) => {
          const electronApi = window.electronAPI as DesktopElectronApi;
          await electronApi.setAuthAccessToken(token);
          window.location.hash = '#/guid';
        }, accessToken)
        .catch(() => undefined);
    }
    await ferrisKeyBrowser?.close();
    // Do not call browser.close() for a CDP connection: that terminates the shared Electron process.
  }
});
