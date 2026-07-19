import { describe, expect, it, vi } from 'vitest';
import { DesktopOidcController } from '@process/auth/desktopOidcController';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function createHarness() {
  let storedToken: string | null = null;
  let storedRefreshToken: string | null = null;
  let now = 1_000;
  const emitAccessTokenChanged = vi.fn();
  const emitLoginCompleted = vi.fn();
  const emitLoginFailed = vi.fn();
  const emitSessionExpired = vi.fn();
  const openExternal = vi.fn(async () => undefined);
  const fetchImpl = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(
      jsonResponse({
        data: {
          authorization_endpoint: 'http://identity.test/authorize',
          client_id: 'desktop-client',
          scopes: ['openid', 'profile'],
          token_exchange_endpoint: '/api/auth/oidc/token',
        },
      })
    )
    .mockResolvedValueOnce(
      jsonResponse({ data: { access_token: 'access-token', expires_in: 300, refresh_token: 'refresh-token' } })
    );
  const randomValues = ['verifier', 'state'];
  const controller = new DesktopOidcController({
    apiUrl: (endpoint) => `http://127.0.0.1:1420${endpoint}`,
    emitAccessTokenChanged,
    emitLoginCompleted,
    emitLoginFailed,
    emitSessionExpired,
    fetchImpl,
    getLoopbackPort: () => 45123,
    getStoredAccessToken: () => storedToken,
    now: () => now,
    openExternal,
    randomValue: () => randomValues.shift() ?? 'fallback',
    refreshTokenStore: {
      clear: vi.fn(async () => {
        storedRefreshToken = null;
      }),
      load: vi.fn(async () => storedRefreshToken),
      save: vi.fn(async (token: string) => {
        storedRefreshToken = token;
      }),
    },
    setTimeoutImpl: vi.fn(() => 1 as unknown as ReturnType<typeof setTimeout>),
    clearTimeoutImpl: vi.fn(),
    setStoredAccessToken: (token) => {
      storedToken = token;
    },
  });

  return {
    controller,
    emitAccessTokenChanged,
    emitLoginCompleted,
    emitLoginFailed,
    emitSessionExpired,
    fetchImpl,
    openExternal,
    setNow: (value: number) => {
      now = value;
    },
    storedToken: () => storedToken,
    storedRefreshToken: () => storedRefreshToken,
  };
}

