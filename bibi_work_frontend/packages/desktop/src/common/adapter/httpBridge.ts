/**
 * HTTP/WS bridge factory — drop-in replacement for bridge.buildProvider / bridge.buildEmitter
 * that routes calls to BiWork backend via REST API and WebSocket.
 *
 * Exported helpers produce objects with the same shape as @office-ai/platform bridge,
 * so existing renderer code works without changes.
 */

import {
  clearAccessToken,
  getAccessToken,
  getAuthorizationHeaders,
  peekAccessToken,
  requestWithAuthorizationRetry,
  subscribeAccessToken,
} from '../auth/authTokenBroker';

// ---------------------------------------------------------------------------
// Base URL
// ---------------------------------------------------------------------------

declare global {
  interface Window {
    __backendPort?: number;
  }
}

/**
 * Resolve the backend port, honoring both renderer and main-process contexts.
 *
 * - Renderer (Electron): the preload bridge writes `window.__backendPort` before
 *   the first HTTP call, so reading from window is authoritative.
 * - Renderer (WebUI browser): no preload, so `window.__backendPort` is missing.
 *   Requests must go to the same origin that served the page; web-host's
 *   static-server reverse-proxies `/api/*` and upgrades `/ws` to the backend
 *   port. See getBaseUrl / getWsUrl below for the WebUI branch.
 * - Main process: `window` is undefined. `src/index.ts` writes the port to
 *   `globalThis.__backendPort` immediately after `backendManager.start()`
 *   resolves, so any main-process ipcBridge caller (e.g. the one-shot
 *   assistant migration hook) hits the correct port.
 * - Fallback `13400` only applies when neither is initialized — the request
 *   will still fail cleanly with ECONNREFUSED rather than masking the bug.
 */
function getBackendPort(): number {
  if (typeof window !== 'undefined' && (window as Window).__backendPort) {
    return (window as Window).__backendPort as number;
  }
  const g = globalThis as typeof globalThis & { __backendPort?: number };
  return g.__backendPort ?? 13400;
}

/**
 * WebUI (browser) mode: no Electron preload, so `window.__backendPort` is not
 * injected. Use same-origin URLs; web-host's static-server handles the reverse
 * proxy / WS upgrade to the backend.
 */
function isWebUiBrowserMode(): boolean {
  return typeof window !== 'undefined' && typeof document !== 'undefined' && !(window as Window).__backendPort;
}

export function getBaseUrl(): string {
  if (isWebUiBrowserMode()) {
    // Same-origin: calls like fetch(`${baseUrl}/api/foo`) resolve to `/api/foo`
    // on whatever host the page was served from.
    return '';
  }
  return `http://127.0.0.1:${getBackendPort()}`;
}

function getWsUrl(): string {
  if (isWebUiBrowserMode()) {
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${proto}//${window.location.host}/ws`;
  }
  return `ws://127.0.0.1:${getBackendPort()}/ws`;
}

// ---------------------------------------------------------------------------
// Structured backend error
// ---------------------------------------------------------------------------

/**
 * Error thrown by `httpRequest` when the backend returns a non-2xx response.
 * Carries the structured error envelope (`success: false, error, code`) so
 * callers can branch on `code` without parsing the stringified message.
 *
 * @example
 *   try { await ipcBridge.conversation.sendMessage.invoke(...); }
 *   catch (e) {
 *     if (isBackendHttpError(e) && e.code === 'CONVERSATION_ARCHIVED') { ... }
 *   }
 */
export class BackendHttpError extends Error {
  readonly status: number;
  /** Machine-readable error code from the backend `ErrorResponse.code`, or `''` when parse failed. */
  readonly code: string;
  /** Backend-provided human message from `ErrorResponse.error`, or the raw body when parse failed. */
  readonly backendMessage: string;
  /** Structured backend metadata from `ErrorResponse.details`, when present. */
  readonly details: unknown;
  /** Backend response trace id, when present. */
  readonly traceId: string;
  /** Raw parsed body (object on JSON response, string on text/non-JSON). */
  readonly body: unknown;

