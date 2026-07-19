/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

// configureChromium sets app name (dev isolation) and Chromium flags — must run before
// ANY module that calls app.getPath('userData'), because Electron caches the path on first call.
import './process/utils/configureChromium';
import { installGpuCrashHandler } from './process/utils/gpuRecovery';
import { captureBackendStartupFailure, initSentry, scheduleStartupLogReport, setSentryDeviceId } from './sentry';

initSentry();

import './process/utils/configureConsoleLog';
import { app, BrowserWindow, ipcMain, nativeImage, powerMonitor, safeStorage, shell } from 'electron';
import fixPath from 'fix-path';
import * as fs from 'fs';
import * as path from 'path';
import { httpRequest } from './common/adapter/httpBridge';
import {
  peekAccessToken as peekMainAccessToken,
  setAccessToken as setMainAccessToken,
  setAccessTokenProvider as setMainAccessTokenProvider,
  setAuthSessionInvalidator as setMainAuthSessionInvalidator,
} from './common/auth/authTokenBroker';
import { initMainAdapterWithWindow } from './common/adapter/main';
import { ipcBridge } from './common';
import { initializeProcess } from './process';
import {
  resolveExternalEnterpriseBackendConfig,
  verifyExternalEnterpriseBackend,
} from './process/startup/externalEnterpriseBackend';
import { classifyBackendStartupFailure } from './process/startup/backendStartupFailure';
import { installQuitCleanup } from './process/startup/quitCleanup';
import { ProcessConfig } from './process/utils/initStorage';
import type { BackendStartupFailureInfo } from './common/types/platform/electron';
import { registerWindowMaximizeListeners } from '@process/bridge';
import { startStaticServer } from '@biwork/web-host';
import {
  MAX_MULTIPART_UPLOAD_BYTES,
  MultipartUploadError,
  parseMultipartUpload,
  writeLocalUploadFile,
} from './process/gateway/upload';
import { handleLocalFsRoute, LocalFsRouteError } from './process/gateway/localFs';
import { browseLocalDirectory } from './process/gateway/directoryBrowser';
import { handleFileSnapshotRoute, FileSnapshotRouteError } from './process/gateway/fileSnapshot';
import { handlePreviewHistoryRoute, PreviewHistoryRouteError } from './process/gateway/previewHistory';
import { discoverLocalStdioMcpTools, LocalMcpError } from './process/gateway/mcpLocal';
import { startLocalMcpWorker } from './process/gateway/mcpLocalWorker';
import { BrowserSessionManager } from './process/browser/browserSessionManager';
import { startBrowserWorker } from './process/browser/browserWorker';
import { startDesktopAcpWorker } from './process/agent/acp/worker';
import { DesktopOidcController } from './process/auth/desktopOidcController';
import { DesktopRefreshTokenStore } from './process/auth/desktopRefreshTokenStore';
import {
  initializeDesktopTelemetry,
  injectDesktopTraceHeaders,
  traceDesktopHttpRequest,
} from './process/telemetry/desktopTelemetry';
import { handleOfficeLocalRoute, OfficeLocalRouteError } from './process/gateway/officeLocal';
import { proxyOfficeWatchRequest } from './process/gateway/officeProxy';
import { DesktopGatewayController } from './process/gateway/desktopGatewayController';
import {
  checkToolInstalled,
  commandExists,
  openFolderWithTool,
  validateExternalUrl,
} from './process/gateway/shellTools';
import { handleChannelLocalRoute, ChannelLocalRouteError } from './process/gateway/channelLocal';
import { applyHubLocalStateToExtensions, handleHubLocalRoute, HubLocalRouteError } from './process/gateway/hubLocal';
import {
  buildExtensionSyncPayload,
  handleExtensionLocalRoute,
  listExtensionChannelPlugins,
  previewExtensionEnabledState,
  readExtensionStaticAsset,
  ExtensionLocalRouteError,
} from './process/gateway/extensionLocal';
import {
  extractHubExtensions,
  isExtensionStaticAssetAllowed,
  mergeChannelPlugins,
  mergeExtensionData,
  replaceBackendData,
} from './process/gateway/extensionGovernance';
import { classifyDesktopGatewayRoute, shouldProxyFsFacadeToRust } from './process/gateway/routeOwnership';
import './process/bridge/feedbackBridge';
import { wasLaunchedAtLogin } from '@process/bridge/applicationBridge';
import { onLanguageChanged } from './process/bridge/systemSettingsBridge';
import { setInitialLanguage } from '@process/services/i18n';
import { setupApplicationMenu } from './process/utils/appMenu';
import { startWebHost } from '@biwork/web-host';
import { initializeZoomFactor, setupZoomForWindow } from './process/utils/zoom';
import { hydrateWindowsProcessPath } from './process/startup/windowsPath';
import {
  MIN_WINDOW_WIDTH,
  MIN_WINDOW_HEIGHT,
  attachWindowBoundsPersistence,
  loadSavedWindowBounds,
  resolveInitialBounds,
} from './process/utils/windowBounds';
import {
  clearPendingDeepLinkUrl,
  getPendingDeepLinkUrl,
  handleDeepLinkUrl,
  PROTOCOL_SCHEME,
} from './process/utils/deepLink';
import { startOidcLoopbackServer } from './process/utils/oidcLoopback';
import {
  bindMainWindowReferences,
  showAndFocusMainWindow,
  showOrCreateMainWindow,
} from './process/utils/mainWindowLifecycle';
import {
  loadUserWebUIConfig,
  resolveRemoteAccess,
  resolveWebUIPort,
  restoreDesktopWebUIFromPreferences,
} from './process/utils/webuiConfig';
import {
  createOrUpdateTray,
  destroyTray,
  getCloseToTrayEnabled,
  getIsQuitting,
  refreshTrayMenu,
  setCloseToTrayEnabled,
  setIsQuitting,
} from './process/utils/tray';
import { readCloseToTraySetting } from './process/utils/closeToTraySetting';
// @ts-expect-error - electron-squirrel-startup doesn't have types
import electronSquirrelStartup from 'electron-squirrel-startup';

// ============ Single Instance Lock ============
// Acquire lock early so the second instance quits before doing unnecessary work.
// When a second instance starts (e.g. from protocol URL), it sends its data
// to the first instance via second-instance event, then quits.
const isE2ETestMode = process.env.BIWORK_E2E_TEST === '1';
const skipSingleInstanceLock = isE2ETestMode || process.env.BIWORK_MULTI_INSTANCE === '1';
const deepLinkFromArgv = process.argv.find((arg) => arg.startsWith(`${PROTOCOL_SCHEME}://`));
const gotTheLock = skipSingleInstanceLock ? true : app.requestSingleInstanceLock({ deepLinkUrl: deepLinkFromArgv });
let dispatchDeepLinkUrl: (url: string) => void = handleDeepLinkUrl;
if (!gotTheLock) {
  console.warn('[BiWork] Another instance is already running; current process will exit.');
  app.quit();
} else {
  app.on('second-instance', (_event, argv, _workingDirectory, additionalData) => {
    // Prefer additionalData (reliable on all platforms), fallback to argv scan
    const deepLinkUrl =
      (additionalData as { deepLinkUrl?: string })?.deepLinkUrl ||
      argv.find((arg) => arg.startsWith(`${PROTOCOL_SCHEME}://`));
    if (deepLinkUrl) {
      dispatchDeepLinkUrl(deepLinkUrl);
    }
    // Focus existing window or recreate one if needed.
    if (isWebUIMode || isResetPasswordMode) {
      return;
    }

    // Skip window creation if app hasn't finished initializing
    if (!appReadyDone) return;

    if (app.isReady()) {
      showOrCreateMainWindow({
        mainWindow,
        createWindow: () => {
          console.log('[BiWork] second-instance received with no active main window, recreating main window');
          createWindow();
        },
      });
    }
  });
}

// Align GUI-launched PATH with what local CLIs expect on each desktop OS.
if (process.platform === 'darwin' || process.platform === 'linux') {
  fixPath();

  // Supplement nvm paths that fix-path might miss (nvm is often only in .zshrc, not .zshenv)
  const nvmDir = process.env.NVM_DIR || path.join(process.env.HOME || '', '.nvm');
  const nvmVersionsDir = path.join(nvmDir, 'versions', 'node');
  if (fs.existsSync(nvmVersionsDir)) {
    try {
      const versions = fs.readdirSync(nvmVersionsDir);
      const nvmPaths = versions.map((v) => path.join(nvmVersionsDir, v, 'bin')).filter((p) => fs.existsSync(p));
      if (nvmPaths.length > 0) {
        const currentPath = process.env.PATH || '';
        const missingPaths = nvmPaths.filter((p) => !currentPath.includes(p));
        if (missingPaths.length > 0) {
          process.env.PATH = [...missingPaths, currentPath].join(path.delimiter);
        }
      }
    } catch {
      // Ignore errors when reading nvm directory
    }
  }
} else if (process.platform === 'win32') {
  hydrateWindowsProcessPath();
}

// Handle Squirrel startup events (Windows installer)
if (electronSquirrelStartup) {
  app.quit();
}

// Global error handlers for main process
// Sentry automatically captures these, but we keep the handlers to prevent Electron's default error dialog
process.on('uncaughtException', (_error) => {
  // Sentry captures this automatically
});

