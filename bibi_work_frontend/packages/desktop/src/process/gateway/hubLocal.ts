/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { createHash, randomUUID, timingSafeEqual } from 'crypto';
import fs from 'fs/promises';
import path from 'path';
import * as tar from 'tar';
import { fileURLToPath } from 'url';
import type { HubExtensionStatus, HubStateChange, IHubAgentItem } from '@/common/types/agent/hub';

export type HubLocalRouteContext = {
  statePath: string;
  installRoot?: string;
  extension?: IHubAgentItem;
  fetchTarball?: (extension: IHubAgentItem) => Promise<Uint8Array>;
  emitStateChange?: (change: HubStateChange) => void;
};

export type HubLocalActionResult = {
  name: string;
  status: HubExtensionStatus;
  error?: string;
};

type StoredHubExtensionState = {
  status: HubExtensionStatus;
  error?: string;
  catalog?: IHubAgentItem;
  updatedAt: number;
};

type StoredHubLocalState = {
  version: 1;
  extensions: Record<string, StoredHubExtensionState>;
};

export class HubLocalRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'HubLocalRouteError';
  }
}

const INSTALLER_NOT_ATTACHED_ERROR = 'Local hub extension installer is not attached.';
const MAX_TARBALL_BYTES = 256 * 1024 * 1024;
const SAFE_EXTENSION_NAME = /^[a-zA-Z0-9][a-zA-Z0-9._-]*$/;
const VALID_STATUSES = new Set<HubExtensionStatus>([
  'not_installed',
  'installing',
  'installed',
  'install_failed',
  'update_available',
  'uninstalling',
]);

function emptyState(): StoredHubLocalState {
  return { version: 1, extensions: {} };
}

function isStoredExtensionState(value: unknown): value is StoredHubExtensionState {
  if (!value || typeof value !== 'object') return false;
  const item = value as Record<string, unknown>;
  return (
    typeof item.updatedAt === 'number' &&
    typeof item.status === 'string' &&
    VALID_STATUSES.has(item.status as HubExtensionStatus) &&
    (item.error === undefined || typeof item.error === 'string') &&
    (item.catalog === undefined ||
      (typeof item.catalog === 'object' && item.catalog !== null && !Array.isArray(item.catalog)))
  );
}

