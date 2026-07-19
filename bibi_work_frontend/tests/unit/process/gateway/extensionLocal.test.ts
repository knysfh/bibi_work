/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  buildExtensionSyncPayload,
  handleExtensionLocalRoute,
  listExtensionChannelPlugins,
  previewExtensionEnabledState,
  readExtensionStaticAsset,
  ExtensionLocalRouteError,
  type ExtensionLocalRouteContext,
} from '@process/gateway/extensionLocal';

let tempDir = '';
let extensionDir = '';
let context: ExtensionLocalRouteContext;

async function writeJson(filePath: string, value: unknown): Promise<void> {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

async function route(pathname: string, body: Record<string, unknown> = {}): Promise<unknown> {
  return handleExtensionLocalRoute(pathname, body, context);
}

beforeEach(async () => {
  tempDir = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-extension-local-test-'));
  extensionDir = path.join(tempDir, 'extensions', 'hello-extension');
  context = {
    extensionRoots: [path.join(tempDir, 'extensions')],
    statePath: path.join(tempDir, 'extension-local-state.json'),
  };
  await writeJson(path.join(extensionDir, 'biwork-extension.json'), {
    name: 'hello-extension',
    displayName: 'Hello Extension',
    version: '1.2.3',
    description: 'Local extension fixture',
    i18n: { localesDir: 'i18n', defaultLocale: 'en-US' },
    permissions: {
      storage: true,
      network: false,
    },
    contributes: {
      skills: '$file:contributes/skills.json',
      themes: [{ id: 'hello-theme', name: 'Hello Theme', file: 'themes/hello.css', cover: 'assets/cover.svg' }],
      channelPlugins: [
        {
          type: 'hello-channel',
          name: 'Hello Channel',
          description: 'Channel from extension',
          icon: 'assets/channel.svg',
          credentialFields: [{ key: 'token', label: 'Token', type: 'password', required: true }],
          configFields: [{ key: 'pollingInterval', label: 'Polling Interval', type: 'number', default: 1000 }],
        },
      ],
      settingsTabs: [{ id: 'hello-settings', name: 'Hello Settings', entryPoint: 'settings/index.html', order: 7 }],
    },
  });
  await writeJson(path.join(extensionDir, 'contributes', 'skills.json'), [
    { name: 'hello-skill', description: 'Skill from extension', file: 'skills/hello.md' },
  ]);
  await writeJson(path.join(extensionDir, 'i18n', 'en-US', 'extension.json'), {
    extension: { settingsTabs: { 'hello-settings': { name: 'Hello Settings' } } },
  });
  await fs.mkdir(path.join(extensionDir, 'settings'), { recursive: true });
  await fs.writeFile(path.join(extensionDir, 'settings', 'index.html'), '<h1>Hello</h1>', 'utf8');
});

afterEach(async () => {
  if (tempDir) {
    await fs.rm(tempDir, { recursive: true, force: true });
  }
});

describe('desktop extension local routes', () => {
  it('lists local extension manifests', async () => {
    await expect(route('/api/extensions')).resolves.toEqual([
      {
        name: 'hello-extension',
        display_name: 'Hello Extension',
        version: '1.2.3',
        description: 'Local extension fixture',
        source: 'local',
        enabled: true,
      },
    ]);
  });

  it('builds a Rust governance sync payload from local extension manifests', async () => {
    const payload = await buildExtensionSyncPayload(context);

    expect(payload.extensions).toHaveLength(1);
    const extension = payload.extensions[0]!;
    expect(extension).toMatchObject({
      name: 'hello-extension',
      source: 'local',
      version: '1.2.3',
      risk_level: 'moderate',
      enabled: true,
      manifest: {
        name: 'hello-extension',
        displayName: 'Hello Extension',
      },
    });
    expect(extension.contributions).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          type: 'skill',
          key: 'hello-skill',
          enabled: true,
          manifest: expect.objectContaining({
            name: 'hello-skill',
            location: path.join(extensionDir, 'skills', 'hello.md'),
          }),
        }),
        expect.objectContaining({
          type: 'settings_tab',
          key: 'hello-settings',
          enabled: true,
          manifest: expect.objectContaining({
            url: '/api/extensions/static/hello-extension/settings/index.html',
            extensionName: 'hello-extension',
          }),
        }),
        expect.objectContaining({
          type: 'channel_plugin',
          key: 'hello-channel',
          enabled: true,
          manifest: expect.objectContaining({
            plugin_id: 'hello-channel',
            extension_meta: expect.objectContaining({
              extensionName: 'hello-extension',
              icon: '/api/extensions/static/hello-extension/assets/channel.svg',
            }),
          }),
        }),
      ])
    );
  });

  it('marks sync contributions disabled when the local extension is disabled', async () => {
    await route('/api/extensions/disable', { name: 'hello-extension' });

    const payload = await buildExtensionSyncPayload(context);

    const extension = payload.extensions[0]!;
    expect(extension.enabled).toBe(false);
    expect(extension.contributions.every((item) => item.enabled === false)).toBe(true);
  });

  it('adds hub local state entries to Rust governance sync payloads', async () => {
    const hubStatePath = path.join(tempDir, 'hub-local-state.json');
    await writeJson(hubStatePath, {
      version: 1,
      extensions: {
        'hub-only-extension': {
          status: 'install_failed',
          error: 'Local hub extension installer is not attached.',
          updatedAt: Date.now(),
        },
      },
    });

    const payload = await buildExtensionSyncPayload({ ...context, hubStatePath });

    expect(payload.extensions).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          name: 'hub-only-extension',
          source: 'hub',
          enabled: false,
          installed: false,
          install_status: 'install_failed',
          last_error: 'Local hub extension installer is not attached.',
          contributions: [],
          manifest: expect.objectContaining({
            name: 'hub-only-extension',
            source: 'hub',
            local_status: 'install_failed',
          }),
        }),
      ])
    );
  });

  it('loads $file contributions and maps skill locations', async () => {
    const skills = (await route('/api/extensions/skills')) as Array<Record<string, unknown>>;

    expect(skills).toHaveLength(1);
    expect(skills[0]).toMatchObject({
      name: 'hello-skill',
      description: 'Skill from extension',
      _extensionName: 'hello-extension',
      _source: 'extension',
    });
    expect(skills[0].location).toBe(path.join(extensionDir, 'skills', 'hello.md'));
  });

  it('maps settings tabs to static extension URLs', async () => {
    const tabs = (await route('/api/extensions/settings-tabs')) as Array<Record<string, unknown>>;

    expect(tabs).toEqual([
      {
        id: 'hello-settings',
        label: 'Hello Settings',
        icon: undefined,
        url: '/api/extensions/static/hello-extension/settings/index.html',
        position: undefined,
        order: 7,
        extensionName: 'hello-extension',
      },
    ]);
  });

  it('reads extension i18n data with default-locale fallback', async () => {
    await expect(route('/api/extensions/i18n', { locale: 'zh-CN' })).resolves.toEqual({
      'hello-extension': {
        extension: { settingsTabs: { 'hello-settings': { name: 'Hello Settings' } } },
      },
    });
  });

  it('persists disable and enable state for local extensions', async () => {
    await expect(route('/api/extensions/disable', { name: 'hello-extension', reason: 'test' })).resolves.toEqual({
      name: 'hello-extension',
      enabled: false,
      reason: 'test',
    });
    await expect(route('/api/extensions')).resolves.toMatchObject([{ name: 'hello-extension', enabled: false }]);
    await expect(route('/api/extensions/settings-tabs')).resolves.toEqual([]);

    await expect(route('/api/extensions/enable', { name: 'hello-extension' })).resolves.toEqual({
      name: 'hello-extension',
      enabled: true,
    });
    await expect(route('/api/extensions/settings-tabs')).resolves.toHaveLength(1);
  });

  it('previews enable/disable state changes without persisting local state', async () => {
    await expect(
      previewExtensionEnabledState({ name: 'hello-extension', reason: 'audit-first' }, context, false)
    ).resolves.toEqual({
      name: 'hello-extension',
      enabled: false,
      reason: 'audit-first',
    });

    await expect(route('/api/extensions')).resolves.toMatchObject([{ name: 'hello-extension', enabled: true }]);
    await expect(route('/api/extensions/settings-tabs')).resolves.toHaveLength(1);
  });

  it('returns local permissions and risk-level projections', async () => {
    await expect(route('/api/extensions/permissions', { name: 'hello-extension' })).resolves.toEqual([
      { name: 'storage', description: 'storage', level: 'moderate', granted: true },
      { name: 'network', description: 'network', level: 'safe', granted: false },
    ]);
    await expect(route('/api/extensions/risk-level', { name: 'hello-extension' })).resolves.toBe('moderate');
  });

  it('projects extension channel plugins for channel settings', async () => {
    await expect(listExtensionChannelPlugins(context)).resolves.toEqual([
      {
        plugin_id: 'hello-channel',
        id: 'hello-channel',
        type: 'hello-channel',
        name: 'Hello Channel',
        enabled: false,
        connected: false,
        status: 'disabled',
        active_users: 0,
        has_token: false,
        is_extension: true,
        extension_meta: {
          extensionName: 'hello-extension',
          description: 'Channel from extension',
          icon: '/api/extensions/static/hello-extension/assets/channel.svg',
          credentialFields: [{ key: 'token', label: 'Token', type: 'password', required: true }],
          configFields: [{ key: 'pollingInterval', label: 'Polling Interval', type: 'number', default: 1000 }],
        },
      },
    ]);
  });

  it('filters extension channel plugins when the extension is disabled', async () => {
    await route('/api/extensions/disable', { name: 'hello-extension' });

    await expect(listExtensionChannelPlugins(context)).resolves.toEqual([]);
  });

  it('serves whitelisted static assets from inside the extension root', async () => {
    const asset = await readExtensionStaticAsset('hello-extension', 'settings/index.html', context);

    expect(asset.contentType).toBe('text/html; charset=utf-8');
    expect(asset.data.toString('utf8')).toBe('<h1>Hello</h1>');
  });

  it('does not serve static assets for disabled extensions', async () => {
    await route('/api/extensions/disable', { name: 'hello-extension' });

    await expect(readExtensionStaticAsset('hello-extension', 'settings/index.html', context)).rejects.toBeInstanceOf(
      ExtensionLocalRouteError
    );
  });

  it('rejects static asset paths that escape the extension root', async () => {
    await expect(readExtensionStaticAsset('hello-extension', '../secret.txt', context)).rejects.toBeInstanceOf(
      ExtensionLocalRouteError
    );
  });
});
