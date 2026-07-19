/**
 * @vitest-environment node
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { clearAccessToken, setAccessToken } from '@/common/auth/authTokenBroker';
import type { BackendHttpError } from '@/common/adapter/httpBridge';
import { configService } from '@/common/config/configService';

describe('configService enterprise settings bootstrap', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
    configService.reset();
    clearAccessToken();
    delete globalThis.__backendPort;
  });

  afterEach(() => {
    configService.reset();
    clearAccessToken();
    delete globalThis.__backendPort;
    vi.unstubAllGlobals();
  });

  it('does not block startup on /api/settings/client when no token is available', async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal('fetch', fetchSpy);

    await configService.initialize();

    expect(configService.isInitialized()).toBe(true);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('uses the main-process gateway port and bearer token for settings bootstrap', async () => {
    globalThis.__backendPort = 24680;
    setAccessToken('token-main-process');
    const fetchSpy = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ success: true, data: { 'theme.activeId': 'light' } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    );
    vi.stubGlobal('fetch', fetchSpy);

    await configService.initialize();

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(fetchSpy.mock.calls[0][0]).toBe('http://127.0.0.1:24680/api/settings/client');
    expect(fetchSpy.mock.calls[0][1]).toMatchObject({
      method: 'GET',
      headers: { Authorization: 'Bearer token-main-process' },
    });
    expect(configService.get('theme.activeId')).toBe('light');
  });

  it('re-syncs enterprise settings when a token arrives after unauthenticated bootstrap', async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal('fetch', fetchSpy);

    await configService.initialize();
    expect(fetchSpy).not.toHaveBeenCalled();

    setAccessToken('token-after-login');
    fetchSpy.mockResolvedValue(
      new Response(JSON.stringify({ success: true, data: { 'theme.activeId': 'dark' } }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      })
    );

    await configService.initialize();

    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(fetchSpy.mock.calls[0][1]).toMatchObject({
      method: 'GET',
      headers: { Authorization: 'Bearer token-after-login' },
    });
    expect(configService.get('theme.activeId')).toBe('dark');
  });

  it('clears session settings on logout without removing subscribers', () => {
    configService.setLocal('theme.activeId', 'dark');
    const listener = vi.fn();
    const unsubscribe = configService.subscribe('theme.activeId', listener);

    configService.clearSessionCache();

    expect(configService.isInitialized()).toBe(true);
    expect(configService.get('theme.activeId')).toBeUndefined();
    expect(listener).toHaveBeenCalledWith(undefined);

    listener.mockClear();
    configService.setLocal('theme.activeId', 'light');

    expect(listener).toHaveBeenCalledWith('light');
    unsubscribe();
  });

  it('preserves backend error envelope fields for settings failures', async () => {
    setAccessToken('token-settings-error');
    const fetchSpy = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          success: false,
          trace_id: 'trace-settings',
          code: 'SETTINGS_UNAVAILABLE',
          error: 'settings unavailable',
          details: { retryable: true },
        }),
        {
          status: 503,
          headers: { 'Content-Type': 'application/json' },
        }
      )
    );
    vi.stubGlobal('fetch', fetchSpy);

    await expect(configService.initialize()).rejects.toMatchObject({
      name: 'BackendHttpError',
      status: 503,
      code: 'SETTINGS_UNAVAILABLE',
      backendMessage: 'settings unavailable',
      traceId: 'trace-settings',
      details: { retryable: true },
    } satisfies Partial<BackendHttpError>);
  });
});