process.on('unhandledRejection', (_reason, _promise) => {
  // Sentry captures this automatically
});

const hasSwitch = (flag: string) => process.argv.includes(`--${flag}`) || app.commandLine.hasSwitch(flag);
const getSwitchValue = (flag: string): string | undefined => {
  const withEqualsPrefix = `--${flag}=`;
  const equalsArg = process.argv.find((arg) => arg.startsWith(withEqualsPrefix));
  if (equalsArg) {
    return equalsArg.slice(withEqualsPrefix.length);
  }

  const argIndex = process.argv.indexOf(`--${flag}`);
  if (argIndex !== -1) {
    const nextArg = process.argv[argIndex + 1];
    if (nextArg && !nextArg.startsWith('--')) {
      return nextArg;
    }
  }

  const cliValue = app.commandLine.getSwitchValue(flag);
  return cliValue || undefined;
};
const hasCommand = (cmd: string) => process.argv.includes(cmd);

const isWebUIMode = hasSwitch('webui');
const isRemoteMode = hasSwitch('remote');
const isResetPasswordMode = hasCommand('--resetpass');
const isVersionMode = hasCommand('--version') || hasCommand('-v');
const externalEnterpriseBackendConfig = resolveExternalEnterpriseBackendConfig();
const desktopTelemetry = initializeDesktopTelemetry({ serviceVersion: app.getVersion() });

// Flag to distinguish intentional quit from unexpected exit in WebUI mode
let isExplicitQuit = false;

// Guard against premature window creation (e.g. macOS 'activate' firing during init).
// The activate event fires on first launch before handleAppReady finishes initializeProcess(),
// causing the renderer to load and compete with initStorage on the serial configFile queue,
// which blocks startup for 100-265 seconds.
let appReadyDone = false;

let mainWindow: BrowserWindow;
let disposeCronResumeListener: (() => void) | null = null;
let desktopGatewayController!: DesktopGatewayController;
let backendReadyPromise: Promise<void> | null = null;
let backendStartedOk = false;
let backendStartupFailed = false;
let backendStartupFailureInfo: BackendStartupFailureInfo | null = null;
let rendererInitialLanguage: string | null = null;
let backendMigrationsScheduled = false;
const desktopRefreshTokenStore = new DesktopRefreshTokenStore(
  path.join(app.getPath('userData'), 'auth', 'refresh-token.v1.json'),
  {
    isAvailable: () =>
      safeStorage.isEncryptionAvailable() &&
      (process.platform !== 'linux' || safeStorage.getSelectedStorageBackend() !== 'basic_text'),
    encrypt: (plaintext) => safeStorage.encryptString(plaintext),
    decrypt: (encrypted) => safeStorage.decryptString(encrypted),
  },
  (message, error) => console.warn(message, error)
);
const desktopOidcController = new DesktopOidcController({
  apiUrl: (endpoint) => desktopAuthApiUrl(endpoint),
  emitAccessTokenChanged: (accessToken) => ipcBridge.auth.accessTokenChanged.emit({ accessToken }),
  emitLoginCompleted: (accessToken) => ipcBridge.auth.loginCompleted.emit({ accessToken, authenticated: true }),
  emitLoginFailed: (message) => ipcBridge.auth.loginFailed.emit({ message }),
  emitSessionExpired: (reason) => ipcBridge.auth.sessionExpired.emit({ reason }),
  getLoopbackPort: () => desktopGatewayController?.oidcLoopbackPort ?? null,
  getStoredAccessToken: peekMainAccessToken,
  openExternal: (url) => shell.openExternal(url),
  refreshTokenStore: desktopRefreshTokenStore,
  setStoredAccessToken: setMainAccessToken,
});
setMainAccessTokenProvider((forceRefresh) => desktopOidcController.getValidAccessToken(forceRefresh));
setMainAuthSessionInvalidator(() => desktopOidcController.invalidateSession());

desktopGatewayController = new DesktopGatewayController({
  startServer: (backendPort) =>
    startStaticServer({
      staticDir: path.join(__dirname, '../renderer'),
      backendPort,
      port: 0,
      backendRequestHeaders: forwardedBackendHeaders,
      localApiHandler: createDesktopLocalApiHandler(backendPort),
      requestContext: traceDesktopHttpRequest,
    }),
  startOidcLoopback: () =>
    startOidcLoopbackServer({
      onCallback: (callback) => desktopOidcController.handleCallback(callback),
    }),
  startLocalMcpWorker: (backendPort) =>
    startLocalMcpWorker({
      backendBaseUrl: `http://127.0.0.1:${backendPort}`,
      getAccessToken: () => desktopOidcController.getAccessToken(),
    }),
  startBrowserWorker: (backendPort) =>
    startBrowserWorker({
      backendBaseUrl: `http://127.0.0.1:${backendPort}`,
      getAccessToken: () => desktopOidcController.getAccessToken(),
      manager: new BrowserSessionManager({
        profilesDirectory: path.join(app.getPath('userData'), 'browser-profiles'),
      }),
    }),
  startDesktopAcpWorker: (backendPort) =>
    startDesktopAcpWorker({
      backendBaseUrl: `http://127.0.0.1:${backendPort}`,
      getAccessToken: () => desktopOidcController.getAccessToken(),
      runsDirectory: path.join(app.getPath('userData'), 'acp-runs'),
    }),
  onLoopbackStartError: (error) => console.error('[BiWork] Failed to start OIDC loopback callback server:', error),
  onStarted: (gatewayPort, backendPort) =>
    console.log(`[BiWork] Desktop API gateway started (port=${gatewayPort}, backendPort=${backendPort})`),
});

ipcMain.on('get-backend-port', (event) => {
  const exposedPort = (globalThis as typeof globalThis & { __backendPort?: number }).__backendPort;
  event.returnValue = exposedPort ?? externalEnterpriseBackendConfig?.backendPort ?? 0;
});

ipcMain.on('get-initial-language', (event) => {
  event.returnValue = rendererInitialLanguage;
});

ipcMain.on('get-backend-startup-failed', (event) => {
  event.returnValue = backendStartupFailed;
});

ipcMain.on('get-backend-startup-failure', (event) => {
  event.returnValue = backendStartupFailureInfo;
});

ipcMain.on('get-biwork-e2e-test', (event) => {
  event.returnValue = process.env.BIWORK_E2E_TEST === '1';
});

ipcMain.handle('backend:recover-corrupted-database', async () => {
  throw new Error('Database recovery is managed by bibi_work_backend');
});

ipcMain.handle('auth:access-token:get', (_event, forceRefresh: unknown) => {
  if (forceRefresh !== undefined && typeof forceRefresh !== 'boolean') {
    throw new Error('forceRefresh must be a boolean');
  }
  return desktopOidcController.getValidAccessToken(forceRefresh === true);
});

if (process.env.BIWORK_E2E_TEST === '1') {
  ipcMain.handle('auth:access-token:set', (_event, token: unknown) => {
    if (token !== null && typeof token !== 'string') throw new Error('auth access token must be a string or null');
    desktopOidcController.setAccessToken(typeof token === 'string' ? token : null);
  });
}

ipcMain.handle('auth:session:logout', () => desktopOidcController.logout());
ipcMain.handle('auth:session:invalidate', () => desktopOidcController.invalidateSession());

ipcMain.handle('auth:oidc-login:start', async () => desktopOidcController.startLogin());

function markBackendStartupFailed(error: unknown): void {
  backendStartupFailed = true;
  backendStartupFailureInfo = classifyBackendStartupFailure(error);
  (globalThis as typeof globalThis & { __backendStartupFailed?: boolean }).__backendStartupFailed = true;
}

function registerCronResumeBridge(backendPort: number): void {
  disposeCronResumeListener?.();

  const onResume = () => {
    void httpRequest('POST', '/api/cron/internal/system-resume', {
      source: 'electron.powerMonitor.resume',
      backendPort,
      resumedAt: Date.now(),
    }).catch((error) => {
      console.error('[BiWork] Failed to notify backend about system resume:', error);
    });
  };

  powerMonitor.on('resume', onResume);
  disposeCronResumeListener = () => {
    powerMonitor.removeListener('resume', onResume);
  };
}

/**
 * Run one-shot backend migrations after the renderer has loaded. Some steps
 * (ConfigStorage.get, ipcBridge.listProviders) route through the renderer via
 * BroadcastChannel, so invoking them before the renderer exists deadlocks the
 * main process. Called from did-finish-load.
 */
const scheduleBackendMigrations = (): void => {
  if (externalEnterpriseBackendConfig) return;
  if (backendMigrationsScheduled || !backendStartedOk) return;
  backendMigrationsScheduled = true;
  void (async () => {
    try {
      const { runBackendMigrations } = await import('./process/utils/runBackendMigrations');
      await runBackendMigrations(ProcessConfig);
      console.info('[BiWork] runBackendMigrations completed');
    } catch (error) {
      console.error('[BiWork] Backend migration hook threw:', error);
    }
  })();
};

function writeGatewayJson(
  res: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[1],
  status: number,
  body: unknown
): void {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(body));
}

const HOP_BY_HOP_HEADERS = new Set([
  'connection',
  'content-length',
  'host',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
]);

