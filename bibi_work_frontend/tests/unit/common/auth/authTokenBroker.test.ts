/**
 * @vitest-environment node
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  clearAccessToken,
  getAccessToken,
  requestWithAuthorizationRetry,
  peekAccessToken,
  setAuthSessionInvalidator,
  setAccessToken,
  setAccessTokenProvider,
  subscribeAccessToken,
} from '@/common/auth/authTokenBroker';

describe('authTokenBroker', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
    clearAccessToken();
    setAccessTokenProvider(null);
    setAuthSessionInvalidator(null);
    delete globalThis.__biworkAccessToken;
  });

  afterEach(() => {
    clearAccessToken();
    setAccessTokenProvider(null);
    setAuthSessionInvalidator(null);
    delete globalThis.__biworkAccessToken;
    vi.unstubAllGlobals();
  });

  it('keeps token in memory without requiring an Electron renderer bridge', () => {
    setAccessToken(' token-node ');

    expect(peekAccessToken()).toBe('token-node');
    expect(globalThis.__biworkAccessToken).toBe('token-node');
  });

  it('notifies subscribers only when the normalized token changes', () => {
    const listener = vi.fn();
    const unsubscribe = subscribeAccessToken(listener);

    setAccessToken(' token-a ');
    setAccessToken('token-a');
    clearAccessToken();
    unsubscribe();
    setAccessToken('token-b');

    expect(listener).toHaveBeenCalledTimes(2);
    expect(listener).toHaveBeenNthCalledWith(1, 'token-a');
    expect(listener).toHaveBeenNthCalledWith(2, null);
  });

  it('stores and notifies provider tokens through the same broker path', async () => {
    const listener = vi.fn();
    const unsubscribe = subscribeAccessToken(listener);
    setAccessTokenProvider(() => ' provider-token ');

    await expect(getAccessToken()).resolves.toBe('provider-token');

    expect(peekAccessToken()).toBe('provider-token');
    expect(listener).toHaveBeenCalledWith('provider-token');

    unsubscribe();
  });

  it('does not return an expired JWT supplied by a provider', async () => {
    const payload = Buffer.from(JSON.stringify({ exp: Math.floor(Date.now() / 1000) - 1 })).toString('base64url');
    setAccessTokenProvider(() => `header.${payload}.signature`);

    await expect(getAccessToken()).resolves.toBeNull();
    expect(peekAccessToken()).toBeNull();
  });

  it('refreshes once after a 401 and retries the request with the replacement token', async () => {
    const provider = vi.fn((forceRefresh?: boolean) => (forceRefresh ? 'token-new' : 'token-old'));
    setAccessTokenProvider(provider);
    setAccessToken('token-old');
    const request = vi
      .fn<() => Promise<Response>>()
      .mockResolvedValueOnce(new Response(null, { status: 401 }))
      .mockResolvedValueOnce(new Response(null, { status: 204 }));

    await expect(requestWithAuthorizationRetry(request)).resolves.toMatchObject({ status: 204 });
    expect(request).toHaveBeenCalledTimes(2);
    expect(provider).toHaveBeenCalledWith(true);
    expect(peekAccessToken()).toBe('token-new');
  });

  it('invalidates the whole desktop session when the retried request is still unauthorized', async () => {
    const invalidate = vi.fn();
    setAuthSessionInvalidator(invalidate);
    setAccessTokenProvider((forceRefresh) => (forceRefresh ? 'token-new' : 'token-old'));
    setAccessToken('token-old');
    const request = vi.fn(async () => new Response(null, { status: 401 }));

    await expect(requestWithAuthorizationRetry(request)).resolves.toMatchObject({ status: 401 });
    expect(request).toHaveBeenCalledTimes(2);
    expect(invalidate).toHaveBeenCalledOnce();
    expect(peekAccessToken()).toBeNull();
  });

  it('clears a JWT access token when its exp claim is reached', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-07-14T00:00:00.000Z'));
    const expiresAtSeconds = Math.floor(Date.now() / 1000) + 2;
    const payload = Buffer.from(JSON.stringify({ exp: expiresAtSeconds })).toString('base64url');
    const token = `header.${payload}.signature`;
    const listener = vi.fn();
    const unsubscribe = subscribeAccessToken(listener);

    setAccessToken(token);
    expect(peekAccessToken()).toBe(token);

    vi.advanceTimersByTime(2_000);

    expect(peekAccessToken()).toBeNull();
    expect(listener).toHaveBeenLastCalledWith(null);
    unsubscribe();
    vi.useRealTimers();
  });
});
