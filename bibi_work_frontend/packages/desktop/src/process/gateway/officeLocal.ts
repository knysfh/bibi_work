/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { execFile } from 'child_process';
import fs from 'fs/promises';
import path from 'path';
import { promisify } from 'util';

const execFileAsync = promisify(execFile);

type DocumentConversionTarget = 'markdown' | 'excel-json' | 'ppt-json';
type OfficeDocType = 'ppt' | 'word' | 'excel';

export type OfficeLocalContext = {
  platform: NodeJS.Platform;
  officeCliAvailable?: boolean | (() => Promise<boolean>);
  allowedRoots?: string[];
};

export class OfficeLocalRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'OfficeLocalRouteError';
  }
}

function bodyString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== 'string' || !value.trim()) {
    throw new OfficeLocalRouteError(400, 'INVALID_INPUT', `${key} is required`);
  }
  return value.trim();
}

function optionalBodyString(body: Record<string, unknown>, key: string): string | undefined {
  const value = body[key];
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

function resolveLocalPath(inputPath: string, workspace?: string): string {
  if (path.isAbsolute(inputPath)) {
    return path.resolve(inputPath);
  }
  if (workspace) {
    return path.resolve(workspace, inputPath);
  }
  return path.resolve(inputPath);
}

function isInsideRoot(filePath: string, root: string): boolean {
  const relative = path.relative(path.resolve(root), path.resolve(filePath));
  return relative === '' || (!!relative && !relative.startsWith('..') && !path.isAbsolute(relative));
}

function assertAllowedLocalPath(filePath: string, workspace: string | undefined, context: OfficeLocalContext): void {
  if (workspace) {
    if (isInsideRoot(filePath, workspace)) return;
    throw new OfficeLocalRouteError(403, 'PATH_OUTSIDE_SANDBOX', 'file_path must be inside workspace');
  }

  const allowedRoots = context.allowedRoots ?? [];
  if (allowedRoots.some((root) => isInsideRoot(filePath, root))) return;
  throw new OfficeLocalRouteError(403, 'PATH_OUTSIDE_SANDBOX', 'file_path is outside the local preview sandbox');
}

async function ensureReadableFile(filePath: string): Promise<void> {
  const stats = await fs.stat(filePath);
  if (!stats.isFile()) {
    throw new OfficeLocalRouteError(400, 'INVALID_INPUT', 'file_path must point to a file');
  }
}

async function convertDocument(body: Record<string, unknown>, context: OfficeLocalContext): Promise<unknown> {
  const target = bodyString(body, 'to') as DocumentConversionTarget;
  const workspace = optionalBodyString(body, 'workspace');
  const filePath = resolveLocalPath(bodyString(body, 'file_path'), workspace);
  assertAllowedLocalPath(filePath, workspace, context);
  await ensureReadableFile(filePath);

  if (target === 'markdown') {
    const ext = path.extname(filePath).toLowerCase();
    if (ext === '.md' || ext === '.markdown' || ext === '.txt') {
      return {
        to: 'markdown',
        result: {
          success: true,
          data: await fs.readFile(filePath, 'utf8'),
        },
      };
    }
    return {
      to: 'markdown',
      result: {
        success: false,
        error: 'UNSUPPORTED_DOCUMENT_CONVERSION',
      },
    };
  }

  if (target === 'excel-json') {
    return {
      to: 'excel-json',
      result: {
        success: false,
        error: 'UNSUPPORTED_DOCUMENT_CONVERSION',
      },
    };
  }

  if (target === 'ppt-json') {
    return {
      to: 'ppt-json',
      result: {
        success: false,
        error: 'UNSUPPORTED_DOCUMENT_CONVERSION',
      },
    };
  }

  throw new OfficeLocalRouteError(400, 'INVALID_INPUT', `unsupported conversion target: ${target}`);
}

async function hasOfficeCli(context: OfficeLocalContext): Promise<boolean> {
  if (typeof context.officeCliAvailable === 'boolean') {
    return context.officeCliAvailable;
  }
  if (typeof context.officeCliAvailable === 'function') {
    return context.officeCliAvailable();
  }
  try {
    await execFileAsync(context.platform === 'win32' ? 'where' : 'which', ['officecli'], { timeout: 3000 });
    return true;
  } catch {
    return false;
  }
}

function docTypeForPathname(pathname: string): OfficeDocType | null {
  if (pathname.startsWith('/api/ppt-preview/')) return 'ppt';
  if (pathname.startsWith('/api/word-preview/')) return 'word';
  if (pathname.startsWith('/api/excel-preview/')) return 'excel';
  return null;
}

async function startOfficePreview(
  pathname: string,
  body: Record<string, unknown>,
  context: OfficeLocalContext
): Promise<{ url: string; error?: string } | { url?: string; error: string }> {
  const docType = docTypeForPathname(pathname);
  if (!docType) {
    throw new OfficeLocalRouteError(404, 'OFFICE_ROUTE_NOT_FOUND', 'desktop office route not found');
  }

  const workspace = optionalBodyString(body, 'workspace');
  const filePath = resolveLocalPath(bodyString(body, 'file_path'), workspace);
  assertAllowedLocalPath(filePath, workspace, context);
  await ensureReadableFile(filePath);

  if (!(await hasOfficeCli(context))) {
    return { error: 'OFFICECLI_NOT_FOUND' };
  }

  // The desktop gateway owns this route, but the OfficeCLI watch-server
  // process contract is intentionally not guessed here. Returning a structured
  // failure keeps the renderer on its existing install/retry path instead of
  // proxying to Rust's generic local-runtime placeholder.
  return { error: 'OFFICECLI_START_FAILED' };
}

function stopOfficePreview(): null {
  return null;
}

export async function handleOfficeLocalRoute(
  pathname: string,
  body: Record<string, unknown>,
  context: OfficeLocalContext
): Promise<unknown> {
  if (pathname === '/api/document/convert') {
    return convertDocument(body, context);
  }
  if (pathname.endsWith('/start')) {
    return startOfficePreview(pathname, body, context);
  }
  if (pathname.endsWith('/stop')) {
    return stopOfficePreview();
  }
  throw new OfficeLocalRouteError(404, 'OFFICE_ROUTE_NOT_FOUND', 'desktop office route not found');
}
