import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { classifyDesktopGatewayRoute, shouldProxyFsFacadeToRust } from '@process/gateway/routeOwnership';

const repoRoot = process.cwd();

function source(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), 'utf8');
}

describe('desktop gateway route ownership', () => {
  it('proxies Rust-owned enterprise routes to the backend', () => {
    expect(classifyDesktopGatewayRoute('GET', '/api/auth/user')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/system/ensure-node-runtime')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/system/ensure-managed-acp-tool')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/google/subscription-status')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/bedrock/test-connection')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/stt')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/stt/stream')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/route-ownership')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/webui/change-username')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/providers/fetch-models')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/providers/detect-protocol')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/providers/provider-a/test')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/remote-agents/test-connection')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-compat-visible-degrade',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/messages/search')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(
      classifyDesktopGatewayRoute('POST', '/api/conversations/00000000-0000-0000-0000-000000000001/messages')
    ).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(
      classifyDesktopGatewayRoute(
        'PATCH',
        '/api/conversations/00000000-0000-0000-0000-000000000001/artifacts/artifact-a'
      )
    ).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/teams/team-a/run-state')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/v1/workflow-runs')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-enterprise-api',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/v1/workflow-runs/workflow-run-a/node-runs')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-enterprise-api',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/v1/workflow-runs/workflow-run-a/cancel')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-enterprise-api',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/v1/tool-result-artifacts/read')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-enterprise-api',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/v1/tool-result-artifacts/stream')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'rust-enterprise-api',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/cron/jobs/job-a/skill')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('PUT', '/api/channel/settings/telegram/default-model')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/channel/plugins/enable')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/channel/ingress/messages')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
      authority: 'desktop-gateway-channel-connector',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/workbench/bootstrap')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/agents/refresh')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/agents/provider-health-check')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('PATCH', '/api/agents/agent-a/enabled')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/agents/agent-a/overrides')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/assistants')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('PUT', '/api/assistants/assistant-a')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('PATCH', '/api/assistants/assistant-a/state')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/import')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/builtin-rule')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/builtin-skill')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/skills/import-history')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/assistant-rule/write')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('DELETE', '/api/skills/assistant-rule/assistant-a')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('DELETE', '/api/skills/sample-alpha')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/market/enable')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/market/disable')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/skills/external-paths')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('DELETE', '/api/skills/external-paths')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/mcp/servers')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('PUT', '/api/mcp/servers/00000000-0000-0000-0000-000000000001')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/mcp/test-connection')).toMatchObject({
      ownership: 'FACADE',
      action: 'handle-local',
      authority: 'desktop-stdio-mcp|rust-mcp-catalog',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/mcp/oauth/check-status')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/mcp/oauth/login')).toMatchObject({
      ownership: 'RUST',
      action: 'proxy-rust',
    });
  });

  it('keeps desktop-only routes in the local capability plane', () => {
    expect(classifyDesktopGatewayRoute('POST', '/api/shell/open-file')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/document/convert')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/ppt-proxy/59324')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
      authority: 'desktop-local',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/office-watch-proxy/59324/index.html')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
      authority: 'desktop-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/snapshot/compare')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/preview-history/save')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/hub/install')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/channel/plugins/test')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
      authority: 'desktop-gateway-local-channel-plugins',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/extensions/static/demo-extension/assets/icon.svg')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
      authority: 'desktop-gateway-local-extension-static+rust-governance',
    });
  });

  it('treats file facade routes as gateway-handled instead of plain Rust proxy routes', () => {
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/read')).toMatchObject({
      ownership: 'FACADE',
      action: 'handle-local',
      authority: 'desktop-gateway-local|rust-file-service',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/fetch-remote-image')).toMatchObject({
      ownership: 'FACADE',
      action: 'handle-local',
      authority: 'desktop-gateway-local|rust-file-service',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/upload')).toMatchObject({
      ownership: 'FACADE',
      action: 'handle-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/copy')).toMatchObject({
      ownership: 'FACADE',
      action: 'handle-local',
      authority: 'desktop-gateway-local',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/watch/start')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
      authority: 'desktop-gateway-local-watch',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/fs/zip')).toMatchObject({
      ownership: 'LOCAL',
      action: 'handle-local',
      authority: 'desktop-gateway-local-zip',
    });
  });

  it('proxies Rust-backed virtual fs facade bodies to Rust while keeping local paths local', () => {
    expect(shouldProxyFsFacadeToRust('/api/fs/write', { path: '/workspace/src/main.ts', data: 'x' })).toBe(true);
    expect(
      shouldProxyFsFacadeToRust('/api/fs/write', { path: '/tmp/local.txt', data: 'x', expected_revision: 1 })
    ).toBe(true);
    expect(shouldProxyFsFacadeToRust('/api/fs/read', { path: '/artifacts/report.md' })).toBe(true);
    expect(shouldProxyFsFacadeToRust('/api/fs/list', { root: '/home/user/project' })).toBe(false);
    expect(shouldProxyFsFacadeToRust('/api/fs/copy', { path: '/workspace/src/main.ts' })).toBe(false);
  });

  it('checks Rust-backed fs facade bodies before local fs fallback in the desktop gateway', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf("url.pathname.startsWith('/api/fs/')");
    const branchEnd = indexSource.indexOf("if (url.pathname.startsWith('/api/preview-history/'))", branchStart);

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const fsBranch = indexSource.slice(branchStart, branchEnd);
    expect(fsBranch).toContain('ensureBackendBearerSession');
    expect(fsBranch).toContain('readGatewayJsonBody');
    expect(fsBranch).toContain('shouldProxyFsFacadeToRust');
    expect(fsBranch).toContain('proxyGatewayJsonBodyToBackend');
    expect(fsBranch).toContain('handleLocalFsRoute');
    expect(fsBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(fsBranch.indexOf('readGatewayJsonBody'));
    expect(fsBranch.indexOf('shouldProxyFsFacadeToRust')).toBeLessThan(fsBranch.indexOf('handleLocalFsRoute'));
  });

  it('requires a Rust bearer session before file local side effects', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const uploadStart = indexSource.indexOf("url.pathname === '/api/fs/upload'");
    const uploadEnd = indexSource.indexOf("if (url.pathname === '/api/fs/browse')", uploadStart);
    const browseStart = indexSource.indexOf("url.pathname === '/api/fs/browse'");
    const browseEnd = indexSource.indexOf("if (url.pathname.startsWith('/api/fs/snapshot/'))", browseStart);
    const snapshotStart = indexSource.indexOf("url.pathname.startsWith('/api/fs/snapshot/')");
    const snapshotEnd = indexSource.indexOf("if (url.pathname.startsWith('/api/fs/'))", snapshotStart);
    const previewHistoryStart = indexSource.indexOf("url.pathname.startsWith('/api/preview-history/')");
    const previewHistoryEnd = indexSource.indexOf("url.pathname === '/api/document/convert'", previewHistoryStart);

    expect(uploadStart).toBeGreaterThan(-1);
    expect(uploadEnd).toBeGreaterThan(uploadStart);
    expect(browseStart).toBeGreaterThan(-1);
    expect(browseEnd).toBeGreaterThan(browseStart);
    expect(snapshotStart).toBeGreaterThan(-1);
    expect(snapshotEnd).toBeGreaterThan(snapshotStart);
    expect(previewHistoryStart).toBeGreaterThan(-1);
    expect(previewHistoryEnd).toBeGreaterThan(previewHistoryStart);

    const uploadBranch = indexSource.slice(uploadStart, uploadEnd);
    const browseBranch = indexSource.slice(browseStart, browseEnd);
    const snapshotBranch = indexSource.slice(snapshotStart, snapshotEnd);
    const previewHistoryBranch = indexSource.slice(previewHistoryStart, previewHistoryEnd);

    expect(uploadBranch).toContain('ensureBackendBearerSession');
    expect(browseBranch).toContain('ensureBackendBearerSession');
    expect(snapshotBranch).toContain('ensureBackendBearerSession');
    expect(previewHistoryBranch).toContain('ensureBackendBearerSession');
    expect(uploadBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(uploadBranch.indexOf('readGatewayRawBody'));
    expect(uploadBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      uploadBranch.indexOf('writeLocalUploadFile')
    );
    expect(browseBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      browseBranch.indexOf('browseLocalDirectory')
    );
    expect(snapshotBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      snapshotBranch.indexOf('handleFileSnapshotRoute')
    );
    expect(previewHistoryBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      previewHistoryBranch.indexOf('handlePreviewHistoryRoute')
    );
  });

  it('marks aggregate routes that merge local and Rust facts', () => {
    expect(classifyDesktopGatewayRoute('GET', '/api/channel/plugins')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/extensions/settings-tabs')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
      authority: 'desktop-gateway-local-extensions+rust-governance',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/extensions/agent-activity')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
      authority: 'desktop-gateway-local-extensions+rust-governance',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/extensions/sync')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
      authority: 'desktop-gateway-local-extensions+rust-governance',
    });
    expect(classifyDesktopGatewayRoute('GET', '/api/hub/extensions')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
    });
    expect(classifyDesktopGatewayRoute('POST', '/api/agents/custom')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
      authority: 'desktop-gateway+rust-governance',
    });
    expect(classifyDesktopGatewayRoute('PUT', '/api/agents/custom/custom-a')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
      authority: 'desktop-gateway+rust-governance',
    });
    expect(classifyDesktopGatewayRoute('GET', '/ws')).toMatchObject({
      ownership: 'AGGREGATE',
      action: 'handle-aggregate',
    });
  });

  it('keeps custom agent aggregate routes explicitly handled by the desktop gateway', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf("url.pathname === '/api/agents/custom/try-connect'");
    const branchEnd = indexSource.indexOf('await proxyGatewayRequestToBackend', branchStart);
    const tryConnectBranch = indexSource.slice(branchStart, branchEnd);

    expect(indexSource).toContain("url.pathname === '/api/agents/custom'");
    expect(indexSource).toContain("url.pathname === '/api/agents/custom/try-connect'");
    expect(indexSource).toContain("url.pathname.startsWith('/api/agents/custom/')");
    expect(indexSource).toContain('CUSTOM_AGENTS_AGGREGATE_FAILED');
    expect(tryConnectBranch).toContain('ensureBackendBearerSession');
    expect(tryConnectBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      tryConnectBranch.indexOf('commandExists(command)')
    );
  });

  it('keeps extension sync owned by the desktop gateway manifest scan', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf("url.pathname === '/api/extensions/sync'");
    const branchEnd = indexSource.indexOf('const localExtensionPostRoutes', branchStart);

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const syncBranch = indexSource.slice(branchStart, branchEnd);
    expect(syncBranch).toContain('ensureBackendBearerSession');
    expect(syncBranch).toContain('readGatewayRawBody');
    expect(syncBranch).toContain('syncLocalExtensionsToBackend');
    expect(syncBranch).toContain('desktopExtensionContext()');
    expect(syncBranch).not.toContain('proxyGatewayRequestToBackend');
    expect(syncBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(syncBranch.indexOf('readGatewayRawBody'));
    expect(syncBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      syncBranch.indexOf('syncLocalExtensionsToBackend')
    );
  });

  it('requires a Rust bearer session before serving extension static assets', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf("url.pathname.startsWith('/api/extensions/static/')");
    const branchEnd = indexSource.indexOf("if (url.pathname === '/api/extensions/sync')", branchStart);

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const staticBranch = indexSource.slice(branchStart, branchEnd);
    expect(staticBranch).toContain('ensureBackendBearerSession');
    expect(staticBranch).toContain('isExtensionStaticAssetAllowed');
    expect(staticBranch).toContain('readExtensionStaticAsset');
    expect(staticBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      staticBranch.indexOf('isExtensionStaticAssetAllowed')
    );
    expect(staticBranch.indexOf('isExtensionStaticAssetAllowed')).toBeLessThan(
      staticBranch.indexOf('readExtensionStaticAsset')
    );
    expect(staticBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      staticBranch.indexOf('readExtensionStaticAsset')
    );
  });

  it('requires a Rust bearer session before aggregating extension local routes', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf('const isLocalExtensionRoute =');
    const branchEnd = indexSource.indexOf("if (url.pathname === '/api/hub/extensions')", branchStart);

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const extensionBranch = indexSource.slice(branchStart, branchEnd);
    expect(extensionBranch).toContain('ensureBackendBearerSession');
    expect(extensionBranch).toContain('readGatewayJsonBody');
    expect(extensionBranch).toContain('handleExtensionLocalRoute');
    expect(extensionBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      extensionBranch.indexOf('readGatewayJsonBody')
    );
    expect(extensionBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      extensionBranch.indexOf('handleExtensionLocalRoute')
    );
  });

  it('requires a Rust bearer session before aggregating channel plugins', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf("url.pathname === '/api/channel/plugins'");
    const branchEnd = indexSource.indexOf("if (url.pathname === '/api/channel/plugins/test')", branchStart);

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const channelBranch = indexSource.slice(branchStart, branchEnd);
    expect(channelBranch).toContain('ensureBackendBearerSession');
    expect(channelBranch).toContain('syncLocalExtensionsToBackend');
    expect(channelBranch).toContain('listExtensionChannelPlugins');
    expect(channelBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      channelBranch.indexOf('syncLocalExtensionsToBackend')
    );
    expect(channelBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      channelBranch.indexOf('listExtensionChannelPlugins')
    );
  });

  it('requires a Rust bearer session before local channel connector dry-runs', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf("url.pathname === '/api/channel/plugins/test'");
    const branchEnd = indexSource.indexOf(
      "if (url.pathname === '/api/extensions' || url.pathname.startsWith('/api/extensions/'))",
      branchStart
    );

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const channelTestBranch = indexSource.slice(branchStart, branchEnd);
    expect(channelTestBranch).toContain('ensureBackendBearerSession');
    expect(channelTestBranch).toContain('readGatewayJsonBody');
    expect(channelTestBranch).toContain('handleChannelLocalRoute');
    expect(channelTestBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      channelTestBranch.indexOf('readGatewayJsonBody')
    );
    expect(channelTestBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(
      channelTestBranch.indexOf('handleChannelLocalRoute')
    );
  });

  it('forwards renderer bearer headers on every direct Rust backend fetch in the gateway', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const directBackendFetches = [...indexSource.matchAll(/fetch\(`http:\/\/127\.0\.0\.1:\$\{backendPort\}[^`]*`/g)];

    const missingHeaders = directBackendFetches
      .map((match) => {
        const start = match.index ?? 0;
        const end = indexSource.indexOf('});', start);
        return indexSource.slice(start, end === -1 ? start + 700 : end + 3);
      })
      .filter((call) => !call.includes('forwardedBackendHeaders(req)'));

    expect(missingHeaders).toEqual([]);
  });

  it('syncs extension governance facts after hub local mutations', () => {
    const indexSource = source('packages/desktop/src/index.ts');
    const branchStart = indexSource.indexOf(
      "url.pathname.startsWith('/api/hub/') && url.pathname !== '/api/hub/extensions'"
    );
    const branchEnd = indexSource.indexOf("if (!url.pathname.startsWith('/api/shell/'))", branchStart);

    expect(branchStart).toBeGreaterThan(-1);
    expect(branchEnd).toBeGreaterThan(branchStart);

    const hubBranch = indexSource.slice(branchStart, branchEnd);
    expect(hubBranch).toContain('ensureBackendBearerSession');
    expect(hubBranch).toContain('readGatewayJsonBody');
    expect(hubBranch).toContain('handleHubLocalRoute');
    expect(hubBranch).toContain('syncLocalExtensionsToBackend');
    expect(hubBranch).toContain('withHubGovernanceSyncResult');
    expect(hubBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(hubBranch.indexOf('readGatewayJsonBody'));
    expect(hubBranch.indexOf('ensureBackendBearerSession')).toBeLessThan(hubBranch.indexOf('handleHubLocalRoute'));
    expect(hubBranch.indexOf('handleHubLocalRoute')).toBeLessThan(hubBranch.indexOf('syncLocalExtensionsToBackend'));
    expect(hubBranch.indexOf('syncLocalExtensionsToBackend')).toBeLessThan(
      hubBranch.indexOf('withHubGovernanceSyncResult')
    );
    expect(hubBranch.indexOf('syncLocalExtensionsToBackend')).toBeLessThan(
      hubBranch.indexOf('writeGatewayJson(res, 200')
    );
  });

  it('keeps office iframe proxy routes in the desktop local handler', () => {
    const indexSource = source('packages/desktop/src/index.ts');

    expect(indexSource).toContain("url.pathname.startsWith('/api/ppt-proxy/')");
    expect(indexSource).toContain("url.pathname.startsWith('/api/office-watch-proxy/')");
    expect(indexSource).toContain('proxyOfficeWatchRequest');
    expect(indexSource).toContain('OFFICE_PROXY_UNREACHABLE');
  });

  it('fails unknown paths toward Rust instead of accidentally claiming local ownership', () => {
    expect(classifyDesktopGatewayRoute('GET', '/api/unclassified/new-feature')).toMatchObject({
      ownership: 'UNKNOWN',
      action: 'proxy-rust',
    });
  });
});
