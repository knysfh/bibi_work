import type { TokenSet } from "../api/token-provider";
import { createBrowserTokenProvider } from "../api/token-provider";
import type { InvokeClient } from "./invoke-client";

export interface OidcCallback {
  code: string;
  state: string;
  error?: string;
}

export interface DeviceInfo {
  deviceName: string;
  platform: string;
  arch: string;
  fingerprint: string;
}

export interface LocalExecBridgeStatus {
  state: "disabled" | "idle" | "connected" | "executing" | "failed";
  detail: string;
}

export interface LocalExecStartBridgeRequest {
  apiBaseUrl: string;
  accessToken: string;
  tenantId: string;
  deviceId: string;
  userAgent?: string;
  pollIntervalMs?: number;
}

export interface LocalMountFolderSelection {
  displayName: string;
  realPath: string;
}

export interface OidcLoginRequest {
  authorizationUrl: string;
  tokenEndpoint: string;
  clientId: string;
  redirectUri: string;
  codeVerifier: string;
  state: string;
}

export interface DesktopAuthApi {
  openLoginUrl(url: string): Promise<void>;
  loginWithOidc(request: OidcLoginRequest): Promise<TokenSet>;
  saveTokenSet(tokenSet: TokenSet): Promise<void>;
  loadTokenSet(): Promise<TokenSet | null>;
  clearTokenSet(): Promise<void>;
  getDeviceInfo(): Promise<DeviceInfo>;
  localExecRegisterDevice(): Promise<LocalExecBridgeStatus>;
  localExecStartBridge(request: LocalExecStartBridgeRequest): Promise<LocalExecBridgeStatus>;
  localExecStopBridge(): Promise<LocalExecBridgeStatus>;
  pickLocalMountFolder(): Promise<LocalMountFolderSelection | null>;
  saveLocalMountRealPath(localMountId: string, realPath: string): Promise<void>;
}

const TOKEN_STORE_KEY = "ferriskey-token-set";

export function createDesktopAuthApi(invokeClient: InvokeClient): DesktopAuthApi {
  const browserTokenProvider = createBrowserTokenProvider();

  if (!invokeClient.isTauriRuntime()) {
    return {
      async openLoginUrl(url) {
        window.open(url, "_blank", "noopener,noreferrer");
      },
      async loginWithOidc(request) {
        window.open(request.authorizationUrl, "_blank", "noopener,noreferrer");
        throw new Error("Desktop OIDC login is only available in the desktop runtime");
      },
      async saveTokenSet(tokenSet) {
        await browserTokenProvider.setTokenSet(tokenSet);
      },
      async loadTokenSet() {
        const accessToken = await browserTokenProvider.getAccessToken();
        return accessToken ? { accessToken } : null;
      },
      async clearTokenSet() {
        await browserTokenProvider.clearTokenSet();
      },
      async getDeviceInfo() {
        return {
          deviceName: navigator.platform || "browser-device",
          platform: navigator.platform,
          arch: "browser",
          fingerprint: "browser-dev"
        };
      },
      async localExecRegisterDevice() {
        return disabledLocalExecStatus();
      },
      async localExecStartBridge() {
        return disabledLocalExecStatus();
      },
      async localExecStopBridge() {
        return disabledLocalExecStatus();
      },
      async pickLocalMountFolder() {
        return null;
      },
      async saveLocalMountRealPath() {
        return;
      }
    };
  }

  return {
    openLoginUrl: (url) => invokeClient.invoke("auth_open_external_browser", { url }),
    async loginWithOidc(request) {
      const tokenSet = normalizeTokenSet(
        await invokeClient.invoke<unknown>("auth_login_with_deep_link", { request })
      );
      if (!tokenSet) {
        throw new Error("OIDC login did not return an access token");
      }
      return tokenSet;
    },
    async saveTokenSet(tokenSet) {
      const normalized = normalizeTokenSet(tokenSet);
      if (!normalized) {
        throw new Error("No access token is available");
      }
      await invokeClient.invoke("secure_store_set", {
        key: TOKEN_STORE_KEY,
        value: JSON.stringify(normalized)
      });
    },
    async loadTokenSet() {
      const raw = await invokeClient.invoke<string | null>("secure_store_get", {
        key: TOKEN_STORE_KEY
      });
      if (!raw) {
        return null;
      }
      return normalizeTokenSet(JSON.parse(raw));
    },
    clearTokenSet: () => invokeClient.invoke("secure_store_delete", { key: TOKEN_STORE_KEY }),
    getDeviceInfo: () => invokeClient.invoke("system_get_device_info"),
    localExecRegisterDevice: () => invokeClient.invoke("local_exec_register_device"),
    localExecStartBridge: (request) => invokeClient.invoke("local_exec_start_bridge", { request }),
    localExecStopBridge: () => invokeClient.invoke("local_exec_stop_bridge"),
    pickLocalMountFolder: () => invokeClient.invoke("local_mount_pick_folder"),
    async saveLocalMountRealPath(localMountId, realPath) {
      if (!localMountId) {
        throw new Error("localMountId is required");
      }
      if (!realPath.trim()) {
        throw new Error("realPath is required");
      }
      await invokeClient.invoke("local_mount_register_real_path", {
        request: { localMountId, realPath }
      });
    }
  };
}

function normalizeTokenSet(value: unknown): TokenSet | null {
  if (!value || typeof value !== "object") {
    return null;
  }
  const payload = value as Record<string, unknown>;
  const accessToken =
    typeof payload.accessToken === "string"
      ? payload.accessToken.trim()
      : typeof payload.access_token === "string"
        ? payload.access_token.trim()
        : "";
  if (!accessToken) {
    return null;
  }
  const refreshToken =
    typeof payload.refreshToken === "string"
      ? payload.refreshToken
      : typeof payload.refresh_token === "string"
        ? payload.refresh_token
        : undefined;
  const expiresAt =
    typeof payload.expiresAt === "string"
      ? payload.expiresAt
      : typeof payload.expires_at === "string"
        ? payload.expires_at
        : undefined;
  return { accessToken, refreshToken, expiresAt };
}

function disabledLocalExecStatus(): LocalExecBridgeStatus {
  return {
    state: "disabled",
    detail: "local executor bridge is not active in browser development mode"
  };
}
