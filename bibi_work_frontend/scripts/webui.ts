#!/usr/bin/env bun
/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { execSync } from 'child_process';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { startWebHost } from '@biwork/web-host';
import { openBrowserUrl, shouldAutoOpenBrowser } from '../packages/web-cli/src/browser.js';

const DEFAULT_PORT = process.env.NODE_ENV === 'production' ? 25808 : 25809;
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const has = (name: string) => args.includes(name);
const getFlag = (name: string): string | undefined => {
  const index = args.indexOf(name);
  const value = index >= 0 ? args[index + 1] : undefined;
  return value && !value.startsWith('--') ? value : undefined;
};
const parseBoolean = (value?: string) => Boolean(value && ['1', 'true', 'yes', 'on'].includes(value.toLowerCase()));

function resolveBackendPort(): number {
  const raw = getFlag('--backend-url') ?? process.env.BIWORK_ENTERPRISE_BACKEND_URL ?? 'http://127.0.0.1:8361';
  const url = new URL(raw);
  if (url.protocol !== 'http:' || !['127.0.0.1', 'localhost', '::1', '[::1]'].includes(url.hostname)) {
    throw new Error('BiWork backend URL must be a loopback http:// URL');
  }
  return url.port ? Number(url.port) : 80;
}

function resolveStaticDir(): string {
  if (process.env.BIWORK_STATIC_DIR) return process.env.BIWORK_STATIC_DIR;
  const candidate = path.join(repoRoot, 'out', 'renderer');
  if (!fs.existsSync(path.join(candidate, 'index.html'))) {
    throw new Error(`Renderer assets not found at ${candidate}. Run "bun run package" first.`);
  }
  return candidate;
}

async function main(): Promise<void> {
  if (!has('--no-build') && !parseBoolean(process.env.BIWORK_NO_BUILD) && !process.env.BIWORK_STATIC_DIR) {
    execSync('bun run package', { cwd: repoRoot, stdio: 'inherit' });
  }
  const backendPort = resolveBackendPort();
  const port = Number(getFlag('--port') ?? process.env.BIWORK_PORT ?? process.env.PORT ?? DEFAULT_PORT);
  const allowRemote = has('--remote') || parseBoolean(process.env.BIWORK_ALLOW_REMOTE);
  const handle = await startWebHost({
    app: { version: '0.0.0', isPackaged: false, resourcesPath: repoRoot, userDataPath: repoRoot },
    staticDir: resolveStaticDir(),
    port,
    allowRemote,
    backend: { kind: 'useExistingBackend', port: backendPort },
  });
  console.log(`BiWork WebUI ready: ${handle.localUrl}`);
  if (shouldAutoOpenBrowser({ allowRemote, env: process.env, openFlag: has('--open'), noOpenFlag: has('--no-open') })) {
    const result = openBrowserUrl(handle.localUrl);
    if (!result.ok) console.warn(`[webui] could not open browser: ${result.reason}`);
  }
  const stop = async () => {
    await handle.stop();
    process.exit(0);
  };
  process.on('SIGINT', () => void stop());
  process.on('SIGTERM', () => void stop());
}

void main().catch((error) => {
  console.error('[webui] startup failed:', error);
  process.exit(1);
});