function forwardedBackendHeaders(
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0]
): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [key, value] of Object.entries(req.headers)) {
    if (HOP_BY_HOP_HEADERS.has(key.toLowerCase()) || value === undefined) continue;
    headers[key] = Array.isArray(value) ? value.join(', ') : value;
  }
  const hasAuthorizationHeader = Object.keys(headers).some((key) => key.toLowerCase() === 'authorization');
  const accessToken = desktopOidcController.getAccessToken();
  if (!hasAuthorizationHeader && accessToken) {
    headers.Authorization = `Bearer ${accessToken}`;
  }
  return injectDesktopTraceHeaders(headers);
}

async function readBackendJson(response: Response): Promise<unknown> {
  const raw = await response.text();
  if (!raw.trim()) return null;
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    return raw;
  }
}

function desktopAuthApiPort(): number {
  const exposedPort = (globalThis as typeof globalThis & { __backendPort?: number }).__backendPort;
  return exposedPort ?? desktopGatewayController?.port ?? externalEnterpriseBackendConfig?.backendPort ?? 0;
}

function desktopAuthApiUrl(endpoint: string): string {
  if (/^https?:\/\//i.test(endpoint)) return endpoint;
  const normalizedEndpoint = endpoint.startsWith('/') ? endpoint : `/${endpoint}`;
  return `http://127.0.0.1:${desktopAuthApiPort()}${normalizedEndpoint}`;
}

function desktopExtensionContext() {
  return {
    extensionRoots: [
      path.join(app.getPath('userData'), 'extensions'),
      path.join(app.getPath('userData'), 'hub-extensions'),
      path.join(process.resourcesPath, 'extensions'),
    ],
    statePath: path.join(app.getPath('userData'), 'extension-local-state.json'),
    hubStatePath: path.join(app.getPath('userData'), 'hub-local-state.json'),
  };
}

async function syncLocalExtensionsToBackend(
  backendPort: number,
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0],
  context: ReturnType<typeof desktopExtensionContext>
): Promise<unknown> {
  const payload = await buildExtensionSyncPayload(context);
  if (payload.extensions.length === 0) {
    return { success: true, data: { synced: 0, contributions: 0 } };
  }

  const response = await fetch(`http://127.0.0.1:${backendPort}/api/extensions/sync`, {
    method: 'POST',
    headers: {
      ...forwardedBackendHeaders(req),
      'content-type': 'application/json',
    },
    body: JSON.stringify(payload),
  });
  const body = await readBackendJson(response);
  if (response.ok) return body ?? { success: true, data: null };

  const detail =
    body && typeof body === 'object' && !Array.isArray(body)
      ? String(
          (body as Record<string, unknown>).error ?? (body as Record<string, unknown>).message ?? JSON.stringify(body)
        )
      : String(body ?? 'backend extension sync request failed');
  throw new Error(`EXTENSIONS_SYNC_FAILED ${response.status}: ${detail}`);
}

function backendEnvelopeData(body: unknown): unknown {
  if (body && typeof body === 'object' && !Array.isArray(body) && 'data' in body) {
    return (body as { data?: unknown }).data;
  }
  return body;
}

function withHubGovernanceSyncResult(data: unknown, syncResult: unknown): unknown {
  if (!data || typeof data !== 'object' || Array.isArray(data)) return data;
  return {
    ...(data as Record<string, unknown>),
    governanceSync: backendEnvelopeData(syncResult),
  };
}

function isExtensionLocalFallbackError(error: unknown): boolean {
  return error instanceof ExtensionLocalRouteError && error.code === 'EXTENSION_NOT_FOUND';
}

function isExtensionBackendFallback(response: Response, body: unknown): boolean {
  if (response.status === 404) return true;
  if (response.status !== 400) return false;
  if (!body || typeof body !== 'object') return false;
  const message = String((body as Record<string, unknown>).error ?? '').toLowerCase();
  return message.includes('extension not found');
}

async function readGatewayRawBody(
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0],
  maxBytes: number,
  tooLargeMessage: string
): Promise<Buffer> {
  const chunks: Buffer[] = [];
  let totalBytes = 0;
  for await (const chunk of req) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    totalBytes += buffer.byteLength;
    if (totalBytes > maxBytes) {
      throw new Error(tooLargeMessage);
    }
    chunks.push(buffer);
  }
  return Buffer.concat(chunks, totalBytes);
}

async function readGatewayJsonBody(
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0]
): Promise<Record<string, unknown>> {
  const rawBuffer = await readGatewayRawBody(req, 1024 * 1024, 'LOCAL_API_BODY_TOO_LARGE');
  if (rawBuffer.length === 0) return {};
  const raw = rawBuffer.toString('utf8').trim();
  if (!raw) return {};
  const parsed = JSON.parse(raw) as unknown;
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('LOCAL_API_BODY_INVALID');
  }
  return parsed as Record<string, unknown>;
}

async function proxyGatewayRequestToBackend(
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0],
  res: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[1],
  backendPort: number,
  url: URL
): Promise<void> {
  const body = await readGatewayRawBody(req, 1024 * 1024, 'LOCAL_API_BODY_TOO_LARGE');
  const backendResponse = await fetch(`http://127.0.0.1:${backendPort}${url.pathname}${url.search}`, {
    method: req.method,
    headers: forwardedBackendHeaders(req),
    body: body.length > 0 ? new Uint8Array(body) : undefined,
  });
  const backendBody = await readBackendJson(backendResponse);
  writeGatewayJson(res, backendResponse.status, backendBody ?? { success: backendResponse.ok, data: null });
}

async function proxyGatewayJsonBodyToBackend(
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0],
  res: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[1],
  backendPort: number,
  url: URL,
  body: Record<string, unknown>
): Promise<void> {
  const backendResponse = await fetch(`http://127.0.0.1:${backendPort}${url.pathname}${url.search}`, {
    method: req.method,
    headers: {
      ...forwardedBackendHeaders(req),
      'content-type': 'application/json',
    },
    body: JSON.stringify(body),
  });
  const backendBody = await readBackendJson(backendResponse);
  writeGatewayJson(res, backendResponse.status, backendBody ?? { success: backendResponse.ok, data: null });
}

async function ensureBackendBearerSession(
  req: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[0],
  res: Parameters<NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']>>[1],
  backendPort: number
): Promise<boolean> {
  const authResponse = await fetch(`http://127.0.0.1:${backendPort}/api/auth/user`, {
    method: 'GET',
    headers: forwardedBackendHeaders(req),
  });
  if (authResponse.ok) {
    return true;
  }
  const authBody = await readBackendJson(authResponse);
  writeGatewayJson(
    res,
    authResponse.status,
    authBody ?? { success: false, code: 'UNAUTHORIZED', error: 'Authorization header is required' }
  );
  return false;
}

function requiredBodyString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== 'string' || !value.trim()) {
    throw new Error(`${key} is required`);
  }
  return value.trim();
}

function queryFlag(value: string | null): boolean {
  return value === 'true' || value === '1';
}

