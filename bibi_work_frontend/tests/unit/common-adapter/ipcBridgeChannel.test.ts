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

describe('ipcBridge channel adapter', () => {
  beforeEach(() => {
    httpBridgeMocks.calls.length = 0;
    httpBridgeMocks.responses.clear();
  });

  it('maps Rust channel plugin status to renderer fields', async () => {
    httpBridgeMocks.responses.set('/api/channel/plugins', [
      {
        plugin_id: 'telegram',
        type: 'telegram',
        name: 'Telegram',
        enabled: true,
        connected: true,
        status: 'configured',
        active_users: 2,
        bot_username: 'ops_bot',
        has_token: true,
        is_extension: false,
      },
    ]);
    const { channel } = await import('@/common/adapter/ipcBridge');

    await expect(channel.getPluginStatus.invoke()).resolves.toEqual([
      {
        id: 'telegram',
        type: 'telegram',
        name: 'Telegram',
        enabled: true,
        connected: true,
        status: 'configured',
        last_connected: undefined,
        activeUsers: 2,
        botUsername: 'ops_bot',
        hasToken: true,
        isExtension: false,
        extensionMeta: undefined,
      },
    ]);
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'GET',
      path: '/api/channel/plugins',
      body: undefined,
    });
  });

  it('uses Rust settings endpoints and preserves model profile refs', async () => {
    httpBridgeMocks.responses.set('/api/channel/settings/telegram', {
      platform: 'telegram',
      assistant: { assistant_id: 'assistant-1', name: 'Ops Assistant' },
      default_model: { id: 'provider-1', model_profile_id: 'model-profile-1', use_model: 'gpt-5' },
    });
    const { channel } = await import('@/common/adapter/ipcBridge');

    const settings = await channel.getPlatformSettings.invoke({ platform: 'telegram' });
    const modelProfileId: string | undefined = settings.default_model?.model_profile_id;

    expect(modelProfileId).toBe('model-profile-1');
    expect(settings.default_model).toEqual({
      id: 'provider-1',
      model_profile_id: 'model-profile-1',
      use_model: 'gpt-5',
    });

    await channel.setAssistantSetting.invoke({
      platform: 'telegram',
      assistant: { assistant_id: 'assistant-1' },
    });
    await channel.setDefaultModelSetting.invoke({
      platform: 'telegram',
      default_model: { id: 'provider-1', model_profile_id: 'model-profile-1', use_model: 'gpt-5' },
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'GET',
      path: '/api/channel/settings/telegram',
      body: undefined,
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'PUT',
      path: '/api/channel/settings/telegram/assistant',
      body: { assistant_id: 'assistant-1' },
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'PUT',
      path: '/api/channel/settings/telegram/default-model',
      body: { id: 'provider-1', model_profile_id: 'model-profile-1', use_model: 'gpt-5' },
    });
  });

  it('returns mapped plugin status from enable and disable mutations', async () => {
    httpBridgeMocks.responses.set('/api/channel/plugins/enable', {
      plugin_id: 'telegram',
      type: 'telegram',
      name: 'Telegram',
      enabled: true,
      connected: false,
      status: 'configured',
      active_users: 0,
      has_token: true,
      is_extension: false,
    });
    httpBridgeMocks.responses.set('/api/channel/plugins/disable', {
      plugin_id: 'telegram',
      type: 'telegram',
      name: 'Telegram',
      enabled: false,
      connected: false,
      status: 'disabled',
      active_users: 0,
      has_token: true,
      is_extension: false,
    });
    const { channel } = await import('@/common/adapter/ipcBridge');

    await expect(
      channel.enablePlugin.invoke({
        plugin_id: 'telegram',
        config: { credentials: { token: 'secret' } },
      })
    ).resolves.toMatchObject({
      id: 'telegram',
      enabled: true,
      status: 'configured',
      hasToken: true,
    });
    await expect(channel.disablePlugin.invoke({ plugin_id: 'telegram' })).resolves.toMatchObject({
      id: 'telegram',
      enabled: false,
      status: 'disabled',
      hasToken: true,
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/plugins/enable',
      body: { plugin_id: 'telegram', config: { credentials: { token: 'secret' } } },
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/plugins/disable',
      body: { plugin_id: 'telegram' },
    });
  });

  it('returns pairing decisions and user revocations from channel mutations', async () => {
    httpBridgeMocks.responses.set('/api/channel/pairings/approve', {
      code: 'PAIR1234',
      status: 'approved',
      platform_type: 'telegram',
      platform_user_id: 'platform-user-1',
      user: {
        id: 'user-1',
        platform_type: 'telegram',
        platform_user_id: 'platform-user-1',
        display_name: 'Alice',
        authorized_at: 42_000,
        last_active: null,
        session_id: null,
      },
    });
    httpBridgeMocks.responses.set('/api/channel/pairings/reject', {
      code: 'PAIR5678',
      status: 'rejected',
      platform_type: 'telegram',
      platform_user_id: 'platform-user-2',
      user: null,
    });
    httpBridgeMocks.responses.set('/api/channel/users/revoke', {
      user_id: 'user-1',
      status: 'revoked',
    });
    const { channel } = await import('@/common/adapter/ipcBridge');

    await expect(channel.approvePairing.invoke({ code: 'PAIR1234' })).resolves.toEqual({
      code: 'PAIR1234',
      status: 'approved',
      platformType: 'telegram',
      platformUserId: 'platform-user-1',
      user: {
        id: 'user-1',
        platformType: 'telegram',
        platformUserId: 'platform-user-1',
        display_name: 'Alice',
        authorizedAt: 42_000,
        lastActive: null,
        session_id: null,
      },
    });
    await expect(channel.rejectPairing.invoke({ code: 'PAIR5678' })).resolves.toEqual({
      code: 'PAIR5678',
      status: 'rejected',
      platformType: 'telegram',
      platformUserId: 'platform-user-2',
      user: null,
    });
    await expect(channel.revokeUser.invoke({ user_id: 'user-1' })).resolves.toEqual({
      user_id: 'user-1',
      status: 'revoked',
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/pairings/approve',
      body: { code: 'PAIR1234' },
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/pairings/reject',
      body: { code: 'PAIR5678' },
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/users/revoke',
      body: { user_id: 'user-1' },
    });
  });

  it('posts channel ingress messages to Rust run gateway contract', async () => {
    httpBridgeMocks.responses.set('/api/channel/ingress/messages', {
      session_id: 'session-1',
      conversation_id: 'conversation-1',
      run_id: 'run-1',
      created_conversation: true,
    });
    const { channel } = await import('@/common/adapter/ipcBridge');

    await expect(
      channel.ingressMessage.invoke({
        platform_type: 'telegram',
        platform_user_id: 'platform-user-1',
        chat_id: 'chat-1',
        content: 'hello',
        message_id: 'msg-1',
        metadata: { chat_type: 'private' },
      })
    ).resolves.toEqual({
      session_id: 'session-1',
      conversation_id: 'conversation-1',
      run_id: 'run-1',
      created_conversation: true,
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/ingress/messages',
      body: {
        platform_type: 'telegram',
        platform_user_id: 'platform-user-1',
        chat_id: 'chat-1',
        content: 'hello',
        message_id: 'msg-1',
        metadata: { chat_type: 'private' },
      },
    });
  });

  it('returns mapped connector status from channel settings sync', async () => {
    httpBridgeMocks.responses.set('/api/channel/settings/sync', {
      platform: 'telegram',
      synced: true,
      synced_at: 42_000,
      connector: {
        plugin_id: 'telegram',
        type: 'telegram',
        name: 'Telegram',
        enabled: false,
        connected: false,
        status: 'disabled',
        active_users: 0,
        has_token: false,
        is_extension: false,
      },
    });
    const { channel } = await import('@/common/adapter/ipcBridge');

    await expect(channel.syncChannelSettings.invoke({ platform: 'telegram' })).resolves.toEqual({
      platform: 'telegram',
      synced: true,
      synced_at: 42_000,
      connector: {
        id: 'telegram',
        type: 'telegram',
        name: 'Telegram',
        enabled: false,
        connected: false,
        status: 'disabled',
        last_connected: undefined,
        activeUsers: 0,
        botUsername: undefined,
        hasToken: false,
        isExtension: false,
        extensionMeta: undefined,
      },
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/channel/settings/sync',
      body: { platform: 'telegram' },
    });
  });
});
