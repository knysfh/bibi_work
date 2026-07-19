export type RouteOwnership = 'RUST' | 'LOCAL' | 'AGGREGATE' | 'FACADE' | 'UNKNOWN';

export type DesktopGatewayAction = 'handle-local' | 'handle-aggregate' | 'proxy-rust';

export type DesktopGatewayRoute = {
  ownership: RouteOwnership;
  action: DesktopGatewayAction;
  authority: string;
};

const LOCAL_PREFIXES = [
  '/api/shell/',
  '/api/ppt-preview/',
  '/api/ppt-proxy/',
  '/api/word-preview/',
  '/api/excel-preview/',
  '/api/office-watch-proxy/',
] as const;

const RUST_PREFIXES = [
  '/api/auth/',
  '/api/bedrock/',
  '/api/google/',
  '/api/route-ownership',
  '/api/settings',
  '/api/system/',
  '/api/stt',
  '/api/webui/',
  '/api/agents/',
  '/api/assistants',
  '/api/skills',
  '/api/mcp/',
  '/api/providers',
  '/api/remote-agents',
  '/api/workbench/',
  '/api/conversations',
  '/api/messages/',
  '/api/teams',
  '/api/cron/',
  '/api/channel/pairings',
  '/api/channel/users',
  '/api/channel/sessions',
  '/api/channel/settings',
] as const;

const FACADE_FS_LOCAL_ROUTES = new Set([
  '/api/fs/browse',
  '/api/fs/dir',
  '/api/fs/fetch-remote-image',
  '/api/fs/image-base64',
  '/api/fs/list',
  '/api/fs/metadata',
  '/api/fs/read',
  '/api/fs/read-buffer',
  '/api/fs/upload',
  '/api/fs/write',
]);

const FACADE_FS_DESKTOP_LOCAL_ROUTES = new Set(['/api/fs/copy', '/api/fs/remove', '/api/fs/rename', '/api/fs/temp']);

const RUST_BACKED_FS_FACADE_ROUTES = new Set([
  '/api/fs/dir',
  '/api/fs/image-base64',
  '/api/fs/list',
  '/api/fs/metadata',
  '/api/fs/read',
  '/api/fs/read-buffer',
  '/api/fs/write',
]);

const RUST_FS_VIRTUAL_PREFIXES = ['/artifacts', '/local/main', '/scratch', '/workspace'] as const;

const LOCAL_FS_ROUTES = new Set([
  '/api/fs/snapshot/baseline',
  '/api/fs/snapshot/branches',
  '/api/fs/snapshot/compare',
  '/api/fs/snapshot/discard',
  '/api/fs/snapshot/dispose',
  '/api/fs/snapshot/info',
  '/api/fs/snapshot/init',
  '/api/fs/snapshot/reset',
  '/api/fs/snapshot/stage',
  '/api/fs/snapshot/stage-all',
  '/api/fs/snapshot/unstage',
  '/api/fs/snapshot/unstage-all',
]);

const LOCAL_FS_WATCH_ROUTES = new Set([
  '/api/fs/office-watch/start',
  '/api/fs/office-watch/stop',
  '/api/fs/watch/start',
  '/api/fs/watch/stop',
  '/api/fs/watch/stop-all',
]);

const LOCAL_FS_ZIP_ROUTES = new Set(['/api/fs/zip', '/api/fs/zip/cancel']);

const LOCAL_PREVIEW_HISTORY_ROUTES = new Set([
  '/api/preview-history/get-content',
  '/api/preview-history/list',
  '/api/preview-history/save',
]);

const LOCAL_HUB_ROUTES = new Set([
  '/api/hub/check-updates',
  '/api/hub/install',
  '/api/hub/retry-install',
  '/api/hub/uninstall',
  '/api/hub/update',
]);

const RUST_VISIBLE_DEGRADE_ROUTES = new Set([
  '/api/bedrock/test-connection',
  '/api/google/subscription-status',
  '/api/remote-agents/test-connection',
  '/api/stt',
  '/api/stt/stream',
  '/api/system/ensure-managed-acp-tool',
  '/api/system/ensure-node-runtime',
]);

const RUST_OIDC_COMPAT_ROUTES = new Set([
  '/api/webui/change-password',
  '/api/webui/generate-qr-token',
  '/api/webui/reset-password',
]);

function startsWithAny(pathname: string, prefixes: readonly string[]): boolean {
  return prefixes.some(
    (prefix) => pathname === prefix || pathname.startsWith(prefix.endsWith('/') ? prefix : `${prefix}/`)
  );
}

function usesRustVirtualFsPath(value: unknown): boolean {
  if (typeof value !== 'string') return false;
  const normalized = value.trim().replace(/\\/g, '/').replace(/\/+$/, '') || '/';
  return RUST_FS_VIRTUAL_PREFIXES.some((prefix) => normalized === prefix || normalized.startsWith(`${prefix}/`));
}

