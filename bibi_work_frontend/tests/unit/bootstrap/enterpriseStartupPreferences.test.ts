/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { clearAccessToken, setAccessToken } from '@/common/auth/authTokenBroker';
import { readCloseToTraySetting } from '@/process/utils/closeToTraySetting';
import { refreshTrayMenu } from '@/process/utils/tray';
import { restoreDesktopWebUIFromPreferences } from '@/process/utils/webuiConfig';

const { activeCountMock, httpRequestMock, processConfigGetMock, processConfigSetMock, startWebHostMock } = vi.hoisted(
  () => ({
    activeCountMock: vi.fn(),
    httpRequestMock: vi.fn(),
    processConfigGetMock: vi.fn(),
    processConfigSetMock: vi.fn(),
    startWebHostMock: vi.fn(),
  })
);

vi.mock('electron', () => ({
  app: {
    getPath: vi.fn(() => '/tmp/biwork-test'),
  },
}));

vi.mock('@biwork/web-host', () => ({
  startWebHost: startWebHostMock,
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    conversation: {
      activeCount: {
        invoke: activeCountMock,
      },
    },
  },
}));

vi.mock('@/common/adapter/httpBridge', () => ({
  httpRequest: httpRequestMock,
}));

vi.mock('@process/services/i18n', () => ({
  default: {
    t: (key: string) => key,
  },
}));

vi.mock('@/process/utils/initStorage', () => ({
  ProcessConfig: {
    get: processConfigGetMock,
    set: processConfigSetMock,
  },
  getSystemDir: () => ({
    cacheDir: '/tmp/biwork-test/cache',
    logDir: '/tmp/biwork-test/log',
    workDir: '/tmp/biwork-test/work',
  }),
}));

vi.mock('@/process/utils/utils', () => ({
  getDataPath: () => '/tmp/biwork-test/data',
}));

beforeEach(() => {
  clearAccessToken();
  vi.clearAllMocks();
  processConfigGetMock.mockResolvedValue(undefined);
  processConfigSetMock.mockResolvedValue(undefined);
});

describe('enterprise startup preferences', () => {
  it('uses local close-to-tray default without querying enterprise settings when logged out', async () => {
    await expect(readCloseToTraySetting()).resolves.toBe(false);

    expect(httpRequestMock).not.toHaveBeenCalled();
  });

  it('reads close-to-tray from enterprise settings after token sync', async () => {
    setAccessToken('token');
    httpRequestMock.mockResolvedValue({ 'system.closeToTray': true });

    await expect(readCloseToTraySetting()).resolves.toBe(true);

    expect(httpRequestMock).toHaveBeenCalledWith('GET', '/api/settings/client?keys=system.closeToTray', undefined, {
      silentStatuses: [404],
    });
  });

  it('does not auto-restore WebUI from enterprise preferences when logged out', async () => {
    await restoreDesktopWebUIFromPreferences();

    expect(httpRequestMock).not.toHaveBeenCalled();
    expect(startWebHostMock).not.toHaveBeenCalled();
  });

  it('does not fetch tray active conversation count when logged out', async () => {
    await refreshTrayMenu();

    expect(activeCountMock).not.toHaveBeenCalled();
  });

  it('fetches tray active conversation count after token sync', async () => {
    setAccessToken('token');
    activeCountMock.mockResolvedValue({ count: 3 });

    await refreshTrayMenu();

    expect(activeCountMock).toHaveBeenCalledOnce();
  });
});
