import type { IncomingHttpHeaders, IncomingMessage, ServerResponse } from 'node:http';
import {
  context,
  defaultTextMapGetter,
  defaultTextMapSetter,
  ROOT_CONTEXT,
  SpanKind,
  SpanStatusCode,
  trace,
  type Attributes,
  type Context,
  type Span,
} from '@opentelemetry/api';
import { W3CTraceContextPropagator } from '@opentelemetry/core';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';
import { resourceFromAttributes } from '@opentelemetry/resources';
import {
  BatchSpanProcessor,
  ParentBasedSampler,
  TraceIdRatioBasedSampler,
  type SpanExporter,
} from '@opentelemetry/sdk-trace-base';
import { NodeTracerProvider } from '@opentelemetry/sdk-trace-node';
import { ATTR_SERVICE_NAME, ATTR_SERVICE_VERSION } from '@opentelemetry/semantic-conventions';

const DEFAULT_SERVICE_NAME = 'bibi-work-desktop';
const DEFAULT_TIMEOUT_MS = 5_000;
const DEFAULT_SAMPLE_RATIO = 1;

export type DesktopTelemetrySettings = {
  enabled: boolean;
  endpoint: string | null;
  sampleRatio: number;
  serviceName: string;
  serviceVersion?: string;
  timeoutMs: number;
};

export type DesktopSpanOptions = {
  attributes?: Attributes;
  kind?: SpanKind;
  parentHeaders?: IncomingHttpHeaders | Record<string, unknown>;
};

export type DesktopTelemetryHandle = {
  forceFlush: () => Promise<void>;
  shutdown: () => Promise<void>;
};

type DesktopTelemetryInitOptions = {
  env?: NodeJS.ProcessEnv;
  exporter?: SpanExporter;
  serviceVersion?: string;
};

let provider: NodeTracerProvider | null = null;
let tracer = trace.getTracer('bibi-work-desktop');
const w3cPropagator = new W3CTraceContextPropagator();

function parseBoolean(value: string | undefined): boolean {
  if (value === undefined || !value.trim()) return false;
  if (value === '1' || value.toLowerCase() === 'true') return true;
  if (value === '0' || value.toLowerCase() === 'false') return false;
  throw new Error('BIWORK_DESKTOP__OTLP_ENABLED must be true, false, 1, or 0');
}

function parsePositiveInteger(value: string | undefined, fallback: number, name: string): number {
  if (value === undefined || !value.trim()) return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) throw new Error(`${name} must be a positive integer`);
  return parsed;
}

function parseSampleRatio(value: string | undefined): number {
  if (value === undefined || !value.trim()) return DEFAULT_SAMPLE_RATIO;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed < 0 || parsed > 1) {
    throw new Error('BIWORK_DESKTOP__TRACE_SAMPLE_RATIO must be between 0 and 1');
  }
  return parsed;
}

export function normalizeOtlpTracesEndpoint(endpoint: string): string {
  const normalized = endpoint.trim().replace(/\/+$/, '');
  if (!normalized) throw new Error('desktop OTLP endpoint must not be empty');
  return normalized.endsWith('/v1/traces') ? normalized : `${normalized}/v1/traces`;
}

export function readDesktopTelemetrySettings(
  env: NodeJS.ProcessEnv = process.env,
  serviceVersion?: string
): DesktopTelemetrySettings {
  const enabled = parseBoolean(env.BIWORK_DESKTOP__OTLP_ENABLED);
  const endpoint =
    env.BIWORK_DESKTOP__OTLP_ENDPOINT?.trim() ||
    env.OTEL_EXPORTER_OTLP_TRACES_ENDPOINT?.trim() ||
    env.OTEL_EXPORTER_OTLP_ENDPOINT?.trim() ||
    null;
  const serviceName = env.BIWORK_DESKTOP__TELEMETRY_SERVICE_NAME?.trim() || DEFAULT_SERVICE_NAME;
  const settings = {
    enabled,
    endpoint,
    sampleRatio: parseSampleRatio(env.BIWORK_DESKTOP__TRACE_SAMPLE_RATIO),
    serviceName,
    serviceVersion,
    timeoutMs: parsePositiveInteger(
      env.BIWORK_DESKTOP__TELEMETRY_TIMEOUT_MILLISECONDS,
      DEFAULT_TIMEOUT_MS,
      'BIWORK_DESKTOP__TELEMETRY_TIMEOUT_MILLISECONDS'
    ),
  };
  if (settings.enabled && !settings.endpoint) {
    throw new Error('BIWORK_DESKTOP__OTLP_ENDPOINT is required when desktop OTLP is enabled');
  }
  return settings;
}

