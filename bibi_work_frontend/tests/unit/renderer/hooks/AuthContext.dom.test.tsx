/**
 * @vitest-environment jsdom
 */

import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import React from 'react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AuthProvider, useAuth } from '@/renderer/hooks/context/AuthContext';

const mocks = vi.hoisted(() => ({
  httpRequest: vi.fn(),
  clearAccessToken: vi.fn(),
  setAccessToken: vi.fn(),
  subscribeAccessToken: vi.fn(() => () => {}),
  configInitialize: vi.fn(),
  clearSessionCache: vi.fn(),
  clearOidcLoginState: vi.fn(),
  completeOidcRedirectIfPresent: vi.fn(),
  startOidcLogin: vi.fn(),
  deepLinkOn: vi.fn(() => () => {}),
  accessTokenChangedOn: vi.fn(() => () => {}),
  loginCompletedOn: vi.fn(() => () => {}),
  loginFailedOn: vi.fn(() => () => {}),
  sessionExpiredOn: vi.fn(() => () => {}),
}));

vi.mock('@/common/adapter/httpBridge', () => ({
  httpRequest: mocks.httpRequest,
  isBackendHttpError: (error: unknown) =>
    Boolean(error && typeof error === 'object' && (error as { name?: unknown }).name === 'BackendHttpError'),
}));

vi.mock('@/common/auth/authTokenBroker', () => ({
  clearAccessToken: mocks.clearAccessToken,
  setAccessToken: mocks.setAccessToken,
  subscribeAccessToken: mocks.subscribeAccessToken,
}));

vi.mock('@/common/config/configService', () => ({
  configService: {
    initialize: mocks.configInitialize,
    clearSessionCache: mocks.clearSessionCache,
  },
}));

vi.mock('@/common/auth/oidcPkce', () => ({
  clearOidcLoginState: mocks.clearOidcLoginState,
  completeOidcRedirectIfPresent: mocks.completeOidcRedirectIfPresent,
  oidcCallbackParamsToHref: (params: Record<string, string>) => `http://127.0.0.1/auth/callback?code=${params.code}`,
  startOidcLogin: mocks.startOidcLogin,
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    deepLink: {
      received: { on: mocks.deepLinkOn },
    },
    auth: {
      accessTokenChanged: { on: mocks.accessTokenChangedOn },
      loginCompleted: { on: mocks.loginCompletedOn },
      loginFailed: { on: mocks.loginFailedOn },
      sessionExpired: { on: mocks.sessionExpiredOn },
    },
  },
}));

const oidcConfig = {
  client_id: 'biwork',
  authorization_endpoint: 'https://ferriskey.example/authorize',
  token_exchange_endpoint: '/api/auth/oidc/token',
};

const wrapper = ({ children }: { children: ReactNode }) => <AuthProvider>{children}</AuthProvider>;

function backendHttpError(status: number) {
  return {
    name: 'BackendHttpError',
    status,
    code: status === 401 ? 'UNAUTHORIZED' : 'BACKEND_ERROR',
  };
}

