/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { handleOfficeLocalRoute, OfficeLocalRouteError } from '@process/gateway/officeLocal';

let tempRoot = '';

async function route(pathname: string, body: Record<string, unknown>): Promise<unknown> {
  return handleOfficeLocalRoute(pathname, body, {
    platform: process.platform,
    officeCliAvailable: false,
    allowedRoots: [tempRoot],
  });
}

beforeEach(async () => {
  tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-office-local-test-'));
});

afterEach(async () => {
  if (tempRoot) {
    await fs.rm(tempRoot, { recursive: true, force: true });
  }
});

describe('desktop office/document local routes', () => {
  it('converts markdown-compatible files to markdown', async () => {
    const filePath = path.join(tempRoot, 'sample.md');
    await fs.writeFile(filePath, '# Hello\n');

    await expect(route('/api/document/convert', { file_path: filePath, to: 'markdown' })).resolves.toEqual({
      to: 'markdown',
      result: {
        success: true,
        data: '# Hello\n',
      },
    });
  });

  it('returns structured unsupported conversion errors inside the conversion envelope', async () => {
    const filePath = path.join(tempRoot, 'sample.docx');
    await fs.writeFile(filePath, 'stub');

    await expect(route('/api/document/convert', { file_path: filePath, to: 'markdown' })).resolves.toEqual({
      to: 'markdown',
      result: {
        success: false,
        error: 'UNSUPPORTED_DOCUMENT_CONVERSION',
      },
    });
  });

  it('returns a renderer-compatible officecli missing error when preview cannot start', async () => {
    const filePath = path.join(tempRoot, 'sample.docx');
    await fs.writeFile(filePath, 'stub');

    await expect(route('/api/word-preview/start', { file_path: filePath })).resolves.toEqual({
      error: 'OFFICECLI_NOT_FOUND',
    });
  });

  it('rejects files outside the sandbox unless an enclosing workspace is provided', async () => {
    const outsideRoot = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-office-outside-'));
    const outsideFile = path.join(outsideRoot, 'outside.md');
    await fs.writeFile(outsideFile, '# outside\n');

    try {
      await expect(
        handleOfficeLocalRoute(
          '/api/document/convert',
          { file_path: outsideFile, to: 'markdown' },
          { platform: process.platform, officeCliAvailable: false, allowedRoots: [tempRoot] }
        )
      ).rejects.toMatchObject({ statusCode: 403, code: 'PATH_OUTSIDE_SANDBOX' });

      await expect(
        handleOfficeLocalRoute(
          '/api/document/convert',
          { file_path: outsideFile, workspace: outsideRoot, to: 'markdown' },
          { platform: process.platform, officeCliAvailable: false, allowedRoots: [tempRoot] }
        )
      ).resolves.toMatchObject({ to: 'markdown', result: { success: true } });
    } finally {
      await fs.rm(outsideRoot, { recursive: true, force: true });
    }
  });

  it('accepts office preview stop requests idempotently', async () => {
    await expect(route('/api/word-preview/stop', { file_path: '/missing.docx' })).resolves.toBeNull();
  });

  it('rejects invalid document conversion payloads', async () => {
    await expect(route('/api/document/convert', { to: 'markdown' })).rejects.toBeInstanceOf(OfficeLocalRouteError);
  });
});
