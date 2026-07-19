/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, expect, it } from 'vitest';
import {
  fromBackendWorkspaceFlatFiles,
  fromBackendWorkspaceList,
  type RawWorkspaceFlatFile,
} from '@/common/adapter/workspaceMapper';

describe('workspaceMapper', () => {
  it('maps workspace flat files from backend snake_case to frontend camelCase', () => {
    const raw: RawWorkspaceFlatFile[] = [
      {
        name: 'main.ts',
        full_path: '/workspace/src/main.ts',
        relative_path: 'src/main.ts',
      },
    ];

    expect(fromBackendWorkspaceFlatFiles(raw)).toEqual([
      {
        name: 'main.ts',
        fullPath: '/workspace/src/main.ts',
        relativePath: 'src/main.ts',
      },
    ]);
  });

  it('does not leak snake_case path fields', () => {
    const [file] = fromBackendWorkspaceFlatFiles([
      {
        name: 'README.md',
        full_path: '/workspace/README.md',
        relative_path: 'README.md',
      },
    ]);

    expect(file).toBeDefined();
    expect((file as Record<string, unknown>).full_path).toBeUndefined();
    expect((file as Record<string, unknown>).relative_path).toBeUndefined();
    expect(file?.fullPath).toBe('/workspace/README.md');
    expect(file?.relativePath).toBe('README.md');
  });

  it('uses backend full_path and relative_path for workspace search results', () => {
    const tree = fromBackendWorkspaceList(
      [
        {
          name: 'report.md',
          type: 'file',
          full_path: '/workspace/docs/report.md',
          relative_path: 'docs/report.md',
        },
      ],
      '/workspace',
      '.'
    );

    expect(tree[0]?.children?.[0]).toMatchObject({
      name: 'report.md',
      fullPath: '/workspace/docs/report.md',
      relativePath: 'docs/report.md',
      isFile: true,
      isDir: false,
    });
  });
});
