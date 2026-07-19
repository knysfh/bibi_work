import * as acp from '@agentclientprotocol/sdk';
import { SpanKind } from '@opentelemetry/api';
import { spawn } from 'node:child_process';
import { Readable, Writable } from 'node:stream';
import { withDesktopSpan } from '../../telemetry/desktopTelemetry';
import { createAcpEventMapper, type PlatformRunEvent } from './events';

export type AcpRuntimeConfig = {
  command: string;
  args?: unknown;
  env?: unknown;
};

type ExecuteOptions = {
  runId: string;
  prompt: string;
  cwd: string;
  runtime: AcpRuntimeConfig;
  timeoutMs: number;
  emit: (events: PlatformRunEvent[]) => Promise<void>;
  signal?: AbortSignal;
  traceHeaders?: Record<string, string>;
  requestPermission?: (request: acp.RequestPermissionRequest) => Promise<acp.RequestPermissionResponse>;
};

export class AcpRuntimeCancelledError extends Error {
  constructor() {
    super('ACP runtime was cancelled');
    this.name = 'AcpRuntimeCancelledError';
  }
}

function stringArray(value: unknown): string[] {
  if (!Array.isArray(value) || value.some((item) => typeof item !== 'string')) {
    throw new Error('ACP runtime args must be an array of strings');
  }
  return value;
}

function runtimeEnv(value: unknown, traceHeaders?: Record<string, string>): NodeJS.ProcessEnv {
  if (!Array.isArray(value)) throw new Error('ACP runtime env must be an array');
  const env = { ...process.env };
  for (const item of value) {
    if (!item || typeof item !== 'object') throw new Error('ACP runtime env entry must be an object');
    const record = item as Record<string, unknown>;
    const key = typeof record.key === 'string' ? record.key : typeof record.name === 'string' ? record.name : null;
    if (!key || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) throw new Error('ACP runtime env key is invalid');
    if (typeof record.value !== 'string') throw new Error(`ACP runtime env value for ${key} must be a string`);
    env[key] = record.value;
  }
  const traceparent = traceHeaders?.traceparent;
  if (traceparent && /^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$/i.test(traceparent)) {
    env.TRACEPARENT = traceparent;
  }
  const tracestate = traceHeaders?.tracestate;
  if (tracestate && tracestate.length <= 512) env.TRACESTATE = tracestate;
  return env;
}

export async function executeAcpRuntime(options: ExecuteOptions): Promise<{ stop_reason: acp.StopReason }> {
  const command = options.runtime.command?.trim();
  if (!command) throw new Error('ACP runtime command is required');
  const child = spawn(command, stringArray(options.runtime.args ?? []), {
    cwd: options.cwd,
    env: runtimeEnv(options.runtime.env ?? [], options.traceHeaders),
    shell: false,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
  });
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk: string) => {
    stderr = `${stderr}${chunk}`.slice(-8_192);
  });

  const mapper = createAcpEventMapper(options.runId);
  let emitChain = Promise.resolve();
  const emitInOrder = (events: PlatformRunEvent[]): Promise<void> => {
    emitChain = emitChain.then(() => options.emit(events));
    return emitChain;
  };
  let timer: ReturnType<typeof setTimeout> | null = null;
  let forceKillTimer: ReturnType<typeof setTimeout> | null = null;
  let connection: acp.ClientSideConnection | null = null;
  let sessionId: string | null = null;
  const onAbort = (): void => {
    if (connection && sessionId) void connection.cancel({ sessionId });
    forceKillTimer ??= setTimeout(() => child.kill('SIGTERM'), 1_500);
  };
  options.signal?.addEventListener('abort', onAbort, { once: true });
  const cancelled = new Promise<never>((_, reject) => {
    if (options.signal?.aborted) {
      onAbort();
      reject(new AcpRuntimeCancelledError());
      return;
    }
    options.signal?.addEventListener('abort', () => reject(new AcpRuntimeCancelledError()), { once: true });
  });
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      child.kill('SIGTERM');
      reject(new Error(`ACP runtime timed out after ${options.timeoutMs}ms`));
    }, options.timeoutMs);
  });

  try {
    const stream = acp.ndJsonStream(
      Writable.toWeb(child.stdin) as WritableStream<Uint8Array>,
      Readable.toWeb(child.stdout) as ReadableStream<Uint8Array>
    );
    connection = new acp.ClientSideConnection(
      () => ({
        requestPermission: options.requestPermission ?? (async () => ({ outcome: { outcome: 'cancelled' } })),
        sessionUpdate: async (notification) => {
          const events = mapper.map(notification);
          if (events.length) await emitInOrder(events);
        },
      }),
      stream
    );
    const prompt = (async () => {
      await emitInOrder([
        { event_id: `desktop-acp.${options.runId}.started`, type: 'run.started', payload: { runtime: 'biwork_cli' } },
      ]);
      await withDesktopSpan('acp.initialize', { kind: SpanKind.CLIENT }, () =>
        connection!.initialize({
          protocolVersion: acp.PROTOCOL_VERSION,
          clientCapabilities: {},
          clientInfo: { name: 'Bibi Work Desktop', version: '1' },
        })
      );
      const session = await withDesktopSpan('acp.session.create', { kind: SpanKind.CLIENT }, () =>
        connection!.newSession({ cwd: options.cwd, mcpServers: [] })
      );
      sessionId = session.sessionId;
      if (options.signal?.aborted) throw new AcpRuntimeCancelledError();
      const response = await withDesktopSpan('acp.prompt', { kind: SpanKind.CLIENT }, () =>
        connection!.prompt({
          sessionId: session.sessionId,
          prompt: [{ type: 'text', text: options.prompt }],
        })
      );
      if (options.signal?.aborted) throw new AcpRuntimeCancelledError();
      await emitInOrder(mapper.finish(response.stopReason));
      return { stop_reason: response.stopReason };
    })();
    return await Promise.race([prompt, timeout, cancelled]);
  } catch (error) {
    if (error instanceof AcpRuntimeCancelledError) throw error;
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(stderr ? `${message}; ACP stderr: ${stderr}` : message, { cause: error });
  } finally {
    if (timer) clearTimeout(timer);
    if (forceKillTimer) clearTimeout(forceKillTimer);
    options.signal?.removeEventListener('abort', onAbort);
    if (child.exitCode === null) child.kill('SIGTERM');
  }
}
