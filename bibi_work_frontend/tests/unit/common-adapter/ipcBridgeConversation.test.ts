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
        return true as Data;
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

describe('ipcBridge conversation adapter', () => {
  beforeEach(() => {
    httpBridgeMocks.calls.length = 0;
  });

  it('deletes conversations through the standard conversation endpoint', async () => {
    const { conversation } = await import('@/common/adapter/ipcBridge');

    await conversation.remove.invoke({ id: 'conv-1' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'DELETE',
      path: '/api/conversations/conv-1',
      body: undefined,
    });
  });

  it('posts active lease heartbeats to the Rust conversation lease endpoint without a body', async () => {
    const { conversation } = await import('@/common/adapter/ipcBridge');

    await conversation.activeLease.invoke({ conversation_id: 'conv-lease' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/conversations/conv-lease/active-lease',
      body: undefined,
    });
  });

  it('posts BiWork sendMessage payload using the Rust run gateway contract', async () => {
    const { conversation } = await import('@/common/adapter/ipcBridge');

    await conversation.sendMessage.invoke({
      conversation_id: 'conv-send',
      input: 'hello',
      files: ['/workspace/a.md'],
      loading_id: 'loading-1',
      inject_skills: ['skill.read_file'],
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/conversations/conv-send/messages',
      body: {
        content: 'hello',
        files: ['/workspace/a.md'],
        loading_id: 'loading-1',
        inject_skills: ['skill.read_file'],
      },
    });
  });

  it('loads slash commands from the Rust conversation command endpoint', async () => {
    const { conversation } = await import('@/common/adapter/ipcBridge');

    await conversation.getSlashCommands.invoke({ conversation_id: 'conv-slash' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'GET',
      path: '/api/conversations/conv-slash/slash-commands',
      body: undefined,
    });
  });

  it('loads paged conversation history with cursor and content-mode query params', async () => {
    const { database } = await import('@/common/adapter/ipcBridge');

    await database.getConversationMessages.invoke({
      conversation_id: 'conv-history',
      limit: 25,
      before: 'cursor-before',
      anchor_message_id: 'msg-anchor',
      content_mode: 'compact',
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'GET',
      path: '/api/conversations/conv-history/messages?limit=25&before=cursor-before&anchor_message_id=msg-anchor&content_mode=compact',
      body: undefined,
    });
  });
});
