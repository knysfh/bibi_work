/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

export type EnterpriseBackendMode = 'desktop-gateway' | 'external-rust-direct';

export interface ExternalEnterpriseBackendConfig {
  backendMode: EnterpriseBackendMode;
  backendUrl: string;
  backendPort: number;
}

type EnvLike = Record<string, string | undefined>;

const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '::1', '[::1]']);

export function resolveEnterpriseBackendMode(env: EnvLike = process.env): EnterpriseBackendMode {
  return env.BIWORK_BACKEND_MODE === 'external-rust-direct' ? 'external-rust-direct' : 'desktop-gateway';
}

export function parseExternalEnterpriseBackendConfig(
  rawBackendUrl: string,
  backendMode: EnterpriseBackendMode
): ExternalEnterpriseBackendConfig {
  const parsed = new URL(rawBackendUrl);
  if (parsed.protocol !== 'http:') {
    throw new Error('BIWORK_ENTERPRISE_BACKEND_URL must use http://');
  }

  const hostname = parsed.hostname.toLowerCase();
  if (!LOOPBACK_HOSTS.has(hostname)) {
    throw new Error('BIWORK_ENTERPRISE_BACKEND_URL must point to a loopback host');
  }

  const backendPort = parsed.port ? Number.parseInt(parsed.port, 10) : 80;
  if (!Number.isInteger(backendPort) || backendPort <= 0 || backendPort > 65535) {
    throw new Error('BIWORK_ENTERPRISE_BACKEND_URL has an invalid port');
  }

  return {
    backendMode,
    backendUrl: parsed.origin,
    backendPort,
  };
}

export function resolveExternalEnterpriseBackendConfig(
  env: EnvLike = process.env,
  warn: (message: string, error: unknown) => void = console.warn
): ExternalEnterpriseBackendConfig | null {
  const rawBackendUrl = env.BIWORK_ENTERPRISE_BACKEND_URL?.trim();
  if (!rawBackendUrl) {
    return null;
  }

  try {
    return parseExternalEnterpriseBackendConfig(rawBackendUrl, resolveEnterpriseBackendMode(env));
  } catch (error) {
    warn('[BiWork] Ignoring invalid BIWORK_ENTERPRISE_BACKEND_URL:', error);
    return null;
  }
}

export async function verifyExternalEnterpriseBackend(
  config: ExternalEnterpriseBackendConfig,
  options: {
    fetchImpl?: typeof fetch;
    timeoutMs?: number;
  } = {}
): Promise<void> {
  const fetchImpl = options.fetchImpl ?? fetch;
  const timeoutMs = options.timeoutMs ?? 5000;
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetchImpl(`${config.backendUrl}/api/route-ownership`, {
      headers: { accept: 'application/json' },
      signal: controller.signal,
    });
    if (!response.ok) {
      throw new Error(`external enterprise backend health check failed: HTTP ${response.status}`);
    }
  } finally {
    clearTimeout(timeout);
  }
}
