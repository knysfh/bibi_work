/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import path from 'path';

export type ExtensionLocalRouteContext = {
  extensionRoots: string[];
  statePath: string;
  hubStatePath?: string;
};

export type ExtensionStaticAsset = {
  data: Buffer;
  contentType: string;
};

export type ExtensionSyncContribution = {
  type: string;
  key: string;
  manifest: Record<string, unknown>;
  enabled: boolean;
};

export type ExtensionSyncPackage = {
  name: string;
  source: 'local' | 'hub' | 'bundled' | 'marketplace';
  version?: string;
  integrity?: string;
  manifest: Record<string, unknown>;
  risk_level: string;
  enabled: boolean;
  installed?: boolean;
  install_status?: string;
  last_error?: string;
  contributions: ExtensionSyncContribution[];
};

export type ExtensionSyncPayload = {
  extensions: ExtensionSyncPackage[];
};

type LocalExtension = {
  dir: string;
  manifest: Record<string, unknown>;
};

type StoredExtensionState = {
  enabled: boolean;
  reason?: string;
  updatedAt: number;
};

type StoredExtensionLocalState = {
  version: 1;
  extensions: Record<string, StoredExtensionState>;
};

type StoredHubExtensionState = {
  status: string;
  error?: string;
  catalog?: Record<string, unknown>;
  updatedAt: number;
};

type StoredHubLocalState = {
  version: 1;
  extensions: Record<string, StoredHubExtensionState>;
};

export class ExtensionLocalRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'ExtensionLocalRouteError';
  }
}

const MANIFEST_FILE = 'biwork-extension.json';
const CONTRIBUTION_BY_PATH: Record<string, string> = {
  '/api/extensions/themes': 'themes',
  '/api/extensions/assistants': 'assistants',
  '/api/extensions/agents': 'agents',
  '/api/extensions/acp-adapters': 'acpAdapters',
  '/api/extensions/mcp-servers': 'mcpServers',
  '/api/extensions/skills': 'skills',
  '/api/extensions/channel-plugins': 'channelPlugins',
  '/api/extensions/settings-tabs': 'settingsTabs',
  '/api/extensions/webui': 'webui',
};
const SYNC_CONTRIBUTION_TYPE_BY_KEY: Record<string, string> = {
  themes: 'theme',
  assistants: 'assistant',
  agents: 'agent',
  acpAdapters: 'acp_adapter',
  mcpServers: 'mcp_server',
  skills: 'skill',
  channelPlugins: 'channel_plugin',
  settingsTabs: 'settings_tab',
  webui: 'webui',
};
const MIME_BY_EXTENSION: Record<string, string> = {
  '.css': 'text/css; charset=utf-8',
  '.gif': 'image/gif',
  '.html': 'text/html; charset=utf-8',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.txt': 'text/plain; charset=utf-8',
  '.webp': 'image/webp',
};

function valueString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

function manifestName(manifest: Record<string, unknown>): string {
  return valueString(manifest.name) ?? '';
}

function displayName(manifest: Record<string, unknown>): string {
  return valueString(manifest.display_name) ?? valueString(manifest.displayName) ?? manifestName(manifest);
}

function emptyState(): StoredExtensionLocalState {
  return { version: 1, extensions: {} };
}

function isStoredExtensionState(value: unknown): value is StoredExtensionState {
  if (!value || typeof value !== 'object') return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.enabled === 'boolean' &&
    typeof item.updatedAt === 'number' &&
    (item.reason === undefined || typeof item.reason === 'string')
  );
}

function isStoredHubExtensionState(value: unknown): value is StoredHubExtensionState {
  if (!value || typeof value !== 'object') return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.status === 'string' &&
    typeof item.updatedAt === 'number' &&
    (item.error === undefined || typeof item.error === 'string') &&
    (item.catalog === undefined ||
      (typeof item.catalog === 'object' && item.catalog !== null && !Array.isArray(item.catalog)))
  );
}

async function readState(statePath: string): Promise<StoredExtensionLocalState> {
  try {
    const raw = await fs.readFile(statePath, 'utf8');
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== 'object') return emptyState();
    const candidate = parsed as { version?: unknown; extensions?: unknown };
    if (candidate.version !== 1 || !candidate.extensions || typeof candidate.extensions !== 'object') {
      return emptyState();
    }

    const extensions: Record<string, StoredExtensionState> = {};
    for (const [name, state] of Object.entries(candidate.extensions as Record<string, unknown>)) {
      if (name && isStoredExtensionState(state)) {
        extensions[name] = state;
      }
    }
    return { version: 1, extensions };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return emptyState();
    throw error;
  }
}