function createDesktopLocalApiHandler(
  backendPort: number
): NonNullable<Parameters<typeof startStaticServer>[0]['localApiHandler']> {
  return async (req, res) => {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');
    const route = classifyDesktopGatewayRoute(req.method ?? 'GET', url.pathname);
    if (route.action === 'proxy-rust') {
      return false;
    }
    if (url.pathname === '/api/mcp/test-connection') {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) return true;
        const body = await readGatewayJsonBody(req);
        const transport = body.transport;
        const transportType =
          transport && typeof transport === 'object' && !Array.isArray(transport)
            ? (transport as Record<string, unknown>).type
            : undefined;
        if (transportType !== 'stdio') {
          await proxyGatewayJsonBodyToBackend(req, res, backendPort, url, body);
          return true;
        }
        const serverId = requiredBodyString(body, 'runtime_scope_id');
        try {
          const result = await discoverLocalStdioMcpTools(transport);
          const reportResponse = await fetch(
            `http://127.0.0.1:${backendPort}/api/mcp/servers/${encodeURIComponent(serverId)}/local-discovery`,
            {
              method: 'POST',
              headers: { ...forwardedBackendHeaders(req), 'content-type': 'application/json' },
              body: JSON.stringify({ success: true, tools: result.tools }),
            }
          );
          const reportBody = await readBackendJson(reportResponse);
          if (!reportResponse.ok) {
            writeGatewayJson(res, reportResponse.status, reportBody ?? { success: false, error: 'MCP report failed' });
            return true;
          }
          writeGatewayJson(res, 200, { success: true, data: { success: true, tools: result.tools } });
        } catch (error) {
          const localError =
            error instanceof LocalMcpError
              ? error
              : new LocalMcpError('MCP_CONNECTION_FAILED', error instanceof Error ? error.message : String(error));
          await fetch(
            `http://127.0.0.1:${backendPort}/api/mcp/servers/${encodeURIComponent(serverId)}/local-discovery`,
            {
              method: 'POST',
              headers: { ...forwardedBackendHeaders(req), 'content-type': 'application/json' },
              body: JSON.stringify({ success: false, error: localError.message }),
            }
          ).catch((_error: unknown): undefined => undefined);
          writeGatewayJson(res, 200, {
            success: true,
            data: {
              success: false,
              code: localError.code,
              error: localError.message,
              details: localError.details,
            },
          });
        }
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        writeGatewayJson(res, 400, { success: false, code: 'INVALID_INPUT', error: message });
      }
      return true;
    }
    if (
      url.pathname === '/api/agents/custom' ||
      url.pathname === '/api/agents/custom/try-connect' ||
      url.pathname.startsWith('/api/agents/custom/')
    ) {
      try {
        if (url.pathname === '/api/agents/custom/try-connect') {
          if (req.method !== 'POST') {
            writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
            return true;
          }
          if (!(await ensureBackendBearerSession(req, res, backendPort))) {
            return true;
          }
          const body = await readGatewayJsonBody(req);
          const command = requiredBodyString(body, 'command');
          if (!(await commandExists(command))) {
            writeGatewayJson(res, 200, {
              success: true,
              data: { step: 'fail_cli', error: `command not found: ${command}` },
            });
            return true;
          }
          writeGatewayJson(res, 200, {
            success: true,
            data: {
              step: 'fail_acp',
              error: 'ACP handshake requires the desktop local runtime, which is not attached to the desktop gateway',
            },
          });
          return true;
        }
        await proxyGatewayRequestToBackend(req, res, backendPort, url);
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 502;
        writeGatewayJson(res, status, {
          success: false,
          code: status === 400 ? 'INVALID_INPUT' : 'CUSTOM_AGENTS_AGGREGATE_FAILED',
          error: message,
        });
      }
      return true;
    }
    if (url.pathname === '/api/fs/upload') {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const body = await readGatewayRawBody(req, MAX_MULTIPART_UPLOAD_BYTES, 'LOCAL_UPLOAD_TOO_LARGE');
        const upload = parseMultipartUpload(req.headers['content-type'], body);
        const targetPath = await writeLocalUploadFile(path.join(app.getPath('userData'), 'uploads'), upload);
        writeGatewayJson(res, 200, { success: true, data: targetPath });
      } catch (error) {
        if (error instanceof MultipartUploadError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message === 'LOCAL_UPLOAD_TOO_LARGE' ? 413 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 413 ? 'FILE_TOO_LARGE' : 'LOCAL_UPLOAD_FAILED',
            error: status === 413 ? 'file too large' : message,
          });
        }
      }
      return true;
    }
    if (url.pathname === '/api/fs/browse') {
      if (req.method !== 'GET') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const data = await browseLocalDirectory(
          url.searchParams.get('path') ?? '',
          queryFlag(url.searchParams.get('showFiles')),
          {
            homePath: app.getPath('home'),
          }
        );
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        writeGatewayJson(res, 400, { success: false, code: 'LOCAL_BROWSE_FAILED', error: message });
      }
      return true;
    }
    if (url.pathname.startsWith('/api/fs/snapshot/')) {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const body = await readGatewayJsonBody(req);
        const data = await handleFileSnapshotRoute(url.pathname, body, {
          storageDir: path.join(app.getPath('userData'), 'file-snapshots'),
        });
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        if (error instanceof FileSnapshotRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'FILE_SNAPSHOT_FAILED',
            error: message,
          });
        }
      }
      return true;
    }
    if (url.pathname.startsWith('/api/fs/')) {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const body = await readGatewayJsonBody(req);
        if (shouldProxyFsFacadeToRust(url.pathname, body)) {
          await proxyGatewayJsonBodyToBackend(req, res, backendPort, url, body);
          return true;
        }
        const data = await handleLocalFsRoute(url.pathname, body, { tempDir: app.getPath('temp') });
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        if (error instanceof LocalFsRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'LOCAL_FS_FAILED',
            error: message,
          });
        }
      }
      return true;
    }
    if (url.pathname.startsWith('/api/preview-history/')) {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const body = await readGatewayJsonBody(req);
        const data = await handlePreviewHistoryRoute(url.pathname, body, {
          storageDir: path.join(app.getPath('userData'), 'preview-history'),
        });
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        if (error instanceof PreviewHistoryRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'PREVIEW_HISTORY_FAILED',
            error: message,
          });
        }
      }
      return true;
    }
    if (
      url.pathname === '/api/document/convert' ||
      url.pathname.startsWith('/api/ppt-preview/') ||
      url.pathname.startsWith('/api/word-preview/') ||
      url.pathname.startsWith('/api/excel-preview/')
    ) {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        const body = await readGatewayJsonBody(req);
        const data = await handleOfficeLocalRoute(url.pathname, body, {
          platform: process.platform,
          allowedRoots: [app.getPath('temp'), app.getPath('userData')],
        });
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        if (error instanceof OfficeLocalRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'OFFICE_LOCAL_FAILED',
            error: message,
          });
        }
      }
      return true;
    }
    if (url.pathname.startsWith('/api/ppt-proxy/') || url.pathname.startsWith('/api/office-watch-proxy/')) {
      if (req.method !== 'GET' && req.method !== 'HEAD') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        await proxyOfficeWatchRequest(req, res, url);
      } catch (error) {
        if (error instanceof OfficeLocalRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          writeGatewayJson(res, 502, { success: false, code: 'OFFICE_PROXY_UNREACHABLE', error: message });
        }
      }
      return true;
    }
    if (url.pathname === '/api/channel/plugins') {
      if (req.method !== 'GET') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const context = desktopExtensionContext();
        await syncLocalExtensionsToBackend(backendPort, req, context);
        const backendResponse = await fetch(`http://127.0.0.1:${backendPort}${url.pathname}${url.search}`, {
          method: 'GET',
          headers: forwardedBackendHeaders(req),
        });
        const backendBodyValue = await readBackendJson(backendResponse);
        if (!backendResponse.ok) {
          writeGatewayJson(
            res,
            backendResponse.status,
            backendBodyValue ?? {
              success: false,
              code: 'CHANNEL_PLUGINS_BACKEND_FAILED',
              error: 'backend channel plugins request failed',
            }
          );
          return true;
        }
        const allowedResponse = await fetch(`http://127.0.0.1:${backendPort}/api/extensions/channel-plugins`, {
          method: 'GET',
          headers: forwardedBackendHeaders(req),
        });
        const allowedBodyValue = await readBackendJson(allowedResponse);
        if (!allowedResponse.ok) {
          writeGatewayJson(
            res,
            allowedResponse.status,
            allowedBodyValue ?? {
              success: false,
              code: 'CHANNEL_PLUGIN_GOVERNANCE_FAILED',
              error: 'backend extension channel-plugin governance request failed',
            }
          );
          return true;
        }
        const localPlugins = await listExtensionChannelPlugins(context);
        writeGatewayJson(
          res,
          backendResponse.status,
          replaceBackendData(backendBodyValue, mergeChannelPlugins(backendBodyValue, localPlugins, allowedBodyValue))
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        writeGatewayJson(res, 502, { success: false, code: 'CHANNEL_PLUGINS_AGGREGATE_FAILED', error: message });
      }
      return true;
    }
    if (url.pathname === '/api/channel/plugins/test') {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const body = await readGatewayJsonBody(req);
        const data = await handleChannelLocalRoute(url.pathname, body);
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        if (error instanceof ChannelLocalRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'CHANNEL_LOCAL_FAILED',
            error: message,
          });
        }
      }
      return true;
    }
    if (url.pathname === '/api/extensions' || url.pathname.startsWith('/api/extensions/')) {
      if (url.pathname.startsWith('/api/extensions/static/')) {
        if (req.method !== 'GET') {
          writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
          return true;
        }
        try {
          if (!(await ensureBackendBearerSession(req, res, backendPort))) {
            return true;
          }
          const [, assetPart = ''] = url.pathname.split('/api/extensions/static/');
          const [rawExtensionName, ...assetParts] = assetPart.split('/');
          const extensionName = decodeURIComponent(rawExtensionName ?? '');
          const assetPath = assetParts.map((part) => decodeURIComponent(part)).join('/');
          const context = desktopExtensionContext();
          const governanceResponse = await fetch(`http://127.0.0.1:${backendPort}/api/extensions`, {
            method: 'GET',
            headers: forwardedBackendHeaders(req),
          });
          const governanceBody = await readBackendJson(governanceResponse);
          if (!governanceResponse.ok) {
            writeGatewayJson(
              res,
              governanceResponse.status,
              governanceBody ?? {
                success: false,
                code: 'EXTENSION_STATIC_GOVERNANCE_FAILED',
                error: 'backend extension governance request failed',
              }
            );
            return true;
          }
          if (!isExtensionStaticAssetAllowed(governanceBody, extensionName)) {
            writeGatewayJson(res, 404, {
              success: false,
              code: 'EXTENSION_STATIC_ASSET_NOT_ALLOWED',
              error: 'extension static asset is not allowed by governance',
            });
            return true;
          }
          const asset = await readExtensionStaticAsset(extensionName, assetPath, context);
          res.writeHead(200, { 'content-type': asset.contentType, 'cache-control': 'no-store' });
          res.end(asset.data);
        } catch (error) {
          if (error instanceof ExtensionLocalRouteError) {
            writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
          } else {
            const message = error instanceof Error ? error.message : String(error);
            writeGatewayJson(res, 500, { success: false, code: 'EXTENSION_STATIC_ASSET_FAILED', error: message });
          }
        }
        return true;
      }
      if (url.pathname === '/api/extensions/sync') {
        if (req.method !== 'POST') {
          writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
          return true;
        }
        try {
          if (!(await ensureBackendBearerSession(req, res, backendPort))) {
            return true;
          }
          await readGatewayRawBody(req, 1024 * 1024, 'LOCAL_API_BODY_TOO_LARGE');
          const result = await syncLocalExtensionsToBackend(backendPort, req, desktopExtensionContext());
          writeGatewayJson(res, 200, result ?? { success: true, data: { synced: 0, contributions: 0 } });
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.startsWith('LOCAL_API_BODY_') ? 400 : 502;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'EXTENSIONS_SYNC_FAILED',
            error: message,
          });
        }
        return true;
      }
      const localExtensionPostRoutes = new Set([
        '/api/extensions/i18n',
        '/api/extensions/enable',
        '/api/extensions/disable',
        '/api/extensions/permissions',
        '/api/extensions/risk-level',
      ]);
      const isLocalExtensionRoute =
        req.method === 'GET' || (req.method === 'POST' && localExtensionPostRoutes.has(url.pathname));
      if (isLocalExtensionRoute) {
        try {
          if (!(await ensureBackendBearerSession(req, res, backendPort))) {
            return true;
          }
          const body = req.method === 'POST' ? await readGatewayJsonBody(req) : {};
          const context = desktopExtensionContext();
          const isExtensionStateMutation =
            req.method === 'POST' &&
            (url.pathname === '/api/extensions/enable' || url.pathname === '/api/extensions/disable');
          let localData: unknown;
          let localFallbackOnly = false;
          try {
            localData = isExtensionStateMutation
              ? await previewExtensionEnabledState(body, context, url.pathname === '/api/extensions/enable')
              : await handleExtensionLocalRoute(url.pathname, body, context);
          } catch (error) {
            if (!isExtensionLocalFallbackError(error)) throw error;
            localFallbackOnly = true;
          }
          await syncLocalExtensionsToBackend(backendPort, req, context);

          const backendResponse = await fetch(`http://127.0.0.1:${backendPort}${url.pathname}${url.search}`, {
            method: req.method,
            headers: {
              ...forwardedBackendHeaders(req),
              ...(req.method === 'POST' ? { 'content-type': 'application/json' } : {}),
            },
            body: req.method === 'POST' ? JSON.stringify(body) : undefined,
          });
          const backendBodyValue = await readBackendJson(backendResponse);
          if (!backendResponse.ok) {
            if (
              !isExtensionStateMutation &&
              localData !== undefined &&
              isExtensionBackendFallback(backendResponse, backendBodyValue)
            ) {
              writeGatewayJson(res, 200, { success: true, data: localData });
              return true;
            }
            writeGatewayJson(
              res,
              backendResponse.status,
              backendBodyValue ?? {
                success: false,
                code: 'EXTENSIONS_BACKEND_FAILED',
                error: 'backend extensions request failed',
              }
            );
            return true;
          }
          if (localFallbackOnly) {
            writeGatewayJson(res, backendResponse.status, backendBodyValue ?? { success: true, data: null });
            return true;
          }
          if (isExtensionStateMutation) {
            // Governance and audit are Rust-owned. Commit local device state only
            // after Rust accepts the toggle, then sync the resulting contribution state.
            localData = await handleExtensionLocalRoute(url.pathname, body, context);
            await syncLocalExtensionsToBackend(backendPort, req, context);
          }
          writeGatewayJson(
            res,
            backendResponse.status,
            replaceBackendData(backendBodyValue, mergeExtensionData(url.pathname, backendBodyValue, localData))
          );
        } catch (error) {
          if (error instanceof ExtensionLocalRouteError) {
            writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
          } else {
            const message = error instanceof Error ? error.message : String(error);
            writeGatewayJson(res, 502, { success: false, code: 'EXTENSIONS_AGGREGATE_FAILED', error: message });
          }
        }
        return true;
      }
    }
    if (url.pathname === '/api/hub/extensions') {
      if (req.method !== 'GET') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        const backendResponse = await fetch(`http://127.0.0.1:${backendPort}${url.pathname}${url.search}`, {
          method: 'GET',
          headers: forwardedBackendHeaders(req),
        });
        const backendBody = await readBackendJson(backendResponse);
        if (!backendResponse.ok) {
          writeGatewayJson(
            res,
            backendResponse.status,
            backendBody ?? {
              success: false,
              code: 'HUB_EXTENSIONS_BACKEND_FAILED',
              error: 'backend hub extensions request failed',
            }
          );
          return true;
        }
        const extensions = extractHubExtensions(backendBody);
        const data = await applyHubLocalStateToExtensions(extensions, {
          statePath: path.join(app.getPath('userData'), 'hub-local-state.json'),
        });
        writeGatewayJson(res, backendResponse.status, replaceBackendData(backendBody, data));
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        writeGatewayJson(res, 502, { success: false, code: 'HUB_EXTENSIONS_AGGREGATE_FAILED', error: message });
      }
      return true;
    }
    if (url.pathname.startsWith('/api/hub/') && url.pathname !== '/api/hub/extensions') {
      if (req.method !== 'POST') {
        writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
        return true;
      }
      try {
        if (!(await ensureBackendBearerSession(req, res, backendPort))) {
          return true;
        }
        const body = await readGatewayJsonBody(req);
        let extension: ReturnType<typeof extractHubExtensions>[number] | undefined;
        if (
          url.pathname === '/api/hub/install' ||
          url.pathname === '/api/hub/retry-install' ||
          url.pathname === '/api/hub/update'
        ) {
          const extensionName = typeof body.name === 'string' ? body.name.trim() : '';
          const hubResponse = await fetch(`http://127.0.0.1:${backendPort}/api/hub/extensions`, {
            method: 'GET',
            headers: forwardedBackendHeaders(req),
          });
          const hubBody = await readBackendJson(hubResponse);
          if (!hubResponse.ok) {
            throw new Error(`HUB_EXTENSIONS_BACKEND_FAILED ${hubResponse.status}`);
          }
          extension = extractHubExtensions(hubBody).find((item) => item.name === extensionName);
          if (!extension) {
            throw new HubLocalRouteError(404, 'HUB_EXTENSION_NOT_FOUND', 'hub extension not found');
          }
        }
        const data = await handleHubLocalRoute(url.pathname, body, {
          statePath: path.join(app.getPath('userData'), 'hub-local-state.json'),
          installRoot: path.join(app.getPath('userData'), 'hub-extensions'),
          extension,
          emitStateChange: (change) => {
            ipcBridge.hub.onStateChanged.emit(change);
          },
        });
        if (url.pathname !== '/api/hub/check-updates') {
          const syncResult = await syncLocalExtensionsToBackend(backendPort, req, desktopExtensionContext());
          writeGatewayJson(res, 200, { success: true, data: withHubGovernanceSyncResult(data, syncResult) });
          return true;
        }
        writeGatewayJson(res, 200, { success: true, data });
      } catch (error) {
        if (error instanceof HubLocalRouteError) {
          writeGatewayJson(res, error.statusCode, { success: false, code: error.code, error: error.message });
        } else {
          const message = error instanceof Error ? error.message : String(error);
          const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
          writeGatewayJson(res, status, {
            success: false,
            code: status === 400 ? 'INVALID_INPUT' : 'HUB_LOCAL_FAILED',
            error: message,
          });
        }
      }
      return true;
    }
    if (!url.pathname.startsWith('/api/shell/')) {
      return false;
    }
    if (req.method !== 'POST') {
      writeGatewayJson(res, 405, { success: false, code: 'METHOD_NOT_ALLOWED', error: 'method not allowed' });
      return true;
    }

    try {
      const body = await readGatewayJsonBody(req);
      switch (url.pathname) {
        case '/api/shell/open-file': {
          const error = await shell.openPath(requiredBodyString(body, 'file_path'));
          if (error) throw new Error(error);
          writeGatewayJson(res, 200, { success: true, data: null });
          return true;
        }
        case '/api/shell/show-item-in-folder':
          shell.showItemInFolder(requiredBodyString(body, 'file_path'));
          writeGatewayJson(res, 200, { success: true, data: null });
          return true;
        case '/api/shell/open-external':
          await shell.openExternal(validateExternalUrl(requiredBodyString(body, 'url')));
          writeGatewayJson(res, 200, { success: true, data: null });
          return true;
        case '/api/shell/check-tool-installed': {
          const installed = await checkToolInstalled(requiredBodyString(body, 'tool'));
          writeGatewayJson(res, 200, { success: true, data: installed });
          return true;
        }
        case '/api/shell/open-folder-with':
          await openFolderWithTool(requiredBodyString(body, 'folder_path'), requiredBodyString(body, 'tool'));
          writeGatewayJson(res, 200, { success: true, data: null });
          return true;
        default:
          writeGatewayJson(res, 404, {
            success: false,
            code: 'LOCAL_ROUTE_NOT_FOUND',
            error: 'desktop local shell route not found',
          });
          return true;
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const status = message.includes('required') || message.startsWith('LOCAL_API_BODY_') ? 400 : 500;
      writeGatewayJson(res, status, {
        success: false,
        code: status === 400 ? 'INVALID_INPUT' : 'LOCAL_RUNTIME_ERROR',
        error: message,
      });
      return true;
    }
  };
}

