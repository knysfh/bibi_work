import { createHash, randomBytes } from 'crypto';
import { buildOidcAuthorizationUrl, type OidcConfig } from '../../common/auth/oidcPkce';
import { accessTokenExpirationMs, isAccessTokenExpired } from '../../common/auth/authTokenBroker';
import { DESKTOP_OIDC_CALLBACK_PORT, type OidcLoopbackCallback } from '../utils/oidcLoopback';

const DEFAULT_LOGIN_TTL_MS = 10 * 60 * 1000;
const DEFAULT_REFRESH_LEAD_MS = 60_000;
const DEFAULT_REFRESH_RETRY_MS = 30_000;

type LoginState = {
  codeVerifier: string;
  config: OidcConfig;
  redirectUri: string;
  startedAt: number;
  state: string;
};

type TokenSet = {
  accessToken: string;
  expiresInSeconds: number | null;
  refreshToken: string | null;
};

export type DesktopRefreshTokenStoreLike = {
  clear: () => Promise<void>;
  load: () => Promise<string | null>;
  save: (token: string) => Promise<void>;
};

export type DesktopOidcControllerOptions = {
  apiUrl: (endpoint: string) => string;
  emitAccessTokenChanged?: (accessToken: string) => void;
  emitLoginCompleted: (accessToken: string) => void;
  emitLoginFailed: (message: string) => void;
  emitSessionExpired?: (reason: string) => void;
  getLoopbackPort: () => number | null;
  getStoredAccessToken: () => string | null;
  openExternal: (url: string) => Promise<unknown>;
  refreshTokenStore: DesktopRefreshTokenStoreLike;
  setStoredAccessToken: (accessToken: string | null) => void;
  fetchImpl?: typeof fetch;
  loginTtlMs?: number;
  now?: () => number;
  randomValue?: () => string;
  refreshLeadMs?: number;
  refreshRetryMs?: number;
  setTimeoutImpl?: typeof setTimeout;
  clearTimeoutImpl?: typeof clearTimeout;
};

