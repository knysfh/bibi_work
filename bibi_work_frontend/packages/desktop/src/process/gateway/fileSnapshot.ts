/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { createHash } from 'crypto';
import fs from 'fs/promises';
import path from 'path';

type FileChangeOperation = 'create' | 'modify' | 'delete';

type SnapshotFileRecord = {
  relative_path: string;
  content_base64: string;
};

type SnapshotStore = {
  workspace: string;
  created_at: number;
  files: Record<string, SnapshotFileRecord>;
  staged: string[];
};

type RawFileChange = {
  file_path: string;
  relative_path: string;
  operation: FileChangeOperation;
};

type RawCompareResult = {
  staged: RawFileChange[];
  unstaged: RawFileChange[];
};

export type FileSnapshotContext = {
  storageDir: string;
};

export class FileSnapshotRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'FileSnapshotRouteError';
  }
}

const IGNORED_DIRS = new Set(['.git', 'node_modules']);

function bodyString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== 'string' || !value.trim()) {
    throw new FileSnapshotRouteError(400, 'INVALID_INPUT', `${key} is required`);
  }
  return value.trim();
}

function workspaceRoot(body: Record<string, unknown>): string {
  return path.resolve(bodyString(body, 'workspace'));
}

function relativeKey(root: string, filePath: string): string {
  return path.relative(root, filePath).replace(/\\/g, '/');
}

function resolveWorkspaceFile(root: string, filePath: string): string {
  const resolved = path.isAbsolute(filePath) ? path.resolve(filePath) : path.resolve(root, filePath);
  const relative = path.relative(root, resolved);
  if (!relative || relative.startsWith('..') || path.isAbsolute(relative)) {
    throw new FileSnapshotRouteError(403, 'PATH_OUTSIDE_WORKSPACE', 'file_path must be inside workspace');
  }
  return resolved;
}

function snapshotPath(context: FileSnapshotContext, workspace: string): string {
  const key = createHash('sha256').update(path.resolve(workspace)).digest('hex').slice(0, 32);
  return path.join(context.storageDir, `${key}.json`);
}

async function collectFiles(root: string, current: string, output: Record<string, SnapshotFileRecord>): Promise<void> {
  let entries;
  try {
    entries = await fs.readdir(current, { withFileTypes: true });
  } catch {
    return;
  }

  for (const entry of entries) {
    if (entry.isDirectory() && IGNORED_DIRS.has(entry.name)) continue;
    const fullPath = path.join(current, entry.name);
    if (entry.isDirectory()) {
      await collectFiles(root, fullPath, output);
      continue;
    }
    if (!entry.isFile()) continue;
    const key = relativeKey(root, fullPath);
    output[key] = {
      relative_path: key,
      content_base64: (await fs.readFile(fullPath)).toString('base64'),
    };
  }
}

async function captureWorkspace(workspace: string): Promise<Record<string, SnapshotFileRecord>> {
  const files: Record<string, SnapshotFileRecord> = {};
  await collectFiles(workspace, workspace, files);
  return files;
}

async function readStore(context: FileSnapshotContext, workspace: string): Promise<SnapshotStore | null> {
  try {
    const raw = await fs.readFile(snapshotPath(context, workspace), 'utf8');
    const parsed = JSON.parse(raw) as SnapshotStore;
    return {
      workspace,
      created_at: typeof parsed.created_at === 'number' ? parsed.created_at : Date.now(),
      files: parsed.files && typeof parsed.files === 'object' ? parsed.files : {},
      staged: Array.isArray(parsed.staged) ? parsed.staged.filter((item) => typeof item === 'string') : [],
    };
  } catch {
    return null;
  }
}

async function writeStore(context: FileSnapshotContext, store: SnapshotStore): Promise<void> {
  await fs.mkdir(context.storageDir, { recursive: true });
  const targetPath = snapshotPath(context, store.workspace);
  const tmpPath = `${targetPath}.${process.pid}.${Date.now()}.tmp`;
  await fs.writeFile(tmpPath, JSON.stringify(store, null, 2), 'utf8');
  await fs.rename(tmpPath, targetPath);
}

async function ensureStore(context: FileSnapshotContext, workspace: string): Promise<SnapshotStore> {
  const existing = await readStore(context, workspace);
  if (existing) return existing;
  const store: SnapshotStore = {
    workspace,
    created_at: Date.now(),
    files: await captureWorkspace(workspace),
    staged: [],
  };
  await writeStore(context, store);
  return store;
}

function fileChange(root: string, relativePath: string, operation: FileChangeOperation): RawFileChange {
  return {
    file_path: path.join(root, relativePath),
    relative_path: relativePath,
    operation,
  };
}

async function compareSnapshot(body: Record<string, unknown>, context: FileSnapshotContext): Promise<RawCompareResult> {
  const workspace = workspaceRoot(body);
  const store = await ensureStore(context, workspace);
  const current = await captureWorkspace(workspace);
  const changes: RawFileChange[] = [];
  const keys = new Set([...Object.keys(store.files), ...Object.keys(current)]);

  for (const key of Array.from(keys).sort((left, right) =>
    left.localeCompare(right, undefined, { sensitivity: 'base' })
  )) {
    const before = store.files[key];
    const after = current[key];
    if (!before && after) {
      changes.push(fileChange(workspace, key, 'create'));
    } else if (before && !after) {
      changes.push(fileChange(workspace, key, 'delete'));
    } else if (before && after && before.content_base64 !== after.content_base64) {
      changes.push(fileChange(workspace, key, 'modify'));
    }
  }

  const stagedKeys = new Set(store.staged);
  return {
    staged: changes.filter((change) => stagedKeys.has(change.relative_path)),
    unstaged: changes.filter((change) => !stagedKeys.has(change.relative_path)),
  };
}

