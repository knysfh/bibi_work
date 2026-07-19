/**
 * @vitest-environment node
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

type HttpCall = {
  method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  path: string;
  body?: unknown;
};

const httpBridgeMocks = vi.hoisted(() => {
  const calls: HttpCall[] = [];
  const provider =
    (method: HttpCall['method']) =>
    <Data, Params = undefined>(path: string | ((params: Params) => string), mapBody?: (params: Params) => unknown) => ({
      provider: vi.fn(),
      invoke: vi.fn(async (params?: Params) => {
        const resolvedPath = typeof path === 'function' ? path(params as Params) : path;
        const requestBody =
          mapBody && params !== undefined
            ? mapBody(params as Params)
            : method === 'GET' || method === 'DELETE'
              ? undefined
              : params;
        calls.push({
          method,
          path: resolvedPath,
          body: requestBody,
        });
        return { conversation_id: 'conv-1', run_id: 'run-1' } as Data;
      }),
    });
  const emitter = () => ({ on: vi.fn(() => vi.fn()), emit: vi.fn() });

  return {
    calls,
    httpGet: provider('GET'),
    httpPost: provider('POST'),
    httpPut: provider('PUT'),
    httpPatch: provider('PATCH'),
    httpDelete: provider('DELETE'),
    httpRequest: vi.fn(),
    stubProvider: vi.fn((name: string, defaultValue: unknown) => ({
      provider: vi.fn(),
      invoke: vi.fn(async () => defaultValue),
    })),
    withResponseMap: vi.fn(
      (
        inner: { provider: unknown; invoke: (params?: unknown) => Promise<unknown> },
        map: (raw: unknown) => unknown
      ) => ({
        provider: inner.provider,
        invoke: vi.fn(async (params?: unknown) => map(await inner.invoke(params))),
      })
    ),
    wsEmitter: vi.fn(emitter),
    wsMappedEmitter: vi.fn(emitter),
    stubEmitter: vi.fn(emitter),
  };
});

vi.mock('@/common/adapter/httpBridge', () => httpBridgeMocks);

vi.mock('@office-ai/platform', () => ({
  bridge: {
    buildProvider: vi.fn(() => ({
      provider: vi.fn(),
      invoke: vi.fn(),
    })),
    buildEmitter: vi.fn(() => ({
      on: vi.fn(() => vi.fn()),
      emit: vi.fn(),
    })),
  },
}));

describe('ipcBridge cron adapter', () => {
  beforeEach(() => {
    httpBridgeMocks.calls.length = 0;
  });

  it('runNow calls POST /api/cron/jobs/{job_id}/run and exposes run_id', async () => {
    const { cron } = await import('@/common/adapter/ipcBridge');
    type RunNowResult = Awaited<ReturnType<typeof cron.runNow.invoke>>;
    const typeCheck: RunNowResult extends { conversation_id: string; run_id: string } ? true : never = true;

    const result = await cron.runNow.invoke({ job_id: 'job-1' });

    expect(typeCheck).toBe(true);
    expect(result).toEqual({ conversation_id: 'conv-1', run_id: 'run-1' });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/cron/jobs/job-1/run',
      body: undefined,
    });
  });

  it('lists cron jobs for a conversation through the Rust query contract', async () => {
    const { cron } = await import('@/common/adapter/ipcBridge');

    await cron.listJobsByConversation.invoke({ conversation_id: 'conv with spaces' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'GET',
      path: '/api/cron/jobs?conversation_id=conv%20with%20spaces',
      body: undefined,
    });
  });

  it('creates cron jobs with the BiWork scheduled job payload', async () => {
    const { cron } = await import('@/common/adapter/ipcBridge');
    const payload = {
      name: 'Daily summary',
      description: 'Summarize every morning',
      schedule: { kind: 'cron' as const, expr: '0 9 * * *', tz: 'Asia/Shanghai', description: 'Daily at 9' },
      prompt: 'Summarize the workspace',
      conversation_id: 'conv-1',
      conversation_title: 'Workspace',
      created_by: 'user' as const,
      execution_mode: 'new_conversation' as const,
      agent_config: {
        name: 'General',
        assistant_id: 'assistant_general',
        model_id: 'model-1',
        workspace: '/workspace/demo',
      },
    };

    await cron.addJob.invoke(payload);

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/cron/jobs',
      body: payload,
    });
  });

  it('updates cron jobs by flattening BiWork form updates for the Rust scheduled job contract', async () => {
    const { cron } = await import('@/common/adapter/ipcBridge');
    const updates = {
      enabled: false,
      schedule: { kind: 'interval' as const, seconds: 3600, description: 'Hourly' },
      target: { execution_mode: 'existing' as const },
      state: { max_retries: 1 },
    };

    await cron.updateJob.invoke({ job_id: 'job-2', updates });

    const call = httpBridgeMocks.calls.find((entry) => entry.method === 'PUT' && entry.path === '/api/cron/jobs/job-2');
    expect(call).toBeTruthy();
    expect(call?.body).toMatchObject({
      enabled: false,
      schedule: { kind: 'interval', seconds: 3600, description: 'Hourly' },
      execution_mode: 'existing',
      max_retries: 1,
    });
    const body = call?.body as Record<string, unknown>;
    expect(body.target).toBeUndefined();
    expect(body.message).toBeUndefined();
  });

  it('deletes cron jobs through the Rust scheduled job endpoint', async () => {
    const { cron } = await import('@/common/adapter/ipcBridge');

    await cron.removeJob.invoke({ job_id: 'job-3' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'DELETE',
      path: '/api/cron/jobs/job-3',
      body: undefined,
    });
  });
});
