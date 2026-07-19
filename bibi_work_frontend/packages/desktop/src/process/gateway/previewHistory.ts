/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { createHash, randomUUID } from 'crypto';
import fs from 'fs/promises';
import path from 'path';

type PreviewContentType = 'markdown' | 'diff' | 'code' | 'html' | 'pdf' | 'ppt' | 'word' | 'excel' | 'image' | 'url';

type PreviewHistoryTarget = {
  content_type?: PreviewContentType;
  contentType?: PreviewContentType;
  file_path?: string;
  workspace?: string;
  file_name?: string;
  title?: string;
  language?: string;
  conversation_id?: string;
};

type PreviewSnapshotInfo = {
  id: string;
  label: string;
  created_at: number;
  size: number;
  contentType: PreviewContentType;
  file_name?: string;
  file_path?: string;
};

type PreviewSnapshotRecord = PreviewSnapshotInfo & {
  content: string;
};

type PreviewHistoryStore = {
  target: PreviewHistoryTarget;
  snapshots: PreviewSnapshotRecord[];
};

export type PreviewHistoryContext = {
  storageDir: string;
};

export class PreviewHistoryRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'PreviewHistoryRouteError';
  }
}

function asRecord(value: unknown, key: string): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new PreviewHistoryRouteError(400, 'INVALID_INPUT', `${key} must be an object`);
  }
  return value as Record<string, unknown>;
}

function optionalString(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === 'string' && value ? value : undefined;
}

function requiredString(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  if (typeof value !== 'string') {
    throw new PreviewHistoryRouteError(400, 'INVALID_INPUT', `${key} must be a string`);
  }
  return value;
}

function readTarget(body: Record<string, unknown>): PreviewHistoryTarget {
  const targetRecord = asRecord(body.target, 'target');
  const contentType = optionalString(targetRecord, 'content_type') ?? optionalString(targetRecord, 'contentType');
  if (!contentType) {
    throw new PreviewHistoryRouteError(400, 'INVALID_INPUT', 'target.content_type is required');
  }
  return {
    contentType: contentType as PreviewContentType,
    file_path: optionalString(targetRecord, 'file_path'),
    workspace: optionalString(targetRecord, 'workspace'),
    file_name: optionalString(targetRecord, 'file_name'),
    title: optionalString(targetRecord, 'title'),
    language: optionalString(targetRecord, 'language'),
    conversation_id: optionalString(targetRecord, 'conversation_id'),
  };
}

function stableTargetKey(target: PreviewHistoryTarget): string {
  const normalized = {
    contentType: target.contentType,
    file_path: target.file_path ?? '',
    workspace: target.workspace ?? '',
    file_name: target.file_name ?? '',
    title: target.title ?? '',
    language: target.language ?? '',
    conversation_id: target.conversation_id ?? '',
  };
  return createHash('sha256').update(JSON.stringify(normalized)).digest('hex').slice(0, 32);
}

function storePath(context: PreviewHistoryContext, target: PreviewHistoryTarget): string {
  return path.join(context.storageDir, `${stableTargetKey(target)}.json`);
}

async function readStore(context: PreviewHistoryContext, target: PreviewHistoryTarget): Promise<PreviewHistoryStore> {
  try {
    const raw = await fs.readFile(storePath(context, target), 'utf8');
    const parsed = JSON.parse(raw) as PreviewHistoryStore;
    return {
      target,
      snapshots: Array.isArray(parsed.snapshots) ? parsed.snapshots : [],
    };
  } catch {
    return { target, snapshots: [] };
  }
}

async function writeStore(
  context: PreviewHistoryContext,
  target: PreviewHistoryTarget,
  store: PreviewHistoryStore
): Promise<void> {
  await fs.mkdir(context.storageDir, { recursive: true });
  const targetPath = storePath(context, target);
  const tmpPath = `${targetPath}.${process.pid}.${Date.now()}.tmp`;
  await fs.writeFile(tmpPath, JSON.stringify(store, null, 2), 'utf8');
  await fs.rename(tmpPath, targetPath);
}

function snapshotInfo(record: PreviewSnapshotRecord): PreviewSnapshotInfo {
  const { content: _content, ...info } = record;
  return info;
}

async function listSnapshots(
  body: Record<string, unknown>,
  context: PreviewHistoryContext
): Promise<PreviewSnapshotInfo[]> {
  const target = readTarget(body);
  const store = await readStore(context, target);
  return store.snapshots
    .slice()
    .sort((left, right) => right.created_at - left.created_at)
    .map(snapshotInfo);
}

async function saveSnapshot(
  body: Record<string, unknown>,
  context: PreviewHistoryContext
): Promise<PreviewSnapshotInfo> {
  const target = readTarget(body);
  const content = requiredString(body, 'content');
  const now = Date.now();
  const record: PreviewSnapshotRecord = {
    id: randomUUID(),
    label: new Date(now).toISOString(),
    created_at: now,
    size: Buffer.byteLength(content, 'utf8'),
    contentType: target.contentType as PreviewContentType,
    file_name: target.file_name,
    file_path: target.file_path,
    content,
  };

  const store = await readStore(context, target);
  store.snapshots = [record, ...store.snapshots].slice(0, 100);
  await writeStore(context, target, store);
  return snapshotInfo(record);
}

async function getSnapshotContent(
  body: Record<string, unknown>,
  context: PreviewHistoryContext
): Promise<{ snapshot: PreviewSnapshotInfo; content: string } | null> {
  const target = readTarget(body);
  const snapshotId = requiredString(body, 'snapshot_id');
  const store = await readStore(context, target);
  const record = store.snapshots.find((snapshot) => snapshot.id === snapshotId);
  if (!record) return null;
  return { snapshot: snapshotInfo(record), content: record.content };
}

export async function handlePreviewHistoryRoute(
  pathname: string,
  body: Record<string, unknown>,
  context: PreviewHistoryContext
): Promise<unknown> {
  switch (pathname) {
    case '/api/preview-history/list':
      return listSnapshots(body, context);
    case '/api/preview-history/save':
      return saveSnapshot(body, context);
    case '/api/preview-history/get-content':
      return getSnapshotContent(body, context);
    default:
      throw new PreviewHistoryRouteError(
        404,
        'PREVIEW_HISTORY_ROUTE_NOT_FOUND',
        'desktop preview history route not found'
      );
  }
}