async function initSnapshot(
  body: Record<string, unknown>,
  context: FileSnapshotContext
): Promise<{ mode: 'snapshot'; branch: null }> {
  const workspace = workspaceRoot(body);
  const store: SnapshotStore = {
    workspace,
    created_at: Date.now(),
    files: await captureWorkspace(workspace),
    staged: [],
  };
  await writeStore(context, store);
  return { mode: 'snapshot', branch: null };
}

async function getInfo(
  body: Record<string, unknown>,
  context: FileSnapshotContext
): Promise<{ mode: 'snapshot'; branch: null }> {
  await ensureStore(context, workspaceRoot(body));
  return { mode: 'snapshot', branch: null };
}

async function getBaselineContent(body: Record<string, unknown>, context: FileSnapshotContext): Promise<string | null> {
  const workspace = workspaceRoot(body);
  const filePath = bodyString(body, 'file_path');
  const absolute = resolveWorkspaceFile(workspace, filePath);
  const key = relativeKey(workspace, absolute);
  const store = await ensureStore(context, workspace);
  const record = store.files[key];
  if (!record) return null;
  return Buffer.from(record.content_base64, 'base64').toString('utf8');
}

async function disposeSnapshot(body: Record<string, unknown>, context: FileSnapshotContext): Promise<void> {
  const workspace = workspaceRoot(body);
  await fs.rm(snapshotPath(context, workspace), { force: true });
}

function updateStageSet(store: SnapshotStore, filePath: string, stage: boolean): void {
  const key = relativeKey(store.workspace, resolveWorkspaceFile(store.workspace, filePath));
  const staged = new Set(store.staged);
  if (stage) staged.add(key);
  else staged.delete(key);
  store.staged = Array.from(staged).sort();
}

async function stageFile(body: Record<string, unknown>, context: FileSnapshotContext, stage: boolean): Promise<void> {
  const workspace = workspaceRoot(body);
  const store = await ensureStore(context, workspace);
  updateStageSet(store, bodyString(body, 'file_path'), stage);
  await writeStore(context, store);
}

async function stageAll(body: Record<string, unknown>, context: FileSnapshotContext, stage: boolean): Promise<void> {
  const workspace = workspaceRoot(body);
  const store = await ensureStore(context, workspace);
  if (stage) {
    const compare = await compareSnapshot(body, context);
    store.staged = [...compare.staged, ...compare.unstaged].map((change) => change.relative_path).sort();
  } else {
    store.staged = [];
  }
  await writeStore(context, store);
}

async function restoreBaseline(body: Record<string, unknown>, context: FileSnapshotContext): Promise<void> {
  const workspace = workspaceRoot(body);
  const filePath = bodyString(body, 'file_path');
  const operation = bodyString(body, 'operation') as FileChangeOperation;
  const absolute = resolveWorkspaceFile(workspace, filePath);
  const key = relativeKey(workspace, absolute);
  const store = await ensureStore(context, workspace);

  if (operation === 'create') {
    await fs.rm(absolute, { recursive: true, force: true });
  } else {
    const baseline = store.files[key];
    if (!baseline) return;
    await fs.mkdir(path.dirname(absolute), { recursive: true });
    await fs.writeFile(absolute, Buffer.from(baseline.content_base64, 'base64'));
  }

  updateStageSet(store, key, false);
  await writeStore(context, store);
}

async function branches(): Promise<string[]> {
  return [];
}

export async function handleFileSnapshotRoute(
  pathname: string,
  body: Record<string, unknown>,
  context: FileSnapshotContext
): Promise<unknown> {
  switch (pathname) {
    case '/api/fs/snapshot/init':
      return initSnapshot(body, context);
    case '/api/fs/snapshot/compare':
      return compareSnapshot(body, context);
    case '/api/fs/snapshot/baseline':
      return getBaselineContent(body, context);
    case '/api/fs/snapshot/info':
      return getInfo(body, context);
    case '/api/fs/snapshot/dispose':
      return disposeSnapshot(body, context);
    case '/api/fs/snapshot/stage':
      return stageFile(body, context, true);
    case '/api/fs/snapshot/stage-all':
      return stageAll(body, context, true);
    case '/api/fs/snapshot/unstage':
      return stageFile(body, context, false);
    case '/api/fs/snapshot/unstage-all':
      return stageAll(body, context, false);
    case '/api/fs/snapshot/discard':
    case '/api/fs/snapshot/reset':
      return restoreBaseline(body, context);
    case '/api/fs/snapshot/branches':
      return branches();
    default:
      throw new FileSnapshotRouteError(404, 'FILE_SNAPSHOT_ROUTE_NOT_FOUND', 'desktop file snapshot route not found');
  }
}
