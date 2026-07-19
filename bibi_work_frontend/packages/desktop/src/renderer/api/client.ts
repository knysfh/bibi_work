/**
 * HTTP client factory for communicating with the BiWork backend server.
 *
 * Usage:
 *   const api = createApiClient('http://127.0.0.1:9123')
 *   const data = await api.get<Foo>('/api/foo')
 */

import { getAuthorizationHeaders, requestWithAuthorizationRetry } from '@/common/auth/authTokenBroker';

export class ApiError extends Error {
  readonly code: string;
  readonly backendMessage: string;
  readonly details: unknown;
  readonly traceId: string;

  constructor(
    public readonly status: number,
    public readonly statusText: string,
    public readonly body: unknown
  ) {
    super(`API error ${status}: ${statusText}`);
    this.name = 'ApiError';
    this.code = '';
    this.backendMessage = '';
    this.details = undefined;
    this.traceId = '';

    if (body && typeof body === 'object') {
      const parsed = body as {
        code?: unknown;
        error?: unknown;
        message?: unknown;
        details?: unknown;
        trace_id?: unknown;
        traceId?: unknown;
      };
      if (typeof parsed.code === 'string') this.code = parsed.code;
      if (typeof parsed.error === 'string') this.backendMessage = parsed.error;
      if (!this.backendMessage && typeof parsed.message === 'string') this.backendMessage = parsed.message;
      if (typeof parsed.trace_id === 'string') this.traceId = parsed.trace_id;
      if (!this.traceId && typeof parsed.traceId === 'string') this.traceId = parsed.traceId;
      this.details = parsed.details;
    } else if (typeof body === 'string') {
      this.backendMessage = body;
    }
  }
}

type RequestOptions = {
  headers?: Record<string, string>;
  signal?: AbortSignal;
};

async function request<T>(
  baseURL: string,
  method: string,
  path: string,
  body?: unknown,
  options?: RequestOptions
): Promise<T> {
  const url = `${baseURL}${path}`;
  const hasExplicitAuthorization = Object.keys(options?.headers ?? {}).some(
    (key) => key.toLowerCase() === 'authorization'
  );
  const send = async () => {
    const headers: Record<string, string> = {
      ...(hasExplicitAuthorization ? {} : await getAuthorizationHeaders()),
      ...options?.headers,
    };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    return fetch(url, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
      signal: options?.signal,
    });
  };
  const response = hasExplicitAuthorization ? await send() : await requestWithAuthorizationRetry(send);

  if (!response.ok) {
    // Response body can only be consumed once — read as text, then try JSON
    const rawText = await response.text().catch(() => '');
    let errorBody: unknown;
    try {
      errorBody = JSON.parse(rawText);
    } catch {
      errorBody = rawText;
    }
    throw new ApiError(response.status, response.statusText, errorBody);
  }

  const contentType = response.headers.get('Content-Type');
  if (contentType?.includes('application/json')) return (await response.json()) as T;
  return undefined as T;
}

export function createApiClient(baseURL: string) {
  return {
    get: <T>(path: string, options?: RequestOptions) => request<T>(baseURL, 'GET', path, undefined, options),
    post: <T>(path: string, body?: unknown, options?: RequestOptions) =>
      request<T>(baseURL, 'POST', path, body, options),
    put: <T>(path: string, body?: unknown, options?: RequestOptions) => request<T>(baseURL, 'PUT', path, body, options),
    patch: <T>(path: string, body?: unknown, options?: RequestOptions) =>
      request<T>(baseURL, 'PATCH', path, body, options),
    delete: <T>(path: string, options?: RequestOptions) => request<T>(baseURL, 'DELETE', path, undefined, options),
  };
}
