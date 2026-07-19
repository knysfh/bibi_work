import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import type { RequestPermissionRequest, RequestPermissionResponse } from '@agentclientprotocol/sdk';
import { SpanKind } from '@opentelemetry/api';
import { injectDesktopTraceHeaders, recordDesktopSpanError, withDesktopSpan } from '../../telemetry/desktopTelemetry';
import { AcpRuntimeCancelledError, executeAcpRuntime, type AcpRuntimeConfig } from './runtime';
import type { PlatformRunEvent } from './events';

const IDLE_POLL_MS = 500;
const ERROR_RETRY_MS = 2_000;

type AcpWorkItem = {
  id: string;
  tenant_id: string;
  timeout_ms: number;
  command: {
    protocol?: unknown;
    kind?: unknown;
    run_id?: unknown;
    trace_context?: unknown;
    trace_id?: unknown;
    input?: unknown;
    runtime?: unknown;
  };
};

type WorkerOptions = {
  backendBaseUrl: string;
  getAccessToken: () => string | null;
  runsDirectory: string;
  fetchImpl?: typeof fetch;
};

export type DesktopAcpWorker = { stop: () => void };

function promptFromInput(input: unknown): string {
  if (!input || typeof input !== 'object') throw new Error('ACP work item input is invalid');
  const messages = (input as Record<string, unknown>).messages;
  if (!Array.isArray(messages)) throw new Error('ACP work item messages are required');
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (!message || typeof message !== 'object') continue;
    const content = (message as Record<string, unknown>).content;
    if (typeof content === 'string' && content.trim()) return content;
  }
  throw new Error('ACP work item prompt is empty');
}

