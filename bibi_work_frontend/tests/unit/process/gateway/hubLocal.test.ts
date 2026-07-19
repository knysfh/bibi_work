/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { createHash } from 'crypto';
import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import * as tar from 'tar';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  applyHubLocalStateToExtensions,
  handleHubLocalRoute,
  HubLocalRouteError,
  type HubLocalActionResult,
} from '@process/gateway/hubLocal';
import type { IHubAgentItem } from '@/common/types/agent/hub';

let tempDir = '';
let statePath = '';
let installRoot = '';

async function route(pathname: string, body: Record<string, unknown> = {}): Promise<unknown> {
  return handleHubLocalRoute(pathname, body, { statePath, installRoot });
}

async function readState(): Promise<Record<string, unknown>> {
  return JSON.parse(await fs.readFile(statePath, 'utf8')) as Record<string, unknown>;
}

beforeEach(async () => {
  tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-hub-local-test-'));
  statePath = path.join(tempDir, 'hub-local-state.json');
  installRoot = path.join(tempDir, 'hub-extensions');
});

afterEach(async () => {
  if (tempDir) {
    await fs.rm(tempDir, { recursive: true, force: true });
  }
});

describe('desktop hub local routes', () => {
  it('returns an empty update list without requiring a local runtime', async () => {
    await expect(route('/api/hub/check-updates')).resolves.toEqual([]);
  });

  it('records install attempts as failed when the local installer is not attached', async () => {
    const emitStateChange = vi.fn();
    const result = (await handleHubLocalRoute(
      '/api/hub/install',
      { name: 'ext-codex' },
      { statePath, emitStateChange }
    )) as HubLocalActionResult;

    expect(result).toEqual({
      name: 'ext-codex',
      status: 'install_failed',
      error: 'Local hub extension installer is not attached.',
    });
    expect(emitStateChange).toHaveBeenCalledWith(result);

    const state = readState();
    await expect(state).resolves.toMatchObject({
      version: 1,
      extensions: {
        'ext-codex': {
          status: 'install_failed',
          error: 'Local hub extension installer is not attached.',
        },
      },
    });
  });

  it('installs a governed tarball atomically after SHA-512 verification', async () => {
    const extensionName = 'ext-governed';
    const packageRoot = path.join(tempDir, 'package');
    const extensionRoot = path.join(packageRoot, extensionName);
    const archivePath = path.join(tempDir, 'extension.tgz');
    await fs.mkdir(extensionRoot, { recursive: true });
    await fs.writeFile(
      path.join(extensionRoot, 'biwork-extension.json'),
      JSON.stringify({ name: extensionName, version: '1.0.0' })
    );
    await fs.writeFile(path.join(extensionRoot, 'payload.txt'), 'installed');
    await tar.c({ gzip: true, file: archivePath, cwd: packageRoot }, [extensionName]);
    const archive = new Uint8Array(await fs.readFile(archivePath));
    const integrity = `sha512-${createHash('sha512').update(archive).digest('base64')}`;
    const extension: IHubAgentItem = {
      name: extensionName,
      display_name: 'Governed extension',
      description: 'test',
      author: 'test',
      dist: {
        tarball: `data:application/gzip;base64,${Buffer.from(archive).toString('base64')}`,
        integrity,
        unpackedSize: 9,
      },
      engines: { biwork: '*' },
      hubs: [],
      status: 'not_installed',
    };
    const emitStateChange = vi.fn();

    const result = (await handleHubLocalRoute(
      '/api/hub/install',
      { name: extensionName },
      {
        statePath,
        installRoot,
        extension,
        emitStateChange,
      }
    )) as HubLocalActionResult;

    expect(result).toEqual({ name: extensionName, status: 'installed' });
    await expect(fs.readFile(path.join(installRoot, extensionName, 'payload.txt'), 'utf8')).resolves.toBe('installed');
    expect(emitStateChange).toHaveBeenNthCalledWith(1, { name: extensionName, status: 'installing' });
    expect(emitStateChange).toHaveBeenNthCalledWith(2, { name: extensionName, status: 'installed' });

    await expect(
      handleHubLocalRoute('/api/hub/uninstall', { name: extensionName }, { statePath, installRoot })
    ).resolves.toEqual({ name: extensionName, status: 'not_installed' });
    await expect(fs.stat(path.join(installRoot, extensionName))).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('marks uninstall requests as not installed', async () => {
    await route('/api/hub/install', { name: 'ext-codex' });
    const emitStateChange = vi.fn();
    const result = (await handleHubLocalRoute(
      '/api/hub/uninstall',
      { name: 'ext-codex' },
      { statePath, installRoot, emitStateChange }
    )) as HubLocalActionResult;

    expect(result).toEqual({ name: 'ext-codex', status: 'not_installed' });
    expect(emitStateChange).toHaveBeenCalledWith(result);
    await expect(readState()).resolves.toMatchObject({
      extensions: {
        'ext-codex': {
          status: 'not_installed',
        },
      },
    });
  });

  it('rejects a tarball whose SHA-512 integrity does not match', async () => {
    const extension: IHubAgentItem = {
      name: 'bad-integrity-extension',
      display_name: 'Bad Integrity Extension',
      description: 'Bad integrity fixture',
      author: 'BiWork',
      dist: {
        tarball: 'https://example.invalid/bad-integrity-extension.tgz',
        integrity: `sha512-${Buffer.alloc(64).toString('base64')}`,
        unpackedSize: 1,
      },
      engines: { biwork: '*' },
      hubs: [],
      status: 'not_installed',
    };

    await expect(
      handleHubLocalRoute(
        '/api/hub/install',
        { name: extension.name },
        {
          statePath,
          installRoot,
          extension,
          fetchTarball: async () => new TextEncoder().encode('not the expected archive'),
        }
      )
    ).resolves.toEqual({
      name: extension.name,
      status: 'install_failed',
      error: 'Hub extension integrity verification failed.',
    });
  });

  it('overlays locally recorded states onto backend hub extensions', async () => {
    await route('/api/hub/install', { name: 'ext-codex' });

    const extensions = await applyHubLocalStateToExtensions(
      [
        {
          name: 'ext-codex',
          display_name: 'Codex',
          description: 'Codex adapter',
          author: 'BiWork',
          dist: { tarball: 'ext-codex.tgz', integrity: 'sha512-test', unpackedSize: 1 },
          engines: { biwork: '*' },
          hubs: ['acpAdapters'],
          status: 'not_installed',
        },
      ] satisfies IHubAgentItem[],
      { statePath }
    );

    expect(extensions[0]).toMatchObject({
      name: 'ext-codex',
      status: 'install_failed',
      installError: 'Local hub extension installer is not attached.',
    });
  });

  it('does not emit state change for update checks', async () => {
    const emitStateChange = vi.fn();

    await expect(handleHubLocalRoute('/api/hub/check-updates', {}, { statePath, emitStateChange })).resolves.toEqual(
      []
    );
    expect(emitStateChange).not.toHaveBeenCalled();
  });

  it('rejects invalid action payloads', async () => {
    await expect(route('/api/hub/install', { name: '' })).rejects.toBeInstanceOf(HubLocalRouteError);
  });
});
