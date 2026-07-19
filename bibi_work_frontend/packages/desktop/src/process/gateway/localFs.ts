/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import path from 'path';

function stripAsciiControlCharacters(value: string): string {
  return [...value]
    .filter((character) => {
      const code = character.charCodeAt(0);
      return code >= 0x20 && code !== 0x7f;
    })
    .join('');
}

export type LocalFsRouteContext = {
  tempDir: string;
};

export class LocalFsRouteError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'LocalFsRouteError';
  }
}

type DirOrFile = {
  name: string;
  fullPath: string;
  relativePath: string;
  isDir: boolean;
  isFile: boolean;
  children?: DirOrFile[];
};

type WorkspaceFlatFile = {
  name: string;
  full_path: string;
  relative_path: string;
};

type FileMetadata = {
  name: string;
  path: string;
  size: number;
  type: string;
  lastModified: number;
  isDirectory?: boolean;
};

type ZipFileInput = {
  name?: unknown;
  content?: unknown;
  source_path?: unknown;
};

const TEXT_DECODER = new TextDecoder('utf-8', { fatal: false });
const IGNORED_DIRS = new Set(['.git', 'node_modules']);

const MIME_BY_EXTENSION: Record<string, string> = {
  '.bmp': 'image/bmp',
  '.gif': 'image/gif',
  '.jpeg': 'image/jpeg',
  '.jpg': 'image/jpeg',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.webp': 'image/webp',
  '.pdf': 'application/pdf',
  '.txt': 'text/plain; charset=utf-8',
  '.md': 'text/markdown; charset=utf-8',
  '.json': 'application/json',
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.ts': 'text/typescript; charset=utf-8',
};

function bodyString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== 'string' || !value.trim()) {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', `${key} is required`);
  }
  return value.trim();
}

function optionalBodyString(body: Record<string, unknown>, key: string): string | undefined {
  const value = body[key];
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

function bodyRawString(body: Record<string, unknown>, key: string): string {
  const value = body[key];
  if (typeof value !== 'string') {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', `${key} must be a string`);
  }
  return value;
}

function bodyStringArray(body: Record<string, unknown>, key: string): string[] {
  const value = body[key];
  if (!Array.isArray(value) || value.some((item) => typeof item !== 'string' || !item.trim())) {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', `${key} must be a string array`);
  }
  return value.map((item) => String(item).trim());
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

function relativePath(root: string, filePath: string): string {
  const rel = path.relative(root, filePath);
  return rel || '';
}

function sortDirEntries(left: DirOrFile, right: DirOrFile): number {
  if (left.isDir !== right.isDir) return left.isDir ? -1 : 1;
  return left.name.localeCompare(right.name, undefined, { sensitivity: 'base' });
}

async function toDirOrFile(filePath: string, root: string, maxDepth: number): Promise<DirOrFile | null> {
  let stats;
  try {
    stats = await fs.stat(filePath);
  } catch {
    return null;
  }

  const node: DirOrFile = {
    name: path.basename(filePath),
    fullPath: filePath,
    relativePath: relativePath(root, filePath),
    isDir: stats.isDirectory(),
    isFile: stats.isFile(),
  };

  if (!stats.isDirectory()) return node;
  node.children = [];
  if (maxDepth <= 0) return node;

  let entries;
  try {
    entries = await fs.readdir(filePath, { withFileTypes: true });
  } catch {
    return node;
  }

  for (const entry of entries) {
    if (entry.isDirectory() && IGNORED_DIRS.has(entry.name)) continue;
    const child = await toDirOrFile(path.join(filePath, entry.name), root, maxDepth - 1);
    if (child) node.children.push(child);
  }
  node.children.sort(sortDirEntries);
  return node;
}

async function listWorkspaceTree(body: Record<string, unknown>): Promise<DirOrFile[]> {
  const root = resolveLocalPath(bodyString(body, 'root'));
  const dir = resolveLocalPath(bodyString(body, 'dir'), root);
  const node = await toDirOrFile(dir, root, 2);
  return node ? [node] : [];
}

async function collectWorkspaceFiles(
  root: string,
  current: string,
  output: WorkspaceFlatFile[],
  limit: number
): Promise<void> {
  if (output.length >= limit) return;
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
      await collectWorkspaceFiles(root, fullPath, output, limit);
      continue;
    }
    if (!entry.isFile()) continue;
    output.push({
      name: entry.name,
      full_path: fullPath,
      relative_path: relativePath(root, fullPath),
    });
    if (output.length >= limit) return;
  }
}

