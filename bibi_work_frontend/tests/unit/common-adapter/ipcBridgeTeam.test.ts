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
        return { active_run: null } as Data;
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

describe('ipcBridge team adapter', () => {
  beforeEach(() => {
    httpBridgeMocks.calls.length = 0;
  });

  it('getRunState calls GET /api/teams/{team_id}/run-state', async () => {
    const { team } = await import('@/common/adapter/ipcBridge');

    await team.getRunState.invoke({ team_id: 'team-1' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'GET',
      path: '/api/teams/team-1/run-state',
      body: undefined,
    });
  });

  it('posts team active lease heartbeats without leaking the path team_id into the body', async () => {
    const { team } = await import('@/common/adapter/ipcBridge');

    await team.activeLease.invoke({ team_id: 'team-1' });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/teams/team-1/active-lease',
      body: undefined,
    });
  });

  it('posts team messages to the Rust team run gateway contract', async () => {
    const { team } = await import('@/common/adapter/ipcBridge');

    await team.sendMessage.invoke({
      team_id: 'team-1',
      input: 'coordinate this',
      files: ['/workspace/spec.md'],
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/teams/team-1/messages',
      body: {
        content: 'coordinate this',
        files: ['/workspace/spec.md'],
      },
    });
  });

  it('posts targeted team agent messages without duplicating team_id or slot_id in the body', async () => {
    const { team } = await import('@/common/adapter/ipcBridge');

    await team.sendMessageToAgent.invoke({
      team_id: 'team-1',
      slot_id: 'slot-lead',
      input: 'lead only',
      files: [],
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/teams/team-1/agents/slot-lead/messages',
      body: {
        content: 'lead only',
        files: [],
      },
    });
  });

  it('posts team run cancellation with target slot and reason only in the body', async () => {
    const { team } = await import('@/common/adapter/ipcBridge');

    await team.cancelRun.invoke({
      team_id: 'team-1',
      team_run_id: 'run-1',
      target_slot_id: 'slot-worker',
      reason: 'user cancel',
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/teams/team-1/runs/run-1/cancel',
      body: {
        target_slot_id: 'slot-worker',
        reason: 'user cancel',
      },
    });
  });

  it('posts child-turn cancel and pause commands to slot-scoped team run endpoints', async () => {
    const { team } = await import('@/common/adapter/ipcBridge');

    await team.cancelChildTurn.invoke({
      team_id: 'team-1',
      team_run_id: 'run-1',
      slot_id: 'slot-worker',
      reason: 'stop child',
    });
    await team.pauseSlotWork.invoke({
      team_id: 'team-1',
      team_run_id: 'run-1',
      slot_id: 'slot-worker',
      reason: 'pause wakeups',
    });

    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/teams/team-1/runs/run-1/agents/slot-worker/cancel',
      body: { reason: 'stop child' },
    });
    expect(httpBridgeMocks.calls).toContainEqual({
      method: 'POST',
      path: '/api/teams/team-1/runs/run-1/agents/slot-worker/pause',
      body: { reason: 'pause wakeups' },
    });
  });
});
