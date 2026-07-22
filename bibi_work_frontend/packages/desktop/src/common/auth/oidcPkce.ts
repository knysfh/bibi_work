import { getBaseUrl } from '../adapter/httpBridge';
import { setAccessToken } from './authTokenBroker';

export type OidcConfig = {
  issuer?: string;
  client_id: string;
  authorization_endpoint?: string;
  token_endpoint?: string;
  token_exchange_endpoint?: string;
  scopes?: string[];
  desktop_callback?: { redirect_uri?: string };
  web_callback?: { redirect_uri?: string };
};

type StoredPkceState = {
  codeVerifier: string;
  createdAt: number;
  redirectUri: string;
  state: string;
};

type BrowserLocationLike = {
  hash?: string;
  href?: string;
  origin?: string;
  pathname?: string;
  search?: string;
};

export type OidcRuntime = {
  crypto?: Crypto;
  fetch?: typeof fetch;
  history?: Pick<History, 'replaceState'>;
  location?: BrowserLocationLike;
  navigate?: (url: string) => void;
  storage?: Storage;
  tokenExchangeEndpoint?: string;
};

const PKCE_STORAGE_KEY = 'biwork:oidc:pkce';
const PKCE_TTL_MS = 10 * 60 * 1000;

function isElectronRenderer(): boolean {
  return typeof window !== 'undefined' && Boolean((window as Window & { electronAPI?: unknown }).electronAPI);
}

function getBrowserLocation(runtime?: OidcRuntime): BrowserLocationLike {
  if (runtime?.location) return runtime.location;
  if (typeof window !== 'undefined') return window.location;
  return {};
}

function getBrowserStorage(runtime?: OidcRuntime): Storage {
  const storage = runtime?.storage ?? (typeof window !== 'undefined' ? window.sessionStorage : undefined);
  if (!storage) {
    throw new Error('OIDC session storage is not available');
  }
  return storage;
}

function getBrowserCrypto(runtime?: OidcRuntime): Crypto {
  const cryptoImpl = runtime?.crypto ?? globalThis.crypto;
  if (!cryptoImpl?.getRandomValues || !cryptoImpl.subtle) {
    throw new Error('OIDC PKCE requires Web Crypto');
  }
  return cryptoImpl;
}

function getBrowserFetch(runtime?: OidcRuntime): typeof fetch {
  const fetchImpl = runtime?.fetch ?? globalThis.fetch;
  if (!fetchImpl) {
    throw new Error('OIDC token exchange requires fetch');
  }
  return fetchImpl.bind(globalThis);
}

