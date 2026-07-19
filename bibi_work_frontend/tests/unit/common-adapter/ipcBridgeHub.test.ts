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
  const responses = new Map<string, unknown>();
  const provider =
    (method: HttpCall['method']) =>
    <Data, Params = undefined>(path: string | ((params: Params) => string), mapBody?: (params: Params) => unknown) => ({
      provider: vi.fn(),
      invoke: vi.fn(async (params?: Params) => {
        const resolvedPath = typeof path === 'function' ? path(params as Params) : path;
        calls.push({
          method,
          path: resolvedPath,
          body: mapBody ? mapBody(params as Params) : method === 'GET' || method === 'DELETE' ? undefined : params,
        });
        return responses.get(resolvedPath) as Data;
      }),
    });
  const emitter = () => ({ on: vi.fn(() => vi.fn()), emit: vi.fn() });

  return {
    calls,
    responses,
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

describe('ipcBridge hub adapter', () => {
  beforeEach(() => {
    httpBridgeMocks.calls.length = 0;
    httpBridgeMocks.responses.clear();
  });

  it('preserves extension governance sync summaries on hub mutations', async () => {
    const installResult = {
      name: 'ext-codex',
      status: 'install_failed',
      error: 'Local hub extension installer is not attached.',
      governanceSync: { synced: 1, contributions: 2 },
    };
    httpBridgeMocks.responses.set('/api/hub/install', installResult);
    httpBridgeMocks.responses.set('/api/hub/retry-install', installResult);
    httpBridgeMocks.responses.set('/api/hub/uninstall', {
      name: 'ext-codex',
      status: 'not_installed',
      governanceSync: { synced: 1, contributions: 0 },
    });
    httpBridgeMocks.responses.set('/api/hub/update', {
      name: 'ext-codex',
      status: 'installed',
      governanceSync: { synced: 1, contributions: 2 },
    });
    const { hub } = await import('@/common/adapter/ipcBridge');

    await expect(hub.install.invoke({ name: 'ext-codex' })).resolves.toEqual(installResult);
    await expect(hub.retryInstall.invoke({ name: 'ext-codex' })).resolves.toEqual(installResult);
    await expect(hub.uninstall.invoke({ name: 'ext-codex' })).resolves.toMatchObject({
      name: 'ext-codex',
      status: 'not_installed',
      governanceSync: { synced: 1, contributions: 0 },
    });
    await expect(hub.update.invoke({ name: 'ext-codex' })).resolves.toMatchObject({
      name: 'ext-codex',
      status: 'installed',
      governanceSync: { synced: 1, contributions: 2 },
    });

    expect(httpBridgeMocks.calls).toEqual([
      { method: 'POST', path: '/api/hub/install', body: { name: 'ext-codex' } },
      { method: 'POST', path: '/api/hub/retry-install', body: { name: 'ext-codex' } },
      { method: 'POST', path: '/api/hub/uninstall', body: { name: 'ext-codex' } },
      { method: 'POST', path: '/api/hub/update', body: { name: 'ext-codex' } },
    ]);
  });
});
