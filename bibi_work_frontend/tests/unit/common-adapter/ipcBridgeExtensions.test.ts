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

describe('ipcBridge extensions adapter', () => {
  beforeEach(() => {
    vi.resetModules();
    httpBridgeMocks.calls.length = 0;
    httpBridgeMocks.responses.clear();
    httpBridgeMocks.wsEmitter.mockClear();
  });

  it('reads extension governance projections from aggregate extension endpoints', async () => {
    httpBridgeMocks.responses.set('/api/extensions', [
      {
        name: 'hello-extension',
        display_name: 'Hello Extension',
        version: '1.0.0',
        source: 'hub',
        enabled: true,
        installed: true,
        install_status: 'installed',
      },
    ]);
    httpBridgeMocks.responses.set('/api/extensions/settings-tabs', [
      {
        id: 'hello-settings',
        label: 'Hello Settings',
        url: '/api/extensions/static/hello-extension/settings/index.html',
        order: 10,
        extensionName: 'hello-extension',
      },
    ]);
    httpBridgeMocks.responses.set('/api/extensions/agent-activity', {
      generatedAt: 42,
      totalConversations: 1,
      runningConversations: 0,
      agents: [],
    });
    const { extensions } = await import('@/common/adapter/ipcBridge');

    await expect(extensions.getLoadedExtensions.invoke()).resolves.toEqual([
      expect.objectContaining({
        name: 'hello-extension',
        enabled: true,
        installed: true,
        install_status: 'installed',
      }),
    ]);
    await expect(extensions.getSettingsTabs.invoke()).resolves.toEqual([
      expect.objectContaining({
        id: 'hello-settings',
        extensionName: 'hello-extension',
      }),
    ]);
    await expect(extensions.getAgentActivitySnapshot.invoke()).resolves.toMatchObject({
      generatedAt: 42,
      totalConversations: 1,
    });

    expect(httpBridgeMocks.calls).toEqual([
      { method: 'GET', path: '/api/extensions', body: undefined },
      { method: 'GET', path: '/api/extensions/settings-tabs', body: undefined },
      { method: 'GET', path: '/api/extensions/agent-activity', body: undefined },
    ]);
  });

  it('posts extension governance mutations and diagnostics without path-encoded names', async () => {
    httpBridgeMocks.responses.set('/api/extensions/permissions', [
      {
        name: 'filesystem',
        description: 'Read local extension files',
        level: 'moderate',
        granted: true,
      },
    ]);
    httpBridgeMocks.responses.set('/api/extensions/risk-level', 'moderate');
    httpBridgeMocks.responses.set('/api/extensions/i18n', {
      'hello-extension': { settings: { title: 'Hello' } },
    });
    const { extensions } = await import('@/common/adapter/ipcBridge');

    await extensions.enableExtension.invoke({ name: 'hello-extension' });
    await extensions.disableExtension.invoke({ name: 'hello-extension', reason: 'policy' });
    await expect(extensions.getPermissions.invoke({ name: 'hello-extension' })).resolves.toEqual([
      expect.objectContaining({ name: 'filesystem', level: 'moderate' }),
    ]);
    await expect(extensions.getRiskLevel.invoke({ name: 'hello-extension' })).resolves.toBe('moderate');
    await expect(extensions.getExtI18nForLocale.invoke({ locale: 'zh-CN' })).resolves.toEqual({
      'hello-extension': { settings: { title: 'Hello' } },
    });

    expect(httpBridgeMocks.calls).toEqual([
      { method: 'POST', path: '/api/extensions/enable', body: { name: 'hello-extension' } },
      {
        method: 'POST',
        path: '/api/extensions/disable',
        body: { name: 'hello-extension', reason: 'policy' },
      },
      { method: 'POST', path: '/api/extensions/permissions', body: { name: 'hello-extension' } },
      { method: 'POST', path: '/api/extensions/risk-level', body: { name: 'hello-extension' } },
      { method: 'POST', path: '/api/extensions/i18n', body: { locale: 'zh-CN' } },
    ]);
    expect(httpBridgeMocks.wsEmitter).toHaveBeenCalledWith('extensions.state-changed');
  });
});
