import { afterEach, describe, expect, it, vi } from 'vitest';
import { clearAccessToken, peekAccessToken } from '@/common/auth/authTokenBroker';
import {
  buildOidcAuthorizationUrl,
  completeOidcRedirectIfPresent,
  oidcCallbackParamsToHref,
  startOidcLogin,
  type OidcConfig,
} from '@/common/auth/oidcPkce';

class MemoryStorage implements Storage {
  private readonly data = new Map<string, string>();

  get length(): number {
    return this.data.size;
  }

  clear(): void {
    this.data.clear();
  }

  getItem(key: string): string | null {
    return this.data.get(key) ?? null;
  }

  key(index: number): string | null {
    return [...this.data.keys()][index] ?? null;
  }

  removeItem(key: string): void {
    this.data.delete(key);
  }

  setItem(key: string, value: string): void {
    this.data.set(key, value);
  }
}

const oidcConfig: OidcConfig = {
  authorization_endpoint: 'http://127.0.0.1:3333/realms/bibi-work/protocol/openid-connect/auth',
  client_id: 'bibi-work-desktop',
  scopes: ['openid', 'profile', 'email', 'roles'],
  token_endpoint: 'http://127.0.0.1:3333/realms/bibi-work/protocol/openid-connect/token',
  token_exchange_endpoint: '/api/auth/oidc/token',
};

afterEach(() => {
  clearAccessToken();
  vi.restoreAllMocks();
});

describe('OIDC PKCE helpers', () => {
  it('builds a FerrisKey authorization URL with PKCE parameters', () => {
    const url = new URL(
      buildOidcAuthorizationUrl({
        codeChallenge: 'challenge-123',
        config: oidcConfig,
        redirectUri: 'http://127.0.0.1:25808/auth/callback',
        state: 'state-123',
      })
    );

    expect(url.origin + url.pathname).toBe(oidcConfig.authorization_endpoint);
    expect(url.searchParams.get('client_id')).toBe('bibi-work-desktop');
    expect(url.searchParams.get('redirect_uri')).toBe('http://127.0.0.1:25808/auth/callback');
    expect(url.searchParams.get('response_type')).toBe('code');
    expect(url.searchParams.get('scope')).toBe('openid profile email roles');
    expect(url.searchParams.get('state')).toBe('state-123');
    expect(url.searchParams.get('code_challenge')).toBe('challenge-123');
    expect(url.searchParams.get('code_challenge_method')).toBe('S256');
  });

  it('stores transient PKCE state and exchanges an auth callback for an access token', async () => {
    const storage = new MemoryStorage();
    let redirectTarget = '';
    const authUrl = await startOidcLogin(oidcConfig, {
      location: { origin: 'http://127.0.0.1:25808' },
      navigate: (url) => {
        redirectTarget = url;
      },
      storage,
    });
    const state = new URL(authUrl).searchParams.get('state');

    expect(redirectTarget).toBe(authUrl);
    expect(state).toBeTruthy();

    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ success: true, data: { access_token: 'access-token-1' } }), {
        headers: { 'content-type': 'application/json' },
        status: 200,
      })
    );
    const history = { replaceState: vi.fn() };

    await expect(
      completeOidcRedirectIfPresent(oidcConfig, {
        fetch: fetchMock,
        history,
        location: {
          href: `http://127.0.0.1:25808/auth/callback?code=code-1&state=${encodeURIComponent(state!)}`,
        },
        storage,
        tokenExchangeEndpoint: '/api/auth/oidc/token',
      })
    ).resolves.toBe(true);

    expect(peekAccessToken()).toBe('access-token-1');
    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, request] = fetchMock.mock.calls[0];
    expect(url).toBe('/api/auth/oidc/token');
    expect((request as RequestInit).headers).toEqual({ 'content-type': 'application/json' });
    const body = JSON.parse(String((request as RequestInit).body));
    expect(body).toMatchObject({
      client_id: 'bibi-work-desktop',
      code: 'code-1',
      grant_type: 'authorization_code',
      redirect_uri: 'http://127.0.0.1:25808/auth/callback',
    });
    expect(body.code_verifier).toEqual(expect.any(String));
    expect(history.replaceState).toHaveBeenCalledWith({}, '', '/#/guid');
  });

  it('rejects mismatched state and does not set a token', async () => {
    const storage = new MemoryStorage();
    await startOidcLogin(oidcConfig, {
      location: { origin: 'http://127.0.0.1:25808' },
      navigate: () => {},
      storage,
    });

    await expect(
      completeOidcRedirectIfPresent(oidcConfig, {
        fetch: vi.fn(),
        location: {
          href: 'http://127.0.0.1:25808/auth/callback?code=code-1&state=wrong-state',
        },
        storage,
      })
    ).rejects.toThrow('OIDC state mismatch');

    expect(peekAccessToken()).toBeNull();
    expect(storage.length).toBe(0);
  });

  it('normalizes desktop deep-link callback params into the same callback URL shape', () => {
    expect(oidcCallbackParamsToHref({ code: 'code-1', state: 'state-1', ignored: 'x' })).toBe(
      'http://127.0.0.1/auth/callback?code=code-1&state=state-1'
    );
  });
});