describe('AuthContext OIDC bootstrap', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    mocks.completeOidcRedirectIfPresent.mockResolvedValue(undefined);
    mocks.configInitialize.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    localStorage.clear();
    Object.defineProperty(window, 'electronAPI', { configurable: true, value: undefined });
  });

  it('treats 401 /api/auth/user as unauthenticated without syncing settings', async () => {
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') throw backendHttpError(401);
      throw new Error(`unexpected request: ${method} ${path}`);
    });

    const { result } = renderHook(() => useAuth(), { wrapper });

    await waitFor(() => expect(result.current.ready).toBe(true));

    expect(result.current.status).toBe('unauthenticated');
    expect(result.current.user).toBeNull();
    expect(mocks.configInitialize).not.toHaveBeenCalled();
  });

  it('syncs token-scoped settings after an authenticated user is loaded', async () => {
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      throw new Error(`unexpected request: ${method} ${path}`);
    });

    const { result } = renderHook(() => useAuth(), { wrapper });

    await waitFor(() => expect(result.current.status).toBe('authenticated'));

    expect(result.current.user).toEqual({ id: 'user-alon', username: 'alon' });
    expect(mocks.configInitialize).toHaveBeenCalledTimes(1);
  });

  it('restores the in-memory desktop token before authenticated bootstrap after a renderer reload', async () => {
    const getAuthAccessToken = vi.fn().mockResolvedValue('desktop-access-token');
    Object.defineProperty(window, 'electronAPI', {
      configurable: true,
      value: { getAuthAccessToken },
    });
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      throw new Error(`unexpected request: ${method} ${path}`);
    });

    const { result } = renderHook(() => useAuth(), { wrapper });
    await waitFor(() => expect(result.current.status).toBe('authenticated'));

    expect(getAuthAccessToken).toHaveBeenCalledTimes(1);
    expect(mocks.setAccessToken).toHaveBeenCalledWith('desktop-access-token');
  });

  it('installs the desktop OIDC token before loading the authenticated user', async () => {
    let authenticated = false;
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        if (!authenticated) throw backendHttpError(401);
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      throw new Error(`unexpected request: ${method} ${path}`);
    });

    const { result } = renderHook(() => useAuth(), { wrapper });
    await waitFor(() => expect(result.current.status).toBe('unauthenticated'));

    const onLoginCompleted = mocks.loginCompletedOn.mock.calls[0]?.[0] as
      | ((payload: { accessToken: string; authenticated: true }) => Promise<void>)
      | undefined;
    expect(onLoginCompleted).toBeTypeOf('function');
    authenticated = true;
    await act(async () => {
      await onLoginCompleted?.({ accessToken: 'desktop-access-token', authenticated: true });
    });

    expect(mocks.setAccessToken).toHaveBeenCalledWith('desktop-access-token');
    expect(result.current.status).toBe('authenticated');
    expect(result.current.user).toEqual({ id: 'user-alon', username: 'alon' });
  });

  it('clears token and session cache on logout even when backend logout fails', async () => {
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      if (method === 'POST' && path === '/api/auth/logout') throw new Error('logout failed');
      throw new Error(`unexpected request: ${method} ${path}`);
    });
    localStorage.setItem('biwork-auth-token', 'stale');

    const { result } = renderHook(() => useAuth(), { wrapper });
    await waitFor(() => expect(result.current.status).toBe('authenticated'));

    mocks.clearAccessToken.mockClear();
    mocks.clearSessionCache.mockClear();

    await act(async () => {
      await result.current.logout();
    });

    expect(result.current.status).toBe('unauthenticated');
    expect(result.current.user).toBeNull();
    expect(mocks.httpRequest).toHaveBeenCalledWith('POST', '/api/auth/logout', {});
    expect(mocks.clearAccessToken).toHaveBeenCalled();
    expect(mocks.clearSessionCache).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem('biwork-auth-token')).toBeNull();
    consoleError.mockRestore();
  });

  it('becomes unauthenticated when the shared access token is cleared', async () => {
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      throw new Error(`unexpected request: ${method} ${path}`);
    });

    const { result } = renderHook(() => useAuth(), { wrapper });
    await waitFor(() => expect(result.current.status).toBe('authenticated'));
    const listener = mocks.subscribeAccessToken.mock.calls[0]?.[0] as ((token: string | null) => void) | undefined;

    act(() => listener?.(null));

    expect(result.current.status).toBe('unauthenticated');
    expect(result.current.user).toBeNull();
    expect(mocks.clearSessionCache).toHaveBeenCalled();
  });

  it('installs proactively refreshed desktop tokens without logging the user out', async () => {
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      throw new Error(`unexpected request: ${method} ${path}`);
    });
    const { result } = renderHook(() => useAuth(), { wrapper });
    await waitFor(() => expect(result.current.status).toBe('authenticated'));
    const listener = mocks.accessTokenChangedOn.mock.calls[0]?.[0] as
      | ((payload: { accessToken: string }) => void)
      | undefined;

    act(() => listener?.({ accessToken: 'access-refreshed' }));

    expect(mocks.setAccessToken).toHaveBeenCalledWith('access-refreshed');
    expect(result.current.status).toBe('authenticated');
  });

  it('reports meaningful desktop activity through the main-process bridge', async () => {
    const recordDesktopAuthActivity = vi.fn().mockResolvedValue(true);
    Object.defineProperty(window, 'electronAPI', {
      configurable: true,
      value: { recordDesktopAuthActivity },
    });
    mocks.httpRequest.mockImplementation(async (method: string, path: string) => {
      if (method === 'GET' && path === '/api/auth/oidc/config') return oidcConfig;
      if (method === 'GET' && path === '/api/auth/user') {
        return { success: true, user: { id: 'user-alon', username: 'alon' } };
      }
      throw new Error(`unexpected request: ${method} ${path}`);
    });

    const { result } = renderHook(() => useAuth(), { wrapper });
    await waitFor(() => expect(result.current.status).toBe('authenticated'));

    act(() => window.dispatchEvent(new Event('pointerdown')));

    await waitFor(() => expect(recordDesktopAuthActivity).toHaveBeenCalledOnce());
  });
});