function exposeBackendPort(backendPort: number): void {
  // Expose the backend port to main-process callers of httpBridge (e.g. the
  // one-shot assistant migration hook below). Must land BEFORE any
  // ipcBridge.* invoke from the main process — the renderer side reads
  // window.__backendPort via preload, but main has no `window`.
  (globalThis as typeof globalThis & { __backendPort?: number }).__backendPort = backendPort;
}

async function markBackendReady(backendPort: number, source: string): Promise<void> {
  if (backendStartedOk) return;
  let rendererApiPort = backendPort;
  try {
    if (
      !isWebUIMode &&
      !isResetPasswordMode &&
      externalEnterpriseBackendConfig?.backendMode !== 'external-rust-direct'
    ) {
      rendererApiPort = await desktopGatewayController.ensureStarted(backendPort);
    }
  } catch (error) {
    console.error('[BiWork] Failed to start desktop API gateway; falling back to direct backend port:', error);
  }
  console.log(`[BiWork] ${source} ready (rendererApiPort=${rendererApiPort}, backendPort=${backendPort})`);
  exposeBackendPort(rendererApiPort);
  registerCronResumeBridge(rendererApiPort);
  backendStartedOk = true;
  backendStartupFailed = false;
  backendStartupFailureInfo = null;
  (globalThis as typeof globalThis & { __backendStartupFailed?: boolean }).__backendStartupFailed = false;
}

