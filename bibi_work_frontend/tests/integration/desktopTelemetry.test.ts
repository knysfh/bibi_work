import { createServer } from 'node:http';
import { afterAll, describe, expect, it } from 'vitest';
import {
  currentDesktopTraceHeaders,
  initializeDesktopTelemetry,
  normalizeOtlpTracesEndpoint,
  readDesktopTelemetrySettings,
  withDesktopSpan,
  type DesktopTelemetryHandle,
} from '../../packages/desktop/src/process/telemetry/desktopTelemetry';

let telemetry: DesktopTelemetryHandle | null = null;

afterAll(async () => {
  await telemetry?.shutdown();
});

describe('desktop telemetry', () => {
  it('validates configuration and normalizes the OTLP traces endpoint', () => {
    expect(normalizeOtlpTracesEndpoint('http://collector:4318/')).toBe('http://collector:4318/v1/traces');
    expect(normalizeOtlpTracesEndpoint('http://collector:4318/v1/traces')).toBe('http://collector:4318/v1/traces');
    expect(() => readDesktopTelemetrySettings({ BIWORK_DESKTOP__OTLP_ENABLED: 'true' })).toThrow(
      'OTLP_ENDPOINT is required'
    );
    expect(() =>
      readDesktopTelemetrySettings({
        BIWORK_DESKTOP__OTLP_ENABLED: 'false',
        BIWORK_DESKTOP__TRACE_SAMPLE_RATIO: '1.1',
      })
    ).toThrow('between 0 and 1');
  });

  it('exports OTLP/HTTP JSON and preserves an incoming W3C trace id', async () => {
    let resolvePayload!: (payload: string) => void;
    const payloadReceived = new Promise<string>((resolve) => {
      resolvePayload = resolve;
    });
    const collector = createServer((req, res) => {
      const chunks: Buffer[] = [];
      req.on('data', (chunk: Buffer) => chunks.push(chunk));
      req.on('end', () => {
        resolvePayload(Buffer.concat(chunks).toString('utf8'));
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{}');
      });
    });
    await new Promise<void>((resolve, reject) => {
      collector.once('error', reject);
      collector.listen(0, '127.0.0.1', () => resolve());
    });
    const address = collector.address();
    if (!address || typeof address === 'string') throw new Error('collector did not bind to a TCP port');

    telemetry = initializeDesktopTelemetry({
      env: {
        BIWORK_DESKTOP__OTLP_ENABLED: 'true',
        BIWORK_DESKTOP__OTLP_ENDPOINT: `http://127.0.0.1:${address.port}`,
        BIWORK_DESKTOP__TELEMETRY_SERVICE_NAME: 'desktop-telemetry-test',
        BIWORK_DESKTOP__TRACE_SAMPLE_RATIO: '1',
      },
      serviceVersion: 'test-version',
    });

    const incomingTraceId = '0123456789abcdeffedcba9876543210';
    await withDesktopSpan(
      'desktop.integration',
      {
        parentHeaders: {
          traceparent: `00-${incomingTraceId}-0123456789abcdef-01`,
        },
      },
      async () => {
        expect(currentDesktopTraceHeaders().traceparent?.split('-')[1]).toBe(incomingTraceId);
      }
    );
    await telemetry.forceFlush();
    const payload = await payloadReceived;
    await new Promise<void>((resolve) => collector.close(() => resolve()));

    expect(payload).toContain(incomingTraceId);
    expect(payload).toContain('desktop.integration');
    expect(payload).toContain('desktop-telemetry-test');
    expect(payload).not.toContain('authorization');
  });
});