  constructor(params: { method: string; path: string; status: number; body: unknown }) {
    const { method, path, status, body } = params;
    let code = '';
    let backendMessage = '';
    let traceId = '';
    let details: unknown;
    if (body && typeof body === 'object') {
      const b = body as { code?: unknown; error?: unknown; details?: unknown; trace_id?: unknown; traceId?: unknown };
      if (typeof b.code === 'string') code = b.code;
      if (typeof b.error === 'string') backendMessage = b.error;
      if (typeof b.trace_id === 'string') traceId = b.trace_id;
      if (!traceId && typeof b.traceId === 'string') traceId = b.traceId;
      details = b.details;
    } else if (typeof body === 'string') {
      backendMessage = body;
    }
    super(`Backend ${method} ${path} failed (${status}): ${JSON.stringify(body)}`);
    this.name = 'BackendHttpError';
    this.status = status;
    this.code = code;
    this.backendMessage = backendMessage;
    this.traceId = traceId;
    this.details = details;
    this.body = body;
  }
}

export function isBackendHttpError(error: unknown): error is BackendHttpError {
  // Prefer instanceof — fast path in production/bundled contexts.
  if (error instanceof BackendHttpError) return true;
  // Fallback: vite-dev HMR can split the module across chunks, breaking
  // instanceof. Detect by duck-typing on the shape produced by our
  // constructor.
  if (
    error &&
    typeof error === 'object' &&
    'name' in error &&
    (error as { name: unknown }).name === 'BackendHttpError' &&
    'status' in error &&
    typeof (error as { status: unknown }).status === 'number' &&
    'code' in error &&
    typeof (error as { code: unknown }).code === 'string'
  ) {
    return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// HTTP request helper
// ---------------------------------------------------------------------------

/**
 * Per-request overrides for `httpRequest`.
 *
 * `silentStatuses` lets known-soft failures (e.g. a runtime-scoped lookup
 * returning 404 before the agent has attached) skip the noisy `console.error`
 * and the Sentry breadcrumb that comes with it. The error is still thrown so
 * the caller's existing try/catch keeps working.
 */
export type HttpRequestOptions = {
  silentStatuses?: number[];
  signal?: AbortSignal;
};

export type HttpRawRequestOptions = HttpRequestOptions & {
  headers?: Record<string, string>;
};

const SENSITIVE_LOG_KEY_PATTERN = /api[_-]?key|authorization|auth[_-]?token|access[_-]?token|refresh[_-]?token|secret/i;

function redactForLog(value: unknown, depth = 0): unknown {
  if (depth > 8 || value === null || typeof value !== 'object') {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => redactForLog(item, depth + 1));
  }

  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).map(([key, entry]) => [
      key,
      SENSITIVE_LOG_KEY_PATTERN.test(key) ? '[REDACTED]' : redactForLog(entry, depth + 1),
    ])
  );
}

export async function httpRequest<T>(
  method: string,
  path: string,
  body?: unknown,
  options?: HttpRequestOptions
): Promise<T> {
  const url = `${getBaseUrl()}${path}`;
  console.debug(
    `[httpBridge] ${method} ${path}`,
    body !== undefined ? JSON.stringify(redactForLog(body)).slice(0, 500) : '(no body)'
  );

  const response = await requestWithAuthorizationRetry(async () => {
    const headers: Record<string, string> = {};
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    Object.assign(headers, await getAuthorizationHeaders());
    return fetch(url, {
      method,
      headers,
      signal: options?.signal,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });
  });

  if (!response.ok) {
    // Response body can only be consumed once — read as text, then try JSON
    const rawText = await response.text().catch(() => '');
    let errorBody: unknown;
    try {
      errorBody = JSON.parse(rawText);
    } catch {
      errorBody = rawText;
    }
    if (options?.silentStatuses?.includes(response.status)) {
      console.debug(`[httpBridge] ${method} ${path} → ${response.status} (silenced)`, errorBody);
    } else {
      console.error(`[httpBridge] ${method} ${path} → ${response.status}`, errorBody);
    }
    throw new BackendHttpError({ method, path, status: response.status, body: errorBody });
  }

  console.debug(`[httpBridge] ${method} ${path} → ${response.status} OK`);

  const contentType = response.headers.get('Content-Type');
  if (!contentType?.includes('application/json')) {
    return undefined as T;
  }

  const json = await response.json();
  // Backend wraps in { success, data, ... } — unwrap when present
  if (json && typeof json === 'object' && 'data' in json) {
    return json.data as T;
  }
  return json as T;
}

