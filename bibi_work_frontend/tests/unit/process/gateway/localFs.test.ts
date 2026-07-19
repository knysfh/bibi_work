/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { handleLocalFsRoute, LocalFsRouteError } from '@process/gateway/localFs';

let tempRoot = '';

async function route(pathname: string, body: Record<string, unknown>): Promise<unknown> {
  return handleLocalFsRoute(pathname, body, { tempDir: path.join(tempRoot, 'tmp') });
}

beforeEach(async () => {
  tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-local-fs-test-'));
});

afterEach(async () => {
  if (tempRoot) {
    await fs.rm(tempRoot, { recursive: true, force: true });
  }
});

describe('desktop gateway local fs routes', () => {
  it('lists workspace files in the raw backend shape', async () => {
    await fs.mkdir(path.join(tempRoot, 'workspace', 'src'), { recursive: true });
    await fs.writeFile(path.join(tempRoot, 'workspace', 'src', 'main.ts'), 'console.log(1);');

    const files = (await route('/api/fs/list', { root: path.join(tempRoot, 'workspace') })) as Array<{
      name: string;
      full_path: string;
      relative_path: string;
    }>;

    expect(files).toEqual([
      {
        name: 'main.ts',
        full_path: path.join(tempRoot, 'workspace', 'src', 'main.ts'),
        relative_path: path.join('src', 'main.ts'),
      },
    ]);
  });

  it('reads, writes and reports metadata for local files', async () => {
    const target = path.join(tempRoot, 'workspace', 'notes', 'a.md');

    await expect(route('/api/fs/write', { path: target, data: '  # hello\n' })).resolves.toBe(true);
    await expect(route('/api/fs/read', { path: target })).resolves.toBe('  # hello\n');

    const metadata = (await route('/api/fs/metadata', { path: target })) as {
      name: string;
      size: number;
      isDirectory?: boolean;
    };
    expect(metadata.name).toBe('a.md');
    expect(metadata.size).toBe(10);
    expect(metadata.isDirectory).toBe(false);

    await expect(route('/api/fs/write', { path: target, data: '' })).resolves.toBe(true);
    await expect(route('/api/fs/read', { path: target })).resolves.toBe('');
  });

  it('returns image data urls', async () => {
    const target = path.join(tempRoot, 'pixel.png');
    await fs.writeFile(target, Buffer.from([137, 80, 78, 71]));

    await expect(route('/api/fs/image-base64', { path: target })).resolves.toBe('data:image/png;base64,iVBORw==');
  });

  it('copies, renames and removes entries inside a workspace', async () => {
    const source = path.join(tempRoot, 'source.txt');
    const workspace = path.join(tempRoot, 'workspace');
    await fs.writeFile(source, 'copy me');

    const copyResult = (await route('/api/fs/copy', { file_paths: [source], workspace })) as { copied_files: string[] };
    expect(copyResult.copied_files).toHaveLength(1);
    expect(await fs.readFile(copyResult.copied_files[0], 'utf8')).toBe('copy me');

    const renameResult = (await route('/api/fs/rename', {
      path: copyResult.copied_files[0],
      new_name: '../renamed.txt',
    })) as { new_path: string };
    expect(renameResult.new_path).toBe(path.join(workspace, 'renamed.txt'));
    expect(await fs.readFile(renameResult.new_path, 'utf8')).toBe('copy me');

    await route('/api/fs/remove', { path: renameResult.new_path });
    await expect(fs.stat(renameResult.new_path)).rejects.toThrow();
  });

  it('creates a stored zip archive for export payloads', async () => {
    const zipPath = path.join(tempRoot, 'exports', 'conversation.zip');

    await expect(
      route('/api/fs/zip', {
        path: zipPath,
        files: [
          { name: 'conversation/conversation.md', content: '# Chat\n' },
          { name: '../safe/note.txt', content: 'note' },
        ],
      })
    ).resolves.toBe(true);

    const archive = await fs.readFile(zipPath);
    expect(archive.readUInt32LE(0)).toBe(0x04034b50);
    expect(archive.includes(Buffer.from('conversation/conversation.md'))).toBe(true);
    expect(archive.includes(Buffer.from('safe/note.txt'))).toBe(true);
    expect(archive.readUInt32LE(archive.length - 22)).toBe(0x06054b50);
  });

  it('acknowledges watch routes without proxying them to Rust placeholders', async () => {
    await expect(route('/api/fs/watch/start', { file_path: path.join(tempRoot, 'a.txt') })).resolves.toBeNull();
    await expect(route('/api/fs/office-watch/start', { workspace: tempRoot })).resolves.toBeNull();
    await expect(route('/api/fs/zip/cancel', { request_id: 'export-1' })).resolves.toBeNull();
  });

  it('rejects unknown fs routes', async () => {
    await expect(route('/api/fs/unknown', {})).rejects.toBeInstanceOf(LocalFsRouteError);
  });
});