export function shouldProxyFsFacadeToRust(pathname: string, body: Record<string, unknown>): boolean {
  if (!RUST_BACKED_FS_FACADE_ROUTES.has(pathname)) return false;
  if (pathname === '/api/fs/write' && Object.prototype.hasOwnProperty.call(body, 'expected_revision')) {
    return true;
  }
  return ['dir', 'path', 'root', 'workspace'].some((key) => usesRustVirtualFsPath(body[key]));
}

function localRoute(authority: string, ownership: RouteOwnership = 'LOCAL'): DesktopGatewayRoute {
  return { ownership, action: 'handle-local', authority };
}

function aggregateRoute(authority: string): DesktopGatewayRoute {
  return { ownership: 'AGGREGATE', action: 'handle-aggregate', authority };
}

function rustRoute(authority = 'rust-compat', ownership: RouteOwnership = 'RUST'): DesktopGatewayRoute {
  return { ownership, action: 'proxy-rust', authority };
}

export function classifyDesktopGatewayRoute(method: string, pathname: string): DesktopGatewayRoute {
  const normalizedMethod = method.toUpperCase();

  if (pathname === '/ws') {
    return aggregateRoute('desktop-ws-multiplexer|rust-enterprise-ws');
  }
  if (pathname.startsWith('/api/v1/tool-result-artifacts/')) {
    return rustRoute('rust-enterprise-api');
  }
  if (pathname.startsWith('/api/v1/workflow-')) {
    return rustRoute('rust-enterprise-api');
  }
  if (pathname === '/api/me' || pathname === '/api/system/info') {
    return rustRoute();
  }
  if (
    RUST_VISIBLE_DEGRADE_ROUTES.has(pathname) ||
    (pathname.startsWith('/api/remote-agents/') && pathname.endsWith('/handshake'))
  ) {
    return rustRoute('rust-compat-visible-degrade');
  }
  if (RUST_OIDC_COMPAT_ROUTES.has(pathname)) {
    return rustRoute('rust-compat-oidc');
  }
  if (pathname === '/api/channel/plugins') {
    return aggregateRoute('desktop-gateway-local-channel-plugins+rust-governance');
  }
  if (pathname === '/api/channel/plugins/test') {
    return localRoute('desktop-gateway-local-channel-plugins');
  }
  if (pathname === '/api/channel/ingress/messages') {
    return rustRoute('desktop-gateway-channel-connector');
  }
  if (pathname === '/api/mcp/test-connection' && normalizedMethod === 'POST') {
    return localRoute('desktop-stdio-mcp|rust-mcp-catalog', 'FACADE');
  }
  if (pathname === '/api/hub/extensions') {
    return aggregateRoute('desktop-gateway-local-hub-index+rust-governance');
  }
  if (LOCAL_HUB_ROUTES.has(pathname)) {
    return localRoute('desktop-gateway-local-hub');
  }
  if (pathname === '/api/document/convert') {
    return localRoute('desktop-local');
  }
  if (LOCAL_PREVIEW_HISTORY_ROUTES.has(pathname)) {
    return localRoute('desktop-gateway-local-preview-history');
  }
  if (pathname.startsWith('/api/fs/snapshot/')) {
    return localRoute('desktop-gateway-local-snapshot');
  }
  if (LOCAL_FS_WATCH_ROUTES.has(pathname)) {
    return localRoute('desktop-gateway-local-watch');
  }
  if (LOCAL_FS_ZIP_ROUTES.has(pathname)) {
    return localRoute('desktop-gateway-local-zip');
  }
  if (LOCAL_FS_ROUTES.has(pathname)) {
    return localRoute(pathname.includes('/snapshot/') ? 'desktop-gateway-local-snapshot' : 'desktop-gateway-local');
  }
  if (FACADE_FS_LOCAL_ROUTES.has(pathname)) {
    return localRoute('desktop-gateway-local|rust-file-service', 'FACADE');
  }
  if (FACADE_FS_DESKTOP_LOCAL_ROUTES.has(pathname)) {
    return localRoute('desktop-gateway-local', 'FACADE');
  }
  if (pathname.startsWith('/api/fs/')) {
    return localRoute('desktop-gateway-local|rust-file-service', 'FACADE');
  }
  if (pathname.startsWith('/api/extensions/static/')) {
    return localRoute('desktop-gateway-local-extension-static+rust-governance');
  }
  if (startsWithAny(pathname, ['/api/extensions'])) {
    return aggregateRoute('desktop-gateway-local-extensions+rust-governance');
  }
  if (startsWithAny(pathname, ['/api/agents/custom'])) {
    return aggregateRoute('desktop-gateway+rust-governance');
  }
  if (startsWithAny(pathname, LOCAL_PREFIXES)) {
    return localRoute('desktop-local');
  }
  if (startsWithAny(pathname, RUST_PREFIXES)) {
    return rustRoute();
  }
  if (pathname.startsWith('/api/channel/')) {
    return rustRoute('rust-compat');
  }
  if (pathname.startsWith('/api/hub/')) {
    return normalizedMethod === 'GET'
      ? aggregateRoute('desktop-gateway-local-hub-index+rust-governance')
      : localRoute('desktop-gateway-local-hub');
  }
  return rustRoute('rust-compat', 'UNKNOWN');
}