/** Send multipart form data without overriding the browser-generated boundary. */
export async function httpFormRequest<T>(path: string, body: FormData, options?: HttpRequestOptions): Promise<T> {
  const method = 'POST';
  const response = await requestWithAuthorizationRetry(async () =>
    fetch(`${getBaseUrl()}${path}`, {
      method,
      headers: await getAuthorizationHeaders(),
      signal: options?.signal,
      body,
    })
  );
  if (!response.ok) {
    const rawText = await response.text().catch(() => '');
    let errorBody: unknown;
    try {
      errorBody = JSON.parse(rawText);
    } catch {
      errorBody = rawText;
    }
    throw new BackendHttpError({ method, path, status: response.status, body: errorBody });
  }
  const contentType = response.headers.get('Content-Type');
  if (!contentType?.includes('application/json')) return undefined as T;
  const json = await response.json();
  if (json && typeof json === 'object' && 'data' in json) return json.data as T;
  return json as T;
}

export async function httpRawRequest(method: string, path: string, options?: HttpRawRequestOptions): Promise<Response> {
  const url = `${getBaseUrl()}${path}`;
  console.debug(`[httpBridge] ${method} ${path}`, '(raw response)');
  const hasExplicitAuthorization = Object.keys(options?.headers ?? {}).some(
    (key) => key.toLowerCase() === 'authorization'
  );
  const send = async () => {
    const headers: Record<string, string> = { ...options?.headers };
    if (!hasExplicitAuthorization) Object.assign(headers, await getAuthorizationHeaders());
    return fetch(url, { method, headers, signal: options?.signal });
  };
  const response = hasExplicitAuthorization ? await send() : await requestWithAuthorizationRetry(send);

  if (!response.ok) {
    const rawText = await response.text().catch(() => '');
    let errorBody: unknown;
    try {
      errorBody = JSON.parse(rawText);
    } catch {
      errorBody = rawText;
    }
    if (options?.silentStatuses?.includes(response.status)) {
      console.debug(`[httpBridge] ${method} ${path} → ${response.status} (silenced)`, errorBody);
    } else {
      console.error(`[httpBridge] ${method} ${path} → ${response.status}`, errorBody);
    }
    throw new BackendHttpError({ method, path, status: response.status, body: errorBody });
  }

  console.debug(`[httpBridge] ${method} ${path} → ${response.status} OK (raw)`);
  return response;
}

// ---------------------------------------------------------------------------
// Provider factories (same shape as bridge.buildProvider)
// ---------------------------------------------------------------------------

type ProviderLike<Data, Params> = {
  provider: (handler: (params: Params) => Promise<Data>) => void;
  invoke: Params extends undefined ? () => Promise<Data> : (params: Params) => Promise<Data>;
};

export function withResponseMap<Raw, Mapped, Params>(
  inner: ProviderLike<Raw, Params>,
  map: (data: Raw) => Mapped
): ProviderLike<Mapped, Params> {
  return {
    provider: () => {},
    invoke: (async (params?: Params) => {
      const raw = await (inner.invoke as (p?: Params) => Promise<Raw>)(params);
      return map(raw);
    }) as ProviderLike<Mapped, Params>['invoke'],
  };
}

export function httpGet<Data, Params = undefined>(
  path: string | ((params: Params) => string),
  options?: HttpRequestOptions
): ProviderLike<Data, Params> {
  return {
    provider: () => {},
    invoke: (async (params?: Params) => {
      const resolvedPath = typeof path === 'function' ? path(params!) : path;
      return httpRequest<Data>('GET', resolvedPath, undefined, options);
    }) as ProviderLike<Data, Params>['invoke'],
  };
}

