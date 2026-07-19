/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, expect, it, vi } from 'vitest';
import {
  parseExternalEnterpriseBackendConfig,
  resolveExternalEnterpriseBackendConfig,
  verifyExternalEnterpriseBackend,
} from '@/process/startup/externalEnterpriseBackend';

describe('external enterprise backend startup config', () => {
  it('resolves a loopback Rust backend with the desktop gateway by default', () => {
    expect(
      resolveExternalEnterpriseBackendConfig({
        BIWORK_ENTERPRISE_BACKEND_URL: 'http://127.0.0.1:8361',
      })
    ).toEqual({
      backendMode: 'desktop-gateway',
      backendUrl: 'http://127.0.0.1:8361',
      backendPort: 8361,
    });
  });

  it('allows explicit direct renderer access for controlled runtimes', () => {
    expect(
      resolveExternalEnterpriseBackendConfig({
        BIWORK_BACKEND_MODE: 'external-rust-direct',
        BIWORK_ENTERPRISE_BACKEND_URL: 'http://localhost:8361/',
      })
    ).toEqual({
      backendMode: 'external-rust-direct',
      backendUrl: 'http://localhost:8361',
      backendPort: 8361,
    });
  });

  it('rejects non-loopback backend URLs', () => {
    const warn = vi.fn();

    expect(
      resolveExternalEnterpriseBackendConfig(
        {
          BIWORK_ENTERPRISE_BACKEND_URL: 'https://example.com:8361',
        },
        warn
      )
    ).toBeNull();

    expect(warn).toHaveBeenCalledOnce();
  });

  it('throws for invalid ports during parsing', () => {
    expect(() => parseExternalEnterpriseBackendConfig('http://127.0.0.1:70000', 'desktop-gateway')).toThrow(
      'Invalid URL'
    );
  });

  it('verifies the Rust backend route ownership endpoint', async () => {
    const fetchImpl = vi.fn(async () => new Response('{}', { status: 200 }));

    await verifyExternalEnterpriseBackend(
      {
        backendMode: 'desktop-gateway',
        backendUrl: 'http://127.0.0.1:8361',
        backendPort: 8361,
      },
      { fetchImpl, timeoutMs: 1000 }
    );

    expect(fetchImpl).toHaveBeenCalledWith('http://127.0.0.1:8361/api/route-ownership', {
      headers: { accept: 'application/json' },
      signal: expect.any(AbortSignal),
    });
  });
});
