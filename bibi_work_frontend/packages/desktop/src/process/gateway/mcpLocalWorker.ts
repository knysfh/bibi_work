import { callLocalStdioMcpTool, LocalMcpError } from './mcpLocal';

const IDLE_POLL_MS = 500;
const ERROR_RETRY_MS = 2_000;

type LocalRuntimeWorkItem = {
  id: string;
  tenant_id: string;
  command: {
    protocol?: unknown;
    kind?: unknown;
    transport?: unknown;
    tool?: {
      name?: unknown;
      arguments?: unknown;
    };
  };
};

type WorkerOptions = {
  backendBaseUrl: string;
  getAccessToken: () => string | null;
  fetchImpl?: typeof fetch;
};

export type LocalMcpWorker = {
  stop: () => void;
};

export async function executeLocalMcpWorkItem(work: LocalRuntimeWorkItem): Promise<Record<string, unknown>> {
  if (work.command.protocol !== 'local_runtime.v1' || work.command.kind !== 'mcp_stdio') {
    throw new LocalMcpError('MCP_LOCAL_WORK_INVALID', 'Unsupported local MCP work item');
  }
  return callLocalStdioMcpTool(work.command.transport, work.command.tool?.name, work.command.tool?.arguments ?? {});
}

export function startLocalMcpWorker(options: WorkerOptions): LocalMcpWorker {
  const fetchImpl = options.fetchImpl ?? fetch;
  const backendBaseUrl = options.backendBaseUrl.replace(/\/$/, '');
  let stopped = false;
  let cachedToken: string | null = null;
  let cachedTenantId: string | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const schedule = (delay: number): void => {
    if (stopped) return;
    timer = setTimeout(() => void poll(), delay);
  };

  const authenticatedFetch = (path: string, token: string, init?: RequestInit): Promise<Response> =>
    fetchImpl(`${backendBaseUrl}${path}`, {
      ...init,
      headers: {
        ...init?.headers,
        Authorization: `Bearer ${token}`,
        Accept: 'application/json',
      },
    });

  const tenantIdForToken = async (token: string): Promise<string> => {
    if (cachedToken === token && cachedTenantId) return cachedTenantId;
    const response = await authenticatedFetch('/api/v1/me', token);
    if (!response.ok) throw new Error(`local MCP worker identity lookup returned HTTP ${response.status}`);
    const body = (await response.json()) as { tenant_id?: unknown };
    if (typeof body.tenant_id !== 'string' || !body.tenant_id) {
      throw new Error('local MCP worker identity response did not contain tenant_id');
    }
    cachedToken = token;
    cachedTenantId = body.tenant_id;
    return body.tenant_id;
  };

  const complete = async (
    token: string,
    work: LocalRuntimeWorkItem,
    status: 'completed' | 'failed',
    result: Record<string, unknown> | null,
    error: string | null
  ): Promise<void> => {
    const response = await authenticatedFetch(
      `/api/v1/local-exec/requests/${encodeURIComponent(work.id)}/complete`,
      token,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          tenant_id: work.tenant_id,
          status,
          result,
          error,
        }),
      }
    );
    if (!response.ok) throw new Error(`local MCP work completion returned HTTP ${response.status}`);
  };

  const poll = async (): Promise<void> => {
    if (stopped) return;
    const token = options.getAccessToken();
    if (!token) {
      cachedToken = null;
      cachedTenantId = null;
      schedule(IDLE_POLL_MS);
      return;
    }
    try {
      const tenantId = await tenantIdForToken(token);
      const response = await authenticatedFetch(
        `/api/v1/local-exec/requests/next?tenant_id=${encodeURIComponent(tenantId)}&kind=mcp_stdio`,
        token
      );
      if (response.status === 401 || response.status === 403) {
        cachedToken = null;
        cachedTenantId = null;
        schedule(ERROR_RETRY_MS);
        return;
      }
      if (!response.ok) throw new Error(`local MCP work poll returned HTTP ${response.status}`);
      const work = (await response.json()) as LocalRuntimeWorkItem | null;
      if (!work) {
        schedule(IDLE_POLL_MS);
        return;
      }
      try {
        const result = await executeLocalMcpWorkItem(work);
        await complete(token, work, 'completed', result, null);
      } catch (error) {
        const candidate = error instanceof LocalMcpError ? error : null;
        const code = candidate?.code ?? 'MCP_LOCAL_EXECUTION_FAILED';
        const message = error instanceof Error ? error.message : String(error);
        await complete(token, work, 'failed', null, `${code}: ${message}`.slice(0, 2_000));
      }
      schedule(0);
    } catch (error) {
      console.error('[BiWork] Local MCP worker poll failed:', error);
      schedule(ERROR_RETRY_MS);
    }
  };

  schedule(0);
  return {
    stop: (): void => {
      stopped = true;
      if (timer) clearTimeout(timer);
      timer = null;
    },
  };
}