export function httpPost<Data, Params = undefined>(
  path: string | ((params: Params) => string),
  mapBody?: (params: Params) => unknown
): ProviderLike<Data, Params> {
  return {
    provider: () => {},
    invoke: (async (params?: Params) => {
      const resolvedPath = typeof path === 'function' ? path(params!) : path;
      const body = mapBody ? mapBody(params!) : params;
      return httpRequest<Data>('POST', resolvedPath, body);
    }) as ProviderLike<Data, Params>['invoke'],
  };
}

export function httpPut<Data, Params = undefined>(
  path: string | ((params: Params) => string),
  mapBody?: (params: Params) => unknown
): ProviderLike<Data, Params> {
  return {
    provider: () => {},
    invoke: (async (params?: Params) => {
      const resolvedPath = typeof path === 'function' ? path(params!) : path;
      const body = mapBody ? mapBody(params!) : params;
      return httpRequest<Data>('PUT', resolvedPath, body);
    }) as ProviderLike<Data, Params>['invoke'],
  };
}

export function httpPatch<Data, Params = undefined>(
  path: string | ((params: Params) => string),
  mapBody?: (params: Params) => unknown
): ProviderLike<Data, Params> {
  return {
    provider: () => {},
    invoke: (async (params?: Params) => {
      const resolvedPath = typeof path === 'function' ? path(params!) : path;
      const body = mapBody ? mapBody(params!) : params;
      return httpRequest<Data>('PATCH', resolvedPath, body);
    }) as ProviderLike<Data, Params>['invoke'],
  };
}

export function httpDelete<Data, Params = undefined>(
  path: string | ((params: Params) => string)
): ProviderLike<Data, Params> {
  return {
    provider: () => {},
    invoke: (async (params?: Params) => {
      const resolvedPath = typeof path === 'function' ? path(params!) : path;
      return httpRequest<Data>('DELETE', resolvedPath);
    }) as ProviderLike<Data, Params>['invoke'],
  };
}

/**
 * Stub provider for features not yet implemented in the backend.
 * Returns a sensible default value and logs a warning.
 */
export function stubProvider<Data, Params = undefined>(name: string, defaultValue: Data): ProviderLike<Data, Params> {
  return {
    provider: () => {},
    invoke: (async (_params?: Params) => {
      console.warn(`[httpBridge] stub: ${name} not yet implemented in backend`);
      return defaultValue;
    }) as ProviderLike<Data, Params>['invoke'],
  };
}

// ---------------------------------------------------------------------------
// WebSocket singleton
// ---------------------------------------------------------------------------

type WsCallback = (data: unknown) => void;
type WsSubscription = {
  scope: 'conversation' | 'team' | 'cron' | 'channel' | 'extensions' | 'hub';
  id?: string;
};
const REALTIME_RECONNECTED_EVENT = 'realtime.reconnected';
const wsListeners = new Map<string, Set<WsCallback>>();
const wsSubscriptions = new Map<string, WsSubscription>();
let ws: WebSocket | null = null;
const wsAuthenticatedTokens = new WeakMap<WebSocket, string>();
let wsReconnectTimer: ReturnType<typeof setTimeout> | null = null;
let wsReconnectAttempt = 0;
let wsHasOpened = false;
let wsAuthenticated = false;
let wsAuthAcknowledged = false;
const wsReconnectSuppressedSockets = new WeakSet<WebSocket>();
const wsReadyWaiters = new Set<() => void>();

function dispatchWsEvent(eventName: string, payload: unknown): void {
  const handlers = wsListeners.get(eventName);
  if (!handlers) return;
  for (const handler of handlers) {
    try {
      handler(payload);
    } catch {
      /* never crash listener */
    }
  }
}

function isTerminalWsAuthEvent(eventName: string | undefined): boolean {
  return eventName === 'auth.revoked' || eventName === 'auth.failed';
}

function subscriptionKey(subscription: WsSubscription): string {
  return `${subscription.scope}:${subscription.id ?? '*'}`;
}