export function startDesktopAcpWorker(options: WorkerOptions): DesktopAcpWorker {
  const fetchImpl = options.fetchImpl ?? fetch;
  const baseUrl = options.backendBaseUrl.replace(/\/$/, '');
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let cachedToken: string | null = null;
  let cachedTenantId: string | null = null;
  const schedule = (delay: number): void => {
    if (!stopped) timer = setTimeout(() => void poll(), delay);
  };
  const request = (path: string, token: string, init?: RequestInit): Promise<Response> =>
    fetchImpl(`${baseUrl}${path}`, {
      ...init,
      headers: injectDesktopTraceHeaders({
        ...(init?.headers as Record<string, string> | undefined),
        Authorization: `Bearer ${token}`,
        Accept: 'application/json',
      }),
    });
  const tenantIdForToken = async (token: string): Promise<string> => {
    if (cachedToken === token && cachedTenantId) return cachedTenantId;
    const response = await request('/api/v1/me', token);
    if (!response.ok) throw new Error(`ACP worker identity lookup returned HTTP ${response.status}`);
    const body = (await response.json()) as { tenant_id?: unknown };
    if (typeof body.tenant_id !== 'string' || !body.tenant_id)
      throw new Error('ACP worker identity is missing tenant_id');
    cachedToken = token;
    cachedTenantId = body.tenant_id;
    return body.tenant_id;
  };
  const postEvents = async (token: string, work: AcpWorkItem, events: PlatformRunEvent[]): Promise<void> => {
    const response = await request(`/api/v1/local-exec/requests/${encodeURIComponent(work.id)}/events`, token, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ tenant_id: work.tenant_id, events }),
    });
    if (!response.ok) throw new Error(`ACP event ingestion returned HTTP ${response.status}`);
  };
  const complete = async (
    token: string,
    work: AcpWorkItem,
    status: 'completed' | 'failed',
    result: unknown,
    error: string | null
  ) => {
    const response = await request(`/api/v1/local-exec/requests/${encodeURIComponent(work.id)}/complete`, token, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ tenant_id: work.tenant_id, status, result, error }),
    });
    if (!response.ok) throw new Error(`ACP work completion returned HTTP ${response.status}`);
  };
  const requestStatus = async (token: string, work: AcpWorkItem): Promise<string> => {
    const response = await request(
      `/api/v1/local-exec/requests/${encodeURIComponent(work.id)}/status?tenant_id=${encodeURIComponent(work.tenant_id)}`,
      token
    );
    if (!response.ok) throw new Error(`ACP work status returned HTTP ${response.status}`);
    const body = (await response.json()) as { status?: unknown };
    if (typeof body.status !== 'string') throw new Error('ACP work status response is invalid');
    return body.status;
  };
  const requestPermission = async (
    token: string,
    work: AcpWorkItem,
    permission: RequestPermissionRequest,
    signal: AbortSignal
  ): Promise<RequestPermissionResponse> => {
    const createResponse = await request(
      `/api/v1/local-exec/requests/${encodeURIComponent(work.id)}/permissions`,
      token,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          tenant_id: work.tenant_id,
          permission_id: permission.toolCall.toolCallId,
          title: permission.toolCall.title ?? permission.toolCall.toolCallId,
          options: permission.options,
          tool_call: permission.toolCall,
        }),
      }
    );
    if (!createResponse.ok) throw new Error(`ACP permission creation returned HTTP ${createResponse.status}`);
    const created = (await createResponse.json()) as { approval_id?: unknown; status?: unknown };
    if (typeof created.approval_id !== 'string') throw new Error('ACP permission response is missing approval_id');
    while (!signal.aborted) {
      const response = await request(
        `/api/v1/local-exec/requests/${encodeURIComponent(work.id)}/permissions/${encodeURIComponent(created.approval_id)}?tenant_id=${encodeURIComponent(work.tenant_id)}`,
        token
      );
      if (!response.ok) throw new Error(`ACP permission status returned HTTP ${response.status}`);
      const status = (await response.json()) as { status?: unknown; selected_option_id?: unknown };
      if (status.status === 'approved' || status.status === 'rejected') {
        return typeof status.selected_option_id === 'string'
          ? { outcome: { outcome: 'selected', optionId: status.selected_option_id } }
          : { outcome: { outcome: 'cancelled' } };
      }
      if (status.status === 'cancelled') return { outcome: { outcome: 'cancelled' } };
      await new Promise((resolve) => setTimeout(resolve, 500));
    }
    return { outcome: { outcome: 'cancelled' } };
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
      const response = await request(
        `/api/v1/local-exec/requests/next?tenant_id=${encodeURIComponent(tenantId)}&kind=biwork_cli`,
        token
      );
      if (!response.ok) throw new Error(`ACP work poll returned HTTP ${response.status}`);
      const work = (await response.json()) as AcpWorkItem | null;
      if (!work) {
        schedule(IDLE_POLL_MS);
        return;
      }
      const runId = typeof work.command.run_id === 'string' ? work.command.run_id : '';
      const traceContext =
        work.command.trace_context && typeof work.command.trace_context === 'object'
          ? (work.command.trace_context as Record<string, unknown>)
          : undefined;
      await withDesktopSpan(
        'acp.run',
        {
          attributes: {
            'biwork.local_exec_request_id': work.id,
            'biwork.run_id': runId || 'missing',
            'biwork.trace_id': typeof work.command.trace_id === 'string' ? work.command.trace_id : 'missing',
          },
          kind: SpanKind.CONSUMER,
          parentHeaders: traceContext,
        },
        async (span) => {
          try {
            if (work.command.protocol !== 'biwork_acp.v1' || work.command.kind !== 'biwork_cli' || !runId) {
              throw new Error('Unsupported ACP work item');
            }
            const cwd = join(options.runsDirectory, runId);
            await mkdir(cwd, { recursive: true });
            const controller = new AbortController();
            let checkingStatus = false;
            const statusTimer = setInterval(() => {
              if (checkingStatus || controller.signal.aborted) return;
              checkingStatus = true;
              void requestStatus(token, work)
                .then((status) => {
                  if (status === 'cancelled') controller.abort();
                })
                .catch((error) => console.error('[BiWork] ACP cancellation status check failed:', error))
                .finally(() => {
                  checkingStatus = false;
                });
            }, 500);
            let result;
            try {
              result = await executeAcpRuntime({
                runId,
                prompt: promptFromInput(work.command.input),
                cwd,
                runtime: work.command.runtime as AcpRuntimeConfig,
                timeoutMs: work.timeout_ms,
                emit: (events) => postEvents(token, work, events),
                signal: controller.signal,
                traceHeaders: injectDesktopTraceHeaders({}),
                requestPermission: (permission) => requestPermission(token, work, permission, controller.signal),
              });
            } finally {
              clearInterval(statusTimer);
            }
            const succeeded = result.stop_reason !== 'refusal' && result.stop_reason !== 'cancelled';
            await complete(
              token,
              work,
              succeeded ? 'completed' : 'failed',
              succeeded ? result : null,
              succeeded ? null : result.stop_reason
            );
          } catch (error) {
            if (error instanceof AcpRuntimeCancelledError) {
              span.setAttribute('biwork.cancelled', true);
              return;
            }
            recordDesktopSpanError(span, error);
            const message = (error instanceof Error ? error.message : String(error)).slice(0, 2_000);
            await postEvents(token, work, [
              { event_id: `desktop-acp.${runId}.failed`, type: 'run.failed', payload: { error: message } },
            ]);
            await complete(token, work, 'failed', null, message);
          }
        }
      );
      schedule(0);
    } catch (error) {
      console.error('[BiWork] Desktop ACP worker poll failed:', error);
      schedule(ERROR_RETRY_MS);
    }
  };
  schedule(0);
  return {
    stop: () => {
      stopped = true;
      if (timer) clearTimeout(timer);
      timer = null;
    },
  };
}
