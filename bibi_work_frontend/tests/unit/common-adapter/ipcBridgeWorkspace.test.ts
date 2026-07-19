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
        calls.push({
          method,
          path: resolvedPath,
          body: mapBody && params !== undefined ? mapBody(params as Params) : undefined,
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

describe('ipcBridge workspace adapter', () => {
  beforeEach(() => {
    vi.resetModules();
    httpBridgeMocks.calls.length = 0;
    httpBridgeMocks.httpRequest.mockReset();
  });

  it('maps workspace search results and emits the legacy search response event', async () => {
    httpBridgeMocks.httpRequest.mockResolvedValue([
      {
        name: 'report.md',
        type: 'file',
        full_path: '/workspace/docs/report.md',
        relative_path: 'docs/report.md',
      },
    ]);
    const { conversation } = await import('@/common/adapter/ipcBridge');
    const events: Array<{ file: number; dir: number; match?: { fullPath: string; relativePath: string } }> = [];

    const unsubscribe = conversation.responseSearchWorkSpace.provider((data) => {
      events.push(data);
    });
    const result = await conversation.getWorkspace.invoke({
      conversation_id: 'conv-1',
      workspace: '/workspace',
      path: '/workspace',
      search: 'report',
    });

    expect(httpBridgeMocks.httpRequest).toHaveBeenCalledWith(
      'GET',
      '/api/conversations/conv-1/workspace?path=.&search=report'
    );
    expect(result[0]?.children?.[0]).toMatchObject({
      fullPath: '/workspace/docs/report.md',
      relativePath: 'docs/report.md',
      isFile: true,
    });
    expect(events).toEqual([
      {
        file: 1,
        dir: 0,
        match: expect.objectContaining({
          fullPath: '/workspace/docs/report.md',
          relativePath: 'docs/report.md',
        }),
      },
    ]);

    unsubscribe();
    await conversation.getWorkspace.invoke({
      conversation_id: 'conv-1',
      workspace: '/workspace',
      path: '/workspace',
      search: 'report',
    });
    expect(events).toHaveLength(1);
  });
});