async function readHubState(statePath?: string): Promise<StoredHubLocalState> {
  if (!statePath) return { version: 1, extensions: {} };
  try {
    const raw = await fs.readFile(statePath, 'utf8');
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== 'object') return { version: 1, extensions: {} };
    const candidate = parsed as { version?: unknown; extensions?: unknown };
    if (candidate.version !== 1 || !candidate.extensions || typeof candidate.extensions !== 'object') {
      return { version: 1, extensions: {} };
    }

    const extensions: Record<string, StoredHubExtensionState> = {};
    for (const [name, state] of Object.entries(candidate.extensions as Record<string, unknown>)) {
      if (name && isStoredHubExtensionState(state)) {
        extensions[name] = state;
      }
    }
    return { version: 1, extensions };
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return { version: 1, extensions: {} };
    throw error;
  }
}

async function writeState(statePath: string, state: StoredExtensionLocalState): Promise<void> {
  await fs.mkdir(path.dirname(statePath), { recursive: true });
  await fs.writeFile(statePath, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
}

async function isExtensionEnabled(extensionName: string, context: ExtensionLocalRouteContext): Promise<boolean> {
  const state = await readState(context.statePath);
  return state.extensions[extensionName]?.enabled ?? true;
}

async function readJsonFile(filePath: string): Promise<unknown> {
  return JSON.parse(await fs.readFile(filePath, 'utf8')) as unknown;
}

async function readManifest(extensionDir: string): Promise<LocalExtension | null> {
  try {
    const manifest = await readJsonFile(path.join(extensionDir, MANIFEST_FILE));
    if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) return null;
    if (!manifestName(manifest as Record<string, unknown>)) return null;
    return { dir: extensionDir, manifest: manifest as Record<string, unknown> };
  } catch {
    return null;
  }
}

async function loadLocalExtensions(context: ExtensionLocalRouteContext): Promise<LocalExtension[]> {
  const extensions = new Map<string, LocalExtension>();
  for (const root of context.extensionRoots) {
    let rootStats;
    try {
      rootStats = await fs.stat(root);
    } catch {
      continue;
    }
    if (!rootStats.isDirectory()) continue;

    const rootExtension = await readManifest(root);
    if (rootExtension) {
      extensions.set(manifestName(rootExtension.manifest), rootExtension);
      continue;
    }

    let entries;
    try {
      entries = await fs.readdir(root, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      if (!entry.isDirectory()) continue;
      const extension = await readManifest(path.join(root, entry.name));
      if (extension) extensions.set(manifestName(extension.manifest), extension);
    }
  }
  return [...extensions.values()].sort((left, right) =>
    manifestName(left.manifest).localeCompare(manifestName(right.manifest))
  );
}

async function findLocalExtension(
  context: ExtensionLocalRouteContext,
  extensionName: string
): Promise<LocalExtension | null> {
  const extensions = await loadLocalExtensions(context);
  return extensions.find((extension) => manifestName(extension.manifest) === extensionName) ?? null;
}

function asArray(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (item): item is Record<string, unknown> => Boolean(item) && typeof item === 'object' && !Array.isArray(item)
  );
}

function resolveInside(root: string, relativePath: string): string {
  const resolved = path.resolve(root, relativePath);
  const rel = path.relative(root, resolved);
  if (rel.startsWith('..') || path.isAbsolute(rel)) {
    throw new ExtensionLocalRouteError(
      400,
      'INVALID_EXTENSION_MANIFEST',
      'extension contribution path escapes extension root'
    );
  }
  return resolved;
}

async function contributionValue(extension: LocalExtension, key: string): Promise<Record<string, unknown>[]> {
  const contributes = extension.manifest.contributes;
  if (!contributes || typeof contributes !== 'object' || Array.isArray(contributes)) return [];
  const raw = (contributes as Record<string, unknown>)[key];
  if (typeof raw === 'string' && raw.startsWith('$file:')) {
    const filePath = resolveInside(extension.dir, raw.slice('$file:'.length));
    return asArray(await readJsonFile(filePath));
  }
  return asArray(raw);
}