async function readState(statePath: string): Promise<StoredHubLocalState> {
  try {
    const raw = await fs.readFile(statePath, 'utf8');
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== 'object') return emptyState();
    const candidate = parsed as { version?: unknown; extensions?: unknown };
    if (candidate.version !== 1 || !candidate.extensions || typeof candidate.extensions !== 'object') {
      return emptyState();
    }

    const extensions: Record<string, StoredHubExtensionState> = {};
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

async function writeState(statePath: string, state: StoredHubLocalState): Promise<void> {
  await fs.mkdir(path.dirname(statePath), { recursive: true });
  await fs.writeFile(statePath, `${JSON.stringify(state, null, 2)}\n`, 'utf8');
}

function requiredName(body: Record<string, unknown>): string {
  const value = body.name;
  if (typeof value !== 'string' || !value.trim()) {
    throw new HubLocalRouteError(400, 'INVALID_INPUT', 'name is required');
  }
  const name = value.trim();
  if (!SAFE_EXTENSION_NAME.test(name) || name === '.' || name === '..') {
    throw new HubLocalRouteError(400, 'INVALID_INPUT', 'name contains unsupported characters');
  }
  return name;
}

async function defaultFetchTarball(extension: IHubAgentItem): Promise<Uint8Array> {
  const tarball = extension.dist?.tarball?.trim();
  if (!tarball) {
    throw new Error('Hub extension tarball URL is missing.');
  }
  if (tarball.startsWith('data:')) {
    const separator = tarball.indexOf(',');
    const metadata = separator >= 0 ? tarball.slice(5, separator) : '';
    const encoded = separator >= 0 ? tarball.slice(separator + 1) : '';
    if (!metadata.toLowerCase().endsWith(';base64') || !encoded || !/^[a-zA-Z0-9+/]*={0,2}$/.test(encoded)) {
      throw new Error('Hub extension data tarball must be valid base64.');
    }
    if (encoded.length > Math.ceil((MAX_TARBALL_BYTES * 4) / 3) + 4) {
      throw new Error('Hub extension tarball exceeds the size limit.');
    }
    const bytes = Buffer.from(encoded, 'base64');
    if (bytes.byteLength > MAX_TARBALL_BYTES) {
      throw new Error('Hub extension tarball exceeds the size limit.');
    }
    return new Uint8Array(bytes);
  }
  let url: URL;
  try {
    url = new URL(tarball);
  } catch {
    throw new Error('Hub extension tarball must be an absolute URL.');
  }
  if (url.protocol === 'file:') {
    if (!extension.bundled) {
      throw new Error('Only bundled Hub extensions may use local tarballs.');
    }
    const bytes = new Uint8Array(await fs.readFile(fileURLToPath(url)));
    if (bytes.byteLength > MAX_TARBALL_BYTES) {
      throw new Error('Hub extension tarball exceeds the size limit.');
    }
    return bytes;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error('Hub extension tarball must use HTTP(S), or file: for bundled packages.');
  }
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Hub extension download failed: HTTP ${response.status}.`);
  }
  const declaredLength = Number(response.headers.get('content-length') ?? 0);
  if (Number.isFinite(declaredLength) && declaredLength > MAX_TARBALL_BYTES) {
    throw new Error('Hub extension tarball exceeds the size limit.');
  }
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.byteLength > MAX_TARBALL_BYTES) {
    throw new Error('Hub extension tarball exceeds the size limit.');
  }
  return bytes;
}

function verifyTarballIntegrity(bytes: Uint8Array, integrity: string): void {
  const [algorithm, encoded, ...extra] = integrity.trim().split('-');
  if (algorithm !== 'sha512' || !encoded || extra.length > 0) {
    throw new Error('Hub extension integrity must be a SHA-512 SRI value.');
  }
  const expected = Buffer.from(encoded, 'base64');
  const actual = createHash('sha512').update(bytes).digest();
  if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) {
    throw new Error('Hub extension integrity verification failed.');
  }
}

async function readInstalledManifest(extensionRoot: string): Promise<Record<string, unknown> | null> {
  try {
    const parsed = JSON.parse(await fs.readFile(path.join(extensionRoot, 'biwork-extension.json'), 'utf8')) as unknown;
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

async function locateExtractedExtensionRoot(extractRoot: string, expectedName: string): Promise<string> {
  const candidates = [extractRoot];
  for (const entry of await fs.readdir(extractRoot, { withFileTypes: true })) {
    if (entry.isDirectory()) candidates.push(path.join(extractRoot, entry.name));
  }
  const manifests = await Promise.all(
    candidates.map(async (candidate) => ({ candidate, manifest: await readInstalledManifest(candidate) }))
  );
  const match = manifests.find(({ manifest }) => manifest?.name === expectedName);
  if (match) return match.candidate;
  throw new Error(`Hub extension archive does not contain a valid ${expectedName} manifest.`);
}

async function preserveHubCatalogMetadata(extensionRoot: string, extension: IHubAgentItem): Promise<void> {
  const manifestPath = path.join(extensionRoot, 'biwork-extension.json');
  const manifest = await readInstalledManifest(extensionRoot);
  if (!manifest) throw new Error('Hub extension manifest is unreadable after extraction.');
  await fs.writeFile(
    manifestPath,
    `${JSON.stringify(
      {
        ...manifest,
        displayName: manifest.displayName ?? manifest.display_name ?? extension.display_name,
        description: manifest.description ?? extension.description,
        author: manifest.author ?? extension.author,
        icon: manifest.icon ?? extension.icon,
        dist: extension.dist,
        engines: extension.engines,
        hubs: extension.hubs,
        tags: extension.tags ?? manifest.tags,
      },
      null,
      2
    )}\n`,
    'utf8'
  );
}

async function replaceInstalledDirectory(source: string, target: string): Promise<void> {
  const backup = `${target}.backup-${randomUUID()}`;
  let movedExisting = false;
  try {
    try {
      await fs.rename(target, backup);
      movedExisting = true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    await fs.rename(source, target);
    if (movedExisting) await fs.rm(backup, { recursive: true, force: true });
  } catch (error) {
    if (movedExisting) {
      await fs.rename(backup, target).catch((): undefined => undefined);
    }
    throw error;
  }
}

async function installHubExtension(name: string, context: HubLocalRouteContext): Promise<void> {
  if (!context.installRoot || !context.extension || context.extension.name !== name) {
    throw new Error(INSTALLER_NOT_ATTACHED_ERROR);
  }
  const fetchTarball = context.fetchTarball ?? defaultFetchTarball;
  const bytes = await fetchTarball(context.extension);
  verifyTarballIntegrity(bytes, context.extension.dist.integrity);

  await fs.mkdir(context.installRoot, { recursive: true });
  const stagingRoot = await fs.mkdtemp(path.join(context.installRoot, '.hub-install-'));
  try {
    const archivePath = path.join(stagingRoot, 'extension.tgz');
    const extractRoot = path.join(stagingRoot, 'unpacked');
    await fs.mkdir(extractRoot);
    await fs.writeFile(archivePath, bytes);
    await tar.x({
      file: archivePath,
      cwd: extractRoot,
      strict: true,
      preservePaths: false,
    });
    const extensionRoot = await locateExtractedExtensionRoot(extractRoot, name);
    await preserveHubCatalogMetadata(extensionRoot, context.extension);
    await replaceInstalledDirectory(extensionRoot, path.join(context.installRoot, name));
  } finally {
    await fs.rm(stagingRoot, { recursive: true, force: true });
  }
}

async function uninstallHubExtension(name: string, context: HubLocalRouteContext): Promise<void> {
  if (!context.installRoot) throw new Error(INSTALLER_NOT_ATTACHED_ERROR);
  await fs.rm(path.join(context.installRoot, name), { recursive: true, force: true });
}

async function setExtensionState(
  context: HubLocalRouteContext,
  name: string,
  status: HubExtensionStatus,
  error?: string
): Promise<HubLocalActionResult> {
  const state = await readState(context.statePath);
  const previous = state.extensions[name];
  state.extensions[name] = {
    status,
    error,
    catalog: context.extension ?? previous?.catalog,
    updatedAt: Date.now(),
  };
  await writeState(context.statePath, state);
  const result = error ? { name, status, error } : { name, status };
  context.emitStateChange?.(result);
  return result;
}

export async function handleHubLocalRoute(
  pathname: string,
  body: Record<string, unknown>,
  context: HubLocalRouteContext
): Promise<unknown> {
  switch (pathname) {
    case '/api/hub/check-updates':
      return [];
    case '/api/hub/install':
    case '/api/hub/retry-install':
    case '/api/hub/update': {
      const name = requiredName(body);
      await setExtensionState(context, name, 'installing');
      try {
        await installHubExtension(name, context);
        return setExtensionState(context, name, 'installed');
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return setExtensionState(context, name, 'install_failed', message);
      }
    }
    case '/api/hub/uninstall': {
      const name = requiredName(body);
      await setExtensionState(context, name, 'uninstalling');
      try {
        await uninstallHubExtension(name, context);
        return setExtensionState(context, name, 'not_installed');
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return setExtensionState(context, name, 'install_failed', message);
      }
    }
    default:
      throw new HubLocalRouteError(404, 'HUB_LOCAL_ROUTE_NOT_FOUND', 'desktop local hub route not found');
  }
}

export async function applyHubLocalStateToExtensions(
  extensions: IHubAgentItem[],
  context: HubLocalRouteContext
): Promise<IHubAgentItem[]> {
  const state = await readState(context.statePath);
  return extensions.map((extension) => {
    const localState = state.extensions[extension.name];
    if (!localState) return extension;
    return {
      ...extension,
      status: localState.status,
      installError: localState.error,
    };
  });
}