async function listWorkspaceFiles(body: Record<string, unknown>): Promise<WorkspaceFlatFile[]> {
  const root = resolveLocalPath(bodyString(body, 'root'));
  const output: WorkspaceFlatFile[] = [];
  await collectWorkspaceFiles(root, root, output, 10000);
  output.sort((left, right) =>
    left.relative_path.localeCompare(right.relative_path, undefined, { sensitivity: 'base' })
  );
  return output;
}

async function readFileText(body: Record<string, unknown>): Promise<string | null> {
  const filePath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  try {
    const data = await fs.readFile(filePath);
    return TEXT_DECODER.decode(data);
  } catch {
    return null;
  }
}

function mimeForPath(filePath: string): string {
  return MIME_BY_EXTENSION[path.extname(filePath).toLowerCase()] ?? 'application/octet-stream';
}

async function readFileDataUrl(body: Record<string, unknown>): Promise<string | null> {
  const filePath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  try {
    const data = await fs.readFile(filePath);
    return `data:${mimeForPath(filePath)};base64,${data.toString('base64')}`;
  } catch {
    return null;
  }
}

async function readFileBufferBase64(body: Record<string, unknown>): Promise<string | null> {
  const filePath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  try {
    return (await fs.readFile(filePath)).toString('base64');
  } catch {
    return null;
  }
}

async function writeFileText(body: Record<string, unknown>): Promise<boolean> {
  const filePath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  const data = bodyRawString(body, 'data');
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, data, 'utf8');
  return true;
}

async function fileMetadata(body: Record<string, unknown>): Promise<FileMetadata> {
  const filePath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  const stats = await fs.stat(filePath);
  return {
    name: path.basename(filePath),
    path: filePath,
    size: stats.size,
    type: stats.isDirectory() ? 'directory' : mimeForPath(filePath),
    lastModified: stats.mtimeMs,
    isDirectory: stats.isDirectory(),
  };
}

function safeBasename(rawName: string): string {
  const name = stripAsciiControlCharacters(path.basename(rawName.replace(/\\/g, '/'))).trim();
  if (!name || name === '.' || name === '..') {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', 'file name is invalid');
  }
  return name;
}

async function uniqueDestination(dir: string, fileName: string): Promise<string> {
  const parsed = path.parse(fileName);
  for (let attempt = 0; attempt < 100; attempt++) {
    const suffix = attempt === 0 ? '' : `-${attempt}`;
    const candidate = path.join(dir, `${parsed.name}${suffix}${parsed.ext}`);
    try {
      await fs.access(candidate);
    } catch {
      return candidate;
    }
  }
  return path.join(dir, `${parsed.name}-${Date.now().toString(36)}${parsed.ext}`);
}

async function createTempFile(body: Record<string, unknown>, context: LocalFsRouteContext): Promise<string> {
  const fileName = safeBasename(bodyString(body, 'file_name'));
  const dir = path.join(context.tempDir, 'biwork-temp-files');
  await fs.mkdir(dir, { recursive: true });
  const target = await uniqueDestination(dir, fileName);
  await fs.writeFile(target, '');
  return target;
}

async function copyEntry(sourcePath: string, destinationDir: string): Promise<string> {
  const stats = await fs.stat(sourcePath);
  const destination = await uniqueDestination(destinationDir, path.basename(sourcePath));
  if (stats.isDirectory()) {
    await fs.cp(sourcePath, destination, { recursive: true, errorOnExist: true });
  } else {
    await fs.copyFile(sourcePath, destination);
  }
  return destination;
}