function staticUrl(extensionName: string, assetPath: unknown): string | undefined {
  const value = valueString(assetPath);
  if (!value) return undefined;
  if (/^https?:\/\//i.test(value) || value.startsWith('/api/extensions/static/')) return value;
  return `/api/extensions/static/${encodeURIComponent(extensionName)}/${value.replace(/^\/+/, '')}`;
}

function mapContribution(
  extension: LocalExtension,
  key: string,
  item: Record<string, unknown>,
  index: number
): Record<string, unknown> {
  const extensionName = manifestName(extension.manifest);
  if (key === 'settingsTabs') {
    const id = valueString(item.id) ?? `tab-${index}`;
    const position =
      item.position && typeof item.position === 'object' ? (item.position as Record<string, unknown>) : undefined;
    return {
      id,
      label: valueString(item.label) ?? valueString(item.name) ?? id,
      icon: staticUrl(extensionName, item.icon),
      url:
        staticUrl(extensionName, item.entryPoint ?? item.url) ??
        `/api/extensions/static/${encodeURIComponent(extensionName)}/${id}`,
      position: position
        ? {
            relativeTo: valueString(position.relativeTo) ?? valueString(position.anchor),
            placement: valueString(position.placement) ?? 'after',
          }
        : undefined,
      order: typeof item.order === 'number' ? item.order : index,
      extensionName,
    };
  }
  if (key === 'skills') {
    return {
      ...item,
      location:
        valueString(item.location) ??
        (valueString(item.file) ? resolveInside(extension.dir, valueString(item.file)!) : extension.dir),
      _extensionName: extensionName,
      _source: 'extension',
    };
  }
  if (key === 'themes') {
    return {
      ...item,
      file: staticUrl(extensionName, item.file),
      cover: staticUrl(extensionName, item.cover),
      _extensionName: extensionName,
    };
  }
  if (key === 'channelPlugins') {
    const type = valueString(item.type) ?? valueString(item.id) ?? `extension-channel-${index}`;
    return {
      plugin_id: type,
      id: type,
      type,
      name: valueString(item.name) ?? type,
      enabled: false,
      connected: false,
      status: 'disabled',
      active_users: 0,
      has_token: false,
      is_extension: true,
      extension_meta: {
        extensionName,
        description: valueString(item.description) ?? '',
        icon: staticUrl(extensionName, item.icon),
        credentialFields: Array.isArray(item.credentialFields) ? item.credentialFields : [],
        configFields: Array.isArray(item.configFields) ? item.configFields : [],
      },
    };
  }
  return {
    ...item,
    _extensionName: extensionName,
    _source: 'extension',
  };
}

function riskLevelForManifest(manifest: Record<string, unknown>): string {
  const permissions = manifest.permissions;
  if (!permissions || typeof permissions !== 'object') return 'safe';
  const values = Object.values(permissions as Record<string, unknown>);
  if (values.includes(true)) return 'moderate';
  return 'safe';
}

function isHubStatusInstalled(status: string): boolean {
  return status === 'installed' || status === 'update_available';
}

function applyHubStateToPackage(extension: ExtensionSyncPackage, state: StoredHubExtensionState): void {
  const installed = isHubStatusInstalled(state.status);
  extension.source = state.catalog?.bundled === true ? 'bundled' : 'hub';
  extension.installed = installed;
  extension.install_status = state.status;
  extension.last_error = state.error;
  if (!installed) {
    extension.enabled = false;
    extension.contributions = extension.contributions.map((contribution) => ({
      ...contribution,
      enabled: false,
    }));
  }
}

function hubCatalogManifest(name: string, state: StoredHubExtensionState): Record<string, unknown> {
  const catalog = state.catalog;
  if (!catalog) {
    return {
      name,
      source: 'hub',
      local_status: state.status,
      local_error: state.error ?? null,
    };
  }
  const manifest = { ...catalog };
  delete manifest.status;
  delete manifest.installError;
  return {
    ...manifest,
    name,
    source: catalog.bundled === true ? 'bundled' : 'hub',
    local_status: state.status,
    local_error: state.error ?? null,
  };
}

function contributionSyncKey(key: string, item: Record<string, unknown>, index: number): string {
  if (key === 'settingsTabs') return valueString(item.id) ?? `tab-${index}`;
  if (key === 'channelPlugins') return valueString(item.type) ?? valueString(item.id) ?? `extension-channel-${index}`;
  return valueString(item.id) ?? valueString(item.name) ?? valueString(item.type) ?? `${key}-${index}`;
}

export async function buildExtensionSyncPayload(context: ExtensionLocalRouteContext): Promise<ExtensionSyncPayload> {
  const extensions = await loadLocalExtensions(context);
  const state = await readState(context.statePath);
  const hubState = await readHubState(context.hubStatePath);
  const packages: ExtensionSyncPackage[] = [];
  const packagesByName = new Map<string, ExtensionSyncPackage>();

  for (const extension of extensions) {
    const name = manifestName(extension.manifest);
    const enabled = state.extensions[name]?.enabled ?? true;
    const contributions: ExtensionSyncContribution[] = [];

    for (const [key, contributionType] of Object.entries(SYNC_CONTRIBUTION_TYPE_BY_KEY)) {
      const items = await contributionValue(extension, key);
      items.forEach((item, index) => {
        contributions.push({
          type: contributionType,
          key: contributionSyncKey(key, item, index),
          manifest: mapContribution(extension, key, item, index),
          enabled,
        });
      });
    }

    const syncPackage: ExtensionSyncPackage = {
      name,
      source: 'local',
      version: valueString(extension.manifest.version) ?? '0.0.0',
      manifest: extension.manifest,
      risk_level: riskLevelForManifest(extension.manifest),
      enabled,
      contributions,
    };
    packages.push(syncPackage);
    packagesByName.set(name, syncPackage);
  }

  for (const [name, localState] of Object.entries(hubState.extensions)) {
    const existingPackage = packagesByName.get(name);
    if (existingPackage) {
      applyHubStateToPackage(existingPackage, localState);
      continue;
    }

    const installed = isHubStatusInstalled(localState.status);
    packages.push({
      name,
      source: 'hub',
      version: valueString(localState.catalog?.version),
      integrity: valueString((localState.catalog?.dist as Record<string, unknown> | undefined)?.integrity),
      manifest: hubCatalogManifest(name, localState),
      risk_level: 'moderate',
      enabled: installed,
      installed,
      install_status: localState.status,
      last_error: localState.error,
      contributions: [],
    });
  }

  return { extensions: packages };
}

async function listExtensionInfos(context: ExtensionLocalRouteContext): Promise<Record<string, unknown>[]> {
  const extensions = await loadLocalExtensions(context);
  const state = await readState(context.statePath);
  return extensions.map(({ manifest }) => {
    const name = manifestName(manifest);
    return {
      name,
      display_name: displayName(manifest),
      version: valueString(manifest.version) ?? '0.0.0',
      description: valueString(manifest.description) ?? '',
      source: 'local',
      enabled: state.extensions[name]?.enabled ?? true,
    };
  });
}

async function listContributions(
  pathname: string,
  context: ExtensionLocalRouteContext
): Promise<Record<string, unknown>[]> {
  const key = CONTRIBUTION_BY_PATH[pathname];
  if (!key) {
    throw new ExtensionLocalRouteError(
      404,
      'EXTENSION_LOCAL_ROUTE_NOT_FOUND',
      'desktop local extension route not found'
    );
  }
  const output: Record<string, unknown>[] = [];
  const extensions = await loadLocalExtensions(context);
  for (const extension of extensions) {
    if (!(await isExtensionEnabled(manifestName(extension.manifest), context))) continue;
    const items = await contributionValue(extension, key);
    items.forEach((item, index) => output.push(mapContribution(extension, key, item, index)));
  }
  return output;
}

async function extensionI18n(
  body: Record<string, unknown>,
  context: ExtensionLocalRouteContext
): Promise<Record<string, unknown>> {
  const locale = valueString(body.locale) ?? valueString(body.language) ?? 'en-US';
  const result: Record<string, unknown> = {};
  const extensions = await loadLocalExtensions(context);
  for (const extension of extensions) {
    if (!(await isExtensionEnabled(manifestName(extension.manifest), context))) continue;
    const i18n = extension.manifest.i18n;
    if (!i18n || typeof i18n !== 'object' || Array.isArray(i18n)) continue;
    const localesDir = valueString((i18n as Record<string, unknown>).localesDir);
    if (!localesDir) continue;
    const defaultLocale = valueString((i18n as Record<string, unknown>).defaultLocale);
    const candidates = [locale, defaultLocale].filter((value): value is string => Boolean(value));
    for (const candidate of candidates) {
      try {
        result[manifestName(extension.manifest)] = await readJsonFile(
          resolveInside(extension.dir, path.join(localesDir, candidate, 'extension.json'))
        );
        break;
      } catch {
        // Try the next locale candidate.
      }
    }
  }
  return result;
}

function requiredName(body: Record<string, unknown>): string {
  const name = valueString(body.name);
  if (!name) {
    throw new ExtensionLocalRouteError(400, 'INVALID_INPUT', 'name is required');
  }
  return name;
}

async function setExtensionEnabled(
  body: Record<string, unknown>,
  context: ExtensionLocalRouteContext,
  enabled: boolean
): Promise<{ name: string; enabled: boolean; reason?: string }> {
  const result = await previewExtensionEnabledState(body, context, enabled);
  const state = await readState(context.statePath);
  state.extensions[result.name] = {
    enabled,
    reason: enabled ? undefined : result.reason,
    updatedAt: Date.now(),
  };
  await writeState(context.statePath, state);
  return result;
}

export async function previewExtensionEnabledState(
  body: Record<string, unknown>,
  context: ExtensionLocalRouteContext,
  enabled: boolean
): Promise<{ name: string; enabled: boolean; reason?: string }> {
  const name = requiredName(body);
  const extension = await findLocalExtension(context, name);
  if (!extension) {
    throw new ExtensionLocalRouteError(404, 'EXTENSION_NOT_FOUND', 'extension not found');
  }
  const reason = valueString(body.reason);
  return reason && !enabled ? { name, enabled, reason } : { name, enabled };
}

async function extensionPermissions(
  body: Record<string, unknown>,
  context: ExtensionLocalRouteContext
): Promise<unknown> {
  const extension = await findLocalExtension(context, requiredName(body));
  if (!extension) {
    throw new ExtensionLocalRouteError(404, 'EXTENSION_NOT_FOUND', 'extension not found');
  }
  const permissions = extension.manifest.permissions;
  if (Array.isArray(permissions)) return permissions;
  if (permissions && typeof permissions === 'object') {
    return Object.entries(permissions as Record<string, unknown>).map(([name, granted]) => ({
      name,
      description: name,
      level: granted === true ? 'moderate' : 'safe',
      granted: Boolean(granted),
    }));
  }
  return [];
}

async function extensionRiskLevel(body: Record<string, unknown>, context: ExtensionLocalRouteContext): Promise<string> {
  const extension = await findLocalExtension(context, requiredName(body));
  if (!extension) {
    throw new ExtensionLocalRouteError(404, 'EXTENSION_NOT_FOUND', 'extension not found');
  }
  return riskLevelForManifest(extension.manifest);
}

export async function handleExtensionLocalRoute(
  pathname: string,
  body: Record<string, unknown>,
  context: ExtensionLocalRouteContext
): Promise<unknown> {
  if (pathname === '/api/extensions') return listExtensionInfos(context);
  if (pathname === '/api/extensions/enable') return setExtensionEnabled(body, context, true);
  if (pathname === '/api/extensions/disable') return setExtensionEnabled(body, context, false);
  if (pathname === '/api/extensions/permissions') return extensionPermissions(body, context);
  if (pathname === '/api/extensions/risk-level') return extensionRiskLevel(body, context);
  if (pathname === '/api/extensions/agent-activity') {
    return {
      generatedAt: Date.now(),
      totalConversations: 0,
      runningConversations: 0,
      agents: [],
    };
  }
  if (pathname === '/api/extensions/i18n') return extensionI18n(body, context);
  return listContributions(pathname, context);
}

export async function listExtensionChannelPlugins(
  context: ExtensionLocalRouteContext
): Promise<Record<string, unknown>[]> {
  return listContributions('/api/extensions/channel-plugins', context);
}

export async function readExtensionStaticAsset(
  extensionName: string,
  assetPath: string,
  context: ExtensionLocalRouteContext
): Promise<ExtensionStaticAsset> {
  const extension = await findLocalExtension(context, extensionName);
  if (!extension) {
    throw new ExtensionLocalRouteError(404, 'EXTENSION_NOT_FOUND', 'extension not found');
  }
  if (!(await isExtensionEnabled(extensionName, context))) {
    throw new ExtensionLocalRouteError(404, 'EXTENSION_DISABLED', 'extension is disabled');
  }
  const filePath = resolveInside(extension.dir, assetPath);
  const stats = await fs.stat(filePath);
  if (!stats.isFile()) {
    throw new ExtensionLocalRouteError(404, 'EXTENSION_STATIC_ASSET_NOT_FOUND', 'extension static asset not found');
  }
  return {
    data: await fs.readFile(filePath),
    contentType: MIME_BY_EXTENSION[path.extname(filePath).toLowerCase()] ?? 'application/octet-stream',
  };
}