function subscriptionForEvent(eventName: string): WsSubscription | null {
  if (
    eventName.startsWith('message.') ||
    eventName.startsWith('turn.') ||
    eventName.startsWith('conversation.') ||
    eventName.startsWith('confirmation.') ||
    eventName.startsWith('fileStream.')
  ) {
    return { scope: 'conversation' };
  }
  if (eventName.startsWith('team.')) return { scope: 'team' };
  if (eventName.startsWith('cron.')) return { scope: 'cron' };
  if (eventName.startsWith('channel.')) return { scope: 'channel' };
  if (eventName.startsWith('extensions.')) return { scope: 'extensions' };
  if (eventName.startsWith('hub.')) return { scope: 'hub' };
  return null;
}

function rememberSubscription(eventName: string): void {
  const subscription = subscriptionForEvent(eventName);
  if (!subscription) return;
  wsSubscriptions.set(subscriptionKey(subscription), subscription);
  sendWsSubscription(subscription);
}

function sendWsSubscription(subscription: WsSubscription): void {
  if (!ws || ws.readyState !== WebSocket.OPEN || !wsAuthenticated) return;
  ws.send(JSON.stringify({ op: 'subscribe', ...subscription }));
}

function sendRememberedSubscriptions(socket: WebSocket): void {
  if (socket.readyState !== WebSocket.OPEN || !wsAuthenticated) return;
  for (const subscription of wsSubscriptions.values()) {
    socket.send(JSON.stringify({ op: 'subscribe', ...subscription }));
  }
}

function hasActiveWsListeners(): boolean {
  for (const handlers of wsListeners.values()) {
    if (handlers.size > 0) return true;
  }
  return false;
}

function clearWsReconnectTimer(): void {
  if (!wsReconnectTimer) return;
  clearTimeout(wsReconnectTimer);
  wsReconnectTimer = null;
}

function closeWsWithoutReconnect(): void {
  clearWsReconnectTimer();
  const current = ws;
  wsAuthenticated = false;
  wsAuthAcknowledged = false;
  if (!current) return;
  ws = null;
  wsReconnectSuppressedSockets.add(current);
  current.close();
}

function authenticateOpenWs(token: string): boolean {
  if (!ws || ws.readyState !== WebSocket.OPEN) return false;
  ws.send(JSON.stringify({ op: 'auth', access_token: token }));
  wsAuthenticatedTokens.set(ws, token);
  wsAuthenticated = true;
  wsAuthAcknowledged = false;
  sendRememberedSubscriptions(ws);
  return true;
}

function ensureWs(): void {
  if (typeof window === 'undefined') {
    console.debug('[ensureWs] skipped: no window');
    return;
  }
  if (!peekAccessToken()) {
    console.debug('[ensureWs] skipped: no broker token');
    return;
  }
  if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) {
    console.debug('[ensureWs] skipped: already open/connecting, readyState=', ws.readyState);
    return;
  }

  const url = getWsUrl();
  console.debug('[ensureWs] connecting to', url);
  try {
    ws = new WebSocket(url);
    wsAuthenticated = false;
    wsAuthAcknowledged = false;
  } catch (e) {
    console.error('[ensureWs] WebSocket constructor threw:', e);
    scheduleWsReconnect();
    return;
  }

  const current = ws;

  current.addEventListener('open', () => {
    console.debug('[ensureWs] CONNECTED');
    const isReconnect = wsHasOpened;
    wsReconnectAttempt = 0;
    void getAccessToken()
      .then((token) => {
        if (token && current.readyState === WebSocket.OPEN && ws === current) {
          current.send(JSON.stringify({ op: 'auth', access_token: token }));
          wsAuthenticatedTokens.set(current, token);
          wsAuthenticated = true;
          sendRememberedSubscriptions(current);
          wsHasOpened = true;
          if (isReconnect) {
            dispatchWsEvent(REALTIME_RECONNECTED_EVENT, { timestamp: Date.now() });
          }
        } else if (ws === current) {
          closeWsWithoutReconnect();
        }
      })
      .catch(() => {
        if (ws === current) {
          closeWsWithoutReconnect();
        }
      });
  });

  current.addEventListener('close', (e) => {
    console.debug('[ensureWs] CLOSED code=' + e.code + ' reason=' + e.reason);
    if (ws === current) {
      ws = null;
      wsAuthenticated = false;
      wsAuthAcknowledged = false;
    }
    if (wsReconnectSuppressedSockets.has(current)) {
      wsReconnectSuppressedSockets.delete(current);
      return;
    }
    scheduleWsReconnect();
  });

  current.addEventListener('error', (e) => {
    console.error('[ensureWs] ERROR', e);
    current.close();
  });

  current.addEventListener('message', (event: MessageEvent) => {
    try {
      const msg = JSON.parse(event.data as string) as {
        name?: string;
        event?: string;
        data?: unknown;
        payload?: unknown;
      };
      const eventName = msg.name ?? msg.event;
      const payload = msg.data ?? msg.payload;
      console.debug('[WS:msg]', eventName, JSON.stringify(payload).slice(0, 200));
      if (eventName) {
        if (eventName === 'auth.ok') {
          wsAuthAcknowledged = true;
          for (const resolve of wsReadyWaiters) resolve();
          wsReadyWaiters.clear();
        }
        dispatchWsEvent(eventName, payload);
        if (isTerminalWsAuthEvent(eventName)) {
          const socketToken = wsAuthenticatedTokens.get(current);
          if (ws === current && (!socketToken || peekAccessToken() === socketToken)) {
            clearAccessToken();
            wsHasOpened = false;
          } else {
            wsReconnectSuppressedSockets.add(current);
            current.close();
          }
          return;
        }
      }
    } catch {
      // ignore non-JSON
    }
  });
}

