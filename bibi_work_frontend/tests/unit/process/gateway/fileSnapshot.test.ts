/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { handleFileSnapshotRoute, FileSnapshotRouteError } from '@process/gateway/fileSnapshot';

let tempRoot = '';
let workspace = '';
let storageDir = '';

async function route(pathname: string, body: Record<string, unknown>): Promise<unknown> {
  return handleFileSnapshotRoute(pathname, { workspace, ...body }, { storageDir });
}

beforeEach(async () => {
  tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-file-snapshot-test-'));
  workspace = path.join(tempRoot, 'workspace');
  storageDir = path.join(tempRoot, 'snapshots');
  await fs.mkdir(path.join(workspace, 'src'), { recursive: true });
  await fs.writeFile(path.join(workspace, 'src', 'app.ts'), 'old\n');
});

afterEach(async () => {
  if (tempRoot) {
    await fs.rm(tempRoot, { recursive: true, force: true });
  }
});

describe('desktop file snapshot routes', () => {
  it('captures a baseline and compares create/modify/delete changes', async () => {
    await fs.writeFile(path.join(workspace, 'delete-me.txt'), 'delete\n');
    await expect(route('/api/fs/snapshot/init', {})).resolves.toEqual({ mode: 'snapshot', branch: null });

    await fs.writeFile(path.join(workspace, 'src', 'app.ts'), 'new\n');
    await fs.writeFile(path.join(workspace, 'created.txt'), 'created\n');
    await fs.rm(path.join(workspace, 'delete-me.txt'));

    const compare = (await route('/api/fs/snapshot/compare', {})) as {
      staged: unknown[];
      unstaged: Array<{ relative_path: string; operation: string }>;
    };

    expect(compare.staged).toEqual([]);
    expect(compare.unstaged).toEqual([
      { file_path: path.join(workspace, 'created.txt'), relative_path: 'created.txt', operation: 'create' },
      { file_path: path.join(workspace, 'delete-me.txt'), relative_path: 'delete-me.txt', operation: 'delete' },
      { file_path: path.join(workspace, 'src', 'app.ts'), relative_path: 'src/app.ts', operation: 'modify' },
    ]);
  });

  it('returns baseline content and can restore modified files', async () => {
    await route('/api/fs/snapshot/init', {});
    await fs.writeFile(path.join(workspace, 'src', 'app.ts'), 'new\n');

    await expect(route('/api/fs/snapshot/baseline', { file_path: 'src/app.ts' })).resolves.toBe('old\n');
    await route('/api/fs/snapshot/reset', { file_path: 'src/app.ts', operation: 'modify' });

    await expect(fs.readFile(path.join(workspace, 'src', 'app.ts'), 'utf8')).resolves.toBe('old\n');
  });

  it('removes newly-created files on reset', async () => {
    await route('/api/fs/snapshot/init', {});
    const created = path.join(workspace, 'created.txt');
    await fs.writeFile(created, 'created\n');

    await route('/api/fs/snapshot/reset', { file_path: 'created.txt', operation: 'create' });

    await expect(fs.stat(created)).rejects.toThrow();
  });

  it('tracks staged files separately when stage routes are used', async () => {
    await route('/api/fs/snapshot/init', {});
    await fs.writeFile(path.join(workspace, 'src', 'app.ts'), 'new\n');

    await route('/api/fs/snapshot/stage', { file_path: 'src/app.ts' });

    const compare = (await route('/api/fs/snapshot/compare', {})) as {
      staged: Array<{ relative_path: string }>;
      unstaged: unknown[];
    };
    expect(compare.staged.map((change) => change.relative_path)).toEqual(['src/app.ts']);
    expect(compare.unstaged).toEqual([]);
  });

  it('rejects snapshot file paths outside the workspace', async () => {
    await route('/api/fs/snapshot/init', {});

    await expect(route('/api/fs/snapshot/baseline', { file_path: '../outside.txt' })).rejects.toBeInstanceOf(
      FileSnapshotRouteError
    );
  });
});