function normalizedCarrier(headers: IncomingHttpHeaders | Record<string, unknown>): Record<string, string> {
  const carrier: Record<string, string> = {};
  for (const [key, value] of Object.entries(headers)) {
    if (typeof value === 'string') carrier[key.toLowerCase()] = value;
    else if (Array.isArray(value)) carrier[key.toLowerCase()] = value.join(',');
  }
  return carrier;
}

function parentContext(headers: DesktopSpanOptions['parentHeaders']): Context {
  return headers
    ? w3cPropagator.extract(ROOT_CONTEXT, normalizedCarrier(headers), defaultTextMapGetter)
    : context.active();
}

export function initializeDesktopTelemetry(options: DesktopTelemetryInitOptions = {}): DesktopTelemetryHandle {
  if (provider) {
    return { forceFlush: () => provider!.forceFlush(), shutdown: () => provider!.shutdown() };
  }

  const settings = readDesktopTelemetrySettings(options.env, options.serviceVersion);
  const spanProcessors = [];
  if (settings.enabled) {
    const exporter =
      options.exporter ??
      new OTLPTraceExporter({
        url: normalizeOtlpTracesEndpoint(settings.endpoint!),
        timeoutMillis: settings.timeoutMs,
        concurrencyLimit: 4,
      });
    spanProcessors.push(
      new BatchSpanProcessor(exporter, {
        exportTimeoutMillis: settings.timeoutMs,
        maxExportBatchSize: 128,
        maxQueueSize: 1_024,
        scheduledDelayMillis: 1_000,
      })
    );
  }

  provider = new NodeTracerProvider({
    resource: resourceFromAttributes({
      [ATTR_SERVICE_NAME]: settings.serviceName,
      ...(settings.serviceVersion ? { [ATTR_SERVICE_VERSION]: settings.serviceVersion } : {}),
    }),
    sampler: new ParentBasedSampler({ root: new TraceIdRatioBasedSampler(settings.sampleRatio) }),
    spanLimits: { attributeCountLimit: 32, attributeValueLengthLimit: 512, eventCountLimit: 32 },
    spanProcessors,
  });
  provider.register({ propagator: w3cPropagator });
  tracer = provider.getTracer('bibi-work-desktop');

  const registeredProvider = provider;
  return {
    forceFlush: () => registeredProvider.forceFlush(),
    shutdown: async () => {
      await registeredProvider.shutdown();
      if (provider === registeredProvider) provider = null;
    },
  };
}

export async function withDesktopSpan<T>(
  name: string,
  options: DesktopSpanOptions,
  operation: (span: Span) => Promise<T>
): Promise<T> {
  return context.with(parentContext(options.parentHeaders), () =>
    tracer.startActiveSpan(name, { attributes: options.attributes, kind: options.kind }, async (span) => {
      try {
        return await operation(span);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        span.recordException(error instanceof Error ? error : new Error(message));
        span.setStatus({ code: SpanStatusCode.ERROR, message: message.slice(0, 512) });
        throw error;
      } finally {
        span.end();
      }
    })
  );
}

export function recordDesktopSpanError(span: Span, error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  span.recordException(error instanceof Error ? error : new Error(message));
  span.setStatus({ code: SpanStatusCode.ERROR, message: message.slice(0, 512) });
}

export function injectDesktopTraceHeaders(headers: Record<string, string>): Record<string, string> {
  w3cPropagator.inject(context.active(), headers, defaultTextMapSetter);
  return headers;
}

export function currentDesktopTraceHeaders(): Record<string, string> {
  return injectDesktopTraceHeaders({});
}

export async function traceDesktopHttpRequest(
  req: IncomingMessage,
  res: ServerResponse,
  operation: () => Promise<void>
): Promise<void> {
  const path = new URL(req.url ?? '/', 'http://127.0.0.1').pathname;
  await withDesktopSpan(
    'http.request',
    {
      attributes: {
        'http.request.method': req.method ?? 'UNKNOWN',
        'url.path': path,
      },
      kind: SpanKind.SERVER,
      parentHeaders: req.headers,
    },
    async (span) => {
      await operation();
      span.setAttribute('http.response.status_code', res.statusCode);
    }
  );
}
