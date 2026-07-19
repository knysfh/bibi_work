/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import React from 'react';
import { ConfigProvider } from '@arco-design/web-react';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { SWRConfig } from 'swr';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  getCdpStatus: vi.fn(),
  isDevToolsOpened: vi.fn(),
  openDevTools: vi.fn(),
  devToolsStateChangedOn: vi.fn(),
  updateCdpConfig: vi.fn(),
  restart: vi.fn(),
  openExternal: vi.fn(),
  workbenchBootstrap: vi.fn(),
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock('@/renderer/utils/appRestart', () => ({
  notifyManualRestartRequired: vi.fn(),
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    application: {
      getCdpStatus: { invoke: mocks.getCdpStatus },
      isDevToolsOpened: { invoke: mocks.isDevToolsOpened },
      openDevTools: { invoke: mocks.openDevTools },
      devToolsStateChanged: { on: mocks.devToolsStateChangedOn },
      updateCdpConfig: { invoke: mocks.updateCdpConfig },
      restart: { invoke: mocks.restart },
    },
    shell: {
      openExternal: { invoke: mocks.openExternal },
    },
    workbench: {
      bootstrap: { invoke: mocks.workbenchBootstrap },
    },
  },
}));

import DevSettings from '@/renderer/components/settings/SettingsModal/contents/SystemModalContent/DevSettings';

const cdpStatus = {
  success: true,
  data: {
    isDevMode: true,
    enabled: false,
    port: null,
    startupEnabled: false,
    configEnabled: false,
    registry: [],
  },
};

const bootstrapWithCdpFlag = (enabled: boolean) => ({
  feature_flags: {
    desktop: {
      cdp_remote_control: enabled,
    },
  },
});

const renderSettings = () =>
  render(
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <ConfigProvider>
        <DevSettings />
      </ConfigProvider>
    </SWRConfig>
  );

describe('DevSettings workbench feature flags', () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    mocks.getCdpStatus.mockResolvedValue(cdpStatus);
    mocks.isDevToolsOpened.mockResolvedValue(false);
    mocks.devToolsStateChangedOn.mockReturnValue(() => undefined);
    mocks.openDevTools.mockResolvedValue(true);
    mocks.updateCdpConfig.mockResolvedValue({ success: true });
    mocks.restart.mockResolvedValue({ restarted: true, manualRestartRequired: false });
    mocks.openExternal.mockResolvedValue(undefined);
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });
  });

  it('keeps DevTools visible while hiding CDP remote controls when the flag is disabled', async () => {
    mocks.workbenchBootstrap.mockResolvedValue(bootstrapWithCdpFlag(false));

    renderSettings();

    await screen.findByText('settings.devTools');
    await waitFor(() => expect(mocks.workbenchBootstrap).toHaveBeenCalledTimes(1));

    expect(screen.queryByText('settings.cdp.title')).not.toBeInTheDocument();
  });

  it('shows CDP remote controls when the flag is enabled', async () => {
    mocks.workbenchBootstrap.mockResolvedValue(bootstrapWithCdpFlag(true));

    renderSettings();

    await screen.findByText('settings.devTools');
    expect(await screen.findByText('settings.cdp.title')).toBeInTheDocument();
  });
});