describe('DesktopOidcController', () => {
  it('owns PKCE state, exchanges the callback, and persists the token', async () => {
    const harness = createHarness();
    const { authorizationUrl } = await harness.controller.startLogin();
    const url = new URL(authorizationUrl);

    expect(url.origin + url.pathname).toBe('http://identity.test/authorize');
    expect(url.searchParams.get('state')).toBe('state');
    expect(url.searchParams.get('redirect_uri')).toBe('http://127.0.0.1:45123/callback');
    expect(url.searchParams.get('scope')).toBe('openid profile offline_access');
    expect(harness.openExternal).toHaveBeenCalledWith(authorizationUrl);

    await harness.controller.handleCallback({ code: 'authorization-code', state: 'state' });

    expect(harness.storedToken()).toBe('access-token');
    expect(harness.storedRefreshToken()).toBe('refresh-token');
    expect(harness.controller.getAccessToken()).toBe('access-token');
    expect(harness.emitLoginCompleted).toHaveBeenCalledWith('access-token');
    expect(harness.emitLoginFailed).not.toHaveBeenCalled();
    expect(JSON.parse(String(harness.fetchImpl.mock.calls[1]?.[1]?.body))).toMatchObject({
      code: 'authorization-code',
      code_verifier: 'verifier',
      redirect_uri: 'http://127.0.0.1:45123/callback',
    });
  });

  it('fails closed on state mismatch and reports the failure', async () => {
    const harness = createHarness();
    await harness.controller.startLogin();

    await expect(harness.controller.handleCallback({ code: 'code', state: 'wrong' })).rejects.toThrow(
      'OIDC state mismatch'
    );
    expect(harness.emitLoginFailed).toHaveBeenCalledWith('OIDC state mismatch');
    expect(harness.fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('rejects expired login state before token exchange', async () => {
    const harness = createHarness();
    await harness.controller.startLogin();
    harness.setNow(1_000 + 10 * 60 * 1_000 + 1);

    await expect(harness.controller.handleCallback({ code: 'code', state: 'state' })).rejects.toThrow(
      'OIDC login state expired'
    );
    expect(harness.fetchImpl).toHaveBeenCalledTimes(1);
  });

  it('treats a duplicate callback as success only when a token is already present', async () => {
    const harness = createHarness();
    harness.controller.setAccessToken('restored-token');

    await harness.controller.handleCallback({ code: 'duplicate', state: 'duplicate' });

    expect(harness.emitLoginCompleted).toHaveBeenCalledWith('restored-token');
    expect(harness.fetchImpl).not.toHaveBeenCalled();
  });

  it('drops an expired stored access token before the main process can reuse it', () => {
    const harness = createHarness();
    const payload = Buffer.from(JSON.stringify({ exp: 1 })).toString('base64url');
    harness.controller.setAccessToken(`header.${payload}.signature`);
    harness.setNow(1_001);

    expect(harness.controller.getAccessToken()).toBeNull();
    expect(harness.storedToken()).toBeNull();
  });

  it('restores a persisted refresh token and coalesces concurrent refresh requests', async () => {
    let storedAccessToken: string | null = null;
    let storedRefreshToken: string | null = 'refresh-old';
    const refreshTokenStore = {
      clear: vi.fn(async () => {
        storedRefreshToken = null;
      }),
      load: vi.fn(async () => storedRefreshToken),
      save: vi.fn(async (token: string) => {
        storedRefreshToken = token;
      }),
    };
    let resolveRefresh!: (response: Response) => void;
    const refreshResponse = new Promise<Response>((resolve) => {
      resolveRefresh = resolve;
    });
    const fetchImpl = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(
        jsonResponse({ data: { client_id: 'desktop-client', token_exchange_endpoint: '/api/auth/oidc/token' } })
      )
      .mockReturnValueOnce(refreshResponse);
    const emitAccessTokenChanged = vi.fn();
    const controller = new DesktopOidcController({
      apiUrl: (endpoint) => `http://127.0.0.1:1420${endpoint}`,
      emitAccessTokenChanged,
      emitLoginCompleted: vi.fn(),
      emitLoginFailed: vi.fn(),
      getLoopbackPort: () => 45123,
      getStoredAccessToken: () => storedAccessToken,
      openExternal: vi.fn(),
      refreshTokenStore,
      setStoredAccessToken: (token) => {
        storedAccessToken = token;
      },
      fetchImpl,
      setTimeoutImpl: vi.fn(() => 1 as unknown as ReturnType<typeof setTimeout>),
      clearTimeoutImpl: vi.fn(),
    });

    const first = controller.getValidAccessToken(true);
    const second = controller.getValidAccessToken(true);
    resolveRefresh(
      jsonResponse({ data: { access_token: 'access-new', expires_in: 300, refresh_token: 'refresh-rotated' } })
    );

    await expect(Promise.all([first, second])).resolves.toEqual(['access-new', 'access-new']);
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    expect(storedRefreshToken).toBe('refresh-rotated');
    expect(emitAccessTokenChanged).toHaveBeenCalledWith('access-new');
  });

  it('clears the persisted session when the refresh token is rejected', async () => {
    let storedRefreshToken: string | null = 'refresh-rejected';
    const emitSessionExpired = vi.fn();
    const controller = new DesktopOidcController({
      apiUrl: (endpoint) => endpoint,
      emitLoginCompleted: vi.fn(),
      emitLoginFailed: vi.fn(),
      emitSessionExpired,
      getLoopbackPort: () => 45123,
      getStoredAccessToken: () => null,
      openExternal: vi.fn(),
      refreshTokenStore: {
        load: vi.fn(async () => storedRefreshToken),
        save: vi.fn(async () => {}),
        clear: vi.fn(async () => {
          storedRefreshToken = null;
        }),
      },
      setStoredAccessToken: vi.fn(),
      fetchImpl: vi
        .fn<typeof fetch>()
        .mockResolvedValueOnce(jsonResponse({ data: { client_id: 'desktop-client' } }))
        .mockResolvedValueOnce(jsonResponse({ code: 'UNAUTHORIZED' }, 401)),
    });

    await expect(controller.getValidAccessToken(true)).resolves.toBeNull();
    expect(storedRefreshToken).toBeNull();
    expect(emitSessionExpired).toHaveBeenCalledWith('refresh_token_rejected');
  });
});
