import { BrowserExecutionError, type BrowserCommand, type BrowserSessionManager } from './browserSessionManager';

const IDLE_POLL_MS = 500;
const ERROR_RETRY_MS = 2_000;

export type BrowserWorkItem = {
  id: string;
  tenant_id: string;
  command: BrowserCommand;
};

export type BrowserWorkCompletion = {
  status: 'completed' | 'failed';
  result: Record<string, unknown> | null;
  error: string | null;
};

type BrowserWorkerOptions = {
  backendBaseUrl: string;
  getAccessToken: () => string | null;
  manager: BrowserSessionManager;
  fetchImpl?: typeof fetch;
};

export type BrowserWorker = {
  stop: () => void;
};

export async function executeBrowserWorkItem(
  manager: BrowserSessionManager,
  work: BrowserWorkItem
): Promise<Record<string, unknown>> {
  return manager.execute(work.command);
}

export async function resolveBrowserWorkItem(
  manager: BrowserSessionManager,
  work: BrowserWorkItem
): Promise<BrowserWorkCompletion> {
  try {
    return {
      status: 'completed',
      result: await executeBrowserWorkItem(manager, work),
      error: null,
    };
  } catch (error) {
    const candidate = error instanceof BrowserExecutionError ? error : null;
    const code = candidate?.code ?? 'BROWSER_EXECUTION_FAILED';
    const message = error instanceof Error ? error.message : String(error);
    const errorText = `${code}: ${message}`.slice(0, 2_000);
    if (!candidate?.retryable) {
      return { status: 'failed', result: null, error: errorText };
    }

    let recoverySnapshot: Record<string, unknown> | null = null;
    let pageRestored = false;
    try {
      recoverySnapshot = await manager.execute({
        ...work.command,
        action: { name: 'snapshot' },
      });
    } catch (snapshotError) {
      const snapshotCandidate = snapshotError instanceof BrowserExecutionError ? snapshotError : null;
      if (snapshotCandidate?.code === 'BROWSER_PAGE_NOT_FOUND') {
        try {
          recoverySnapshot = await manager.recoverSession(String(work.command.session_id));
          pageRestored = true;
        } catch {
          // A missing in-memory session requires browser_open with a URL from conversation context.
        }
      }
    }
    const actionName = typeof work.command.action?.name === 'string' ? work.command.action.name : 'unknown';
    const sessionId = typeof work.command.session_id === 'string' ? work.command.session_id : null;
    if (pageRestored && actionName === 'snapshot' && recoverySnapshot) {
      return {
        status: 'completed',
        result: {
          ...recoverySnapshot,
          recovered: true,
          recovery_action: 'page_restored',
        },
        error: null,
      };
    }
    const sessionMissing =
      code === 'BROWSER_SESSION_NOT_FOUND' || (code === 'BROWSER_PAGE_NOT_FOUND' && !recoverySnapshot);
    const recoveryInstruction = recoverySnapshot
      ? pageRestored
        ? 'The browser page was restored from the persistent profile. Continue from recovery_snapshot using only its fresh refs.'
        : 'Inspect recovery_snapshot, choose a different valid action or ref, and continue. Do not repeat the unchanged failing action.'
      : sessionMissing
        ? 'The previous browser environment no longer exists. Use the conversation and current task context to call browser_open with the relevant prior URL, then inspect its fresh snapshot and continue the workflow.'
        : 'Take a new browser snapshot or reopen the relevant URL before choosing another action. Do not repeat the unchanged failing action.';
    return {
      status: 'failed',
      result: {
        kind: 'browser',
        action: actionName,
        session_id: sessionId,
        status: 'failed',
        retryable: true,
        error: { code, message },
        recovery_action: pageRestored
          ? 'page_restored'
          : sessionMissing
            ? 'browser_open_required'
            : 'snapshot_required',
        recovery_instruction: recoveryInstruction,
        recovery_snapshot: recoverySnapshot,
      },
      error: errorText,
    };
  }
}

export function startBrowserWorker(options: BrowserWorkerOptions): BrowserWorker {
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
    if (!response.ok) throw new Error(`browser worker identity lookup returned HTTP ${response.status}`);
    const body = (await response.json()) as { tenant_id?: unknown };
    if (typeof body.tenant_id !== 'string' || !body.tenant_id) {
      throw new Error('browser worker identity response did not contain tenant_id');
    }
    cachedToken = token;
    cachedTenantId = body.tenant_id;
    return body.tenant_id;
  };

  const complete = async (
    token: string,
    work: BrowserWorkItem,
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
        body: JSON.stringify({ tenant_id: work.tenant_id, status, result, error }),
      }
    );
    if (!response.ok) throw new Error(`browser work completion returned HTTP ${response.status}`);
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
        `/api/v1/local-exec/requests/next?tenant_id=${encodeURIComponent(tenantId)}&kind=browser`,
        token
      );
      if (response.status === 401 || response.status === 403) {
        cachedToken = null;
        cachedTenantId = null;
        schedule(ERROR_RETRY_MS);
        return;
      }
      if (!response.ok) throw new Error(`browser work poll returned HTTP ${response.status}`);
      const work = (await response.json()) as BrowserWorkItem | null;
      if (!work) {
        schedule(IDLE_POLL_MS);
        return;
      }
      const completion = await resolveBrowserWorkItem(options.manager, work);
      await complete(token, work, completion.status, completion.result, completion.error);
      schedule(0);
    } catch (error) {
      console.error('[BiWork] Browser worker poll failed:', error);
      schedule(ERROR_RETRY_MS);
    }
  };

  schedule(0);
  return {
    stop: (): void => {
      stopped = true;
      if (timer) clearTimeout(timer);
      timer = null;
      void options.manager.closeAll();
    },
  };
}