function base64Url(buffer: Buffer): string {
  return buffer.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

function defaultRandomValue(): string {
  return base64Url(randomBytes(32));
}

function codeChallenge(codeVerifier: string): string {
  return base64Url(createHash('sha256').update(codeVerifier).digest());
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function envelopeData(body: unknown): unknown {
  return isRecord(body) && 'data' in body ? body.data : body;
}

function oidcConfigFromResponse(body: unknown): OidcConfig {
  const data = envelopeData(body);
  if (!isRecord(data) || typeof data.client_id !== 'string') {
    throw new Error('OIDC config response is invalid');
  }
  return data as OidcConfig;
}

function tokenSetFromResponse(body: unknown): TokenSet | null {
  const data = envelopeData(body);
  if (!isRecord(data)) return null;
  const accessToken = data.access_token;
  if (typeof accessToken !== 'string' || !accessToken.trim()) return null;
  const refreshToken = data.refresh_token;
  const expiresIn = data.expires_in;
  return {
    accessToken: accessToken.trim(),
    refreshToken: typeof refreshToken === 'string' && refreshToken.trim() ? refreshToken.trim() : null,
    expiresInSeconds: typeof expiresIn === 'number' && Number.isFinite(expiresIn) && expiresIn > 0 ? expiresIn : null,
  };
}

async function readJson(response: Response): Promise<unknown> {
  const raw = await response.text();
  if (!raw.trim()) return null;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return raw;
  }
}

function withOfflineAccess(config: OidcConfig): OidcConfig {
  const configured = config.scopes?.filter((scope) => scope.trim()) ?? ['openid', 'profile', 'email', 'roles'];
  return configured.includes('offline_access') ? config : { ...config, scopes: [...configured, 'offline_access'] };
}

export class DesktopOidcController {
  private accessToken: string | null = null;
  private accessTokenExpiresAt: number | null = null;
  private accessTokenRefreshAt: number | null = null;
  private config: OidcConfig | null = null;
  private loginState: LoginState | null = null;
  private refreshPromise: Promise<string | null> | null = null;
  private refreshTimer: ReturnType<typeof setTimeout> | null = null;
  private refreshToken: string | null = null;
  private refreshTokenLoaded = false;

  constructor(private readonly options: DesktopOidcControllerOptions) {}

  getAccessToken(): string | null {
    const accessToken = this.accessToken ?? this.options.getStoredAccessToken();
    if (accessToken && isAccessTokenExpired(accessToken, this.now())) {
      this.setAccessTokenState(null);
      return null;
    }
    return accessToken;
  }

  async getValidAccessToken(forceRefresh = false): Promise<string | null> {
    await this.loadRefreshToken();
    const accessToken = this.getAccessToken();
    const refreshDue = this.accessTokenRefreshAt !== null && this.now() >= this.accessTokenRefreshAt;
    if (this.refreshToken && (forceRefresh || !accessToken || refreshDue)) {
      return this.refreshAccessToken();
    }
    return accessToken;
  }

  setAccessToken(accessToken: string | null): void {
    this.setAccessTokenState(accessToken);
    if (accessToken) this.scheduleRefresh();
  }

  async startLogin(): Promise<{ authorizationUrl: string }> {
    const config = withOfflineAccess(await this.fetchConfig());
    const redirectUri =
      config.desktop_callback?.redirect_uri?.trim() ||
      `http://127.0.0.1:${this.options.getLoopbackPort() ?? DESKTOP_OIDC_CALLBACK_PORT}/callback`;
    const randomValue = this.options.randomValue ?? defaultRandomValue;
    const codeVerifier = randomValue();
    const state = randomValue();
    const authorizationUrl = buildOidcAuthorizationUrl({
      codeChallenge: codeChallenge(codeVerifier),
      config,
      redirectUri,
      state,
    });

    this.loginState = { codeVerifier, config, redirectUri, startedAt: this.now(), state };
    try {
      await this.options.openExternal(authorizationUrl);
    } catch (error) {
      this.loginState = null;
      throw error;
    }
    return { authorizationUrl };
  }

  async handleCallback(callback: OidcLoopbackCallback): Promise<void> {
    try {
      await this.exchangeCallback(callback);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      this.options.emitLoginFailed(message);
      throw error;
    }
  }

  async logout(): Promise<void> {
    await this.loadRefreshToken();
    const refreshToken = this.refreshToken;
    try {
      if (refreshToken) {
        const config = await this.fetchConfig();
        await (this.options.fetchImpl ?? fetch)(this.options.apiUrl('/api/auth/oidc/revoke'), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ client_id: config.client_id, refresh_token: refreshToken }),
        });
      }
    } finally {
      await this.clearSession(false);
    }
  }

  async invalidateSession(): Promise<void> {
    await this.clearSession(false, 'access_token_rejected');
  }

  private now(): number {
    return (this.options.now ?? Date.now)();
  }

  private async fetchConfig(): Promise<OidcConfig> {
    if (this.config) return this.config;
    const response = await (this.options.fetchImpl ?? fetch)(this.options.apiUrl('/api/auth/oidc/config'));
    const body = await readJson(response);
    if (!response.ok) throw new Error(`OIDC config request failed (${response.status})`);
    this.config = oidcConfigFromResponse(body);
    return this.config;
  }

  private async loadRefreshToken(): Promise<void> {
    if (this.refreshTokenLoaded) return;
    this.refreshToken = await this.options.refreshTokenStore.load();
    this.refreshTokenLoaded = true;
  }

  private setAccessTokenState(accessToken: string | null, expiresInSeconds: number | null = null): void {
    this.accessToken = accessToken;
    this.options.setStoredAccessToken(accessToken);
    if (!accessToken) {
      this.accessTokenExpiresAt = null;
      this.accessTokenRefreshAt = null;
      return;
    }
    const now = this.now();
    this.accessTokenExpiresAt =
      accessTokenExpirationMs(accessToken) ?? (expiresInSeconds ? now + expiresInSeconds * 1000 : null);
    if (this.accessTokenExpiresAt === null) {
      this.accessTokenRefreshAt = null;
      return;
    }
    const lifetime = Math.max(1_000, this.accessTokenExpiresAt - now);
    const lead = Math.min(this.options.refreshLeadMs ?? DEFAULT_REFRESH_LEAD_MS, Math.max(1_000, lifetime / 5));
    this.accessTokenRefreshAt = this.accessTokenExpiresAt - lead;
  }

  private async applyTokenSet(tokenSet: TokenSet, notifyRenderer: boolean): Promise<string> {
    if (tokenSet.refreshToken) {
      this.refreshToken = tokenSet.refreshToken;
      this.refreshTokenLoaded = true;
      await this.options.refreshTokenStore.save(tokenSet.refreshToken);
    }
    this.setAccessTokenState(tokenSet.accessToken, tokenSet.expiresInSeconds);
    this.scheduleRefresh();
    if (notifyRenderer) this.options.emitAccessTokenChanged?.(tokenSet.accessToken);
    return tokenSet.accessToken;
  }

  private scheduleRefresh(delayOverride?: number): void {
    this.clearRefreshTimer();
    if (!this.refreshToken && !this.refreshTokenLoaded) {
      void this.loadRefreshToken().then(() => this.scheduleRefresh());
      return;
    }
    if (!this.refreshToken) return;
    if (delayOverride === undefined && this.accessTokenRefreshAt === null) return;
    const delay = delayOverride ?? Math.max(0, this.accessTokenRefreshAt! - this.now());
    this.refreshTimer = (this.options.setTimeoutImpl ?? setTimeout)(
      () => {
        this.refreshTimer = null;
        void this.refreshAccessToken().catch(() => {
          this.scheduleRefresh(this.options.refreshRetryMs ?? DEFAULT_REFRESH_RETRY_MS);
        });
      },
      Math.max(1, delay)
    );
  }

  private clearRefreshTimer(): void {
    if (!this.refreshTimer) return;
    (this.options.clearTimeoutImpl ?? clearTimeout)(this.refreshTimer);
    this.refreshTimer = null;
  }

  private async refreshAccessToken(): Promise<string | null> {
    if (this.refreshPromise) return this.refreshPromise;
    this.refreshPromise = this.performRefresh().finally(() => {
      this.refreshPromise = null;
    });
    return this.refreshPromise;
  }

  private async performRefresh(): Promise<string | null> {
    await this.loadRefreshToken();
    if (!this.refreshToken) return this.getAccessToken();
    const config = await this.fetchConfig();
    const response = await (this.options.fetchImpl ?? fetch)(this.options.apiUrl('/api/auth/oidc/token'), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        client_id: config.client_id,
        grant_type: 'refresh_token',
        refresh_token: this.refreshToken,
      }),
    });
    const body = await readJson(response);
    if (!response.ok) {
      if (response.status === 400 || response.status === 401) {
        await this.clearSession(true, 'refresh_token_rejected');
        return null;
      }
      throw new Error(`OIDC token refresh failed (${response.status})`);
    }
    const tokenSet = tokenSetFromResponse(body);
    if (!tokenSet) throw new Error('OIDC refresh response is missing access_token');
    return this.applyTokenSet(tokenSet, true);
  }

  private async clearSession(notifyRenderer: boolean, reason = 'session_expired'): Promise<void> {
    this.clearRefreshTimer();
    this.setAccessTokenState(null);
    this.refreshToken = null;
    this.refreshTokenLoaded = true;
    await this.options.refreshTokenStore.clear();
    if (notifyRenderer) this.options.emitSessionExpired?.(reason);
  }

  private async exchangeCallback(callback: OidcLoopbackCallback): Promise<void> {
    const loginState = this.loginState;
    if (!loginState) {
      const accessToken = await this.getValidAccessToken();
      if (accessToken) {
        this.options.emitLoginCompleted(accessToken);
        return;
      }
      throw new Error('OIDC login was not started from BiWork');
    }
    if (this.now() - loginState.startedAt > (this.options.loginTtlMs ?? DEFAULT_LOGIN_TTL_MS)) {
      this.loginState = null;
      throw new Error('OIDC login state expired');
    }
    if (callback.error) {
      this.loginState = null;
      throw new Error(callback.error_description || callback.error);
    }
    if (!callback.code || !callback.state) throw new Error('OIDC callback is missing code or state');
    if (callback.state !== loginState.state) {
      this.loginState = null;
      throw new Error('OIDC state mismatch');
    }

    const tokenEndpoint = loginState.config.token_exchange_endpoint ?? '/api/auth/oidc/token';
    const response = await (this.options.fetchImpl ?? fetch)(this.options.apiUrl(tokenEndpoint), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        client_id: loginState.config.client_id,
        code: callback.code,
        code_verifier: loginState.codeVerifier,
        grant_type: 'authorization_code',
        redirect_uri: loginState.redirectUri,
      }),
    });
    const body = await readJson(response);
    if (!response.ok) {
      this.loginState = null;
      throw new Error(`OIDC token exchange failed (${response.status})`);
    }
    const tokenSet = tokenSetFromResponse(body);
    if (!tokenSet) {
      this.loginState = null;
      throw new Error('OIDC token response is missing access_token');
    }
    const accessToken = await this.applyTokenSet(tokenSet, false);
    this.loginState = null;
    this.options.emitLoginCompleted(accessToken);
  }
}
