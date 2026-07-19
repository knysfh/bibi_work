/**
 * Shared in-memory broker for enterprise access tokens.
 *
 * The token is intentionally not persisted here. Desktop persistence, if needed,
 * belongs in Electron main process secure storage / OS keychain and should feed
 * this broker through setAccessTokenProvider().
 */

export type AccessTokenProvider = (
  forceRefresh?: boolean
) => string | null | undefined | Promise<string | null | undefined>;
export type AccessTokenListener = (token: string | null) => void;

declare global {
  // Dev/test escape hatch used by main/preload or integration tests. Do not use
  // localStorage/sessionStorage for bearer tokens.
  // eslint-disable-next-line no-var
  var __biworkAccessToken: string | undefined;
}

let accessToken: string | null = null;
let provider: AccessTokenProvider | null = null;
let sessionInvalidator: (() => void | Promise<void>) | null = null;
const listeners = new Set<AccessTokenListener>();
let expiryTimer: ReturnType<typeof setTimeout> | null = null;

const MAX_TIMER_DELAY_MS = 2_147_000_000;

function normalizeToken(token: string | null | undefined): string | null {
  const normalized = token?.trim();
  return normalized ? normalized : null;
}

export function accessTokenExpirationMs(token: string): number | null {
  const payload = token.split('.')[1];
  if (!payload || typeof globalThis.atob !== 'function') return null;
  try {
    const normalized = payload.replace(/-/g, '+').replace(/_/g, '/');
    const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, '=');
    const claims = JSON.parse(globalThis.atob(padded)) as { exp?: unknown };
    return typeof claims.exp === 'number' && Number.isFinite(claims.exp) ? claims.exp * 1000 : null;
  } catch {
    return null;
  }
}

export function isAccessTokenExpired(token: string, now = Date.now()): boolean {
  const expiresAt = accessTokenExpirationMs(token);
  return expiresAt !== null && expiresAt <= now;
}

function clearExpiryTimer(): void {
  if (!expiryTimer) return;
  clearTimeout(expiryTimer);
  expiryTimer = null;
}

function scheduleExpiry(token: string, expiresAt: number): void {
  clearExpiryTimer();
  const remaining = expiresAt - Date.now();
  if (remaining <= 0) {
    setAccessToken(null);
    return;
  }
  expiryTimer = setTimeout(
    () => {
      expiryTimer = null;
      if (peekAccessToken() !== token) return;
      if (Date.now() >= expiresAt) {
        if (provider) {
          void refreshAccessToken().catch((error) => {
            console.warn('[authTokenBroker] access token refresh failed at expiry', error);
          });
        } else {
          setAccessToken(null);
        }
      } else {
        scheduleExpiry(token, expiresAt);
      }
    },
    Math.min(remaining, MAX_TIMER_DELAY_MS)
  );
}

function notifyAccessTokenListeners(token: string | null): void {
  for (const listener of listeners) {
    try {
      listener(token);
    } catch (error) {
      console.warn('[authTokenBroker] access token listener failed', error);
    }
  }
}

export function subscribeAccessToken(listener: AccessTokenListener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function setAccessToken(token: string | null | undefined): void {
  let nextToken = normalizeToken(token);
  const expiresAt = nextToken ? accessTokenExpirationMs(nextToken) : null;
  if (nextToken && isAccessTokenExpired(nextToken)) nextToken = null;
  const previousToken = peekAccessToken();
  accessToken = nextToken;
  if (accessToken) {
    globalThis.__biworkAccessToken = accessToken;
  } else {
    delete globalThis.__biworkAccessToken;
  }
  clearExpiryTimer();
  if (accessToken && expiresAt !== null) scheduleExpiry(accessToken, expiresAt);
  if (previousToken !== accessToken) {
    notifyAccessTokenListeners(accessToken);
  }
}

export function clearAccessToken(): void {
  setAccessToken(null);
}

export function setAccessTokenProvider(nextProvider: AccessTokenProvider | null): void {
  provider = nextProvider;
}

export function setAuthSessionInvalidator(nextInvalidator: (() => void | Promise<void>) | null): void {
  sessionInvalidator = nextInvalidator;
}

export function peekAccessToken(): string | null {
  return accessToken ?? normalizeToken(globalThis.__biworkAccessToken);
}

export async function getAccessToken(): Promise<string | null> {
  if (provider) {
    const provided = normalizeToken(await provider(false));
    if (provided) {
      setAccessToken(provided);
      return peekAccessToken();
    }
    clearAccessToken();
    return null;
  }
  const cached = peekAccessToken();
  return cached && !isAccessTokenExpired(cached) ? cached : null;
}

export async function refreshAccessToken(): Promise<string | null> {
  if (!provider) {
    clearAccessToken();
    return null;
  }
  const refreshed = normalizeToken(await provider(true));
  if (!refreshed || isAccessTokenExpired(refreshed)) {
    clearAccessToken();
    return null;
  }
  setAccessToken(refreshed);
  return refreshed;
}

export async function invalidateAuthSession(): Promise<void> {
  try {
    await sessionInvalidator?.();
  } finally {
    clearAccessToken();
  }
}

export async function requestWithAuthorizationRetry(request: () => Promise<Response>): Promise<Response> {
  const response = await request();
  if (response.status !== 401) return response;
  const refreshed = await refreshAccessToken();
  if (!refreshed) return response;
  const retried = await request();
  if (retried.status === 401) await invalidateAuthSession();
  return retried;
}

export async function getAuthorizationHeaders(): Promise<Record<string, string>> {
  const token = await getAccessToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}