function base64Url(bytes: Uint8Array): string {
  let base64: string;
  if (typeof Buffer !== 'undefined') {
    base64 = Buffer.from(bytes).toString('base64');
  } else {
    let binary = '';
    bytes.forEach((byte) => {
      binary += String.fromCharCode(byte);
    });
    base64 = btoa(binary);
  }
  return base64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

function randomPkceValue(cryptoImpl: Crypto): string {
  const bytes = new Uint8Array(32);
  cryptoImpl.getRandomValues(bytes);
  return base64Url(bytes);
}

async function codeChallenge(verifier: string, cryptoImpl: Crypto): Promise<string> {
  const digest = await cryptoImpl.subtle.digest('SHA-256', new TextEncoder().encode(verifier));
  return base64Url(new Uint8Array(digest));
}

function callbackUriFromLocation(location: BrowserLocationLike): string | null {
  const origin = location.origin?.trim();
  if (!origin || origin === 'null') return null;
  return `${origin}/auth/callback`;
}

export function resolveOidcRedirectUri(config: OidcConfig, runtime?: OidcRuntime): string {
  if (isElectronRenderer() && config.desktop_callback?.redirect_uri) {
    return config.desktop_callback.redirect_uri;
  }
  return (
    callbackUriFromLocation(getBrowserLocation(runtime)) ??
    config.web_callback?.redirect_uri ??
    config.desktop_callback?.redirect_uri ??
    ''
  );
}

function requiredString(value: string | undefined, label: string): string {
  const normalized = value?.trim();
  if (!normalized) throw new Error(`${label} is required`);
  return normalized;
}

function scopes(config: OidcConfig): string {
  const configured = config.scopes?.filter((scope) => scope.trim()) ?? [];
  return configured.length > 0 ? configured.join(' ') : 'openid profile email roles';
}

function localApiUrl(path: string): string {
  if (/^https?:\/\//i.test(path)) return path;
  const normalizedPath = path.startsWith('/') ? path : `/${path}`;
  const baseUrl = getBaseUrl();
  return baseUrl ? `${baseUrl}${normalizedPath}` : normalizedPath;
}

function tokenExchangeEndpoint(config: OidcConfig, runtime?: OidcRuntime): string {
  if (runtime?.tokenExchangeEndpoint) return runtime.tokenExchangeEndpoint;
  return localApiUrl(config.token_exchange_endpoint ?? '/api/auth/oidc/token');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object';
}

function accessTokenFromExchangeResponse(payload: unknown): string | null {
  const record = isRecord(payload) ? payload : undefined;
  const tokenContainer = isRecord(record?.data) ? record.data : record;
  const accessToken = tokenContainer?.access_token;
  return typeof accessToken === 'string' && accessToken.trim() ? accessToken : null;
}

export function buildOidcAuthorizationUrl(input: {
  codeChallenge: string;
  config: OidcConfig;
  redirectUri: string;
  state: string;
}): string {
  const endpoint = requiredString(input.config.authorization_endpoint ?? input.config.issuer, 'authorization_endpoint');
  const url = new URL(endpoint);
  url.searchParams.set('client_id', requiredString(input.config.client_id, 'client_id'));
  url.searchParams.set('redirect_uri', requiredString(input.redirectUri, 'redirect_uri'));
  url.searchParams.set('response_type', 'code');
  const requestedScopes = scopes(input.config);
  url.searchParams.set('scope', requestedScopes);
  if (requestedScopes.split(/\s+/).includes('offline_access')) {
    url.searchParams.set('prompt', 'consent');
  }
  url.searchParams.set('state', input.state);
  url.searchParams.set('code_challenge', input.codeChallenge);
  url.searchParams.set('code_challenge_method', 'S256');
  return url.toString();
}

export async function startOidcLogin(config: OidcConfig, runtime?: OidcRuntime): Promise<string> {
  const cryptoImpl = getBrowserCrypto(runtime);
  const redirectUri = resolveOidcRedirectUri(config, runtime);
  const codeVerifier = randomPkceValue(cryptoImpl);
  const state = randomPkceValue(cryptoImpl);
  const challenge = await codeChallenge(codeVerifier, cryptoImpl);

  getBrowserStorage(runtime).setItem(
    PKCE_STORAGE_KEY,
    JSON.stringify({
      codeVerifier,
      createdAt: Date.now(),
      redirectUri,
      state,
    } satisfies StoredPkceState)
  );

  const target = buildOidcAuthorizationUrl({ codeChallenge: challenge, config, redirectUri, state });
  if (runtime?.navigate) {
    runtime.navigate(target);
  } else if (typeof window !== 'undefined') {
    window.location.assign(target);
  }
  return target;
}

export function clearOidcLoginState(runtime?: OidcRuntime): void {
  try {
    getBrowserStorage(runtime).removeItem(PKCE_STORAGE_KEY);
  } catch {
    /* ignore */
  }
}

function currentUrl(runtime?: OidcRuntime): URL {
  const location = getBrowserLocation(runtime);
  if (location.href) return new URL(location.href);
  const origin = location.origin ?? 'http://127.0.0.1';
  return new URL(`${origin}${location.pathname ?? ''}${location.search ?? ''}${location.hash ?? ''}`);
}

function loadStoredState(runtime?: OidcRuntime): StoredPkceState {
  const raw = getBrowserStorage(runtime).getItem(PKCE_STORAGE_KEY);
  if (!raw) throw new Error('OIDC login state is missing');
  const parsed = JSON.parse(raw) as Partial<StoredPkceState>;
  if (!parsed.state || !parsed.codeVerifier || !parsed.redirectUri || typeof parsed.createdAt !== 'number') {
    throw new Error('OIDC login state is invalid');
  }
  if (Date.now() - parsed.createdAt > PKCE_TTL_MS) {
    clearOidcLoginState(runtime);
    throw new Error('OIDC login state expired');
  }
  return parsed as StoredPkceState;
}

export function isOidcRedirectCallback(runtime?: OidcRuntime): boolean {
  const url = currentUrl(runtime);
  return url.pathname === '/auth/callback' && (url.searchParams.has('code') || url.searchParams.has('error'));
}

export function oidcCallbackParamsToHref(params: Record<string, string>): string {
  const url = new URL('http://127.0.0.1/auth/callback');
  for (const key of ['code', 'state', 'error', 'error_description']) {
    const value = params[key];
    if (value) url.searchParams.set(key, value);
  }
  return url.toString();
}

export async function completeOidcRedirectIfPresent(config: OidcConfig, runtime?: OidcRuntime): Promise<boolean> {
  if (!isOidcRedirectCallback(runtime)) return false;

  const url = currentUrl(runtime);
  const error = url.searchParams.get('error');
  if (error) {
    clearOidcLoginState(runtime);
    throw new Error(url.searchParams.get('error_description') ?? error);
  }

  const code = requiredString(url.searchParams.get('code') ?? undefined, 'code');
  const state = requiredString(url.searchParams.get('state') ?? undefined, 'state');
  const stored = loadStoredState(runtime);
  if (state !== stored.state) {
    clearOidcLoginState(runtime);
    throw new Error('OIDC state mismatch');
  }

  const body = {
    client_id: requiredString(config.client_id, 'client_id'),
    code,
    code_verifier: stored.codeVerifier,
    grant_type: 'authorization_code',
    redirect_uri: stored.redirectUri,
  };

  const response = await getBrowserFetch(runtime)(tokenExchangeEndpoint(config, runtime), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    clearOidcLoginState(runtime);
    throw new Error(`OIDC token exchange failed (${response.status})`);
  }

  const accessToken = accessTokenFromExchangeResponse(await response.json());
  if (!accessToken) {
    clearOidcLoginState(runtime);
    throw new Error('OIDC token response is missing access_token');
  }

  setAccessToken(accessToken);
  clearOidcLoginState(runtime);
  const history = runtime?.history ?? (typeof window !== 'undefined' ? window.history : undefined);
  history?.replaceState({}, '', '/#/guid');
  return true;
}