async function copyFilesToWorkspace(
  body: Record<string, unknown>
): Promise<{ copied_files: string[]; failed_files?: Array<{ path: string; error: string }> }> {
  const filePaths = bodyStringArray(body, 'file_paths');
  const workspace = resolveLocalPath(bodyString(body, 'workspace'));
  await fs.mkdir(workspace, { recursive: true });

  const copied_files: string[] = [];
  const failed_files: Array<{ path: string; error: string }> = [];
  for (const source of filePaths) {
    const sourcePath = resolveLocalPath(source, optionalBodyString(body, 'source_root'));
    try {
      copied_files.push(await copyEntry(sourcePath, workspace));
    } catch (error) {
      failed_files.push({ path: source, error: error instanceof Error ? error.message : String(error) });
    }
  }
  return failed_files.length ? { copied_files, failed_files } : { copied_files };
}

async function removeEntry(body: Record<string, unknown>): Promise<void> {
  const targetPath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  await fs.rm(targetPath, { recursive: true, force: true });
}

async function renameEntry(body: Record<string, unknown>): Promise<{ new_path: string }> {
  const sourcePath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  const newName = safeBasename(bodyString(body, 'new_name'));
  const newPath = path.join(path.dirname(sourcePath), newName);
  await fs.rename(sourcePath, newPath);
  return { new_path: newPath };
}

const CRC32_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i++) {
    let value = i;
    for (let bit = 0; bit < 8; bit++) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table[i] = value >>> 0;
  }
  return table;
})();