function scheduleWsReconnect(): void {
  if (wsReconnectTimer) return;
  if (!peekAccessToken()) return;
  const delay = Math.min(1000 * Math.pow(2, wsReconnectAttempt), 30000);
  wsReconnectAttempt++;
  wsReconnectTimer = setTimeout(() => {
    wsReconnectTimer = null;
    ensureWs();
  }, delay);
}

subscribeAccessToken((token) => {
  if (!token) {
    closeWsWithoutReconnect();
    wsHasOpened = false;
    return;
  }
  if (authenticateOpenWs(token)) return;
  if (hasActiveWsListeners()) ensureWs();
});

export function waitForWsReady(): Promise<void> {
  if (ws?.readyState === WebSocket.OPEN && wsAuthAcknowledged) return Promise.resolve();
  ensureWs();
  return new Promise((resolve) => {
    wsReadyWaiters.add(resolve);
  });
}

// ---------------------------------------------------------------------------
// Emitter factory (same shape as bridge.buildEmitter)
// ---------------------------------------------------------------------------

type EmitterLike<Params> = {
  on: (callback: Params extends undefined ? () => void : (params: Params) => void) => () => void;
  emit: Params extends undefined ? () => void : (params: Params) => void;
};

export function wsEmitter<Params = undefined>(eventName: string): EmitterLike<Params> {
  return {
    on: (callback: (params: Params) => void) => {
      ensureWs();
      if (!wsListeners.has(eventName)) {
        wsListeners.set(eventName, new Set());
      }
      const cb = callback as WsCallback;
      wsListeners.get(eventName)!.add(cb);
      rememberSubscription(eventName);
      return () => {
        wsListeners.get(eventName)?.delete(cb);
      };
    },
    emit: (() => {}) as EmitterLike<Params>['emit'],
  };
}

export function wsMappedEmitter<Params = undefined>(
  eventName: string,
  transform: (raw: unknown) => Params
): EmitterLike<Params> {
  const inner = wsEmitter<unknown>(eventName);
  return {
    on: (callback: (params: Params) => void) => {
      return inner.on((raw) => {
        callback(transform(raw));
      });
    },
    emit: (() => {}) as EmitterLike<Params>['emit'],
  };
}

/**
 * Stub emitter for events not yet implemented in the backend.
 */
export function stubEmitter<Params = undefined>(_name: string): EmitterLike<Params> {
  return {
    on: () => () => {},
    emit: (() => {}) as EmitterLike<Params>['emit'],
  };
}
