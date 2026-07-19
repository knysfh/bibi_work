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
        return undefined as Data;
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

describe('ipcBridge provider adapter', () => {
  beforeEach(() => {
    vi.resetModules();
    httpBridgeMocks.calls.length = 0;
  });

  it('routes provider model health checks to the provider compat endpoint', async () => {
    const { mode, acpConversation } = await import('@/common/adapter/ipcBridge');

    await mode.testProvider.invoke({
      provider_id: 'provider-1',
      model: 'gpt-5',
    });
    await acpConversation.checkProviderHealth.invoke({
      provider_id: 'provider-1',
      model: 'gpt-5-mini',
    });

    expect(httpBridgeMocks.calls).toEqual([
      {
        method: 'POST',
        path: '/api/providers/provider-1/test',
        body: { model: 'gpt-5' },
      },
      {
        method: 'POST',
        path: '/api/providers/provider-1/test',
        body: { model: 'gpt-5-mini' },
      },
    ]);
  });
});
