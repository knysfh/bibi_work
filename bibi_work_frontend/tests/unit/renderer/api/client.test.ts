/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 *
 * Unit tests for renderer/api/client.ts error handling.
 * Regression tests for #3249: Response body double consumption on non-JSON errors.
 *
 * @vitest-environment node
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createApiClient, ApiError } from '@renderer/api/client';
import { clearAccessToken, peekAccessToken, setAccessToken } from '@/common/auth/authTokenBroker';

describe('api/client', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
  });

  afterEach(() => {
    clearAccessToken();
    vi.unstubAllGlobals();
    delete globalThis.__biworkAccessToken;
  });

  describe('error handling', () => {
    it('JSON error response is parsed into ApiError contract fields', async () => {
      const fetchSpy = vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({
            trace_id: 'trace-model',
            error: 'invalid model',
            code: 'MODEL_INVALID',
            details: { model: 'x' },
          }),
          {
            status: 400,
            headers: { 'Content-Type': 'application/json' },
          }
        )
      );
      vi.stubGlobal('fetch', fetchSpy);
      setAccessToken('expired-access-token');

      const api = createApiClient('http://127.0.0.1:9123');

      try {
        await api.get('/api/models');
        expect.fail('Should have thrown');
      } catch (e) {
        expect(e).toBeInstanceOf(ApiError);
        const err = e as ApiError;
        expect(err.status).toBe(400);
        expect(err.name).toBe('ApiError');
        expect(err.code).toBe('MODEL_INVALID');
        expect(err.backendMessage).toBe('invalid model');
        expect(err.traceId).toBe('trace-model');
        expect(err.details).toEqual({ model: 'x' });
        expect(err.body).toEqual({
          trace_id: 'trace-model',
          error: 'invalid model',
          code: 'MODEL_INVALID',
          details: { model: 'x' },
        });
      }
    });

    it('non-JSON error response captures raw text without double body consumption (#3249)', async () => {
      const fetchSpy = vi.fn().mockResolvedValue(
        new Response('Unauthorized', {
          status: 401,
          statusText: 'Unauthorized',
          headers: { 'Content-Type': 'text/plain' },
        })
      );
      vi.stubGlobal('fetch', fetchSpy);

      const api = createApiClient('http://127.0.0.1:9123');

      try {
        await api.post('/api/models', { provider: 'deepseek' });
        expect.fail('Should have thrown');
      } catch (e) {
        // Before the fix this threw TypeError "body stream already read" instead
        expect(e).toBeInstanceOf(ApiError);
        const err = e as ApiError;
        expect(err.status).toBe(401);
        expect(err.backendMessage).toBe('Unauthorized');
        expect(err.traceId).toBe('');
        expect(err.body).toBe('Unauthorized');
        expect(peekAccessToken()).toBeNull();
      }
    });

    it('empty error body falls back to empty string', async () => {
      const fetchSpy = vi.fn().mockResolvedValue(new Response('', { status: 502 }));
      vi.stubGlobal('fetch', fetchSpy);

      const api = createApiClient('http://127.0.0.1:9123');

      try {
        await api.get('/api/models');
        expect.fail('Should have thrown');
      } catch (e) {
        expect(e).toBeInstanceOf(ApiError);
        const err = e as ApiError;
        expect(err.status).toBe(502);
        expect(err.body).toBe('');
      }
    });
  });

  describe('success path', () => {
    it('returns parsed JSON for application/json responses', async () => {
      const fetchSpy = vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        })
      );
      vi.stubGlobal('fetch', fetchSpy);

      const api = createApiClient('http://127.0.0.1:9123');
      const result = await api.get<{ ok: boolean }>('/api/health');

      expect(result).toEqual({ ok: true });
    });

    it('adds the shared broker bearer token to requests', async () => {
      globalThis.__biworkAccessToken = 'api-client-token';
      const fetchSpy = vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        })
      );
      vi.stubGlobal('fetch', fetchSpy);

      const api = createApiClient('http://127.0.0.1:9123');
      await api.get('/api/health');

      expect(fetchSpy.mock.calls[0][1]?.headers).toEqual({ Authorization: 'Bearer api-client-token' });
    });

    it('allows explicit request headers to override the broker bearer token', async () => {
      globalThis.__biworkAccessToken = 'api-client-token';
      const fetchSpy = vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        })
      );
      vi.stubGlobal('fetch', fetchSpy);

      const api = createApiClient('http://127.0.0.1:9123');
      await api.get('/api/health', { headers: { Authorization: 'Bearer explicit-token' } });

      expect(fetchSpy.mock.calls[0][1]?.headers).toEqual({ Authorization: 'Bearer explicit-token' });
    });

    it('returns undefined for non-JSON responses', async () => {
      const fetchSpy = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
      vi.stubGlobal('fetch', fetchSpy);

      const api = createApiClient('http://127.0.0.1:9123');
      const result = await api.delete('/api/models/1');

      expect(result).toBeUndefined();
    });
  });
});