function resolveDebugBackendStartupFailure(): BackendStartupFailureInfo | null {
  const reason = process.env.BIWORK_DEBUG_BACKEND_STARTUP_FAILURE as BackendStartupFailureInfo['reason'] | undefined;
  if (!reason) {
    return null;
  }
  if ((app.isPackaged && !isE2ETestMode) || isWebUIMode || isResetPasswordMode) {
    console.warn('[BiWork] Ignoring BIWORK_DEBUG_BACKEND_STARTUP_FAILURE outside desktop dev/e2e mode.');
    return null;
  }

  if (reason === 'backend_incompatible_runtime') {
    return { reason, runtime: 'glibc', requiredVersions: ['2.28'] };
  }
  if (reason === 'backend_package_architecture_mismatch') {
    return {
      reason,
      deviceArch: process.arch === 'arm64' ? 'arm64' : 'x64',
      expectedDownloadArch: process.arch === 'arm64' ? 'arm64' : 'x64',
      packageArch: process.arch === 'arm64' ? 'x64' : 'arm64',
    };
  }
  if (reason === 'backend_startup_failed') {
    return {
      reason,
      backendBoundaryCode: 'E2E_DEBUG_BACKEND_STARTUP_FAILURE',
      backendBoundaryStage: 'debug_injection',
    };
  }
  if (reason === 'backend_incomplete_installation') {
    return {
      reason,
      incompleteInstallationKind: 'missing_directory_resources',
      missingRuntimeDir: true,
      missingResources: ['managed node runtime', 'ACP adapters'],
    };
  }

  console.warn(`[BiWork] Ignoring unknown BIWORK_DEBUG_BACKEND_STARTUP_FAILURE value: ${reason}`);
  return null;
}

function applyDebugBackendStartupFailure(failure: BackendStartupFailureInfo): void {
  backendStartupFailed = true;
  backendStartupFailureInfo = failure;
  (globalThis as typeof globalThis & { __backendStartupFailed?: boolean }).__backendStartupFailed = true;
}

const createWindow = ({ showOnReady = true }: { showOnReady?: boolean } = {}): void => {
  console.log('[BiWork] Creating main window...');
  const { x: windowX, y: windowY, width: windowWidth, height: windowHeight } = resolveInitialBounds();

  // Get app icon for development mode (Windows/Linux need icon in BrowserWindow)
  // In production, icons are set via forge.config.ts packagerConfig
  let devIcon: Electron.NativeImage | undefined;
  if (!app.isPackaged) {
    try {
      // Windows: app.ico (no dev version), Linux: app_dev.png (with padding)
      const iconFile = process.platform === 'win32' ? 'app.ico' : 'app_dev.png';
      const iconPath = path.join(process.cwd(), 'resources', iconFile);
      if (fs.existsSync(iconPath)) {
        devIcon = nativeImage.createFromPath(iconPath);
        if (devIcon.isEmpty()) devIcon = undefined;
      }
    } catch {
      // Ignore icon loading errors in development
    }
  }

  // Create the browser window.
  mainWindow = new BrowserWindow({
    width: windowWidth,
    height: windowHeight,
    ...(windowX !== undefined && windowY !== undefined ? { x: windowX, y: windowY } : {}),
    minWidth: MIN_WINDOW_WIDTH,
    minHeight: MIN_WINDOW_HEIGHT,
    show: false, // Hide until CSS is loaded to prevent FOUC
    backgroundColor: '#ffffff',
    autoHideMenuBar: true,
    // Set icon for Windows/Linux in development mode
    ...(devIcon && process.platform !== 'darwin' ? { icon: devIcon } : {}),
    // Custom titlebar configuration / 自定义标题栏配置
    ...(process.platform === 'darwin'
      ? {
          titleBarStyle: 'hidden',
          // Align traffic-light vertical center with the titlebar button centers.
          // Titlebar is 45px; buttons are 36px flex-centered → button center y≈22.5.
          // Empirically y=13 places the traffic lights on the same horizontal line
          // as the sidebar / back / forward icons.
          // NOTE: requires a full app restart to take effect (BrowserWindow option).
          trafficLightPosition: { x: 10, y: 13 },
        }
      : { frame: false }),
    webPreferences: {
      preload: path.join(__dirname, '../preload/index.js'),
      webviewTag: true, // 启用 webview 标签用于 HTML 预览 / Enable webview tag for HTML preview
    },
  });
  console.log(`[BiWork] Main window created (id=${mainWindow.id})`);

  scheduleStartupLogReport(mainWindow);

  // Show window after content is ready to prevent FOUC (Flash of Unstyled Content)
  // Use 'ready-to-show' which fires when renderer has painted first frame,
  // combined with 'did-finish-load' as belt-and-suspenders approach.
  if (showOnReady) {
    const showWindow = () => {
      if (!mainWindow.isDestroyed() && !mainWindow.isVisible()) {
        console.log('[BiWork] Showing main window');
        mainWindow.show();
        mainWindow.focus();
      }
    };
    mainWindow.once('ready-to-show', () => {
      console.log('[BiWork] Window ready-to-show');
      showWindow();
    });
    // Belt-and-suspenders: also show on did-finish-load in case ready-to-show already fired
    mainWindow.webContents.once('did-finish-load', () => {
      console.log('[BiWork] Renderer did-finish-load');
      showWindow();
      scheduleBackendMigrations();
    });
    // Fallback: show window after 5s even if events don't fire (e.g. loadURL failure)
    setTimeout(showWindow, 5000);
  } else if (process.platform === 'darwin' && app.dock) {
    void app.dock.hide();
  }

  initMainAdapterWithWindow(mainWindow);
  bindMainWindowReferences(mainWindow);

  setupApplicationMenu();

  setupZoomForWindow(mainWindow);
  registerWindowMaximizeListeners(mainWindow);
  attachWindowBoundsPersistence(mainWindow, (bounds) => ProcessConfig.set('window.bounds', bounds));

  // Initialize auto-updater service (skip when disabled via env, e.g. E2E / CI)
  // 初始化自动更新服务（通过环境变量禁用时跳过，例如 E2E / CI 场景）
  const isCiRuntime = process.env.CI === 'true' || process.env.CI === '1' || process.env.GITHUB_ACTIONS === 'true';
  const disableAutoUpdater =
    process.env.BIWORK_DISABLE_AUTO_UPDATE === '1' || process.env.BIWORK_E2E_TEST === '1' || isCiRuntime;
  if (!disableAutoUpdater) {
    Promise.all([import('./process/services/autoUpdaterService'), import('./process/bridge/updateBridge')])
      .then(([{ autoUpdaterService }, { createAutoUpdateStatusBroadcast }]) => {
        // Create status broadcast callback that emits via ipcBridge (pure emitter, no window binding)
        const statusBroadcast = createAutoUpdateStatusBroadcast();
        autoUpdaterService.initialize(statusBroadcast);
        autoUpdaterService.setBeforeQuitAndInstall(async () => desktopGatewayController.stop());
        // Check for updates after 3 seconds delay
        // 3秒后检查更新
        setTimeout(() => {
          void autoUpdaterService.checkForUpdatesAndNotify();
        }, 3000);
      })
      .catch((error) => {
        console.error('[App] Failed to initialize autoUpdaterService:', error);
      });
  } else {
    console.log('[BiWork] Auto-updater disabled via env/CI guard');
  }

  // Load the renderer: dev server URL in development, built HTML file in production
  const rendererUrl = process.env['ELECTRON_RENDERER_URL'];
  const fallbackFile = path.join(__dirname, '../renderer/index.html');

  const isRendererLoaded = (): boolean => {
    const currentUrl = mainWindow.webContents.getURL();
    if (!currentUrl) return false;
    if (!app.isPackaged && rendererUrl) {
      return currentUrl.startsWith(rendererUrl);
    }
    return currentUrl.startsWith('file://');
  };

  const loadRendererForDeepLink = async (): Promise<void> => {
    if (!app.isPackaged && rendererUrl) {
      try {
        await mainWindow.loadURL(rendererUrl);
        return;
      } catch (error) {
        console.error('[BiWork] Deep-link renderer reload failed, falling back to file:', error);
      }
    }
    await mainWindow.loadFile(fallbackFile);
  };

  dispatchDeepLinkUrl = (deepLinkUrl: string): void => {
    if (mainWindow.isDestroyed()) {
      handleDeepLinkUrl(deepLinkUrl);
      return;
    }

    const deliverDeepLink = () => {
      if (mainWindow.isDestroyed()) return;
      handleDeepLinkUrl(deepLinkUrl);
      showAndFocusMainWindow(mainWindow);
    };

    if (isRendererLoaded()) {
      deliverDeepLink();
      return;
    }

    mainWindow.webContents.once('did-finish-load', deliverDeepLink);
    void loadRendererForDeepLink().catch((error) => {
      mainWindow.webContents.removeListener('did-finish-load', deliverDeepLink);
      console.error('[BiWork] Failed to load renderer for deep link:', error);
      handleDeepLinkUrl(deepLinkUrl);
    });
  };

  const isDeepLinkNavigation = (url: string): boolean => url.startsWith(`${PROTOCOL_SCHEME}://`);

  mainWindow.webContents.on('will-frame-navigate', (event) => {
    if (!event.isMainFrame || !isDeepLinkNavigation(event.url)) return;
    event.preventDefault();
    dispatchDeepLinkUrl(event.url);
  });

  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    if (isDeepLinkNavigation(url)) {
      dispatchDeepLinkUrl(url);
      return { action: 'deny' };
    }
    return { action: 'allow' };
  });

  if (!app.isPackaged && rendererUrl) {
    console.log(`[BiWork] Loading renderer URL: ${rendererUrl}`);
    mainWindow.loadURL(rendererUrl).catch((error) => {
      console.error('[BiWork] loadURL failed, falling back to file:', error.message || error);
      mainWindow.loadFile(fallbackFile).catch((e2) => {
        console.error('[BiWork] loadFile fallback also failed:', e2.message || e2);
      });
    });
  } else {
    console.log(`[BiWork] Loading renderer file: ${fallbackFile}`);
    mainWindow.loadFile(fallbackFile).catch((error) => {
      console.error('[BiWork] loadFile failed:', error.message || error);
    });
  }

  mainWindow.webContents.on('did-fail-load', (_event, errorCode, errorDescription, validatedURL, isMainFrame) => {
    console.error('[BiWork] did-fail-load:', { errorCode, errorDescription, validatedURL, isMainFrame });
  });

  mainWindow.webContents.on('render-process-gone', (_event, details) => {
    console.error('[BiWork] render-process-gone:', details);

    // Reload the renderer to recover from the crash.
    // The isDestroyed() guard in adapter/main.ts prevents further sends
    // to the dead webContents while the reload is in progress.
    if (!mainWindow.isDestroyed()) {
      console.log('[BiWork] Attempting to recover from renderer crash by reloading...');

      if (!app.isPackaged && rendererUrl) {
        mainWindow.loadURL(rendererUrl).catch((error) => {
          console.error('[BiWork] Recovery loadURL failed:', error.message || error);
        });
      } else {
        mainWindow.loadFile(fallbackFile).catch((error) => {
          console.error('[BiWork] Recovery loadFile failed:', error.message || error);
        });
      }
    }
  });

  mainWindow.webContents.on('unresponsive', () => {
    console.warn('[BiWork] Renderer became unresponsive');
  });

  mainWindow.on('closed', () => {
    console.log('[BiWork] Main window closed');
  });

  // DevTools is no longer auto-opened at startup.
  // Use the DevTools toggle in Settings > System (dev mode only) to open it.

  // Listen to DevTools state changes and notify Renderer
  mainWindow.webContents.on('devtools-opened', () => {
    ipcBridge.application.devToolsStateChanged.emit({ isOpen: true });
  });

  mainWindow.webContents.on('devtools-closed', () => {
    ipcBridge.application.devToolsStateChanged.emit({ isOpen: false });
  });

  // 关闭拦截：当启用"关闭到托盘"时，隐藏窗口而非关闭
  // Close interception: hide window instead of closing when "close to tray" is enabled
  mainWindow.on('close', (event) => {
    if (mainWindow.isDestroyed()) return;
    if (getCloseToTrayEnabled() && !getIsQuitting()) {
      event.preventDefault();
      mainWindow.hide();
    }
  });
};

