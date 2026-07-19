import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { AcpRuntimeCancelledError, executeAcpRuntime } from '../../packages/desktop/src/process/agent/acp/runtime';

const temporaryDirectories: string[] = [];

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

describe('desktop ACP runtime', () => {
  it('performs a real ACP initialize/session/prompt exchange over stdio', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'bibi-acp-'));
    temporaryDirectories.push(cwd);
    const batches: Array<Array<{ type: string; payload: Record<string, unknown> }>> = [];
    const result = await executeAcpRuntime({
      runId: 'run-fixture',
      prompt: 'hello ACP',
      cwd,
      runtime: {
        command: process.execPath,
        args: [resolve('tests/fixtures/acp-echo-agent.mjs')],
        env: [],
      },
      timeoutMs: 10_000,
      emit: async (events) => {
        batches.push(events);
      },
    });
    expect(result).toEqual({ stop_reason: 'end_turn' });
    const events = batches.flat();
    expect(events.map((event) => event.type)).toEqual([
      'run.started',
      'message.started',
      'message.delta',
      'message.completed',
      'run.completed',
    ]);
    expect(events.find((event) => event.type === 'message.completed')?.payload.content).toBe('echo:hello ACP');
  });

  it('rejects malformed runtime arguments before protocol execution', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'bibi-acp-'));
    temporaryDirectories.push(cwd);
    await expect(
      executeAcpRuntime({
        runId: 'run-invalid',
        prompt: 'hello',
        cwd,
        runtime: { command: process.execPath, args: [7], env: [] },
        timeoutMs: 1_000,
        emit: async () => {},
      })
    ).rejects.toThrow('args must be an array of strings');
  });

  it('propagates AbortSignal as ACP session/cancel without emitting a false terminal event', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'bibi-acp-'));
    temporaryDirectories.push(cwd);
    const controller = new AbortController();
    const eventTypes: string[] = [];
    const execution = executeAcpRuntime({
      runId: 'run-cancel',
      prompt: 'wait-for-cancel',
      cwd,
      runtime: {
        command: process.execPath,
        args: [resolve('tests/fixtures/acp-echo-agent.mjs')],
        env: [],
      },
      timeoutMs: 10_000,
      signal: controller.signal,
      emit: async (events) => {
        eventTypes.push(...events.map((event) => event.type));
        if (events.some((event) => event.type === 'run.started')) {
          setTimeout(() => controller.abort(), 100);
        }
      },
    });
    await expect(execution).rejects.toBeInstanceOf(AcpRuntimeCancelledError);
    expect(eventTypes).toEqual(['run.started']);
  });

  it('bridges ACP permission requests through the injected governance callback', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'bibi-acp-'));
    temporaryDirectories.push(cwd);
    const eventPayloads: Array<Record<string, unknown>> = [];
    let requestedToolCallId = '';
    const eventTypes: string[] = [];
    await executeAcpRuntime({
      runId: 'run-permission',
      prompt: 'request-permission',
      cwd,
      runtime: {
        command: process.execPath,
        args: [resolve('tests/fixtures/acp-echo-agent.mjs')],
        env: [],
      },
      timeoutMs: 10_000,
      requestPermission: async (request) => {
        requestedToolCallId = request.toolCall.toolCallId;
        return { outcome: { outcome: 'selected', optionId: 'allow-fixture' } };
      },
      emit: async (events) => {
        if (events.some((event) => event.type === 'message.delta')) {
          await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
        }
        eventTypes.push(...events.map((event) => event.type));
        eventPayloads.push(...events.map((event) => event.payload));
      },
    });
    expect(requestedToolCallId).toBe('fixture-sensitive-tool');
    expect(eventPayloads.some((payload) => payload.content === 'permission:allow-fixture')).toBe(true);
    expect(eventTypes.indexOf('message.delta')).toBeLessThan(eventTypes.indexOf('message.completed'));
  });

  it('propagates the W3C trace context to the ACP child without exposing other headers', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'bibi-acp-'));
    temporaryDirectories.push(cwd);
    const completedContent: string[] = [];
    const traceparent = '00-0123456789abcdeffedcba9876543210-0123456789abcdef-01';
    await executeAcpRuntime({
      runId: 'run-trace-context',
      prompt: 'echo-traceparent',
      cwd,
      runtime: {
        command: process.execPath,
        args: [resolve('tests/fixtures/acp-echo-agent.mjs')],
        env: [{ key: 'TRACEPARENT', value: 'untrusted-runtime-value' }],
      },
      traceHeaders: { traceparent, authorization: 'must-not-be-propagated' },
      timeoutMs: 10_000,
      emit: async (events) => {
        for (const event of events) {
          if (event.type === 'message.completed' && typeof event.payload.content === 'string') {
            completedContent.push(event.payload.content);
          }
        }
      },
    });

    expect(completedContent).toContain(`traceparent:${traceparent}`);
    expect(completedContent.join('\n')).not.toContain('must-not-be-propagated');
  });
});
