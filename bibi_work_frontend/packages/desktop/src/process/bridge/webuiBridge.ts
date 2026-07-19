/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 *
 * Desktop IPC bridge for WebUI lifecycle (start/stop/getStatus).
 *
 * WebUI credential operations (change-password / change-username / reset-password /
 * generate-qr-token) are NOT handled here. They are protected enterprise routes
 * called directly by the renderer via ipcBridge HTTP.
 *
 * This bridge owns only the lifecycle + status snapshot, because spawning a
 * WebUI instance requires Electron's app.* / Node child_process — BiWork backend
 * has no way to start a WebUI wrapper around itself.
 */

import { ipcBridge } from '@/common';
import {
  startDesktopWebUI,
  stopDesktopWebUI,
  getDesktopWebUIStatus,
  setDesktopWebUIInitialPassword,
} from '@process/utils/webuiConfig';

type AdminUsernameResult = { username?: string };
type AuthStatusResult = {
  auth_mode?: string;
  needs_setup?: boolean;
  data?: {
    auth_mode?: string;
    needs_setup?: boolean;
  };
};

type BackendAuthStatus = {
  authMode?: string;
  needsSetup: boolean;
};

function getBackendPort(): number | undefined {
  return (globalThis as typeof globalThis & { __backendPort?: number }).__backendPort;
}

async function fetchAdminUsername(): Promise<string> {
  const port = getBackendPort();
  if (!port) return 'admin';
  try {
    const res = await fetch(`http://127.0.0.1:${port}/api/auth/internal/users/system`);
    if (!res.ok) return 'admin';
    const json = (await res.json()) as { data?: AdminUsernameResult | null };
    return json.data?.username ?? 'admin';
  } catch {
    return 'admin';
  }
}

async function fetchBackendAuthStatus(port: number): Promise<BackendAuthStatus> {
  const statusRes = await fetch(`http://127.0.0.1:${port}/api/auth/status`);
  if (!statusRes.ok) {
    throw new Error(`[WebUI] /api/auth/status returned ${statusRes.status}`);
  }
  const statusJson = (await statusRes.json()) as AuthStatusResult;
  return {
    authMode: statusJson.auth_mode ?? statusJson.data?.auth_mode,
    needsSetup: statusJson.needs_setup ?? statusJson.data?.needs_setup ?? false,
  };
}

/**
 * FerrisKey/OIDC owns WebUI authentication in enterprise mode. The desktop
 * lifecycle bridge only clears any old plaintext password cache and refuses to
 * seed local passwords when the backend reports an unsupported password setup.
 */
async function maybeSeedInitialPassword(): Promise<string | undefined> {
  const port = getBackendPort();
  if (!port) {
    throw new Error('[WebUI] Cannot start: BiWork backend is not running (globalThis.__backendPort unset)');
  }
  const { authMode, needsSetup } = await fetchBackendAuthStatus(port);
  if (!needsSetup || authMode === 'ferriskey_oidc') {
    setDesktopWebUIInitialPassword(undefined);
    return authMode;
  }
  throw new Error(
    `[WebUI] local password setup is not supported by the enterprise backend (auth_mode=${authMode ?? 'unknown'})`
  );
}

export function initWebuiBridge(): void {
  ipcBridge.webui.getStatus.provider(async () => {
    const snapshot = getDesktopWebUIStatus();
    const adminUsername = await fetchAdminUsername();
    const port = getBackendPort();
    const authStatus = port ? await fetchBackendAuthStatus(port).catch((): undefined => undefined) : undefined;
    return { ...snapshot, adminUsername, authMode: authStatus?.authMode };
  });

  ipcBridge.webui.start.provider(async (params) => {
    const authMode = await maybeSeedInitialPassword();
    const handle = await startDesktopWebUI({
      port: params?.port,
      allowRemote: params?.allowRemote,
    });
    ipcBridge.webui.statusChanged.emit({
      running: true,
      port: handle.port,
      localUrl: handle.localUrl,
      networkUrl: handle.networkUrl,
      lanIP: handle.lanIP,
      authMode,
      initialPassword: handle.initialPassword,
    });
    return { ...handle, authMode };
  });

  ipcBridge.webui.stop.provider(async () => {
    await stopDesktopWebUI();
    ipcBridge.webui.statusChanged.emit({ running: false });
  });
}