const handleAppReady = async (): Promise<void> => {
  const t0 = performance.now();
  const mark = (label: string) => console.log(`[BiWork:ready] ${label} +${Math.round(performance.now() - t0)}ms`);
  mark('start');

  const shouldInstallReactDevTools =
    !app.isPackaged &&
    !isE2ETestMode &&
    process.env.BIWORK_DISABLE_DEVTOOLS !== '1' &&
    process.env.BIWORK_INSTALL_REACT_DEVTOOLS === '1';
  if (shouldInstallReactDevTools) {
    try {
      const { default: installExtension, REACT_DEVELOPER_TOOLS } = await import('electron-devtools-installer');
      await installExtension(REACT_DEVELOPER_TOOLS);
      console.log('[DevTools] React Developer Tools installed');
    } catch (e) {
      console.warn('[DevTools] Failed to install React DevTools:', e);
    }
  }

  // CLI mode: print app version and exit immediately (used by CI smoke tests)
  if (isVersionMode) {
    console.log(app.getVersion());
    app.exit(0);
    return;
  }

  // Set dock icon in development mode on macOS
  // In production, the icon is set via forge.config.ts packagerConfig.icon
  if (process.platform === 'darwin' && !app.isPackaged && app.dock) {
    try {
      const iconPath = path.join(process.cwd(), 'resources', 'app_dev.png');
      if (fs.existsSync(iconPath)) {
        const icon = nativeImage.createFromPath(iconPath);
        if (!icon.isEmpty()) {
          app.dock.setIcon(icon);
        }
      }
    } catch {
      // Ignore dock icon errors in development
    }
  }

  setSentryDeviceId();

  try {
    await initializeProcess();
    rendererInitialLanguage = ProcessConfig.getSync('language') ?? null;
    mark('initializeProcess');
  } catch (error) {
    console.error('Failed to initialize process:', error);
    app.exit(1);
    return;
  }

  const debugBackendStartupFailure = resolveDebugBackendStartupFailure();
  if (debugBackendStartupFailure) {
    applyDebugBackendStartupFailure(debugBackendStartupFailure);
    mark(`debugBackendStartupFailure:${debugBackendStartupFailure.reason}`);
  } else if (externalEnterpriseBackendConfig) {
    try {
      await verifyExternalEnterpriseBackend(externalEnterpriseBackendConfig);
      backendReadyPromise = markBackendReady(externalEnterpriseBackendConfig.backendPort, 'enterpriseBackend.external');
      await backendReadyPromise;
      mark(`enterpriseBackend.external:${externalEnterpriseBackendConfig.backendPort}`);
    } catch (error) {
      console.error('[BiWork] Failed to connect external enterprise backend:', error);
      markBackendStartupFailed(error);
      await captureBackendStartupFailure(error);
      if (isWebUIMode || isResetPasswordMode) {
        app.exit(1);
        return;
      }
    }
  } else {
    const error = new Error('BIWORK_ENTERPRISE_BACKEND_URL is required');
    console.error('[BiWork] External backend configuration is missing');
    markBackendStartupFailed(error);
    await captureBackendStartupFailure(error);
  }

  // Backend initialization are deferred until after the renderer finishes
  // loading. Some migration steps (ConfigStorage.get, ipcBridge.listProviders)
  // route through the renderer via BroadcastChannel; running them here would
  // deadlock because the renderer does not exist yet. See scheduleBackendMigrations().

  try {
    initializeZoomFactor(await ProcessConfig.get('ui.zoomFactor'));
    mark('initializeZoomFactor');
  } catch (error) {
    console.error('[BiWork] Failed to restore zoom factor:', error);
    initializeZoomFactor(undefined);
  }

  try {
    loadSavedWindowBounds(await ProcessConfig.get('window.bounds'));
    mark('restoreWindowBounds');
  } catch (error) {
    console.error('[BiWork] Failed to restore window bounds:', error);
    loadSavedWindowBounds(undefined);
  }

  if (isResetPasswordMode) {
    // Handle password reset without creating window
    try {
      const { resetPasswordCLI, resolveResetPasswordUsername } = await import('./process/utils/resetPasswordCLI');
      const username = resolveResetPasswordUsername(process.argv);

      await resetPasswordCLI(username);

      app.quit();
    } catch {
      app.exit(1);
    }
  } else if (isWebUIMode) {
    const userConfigInfo = loadUserWebUIConfig();
    if (userConfigInfo.exists && userConfigInfo.path) {
      // Config file loaded from user directory
    }
    const resolvedPort = resolveWebUIPort(userConfigInfo.config, getSwitchValue);
    const allowRemote = resolveRemoteAccess(userConfigInfo.config, isRemoteMode);
    try {
      // Inside Electron (`BiWork --webui` or packaged `biwork-web` mode that
      // launches via the Electron shell), reuse the desktop app's data-dir so
      // that conversations / cron jobs created in any path show up everywhere.
      // Matches the desktop IPC path at line 493 above.
      const { getDataPath } = await import('./process/utils/utils');
      const { getSystemDir } = await import('./process/utils/initStorage');
      const sysDirWebUI = getSystemDir();
      // M6: Switch to @biwork/web-host
      const handle = await startWebHost({
        app: {
          version: app.getVersion(),
          isPackaged: app.isPackaged,
          resourcesPath: app.getAppPath(),
          // Same reason as dataDir below: webui.config.json must live next to
          // the DB under the CLI-safe symlink path, so every password-change
          // entry point (CLI --resetpass, settings-toggle IPC, browser login)
          // reads the same file.
          userDataPath: getDataPath(),
        },
        staticDir: path.join(__dirname, '../renderer'),
        port: resolvedPort,
        allowRemote,
        dataDir: getDataPath(),
        logDir: sysDirWebUI.logDir,
        // Expose the same BIWORK_{CACHE,WORK,LOG}_DIR env the desktop IPC path
        // passes at line 493, so /api/system/info reports the symlink workDir
        // instead of the path-with-spaces userData root.
        dirs: {
          cacheDir: sysDirWebUI.cacheDir,
          workDir: sysDirWebUI.workDir,
          logDir: sysDirWebUI.logDir,
        },
        backend: {
          kind: 'useExistingBackend',
          port: (() => {
            const port = (globalThis as typeof globalThis & { __backendPort?: number }).__backendPort;
            if (!port) {
              throw new Error('[WebUI] Cannot start: BiWork backend is unavailable');
            }
            return port;
          })(),
        },
      });
      console.log(`[WebUI] Headless server started (port=${handle.port}, backendPort=${handle.backendPort})`);
    } catch (err) {
      console.error(`[WebUI] Failed to start server on port ${resolvedPort}:`, err);
      app.exit(1);
      return;
    }

    // Keep the process alive in WebUI mode by preventing default quit behavior.
    // On Linux headless (systemd), Electron may attempt to quit when no windows exist.
    app.on('will-quit', (event) => {
      // Only prevent quit if this is an unexpected exit (server still running).
      // Explicit app.exit() calls bypass will-quit, so they are unaffected.
      if (!isExplicitQuit) {
        event.preventDefault();
        console.warn('[WebUI] Prevented unexpected quit — server is still running');
      }
    });
  } else {
    // 初始化关闭到托盘设置 / Initialize close-to-tray setting
    if (isE2ETestMode) {
      setCloseToTrayEnabled(false);
      destroyTray();
    } else {
      try {
        const savedCloseToTray = await readCloseToTraySetting();
        setCloseToTrayEnabled(savedCloseToTray);
        if (getCloseToTrayEnabled()) {
          createOrUpdateTray();
        }
      } catch {
        // Ignore storage read errors, default to false
      }
    }

    const showMainWindowOnReady = !(wasLaunchedAtLogin() && getCloseToTrayEnabled());

    createWindow({ showOnReady: showMainWindowOnReady });
    appReadyDone = true;
    mark('createWindow');

    // Initialize desktop pet (delayed to not block main window)
    setTimeout(() => {
      void (async () => {
        try {
          const petEnabled = await ProcessConfig.get('pet.enabled');
          if (petEnabled === true) {
            // Read pet sub-settings before creating the pet so flags are honored
            // on the first createPetWindow() call (which is sync).
            const confirmEnabled = (await ProcessConfig.get('pet.confirmEnabled')) ?? true;
            const { createPetWindow, setPetConfirmEnabled } = await import('./process/pet/petManager');
            setPetConfirmEnabled(confirmEnabled);
            createPetWindow();
          }
        } catch (error) {
          console.error('[Pet] Failed to initialize:', error);
        }
      })();
    }, 3000);

    // 读取语言设置并初始化主进程 i18n，然后刷新托盘菜单
    // Read language setting and initialize main process i18n, then refresh tray menu
    try {
      const savedLanguage = await ProcessConfig.get('language');
      await setInitialLanguage(savedLanguage);
      // After language is set, refresh tray menu if it exists
      await refreshTrayMenu();
    } catch (error) {
      console.error('[index] Failed to initialize i18n language:', error);
    }

    // 监听语言变更，刷新托盘菜单文案 / Listen for language changes to refresh tray menu labels
    onLanguageChanged(() => {
      void refreshTrayMenu();
    });

    if (!isE2ETestMode) {
      // 窗口创建后异步恢复 WebUI，不阻塞 UI / Restore WebUI async after window creation, non-blocking
      restoreDesktopWebUIFromPreferences().catch((error) => {
        console.error('[WebUI] Failed to auto-restore:', error);
      });
    }

    // Flush pending deep-link URL (received before window was ready)
    const pendingUrl = getPendingDeepLinkUrl();
    if (pendingUrl) {
      clearPendingDeepLinkUrl();
      mainWindow.webContents.once('did-finish-load', () => {
        handleDeepLinkUrl(pendingUrl);
      });
    }
  }

  // Verify CDP is ready and log status
  const { cdpPort, verifyCdpReady } = await import('./process/utils/configureChromium');
  if (cdpPort) {
    const cdpReady = await verifyCdpReady(cdpPort);
    if (cdpReady) {
      console.log(`[CDP] Remote debugging server ready at http://127.0.0.1:${cdpPort}`);
      console.log(
        `[CDP] MCP chrome-devtools: npx chrome-devtools-mcp@0.16.0 --browser-url=http://127.0.0.1:${cdpPort}`
      );
    } else {
      console.warn(`[CDP] Warning: Remote debugging port ${cdpPort} not responding`);
    }
  }
};

