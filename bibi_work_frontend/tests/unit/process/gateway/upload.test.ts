/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import fs from 'fs/promises';
import os from 'os';
import path from 'path';
import { describe, expect, it } from 'vitest';
import {
  MultipartUploadError,
  parseMultipartUpload,
  resolveUploadDirectory,
  sanitizeUploadDirectorySegment,
  sanitizeUploadFileName,
  writeLocalUploadFile,
} from '@process/gateway/upload';

function multipartBody(boundary: string, parts: string[]): Buffer {
  return Buffer.from(`${parts.map((part) => `--${boundary}\r\n${part}`).join('')}--${boundary}--\r\n`, 'utf8');
}

describe('desktop gateway upload helpers', () => {
  it('parses file upload multipart bodies', () => {
    const boundary = 'test-boundary';
    const body = multipartBody(boundary, [
      'Content-Disposition: form-data; name="conversation_id"\r\n\r\nconv/1\r\n',
      'Content-Disposition: form-data; name="file_name"\r\n\r\nfinal-name.txt\r\n',
      'Content-Disposition: form-data; name="file"; filename="../ignored.txt"\r\nContent-Type: text/plain\r\n\r\nhello\r\n',
    ]);

    const parsed = parseMultipartUpload(`multipart/form-data; boundary=${boundary}`, body);

    expect(parsed.conversationId).toBe('conv/1');
    expect(parsed.fileName).toBe('final-name.txt');
    expect(parsed.file.filename).toBe('../ignored.txt');
    expect(parsed.file.contentType).toBe('text/plain');
    expect(parsed.file.data.toString('utf8')).toBe('hello');
  });

  it('rejects multipart bodies without a file part', () => {
    const boundary = 'missing-file';
    const body = multipartBody(boundary, ['Content-Disposition: form-data; name="file_name"\r\n\r\nname.txt\r\n']);

    expect(() => parseMultipartUpload(`multipart/form-data; boundary=${boundary}`, body)).toThrow(MultipartUploadError);
    expect(() => parseMultipartUpload(`multipart/form-data; boundary=${boundary}`, body)).toThrow(
      'multipart file field is required'
    );
  });

  it('rejects multipart requests without a boundary', () => {
    expect(() => parseMultipartUpload('multipart/form-data', Buffer.from(''))).toThrow(
      'multipart boundary is required'
    );
  });

  it('sanitizes uploaded filenames', () => {
    expect(sanitizeUploadFileName('../secret.txt')).toBe('secret.txt');
    expect(sanitizeUploadFileName('..\\nested\\image?.png')).toBe('image_.png');
    expect(sanitizeUploadFileName(' CON ')).toBe('upload-CON');
    expect(sanitizeUploadFileName('   ')).toBe('upload.bin');
  });

  it('keeps uploads inside controlled directory segments', () => {
    expect(sanitizeUploadDirectorySegment('conv/../1')).toBe('conv_.._1');
    expect(resolveUploadDirectory('/tmp/biwork-uploads', undefined)).toBe(path.join('/tmp/biwork-uploads', 'temp'));
    expect(resolveUploadDirectory('/tmp/biwork-uploads', 'conv/../1')).toBe(
      path.join('/tmp/biwork-uploads', 'conversations', 'conv_.._1')
    );
  });

  it('writes uploads without overwriting an existing file', async () => {
    const root = await fs.mkdtemp(path.join(os.tmpdir(), 'biwork-upload-test-'));
    try {
      const upload = {
        file: { data: Buffer.from('first'), filename: 'report.txt' },
        conversationId: 'conversation-1',
      };
      const firstPath = await writeLocalUploadFile(root, upload);
      const secondPath = await writeLocalUploadFile(root, {
        ...upload,
        file: { ...upload.file, data: Buffer.from('second') },
      });

      expect(path.basename(firstPath)).toBe('report.txt');
      expect(path.basename(secondPath)).toBe('report-1.txt');
      expect(await fs.readFile(firstPath, 'utf8')).toBe('first');
      expect(await fs.readFile(secondPath, 'utf8')).toBe('second');
    } finally {
      await fs.rm(root, { recursive: true, force: true });
    }
  });
});
