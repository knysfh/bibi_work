import React, { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { ipcBridge } from '@/common';
import { clearAccessToken, setAccessToken, subscribeAccessToken } from '@/common/auth/authTokenBroker';
import { httpRequest, isBackendHttpError } from '@/common/adapter/httpBridge';
import { configService } from '@/common/config/configService';
import {
  clearOidcLoginState,
  completeOidcRedirectIfPresent,
  oidcCallbackParamsToHref,
  startOidcLogin,
  type OidcConfig,
} from '@/common/auth/oidcPkce';

type AuthStatus = 'checking' | 'authenticated' | 'unauthenticated';

export interface AuthUser {
  id: string;
  username: string;
}

type LoginErrorCode = 'networkError' | 'redirecting';

interface LoginResult {
  success: boolean;
  message?: string;
  code?: LoginErrorCode;
  redirected?: boolean;
}

interface AuthContextValue {
  ready: boolean;
  user: AuthUser | null;
  status: AuthStatus;
  login: () => Promise<LoginResult>;
  logout: () => Promise<void>;
  refresh: () => Promise<void>;
  clearAuthCache: () => void;
}

const AuthContext = createContext<AuthContextValue | undefined>(undefined);

const AUTH_USER_ENDPOINT = '/api/auth/user';
const DESKTOP_ACTIVITY_REPORT_INTERVAL_MS = 30_000;

async function fetchOidcConfig(): Promise<OidcConfig> {
  return httpRequest<OidcConfig>('GET', '/api/auth/oidc/config');
}

// Clear expired auth cache including cookies and localStorage
// 清除过期的认证缓存，包括 Cookie 和 localStorage
function clearAuthCache(): void {
  if (typeof window === 'undefined') return;

  try {
    clearAccessToken();
    configService.clearSessionCache();
    clearOidcLoginState();

    // Clear localStorage auth-related items
    const keysToRemove: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && (key.includes('auth') || key.includes('csrf') || key.includes('token'))) {
        keysToRemove.push(key);
      }
    }
    keysToRemove.forEach((key) => localStorage.removeItem(key));
  } catch (error) {
    console.error('Failed to clear auth cache:', error);
  }
}

async function fetchCurrentUser(signal?: AbortSignal): Promise<AuthUser | null> {
  try {
    const data = await httpRequest<{ success: boolean; user?: AuthUser }>('GET', AUTH_USER_ENDPOINT, undefined, {
      signal,
      silentStatuses: [401],
    });
    if (data.success && data.user) {
      return data.user;
    }
  } catch (error) {
    if ((error as Error).name === 'AbortError') {
      return null;
    }
    if (isBackendHttpError(error) && error.status === 401) {
      return null;
    }
    console.error('Failed to fetch current user:', error);
  }

  return null;
}

