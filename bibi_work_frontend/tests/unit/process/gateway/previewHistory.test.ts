/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { handlePreviewHistoryRoute, PreviewHistoryRouteError } from '@process/gateway/previewHistory';

let storageDir = '';

async function route(pathname: string, body: Record<string, unknown>): Promise<unknown> {
  return handlePreviewHistoryRoute(pathname, body, { storageDir });
}

const target = {
  content_type: 'markdown',
  file_path: '/workspace/readme.md',
  workspace: '/workspace',
  file_name: 'readme.md',
};

beforeEach(async () => {
  storageDir = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-preview-history-test-'));
});

afterEach(async () => {
  if (storageDir) {
    await fs.rm(storageDir, { recursive: true, force: true });
  }
});

describe('desktop preview-history routes', () => {
  it('saves, lists and reads snapshot content', async () => {
    const saved = (await route('/api/preview-history/save', { target, content: '# v1' })) as {
      id: string;
      size: number;
      contentType: string;
      file_name?: string;
    };

    expect(saved.id).toBeTruthy();
    expect(saved.size).toBe(4);
    expect(saved.contentType).toBe('markdown');
    expect(saved.file_name).toBe('readme.md');

    const listed = (await route('/api/preview-history/list', { target })) as Array<{ id: string }>;
    expect(listed.map((item) => item.id)).toEqual([saved.id]);

    const content = (await route('/api/preview-history/get-content', {
      target,
      snapshot_id: saved.id,
    })) as { snapshot: { id: string }; content: string };
    expect(content.snapshot.id).toBe(saved.id);
    expect(content.content).toBe('# v1');
  });

  it('keeps different targets isolated', async () => {
    const saved = (await route('/api/preview-history/save', { target, content: '# v1' })) as { id: string };
    const otherTarget = { ...target, file_path: '/workspace/other.md', file_name: 'other.md' };

    await expect(route('/api/preview-history/list', { target: otherTarget })).resolves.toEqual([]);
    await expect(
      route('/api/preview-history/get-content', { target: otherTarget, snapshot_id: saved.id })
    ).resolves.toBeNull();
  });

  it('sorts snapshots newest first', async () => {
    const first = (await route('/api/preview-history/save', { target, content: 'first' })) as { id: string };
    await new Promise((resolve) => setTimeout(resolve, 2));
    const second = (await route('/api/preview-history/save', { target, content: 'second' })) as { id: string };

    const listed = (await route('/api/preview-history/list', { target })) as Array<{ id: string }>;
    expect(listed.map((item) => item.id)).toEqual([second.id, first.id]);
  });

  it('rejects invalid targets', async () => {
    await expect(route('/api/preview-history/list', { target: {} })).rejects.toBeInstanceOf(PreviewHistoryRouteError);
  });
});