function crc32(data: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc = CRC32_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function dosDateTime(date: Date): { time: number; date: number } {
  const year = Math.max(1980, date.getFullYear());
  return {
    time: (date.getHours() << 11) | (date.getMinutes() << 5) | Math.floor(date.getSeconds() / 2),
    date: ((year - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate(),
  };
}

function writeUInt32(value: number): Buffer {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32LE(value >>> 0, 0);
  return buffer;
}

function writeUInt16(value: number): Buffer {
  const buffer = Buffer.alloc(2);
  buffer.writeUInt16LE(value & 0xffff, 0);
  return buffer;
}

function zipPath(rawName: string): string {
  const name = rawName.replace(/\\/g, '/').replace(/^\/+/, '');
  const safeParts = name
    .split('/')
    .filter((part) => part && part !== '.' && part !== '..')
    .map((part) => stripAsciiControlCharacters(part).trim())
    .filter(Boolean);
  if (safeParts.length === 0) {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', 'zip file name is invalid');
  }
  return safeParts.join('/');
}

function contentToBuffer(content: unknown): Buffer {
  if (typeof content === 'string') return Buffer.from(content, 'utf8');
  if (Array.isArray(content) && content.every((item) => Number.isInteger(item) && item >= 0 && item <= 255)) {
    return Buffer.from(content as number[]);
  }
  if (content && typeof content === 'object') {
    const entries = Object.entries(content as Record<string, unknown>);
    if (
      entries.every(
        ([key, value]) =>
          /^\d+$/.test(key) && Number.isInteger(value) && (value as number) >= 0 && (value as number) <= 255
      )
    ) {
      return Buffer.from(
        entries.sort(([left], [right]) => Number(left) - Number(right)).map(([, value]) => value as number)
      );
    }
  }
  throw new LocalFsRouteError(400, 'INVALID_INPUT', 'zip file content must be a string or byte array');
}

async function readZipEntryData(
  entry: ZipFileInput,
  sourceRoot: string | undefined
): Promise<{ name: string; data: Buffer }> {
  if (typeof entry.name !== 'string' || !entry.name.trim()) {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', 'zip file name is required');
  }
  const name = zipPath(entry.name);
  if (entry.content !== undefined) {
    return { name, data: contentToBuffer(entry.content) };
  }
  if (typeof entry.source_path === 'string' && entry.source_path.trim()) {
    const sourcePath = resolveLocalPath(entry.source_path.trim(), sourceRoot);
    return { name, data: await fs.readFile(sourcePath) };
  }
  throw new LocalFsRouteError(400, 'INVALID_INPUT', 'zip file requires content or source_path');
}

function buildStoredZip(entries: Array<{ name: string; data: Buffer }>): Buffer {
  const now = dosDateTime(new Date());
  const chunks: Buffer[] = [];
  const central: Buffer[] = [];
  let offset = 0;

  for (const entry of entries) {
    const name = Buffer.from(entry.name, 'utf8');
    const crc = crc32(entry.data);
    const localHeader = Buffer.concat([
      writeUInt32(0x04034b50),
      writeUInt16(20),
      writeUInt16(0),
      writeUInt16(0),
      writeUInt16(now.time),
      writeUInt16(now.date),
      writeUInt32(crc),
      writeUInt32(entry.data.length),
      writeUInt32(entry.data.length),
      writeUInt16(name.length),
      writeUInt16(0),
      name,
    ]);
    chunks.push(localHeader, entry.data);

    central.push(
      Buffer.concat([
        writeUInt32(0x02014b50),
        writeUInt16(20),
        writeUInt16(20),
        writeUInt16(0),
        writeUInt16(0),
        writeUInt16(now.time),
        writeUInt16(now.date),
        writeUInt32(crc),
        writeUInt32(entry.data.length),
        writeUInt32(entry.data.length),
        writeUInt16(name.length),
        writeUInt16(0),
        writeUInt16(0),
        writeUInt16(0),
        writeUInt16(0),
        writeUInt32(0),
        writeUInt32(offset),
        name,
      ])
    );
    offset += localHeader.length + entry.data.length;
  }

  const centralDirectory = Buffer.concat(central);
  const end = Buffer.concat([
    writeUInt32(0x06054b50),
    writeUInt16(0),
    writeUInt16(0),
    writeUInt16(entries.length),
    writeUInt16(entries.length),
    writeUInt32(centralDirectory.length),
    writeUInt32(offset),
    writeUInt16(0),
  ]);
  return Buffer.concat([...chunks, centralDirectory, end]);
}

async function createZip(body: Record<string, unknown>): Promise<boolean> {
  const targetPath = resolveLocalPath(bodyString(body, 'path'), optionalBodyString(body, 'workspace'));
  const files = body.files;
  if (!Array.isArray(files)) {
    throw new LocalFsRouteError(400, 'INVALID_INPUT', 'files must be an array');
  }
  const sourceRoot = optionalBodyString(body, 'source_root');
  const entries = await Promise.all(files.map((entry) => readZipEntryData(entry as ZipFileInput, sourceRoot)));
  await fs.mkdir(path.dirname(targetPath), { recursive: true });
  await fs.writeFile(targetPath, buildStoredZip(entries));
  return true;
}

function acknowledgeLocalWatchRoute(): null {
  return null;
}

async function fetchRemoteImage(body: Record<string, unknown>): Promise<string> {
  const url = bodyString(body, 'url');
  const response = await fetch(url);
  if (!response.ok) {
    throw new LocalFsRouteError(
      response.status,
      'REMOTE_IMAGE_FETCH_FAILED',
      `remote image fetch failed: ${response.status}`
    );
  }
  const contentType = response.headers.get('content-type') || 'application/octet-stream';
  const data = Buffer.from(await response.arrayBuffer());
  return `data:${contentType};base64,${data.toString('base64')}`;
}

export async function handleLocalFsRoute(
  pathname: string,
  body: Record<string, unknown>,
  context: LocalFsRouteContext
): Promise<unknown> {
  switch (pathname) {
    case '/api/fs/dir':
      return listWorkspaceTree(body);
    case '/api/fs/list':
      return listWorkspaceFiles(body);
    case '/api/fs/image-base64':
      return readFileDataUrl(body);
    case '/api/fs/fetch-remote-image':
      return fetchRemoteImage(body);
    case '/api/fs/read':
      return readFileText(body);
    case '/api/fs/read-buffer':
      return readFileBufferBase64(body);
    case '/api/fs/temp':
      return createTempFile(body, context);
    case '/api/fs/zip':
      return createZip(body);
    case '/api/fs/zip/cancel':
    case '/api/fs/watch/start':
    case '/api/fs/watch/stop':
    case '/api/fs/watch/stop-all':
    case '/api/fs/office-watch/start':
    case '/api/fs/office-watch/stop':
      return acknowledgeLocalWatchRoute();
    case '/api/fs/write':
      return writeFileText(body);
    case '/api/fs/metadata':
      return fileMetadata(body);
    case '/api/fs/copy':
      return copyFilesToWorkspace(body);
    case '/api/fs/remove':
      return removeEntry(body);
    case '/api/fs/rename':
      return renameEntry(body);
    default:
      throw new LocalFsRouteError(404, 'LOCAL_FS_ROUTE_NOT_FOUND', 'desktop local fs route not found');
  }
}