async function syncAuthenticatedConfig(): Promise<void> {
  try {
    await configService.initialize();
  } catch (error) {
    console.error('Failed to sync authenticated config:', error);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

export const AuthProvider: React.FC<React.PropsWithChildren> = ({ children }) => {
  const [user, setUser] = useState<AuthUser | null>(null);
  const [status, setStatus] = useState<AuthStatus>('checking');
  const [ready, setReady] = useState(false);
  const abortRef = useRef<AbortController | null>(null);

  const loadCurrentUser = useCallback(async (signal?: AbortSignal) => {
    const currentUser = await fetchCurrentUser(signal);
    if (currentUser) {
      await syncAuthenticatedConfig();
      setUser(currentUser);
      setStatus('authenticated');
    } else {
      setUser(null);
      setStatus('unauthenticated');
    }
    setReady(true);
  }, []);

  const waitForDesktopLoginCompletion = useCallback(async () => {
    const deadline = Date.now() + 60_000;
    while (Date.now() < deadline) {
      await sleep(1000);
      const currentUser = await fetchCurrentUser();
      if (!currentUser) continue;

      await syncAuthenticatedConfig();
      setUser(currentUser);
      setStatus('authenticated');
      setReady(true);
      return;
    }
  }, []);

  const refresh = useCallback(async () => {
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;
    setStatus('checking');

    try {
      const desktopAccessToken = await window.electronAPI?.getAuthAccessToken?.();
      if (desktopAccessToken) {
        setAccessToken(desktopAccessToken);
      }
    } catch (error) {
      console.warn('Failed to restore desktop access token:', error);
    }

    try {
      await completeOidcRedirectIfPresent(await fetchOidcConfig());
    } catch (error) {
      console.error('OIDC callback failed:', error);
      clearAccessToken();
      clearOidcLoginState();
    }

    await loadCurrentUser(controller.signal);
  }, [loadCurrentUser]);

  useEffect(() => {
    void refresh();
    return () => {
      abortRef.current?.abort();
    };
  }, [refresh]);

  useEffect(() => {
    return subscribeAccessToken((token) => {
      if (token) return;
      setUser(null);
      setStatus('unauthenticated');
      setReady(true);
      configService.clearSessionCache();
      clearOidcLoginState();
    });
  }, []);

  useEffect(() => {
    const recordActivity = window.electronAPI?.recordDesktopAuthActivity;
    if (status !== 'authenticated' || !recordActivity) return;

    let lastReportedAt = 0;
    const reportActivity = () => {
      const now = Date.now();
      if (now - lastReportedAt < DESKTOP_ACTIVITY_REPORT_INTERVAL_MS) return;
      lastReportedAt = now;
      void recordActivity().catch((error) => {
        console.warn('Failed to record desktop auth activity:', error);
      });
    };

    const eventOptions: AddEventListenerOptions = { capture: true, passive: true };
    window.addEventListener('pointerdown', reportActivity, eventOptions);
    window.addEventListener('keydown', reportActivity, eventOptions);
    window.addEventListener('touchstart', reportActivity, eventOptions);
    window.addEventListener('wheel', reportActivity, eventOptions);
    window.addEventListener('focus', reportActivity);

    return () => {
      window.removeEventListener('pointerdown', reportActivity, eventOptions);
      window.removeEventListener('keydown', reportActivity, eventOptions);
      window.removeEventListener('touchstart', reportActivity, eventOptions);
      window.removeEventListener('wheel', reportActivity, eventOptions);
      window.removeEventListener('focus', reportActivity);
    };
  }, [status]);

  useEffect(() => {
    return ipcBridge.deepLink.received.on(async (payload) => {
      if (payload.action !== 'auth/callback') return;
      setStatus('checking');
      try {
        await completeOidcRedirectIfPresent(await fetchOidcConfig(), {
          location: { href: oidcCallbackParamsToHref(payload.params) },
        });
      } catch (error) {
        console.error('OIDC deep-link callback failed:', error);
        clearAccessToken();
        clearOidcLoginState();
      }

      await loadCurrentUser();
    });
  }, [loadCurrentUser]);

  useEffect(() => {
    const offChanged = ipcBridge.auth.accessTokenChanged.on(({ accessToken }) => {
      setAccessToken(accessToken);
    });
    const offCompleted = ipcBridge.auth.loginCompleted.on(async ({ accessToken }) => {
      setAccessToken(accessToken);
      setStatus('checking');
      await loadCurrentUser();
    });
    const offFailed = ipcBridge.auth.loginFailed.on((payload) => {
      console.error('OIDC desktop login failed:', payload.message);
      clearOidcLoginState();
      setUser(null);
      setStatus('unauthenticated');
      setReady(true);
    });
    const offExpired = ipcBridge.auth.sessionExpired.on(() => {
      clearAccessToken();
      setUser(null);
      setStatus('unauthenticated');
      setReady(true);
      configService.clearSessionCache();
    });
    return () => {
      offChanged();
      offCompleted();
      offFailed();
      offExpired();
    };
  }, [loadCurrentUser]);

  const login = useCallback(async (): Promise<LoginResult> => {
    try {
      const desktopLogin = window.electronAPI?.startDesktopOidcLogin;
      if (desktopLogin) {
        clearOidcLoginState();
        await desktopLogin();
        void waitForDesktopLoginCompletion();
      } else {
        await startOidcLogin(await fetchOidcConfig());
      }
      return {
        success: false,
        message: 'Redirecting to FerrisKey...',
        code: 'redirecting',
        redirected: true,
      };
    } catch (error) {
      console.error('Login request failed:', error);
      return {
        success: false,
        message: 'Network error. Please try again.',
        code: 'networkError',
      };
    }
  }, []);

  const logout = useCallback(async () => {
    try {
      await httpRequest<void>('POST', '/api/auth/logout', {});
    } catch (error) {
      console.error('Logout request failed:', error);
    } finally {
      try {
        await window.electronAPI?.logoutDesktopAuth?.();
      } catch (error) {
        console.error('Desktop OIDC logout failed:', error);
      }
      setUser(null);
      setStatus('unauthenticated');
      clearAccessToken();
      // Clear cache on logout for security
      clearAuthCache();
    }
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({
      ready,
      user,
      status,
      login,
      logout,
      refresh,
      clearAuthCache,
    }),
    [login, logout, ready, refresh, status, user]
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
};

export function useAuth(): AuthContextValue {
  const context = useContext(AuthContext);
  if (!context) {
    throw new Error('useAuth must be used within an AuthProvider');
  }
  return context;
}