// ============ Protocol Registration ============
// Register biwork:// as the default protocol client
if (process.defaultApp) {
  // Dev mode: need to pass execPath explicitly
  app.setAsDefaultProtocolClient(PROTOCOL_SCHEME, process.execPath, [path.resolve(process.argv[1])]);
} else {
  app.setAsDefaultProtocolClient(PROTOCOL_SCHEME);
}

// macOS: handle biwork:// URLs via the open-url event
app.on('open-url', (event, url) => {
  event.preventDefault();
  dispatchDeepLinkUrl(url);
  if (isWebUIMode || isResetPasswordMode || !app.isReady()) {
    return;
  }
  // Focus existing window so user sees the result
  showOrCreateMainWindow({ mainWindow, createWindow });
});

// 监听 GPU 子进程崩溃，连续多次后下次启动自动关闭硬件加速（参见 ELECTRON-9A / ELECTRON-9D）。
installGpuCrashHandler();

// Ensure we don't miss the ready event when running in CLI/WebUI mode
void app
  .whenReady()
  .then(handleAppReady)
  .catch((error) => {
    // App initialization failed
    console.error('[BiWork] App initialization failed:', error);
    app.quit();
  });

// Quit when all windows are closed, except on macOS. There, it's common
// for applications and their menu bar to stay active until the user quits
// explicitly with Cmd + Q.
app.on('window-all-closed', () => {
  // 当关闭到托盘启用时，不退出应用 / Don't quit when close-to-tray is enabled
  if (getCloseToTrayEnabled()) {
    return;
  }
  // In WebUI mode, don't quit when windows are closed since we're running a web server
  if (!isWebUIMode && process.platform !== 'darwin') {
    app.quit();
  }
});

app.on('activate', () => {
  // On OS X it's common to re-create a window in the app when the
  // dock icon is clicked and there are no other windows open.
  // Skip if handleAppReady hasn't finished — it will create the window itself.
  if (!appReadyDone) return;
  if (!isWebUIMode && app.isReady()) {
    if (mainWindow && !mainWindow.isDestroyed()) {
      // 从托盘恢复隐藏的窗口 / Restore hidden window from tray
      showAndFocusMainWindow(mainWindow);
      if (process.platform === 'darwin' && app.dock) {
        void app.dock.show();
      }
    } else {
      createWindow();
    }
  }
});

installQuitCleanup({
  onBeforeQuit: (handler) => app.on('before-quit', (event) => handler(event)),
  onTerminationSignal: (handler) => {
    process.once('SIGTERM', handler);
    process.once('SIGINT', handler);
  },
  quitApp: () => app.quit(),
  setIsQuitting,
  markExplicitQuit: () => {
    isExplicitQuit = true;
  },
  destroyTray,
  disposeCronResumeListener: () => {
    disposeCronResumeListener?.();
    disposeCronResumeListener = null;
  },
  stopBackend: async () => {
    await desktopGatewayController
      .stop()
      .catch((error) => console.error('[BiWork] Failed to stop desktop gateway:', error));
    await desktopTelemetry
      .shutdown()
      .catch((error) => console.error('[BiWork] Failed to flush desktop telemetry:', error));
  },
  destroyPetWindow: async () => {
    const { destroyPetWindow } = await import('./process/pet/petManager');
    destroyPetWindow();
  },
  logInfo: console.log,
  logWarn: console.warn,
  logError: console.error,
});

app.on('will-quit', () => {
  console.log('[BiWork] will-quit — all cleanup should be complete');
});

app.on('quit', (_event, exitCode) => {
  console.log(`[BiWork] quit (exitCode=${exitCode})`);
});

// In this file you can include the rest of your app's specific main process
// code. You can also put them in separate files and import them here.
