import type { z } from "zod";
import { ApiError, AuthExpiredError, ContractError, ForbiddenError } from "./errors";
import type { TokenProvider } from "./token-provider";

export interface HttpClientOptions {
  baseUrl: string;
  tokenProvider: TokenProvider;
  fetchImpl?: typeof fetch;
}

export interface RequestOptions {
  auth?: boolean;
  query?: Record<string, string | number | boolean | null | undefined>;
  signal?: AbortSignal;
}

export interface HttpClient {
  get<T>(path: string, schema: z.ZodType<T>, options?: RequestOptions): Promise<T>;
  getRaw(path: string, options?: RequestOptions): Promise<Response>;
  post<T>(path: string, body: unknown, schema: z.ZodType<T>, options?: RequestOptions): Promise<T>;
  patch<T>(path: string, body: unknown, schema: z.ZodType<T>, options?: RequestOptions): Promise<T>;
  postRaw(path: string, body: unknown, options?: RequestOptions): Promise<Response>;
  baseUrl: string;
}

export function createHttpClient(options: HttpClientOptions): HttpClient {
  const fetcher = options.fetchImpl ?? fetch;
  const baseUrl = options.baseUrl.replace(/\/+$/, "");

  async function request<T>(
    method: "GET" | "POST" | "PATCH",
    path: string,
    schema: z.ZodType<T>,
    body?: unknown,
    requestOptions: RequestOptions = {}
  ): Promise<T> {
    const response = await rawRequest(method, path, body, requestOptions);
    const payload = await parseJson(response);
    const parsed = schema.safeParse(payload);
    if (!parsed.success) {
      throw new ContractError(`Response contract failed for ${method} ${path}`, parsed.error);
    }
    return parsed.data;
  }

  async function rawRequest(
    method: "GET" | "POST" | "PATCH",
    path: string,
    body?: unknown,
    requestOptions: RequestOptions = {}
  ): Promise<Response> {
    const auth = requestOptions.auth ?? true;
    const token = auth ? await options.tokenProvider.getAccessToken() : null;
    if (auth && !token) {
      throw new AuthExpiredError("No access token is available");
    }

    const response = await fetcher(buildUrl(baseUrl, path, requestOptions.query), {
      method,
      headers: {
        Accept: "application/json",
        ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        ...(token ? { Authorization: `Bearer ${token}` } : {})
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: requestOptions.signal
    });

    if (response.status === 401) {
      throw new AuthExpiredError(await responseMessage(response));
    }
    if (response.status === 403) {
      throw new ForbiddenError(await responseMessage(response));
    }
    if (!response.ok) {
      throw new ApiError(await responseMessage(response), response.status);
    }

    return response;
  }

  return {
    baseUrl,
    get: (path, schema, requestOptions) => request("GET", path, schema, undefined, requestOptions),
    getRaw: (path, requestOptions) => rawRequest("GET", path, undefined, requestOptions),
    post: (path, body, schema, requestOptions) =>
      request("POST", path, schema, body, requestOptions),
    patch: (path, body, schema, requestOptions) =>
      request("PATCH", path, schema, body, requestOptions),
    postRaw: (path, body, requestOptions) => rawRequest("POST", path, body, requestOptions)
  };
}

function buildUrl(
  baseUrl: string,
  path: string,
  query?: Record<string, string | number | boolean | null | undefined>
): string {
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  const url = new URL(`${baseUrl}${normalizedPath}`);
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value !== undefined && value !== null && value !== "") {
      url.searchParams.set(key, String(value));
    }
  }
  return url.toString();
}

async function parseJson(response: Response): Promise<unknown> {
  if (response.status === 204) {
    return null;
  }
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

async function responseMessage(response: Response): Promise<string> {
  const text = await response.text();
  if (!text) {
    return `${response.status} ${response.statusText}`;
  }
  try {
    const payload = JSON.parse(text) as { message?: string; error?: string; code?: string };
    return payload.message ?? payload.error ?? payload.code ?? text;
  } catch {
    return text;
  }
}
