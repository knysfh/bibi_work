import { describe, expect, it } from 'vitest';
import {
  isExtensionStaticAssetAllowed,
  mergeChannelPlugins,
  mergeExtensionData,
} from '@process/gateway/extensionGovernance';

describe('desktop extension governance merge helpers', () => {
  it('treats Rust extension contribution arrays as the governance allow-list', () => {
    const backend = {
      success: true,
      data: [{ id: 'allowed-skill', name: 'Allowed Skill', extensionName: 'allowed-extension' }],
    };
    const local = [
      { id: 'allowed-skill', name: 'Local Override', extensionName: 'allowed-extension' },
      { id: 'blocked-skill', name: 'Blocked Skill', extensionName: 'blocked-extension' },
    ];

    expect(mergeExtensionData('/api/extensions/skills', backend, local)).toEqual([
      { id: 'allowed-skill', name: 'Allowed Skill', extensionName: 'allowed-extension' },
    ]);
  });

  it('merges extension agent activity from Rust and local runtime facts', () => {
    const backend = {
      success: true,
      data: {
        generatedAt: 100,
        totalConversations: 3,
        runningConversations: 1,
        agents: [{ id: 'rust-agent', backend: 'rust', agentName: 'Rust Agent' }],
      },
    };
    const local = {
      generatedAt: 110,
      totalConversations: 0,
      runningConversations: 0,
      agents: [{ id: 'local-agent', backend: 'desktop', agentName: 'Local Extension Agent' }],
    };

    expect(mergeExtensionData('/api/extensions/agent-activity', backend, local)).toEqual({
      generatedAt: 100,
      totalConversations: 3,
      runningConversations: 1,
      agents: [
        { id: 'rust-agent', backend: 'rust', agentName: 'Rust Agent' },
        { id: 'local-agent', backend: 'desktop', agentName: 'Local Extension Agent' },
      ],
    });
  });

  it('filters local channel plugins through Rust channel-plugin contributions', () => {
    const backend = {
      success: true,
      data: [
        { plugin_id: 'telegram', enabled: false, is_extension: false },
        { plugin_id: 'allowed-channel', enabled: true, status: 'configured', is_extension: true },
      ],
    };
    const allowedExtensions = {
      success: true,
      data: [{ plugin_id: 'allowed-channel', extensionName: 'allowed-extension' }],
    };
    const localPlugins = [
      {
        plugin_id: 'allowed-channel',
        enabled: false,
        status: 'disabled',
        is_extension: true,
        extension_meta: { extensionName: 'allowed-extension', icon: '/api/extensions/static/allowed/icon.svg' },
      },
      {
        plugin_id: 'blocked-channel',
        enabled: false,
        status: 'disabled',
        is_extension: true,
        extension_meta: { extensionName: 'blocked-extension' },
      },
    ];

    expect(mergeChannelPlugins(backend, localPlugins, allowedExtensions)).toEqual([
      { plugin_id: 'telegram', enabled: false, is_extension: false },
      {
        plugin_id: 'allowed-channel',
        enabled: true,
        status: 'configured',
        is_extension: true,
        extension_meta: { extensionName: 'allowed-extension', icon: '/api/extensions/static/allowed/icon.svg' },
      },
    ]);
  });

  it('requires Rust installed and enabled extension rows for static assets', () => {
    const backend = {
      success: true,
      data: [
        { name: 'allowed-extension', enabled: true, installed: true, install_status: 'installed' },
        { name: 'disabled-extension', enabled: false, installed: true, install_status: 'installed' },
        { name: 'failed-extension', enabled: true, installed: false, install_status: 'install_failed' },
        { extension_name: 'update-extension', enabled: true, installed: true, install_status: 'update_available' },
      ],
    };

    expect(isExtensionStaticAssetAllowed(backend, 'allowed-extension')).toBe(true);
    expect(isExtensionStaticAssetAllowed(backend, 'update-extension')).toBe(true);
    expect(isExtensionStaticAssetAllowed(backend, 'disabled-extension')).toBe(false);
    expect(isExtensionStaticAssetAllowed(backend, 'failed-extension')).toBe(false);
    expect(isExtensionStaticAssetAllowed(backend, 'blocked-extension')).toBe(false);
  });

  it('uses Rust allowed channel-plugin records when backend channel state is absent', () => {
    const backend = {
      success: true,
      data: [{ plugin_id: 'telegram', enabled: false, is_extension: false }],
    };
    const allowedExtensions = {
      success: true,
      data: [
        {
          plugin_id: 'allowed-channel',
          id: 'allowed-channel',
          type: 'allowed-channel',
          name: 'Allowed Channel',
          enabled: false,
          connected: false,
          status: 'disabled',
          is_extension: true,
          extension_meta: { extensionName: 'allowed-extension', icon: '/api/extensions/static/allowed/icon.svg' },
        },
      ],
    };
    const localPlugins = [
      {
        plugin_id: 'allowed-channel',
        id: 'allowed-channel',
        type: 'allowed-channel',
        name: 'Local Channel',
        enabled: true,
        connected: true,
        status: 'configured',
        is_extension: true,
        extension_meta: {
          extensionName: 'allowed-extension',
          icon: '/local/icon.svg',
          configFields: [{ name: 'token' }],
        },
      },
    ];

    expect(mergeChannelPlugins(backend, localPlugins, allowedExtensions)).toEqual([
      { plugin_id: 'telegram', enabled: false, is_extension: false },
      {
        plugin_id: 'allowed-channel',
        id: 'allowed-channel',
        type: 'allowed-channel',
        name: 'Allowed Channel',
        enabled: false,
        connected: false,
        status: 'disabled',
        is_extension: true,
        extension_meta: {
          extensionName: 'allowed-extension',
          icon: '/api/extensions/static/allowed/icon.svg',
          configFields: [{ name: 'token' }],
        },
      },
    ]);
  });
});
