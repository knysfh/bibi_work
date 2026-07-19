/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import path from 'path';
import fs from 'fs';

export const MAX_MULTIPART_UPLOAD_BYTES = 64 * 1024 * 1024;

function stripAsciiControlCharacters(value: string): string {
  return [...value]
    .filter((character) => {
      const code = character.charCodeAt(0);
      return code >= 0x20 && code !== 0x7f;
    })
    .join('');
}

export type ParsedMultipartUpload = {
  file: {
    data: Buffer;
    filename?: string;
    contentType?: string;
  };
  fileName?: string;
  conversationId?: string;
};

export class MultipartUploadError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = 'MultipartUploadError';
  }
}

function normalizeContentType(contentType: string | string[] | undefined): string {
  if (Array.isArray(contentType)) return contentType[0] ?? '';
  return contentType ?? '';
}

function readBoundary(contentType: string | string[] | undefined): string {
  const normalized = normalizeContentType(contentType);
  const match = /(?:^|;)\s*boundary=(?:"([^"]+)"|([^;]+))/i.exec(normalized);
  const boundary = (match?.[1] ?? match?.[2] ?? '').trim();
  if (!boundary) {
    throw new MultipartUploadError(400, 'INVALID_MULTIPART', 'multipart boundary is required');
  }
  return boundary;
}

function splitMultipartBody(body: Buffer, boundary: string): Buffer[] {
  const delimiter = Buffer.from(`--${boundary}`);
  const parts: Buffer[] = [];
  let offset = body.indexOf(delimiter);
  if (offset === -1) {
    throw new MultipartUploadError(400, 'INVALID_MULTIPART', 'multipart boundary not found');
  }
  offset += delimiter.length;

  while (offset < body.length) {
    if (body[offset] === 45 && body[offset + 1] === 45) {
      break;
    }
    if (body[offset] === 13 && body[offset + 1] === 10) {
      offset += 2;
    } else if (body[offset] === 10) {
      offset += 1;
    } else {
      throw new MultipartUploadError(400, 'INVALID_MULTIPART', 'invalid multipart delimiter');
    }

    const nextDelimiter = body.indexOf(delimiter, offset);
    if (nextDelimiter === -1) {
      throw new MultipartUploadError(400, 'INVALID_MULTIPART', 'multipart closing boundary not found');
    }

    let end = nextDelimiter;
    if (end >= 2 && body[end - 2] === 13 && body[end - 1] === 10) {
      end -= 2;
    } else if (end >= 1 && body[end - 1] === 10) {
      end -= 1;
    }
    parts.push(body.subarray(offset, end));
    offset = nextDelimiter + delimiter.length;
  }

  return parts;
}

function parsePart(part: Buffer): { headers: Record<string, string>; data: Buffer } {
  let separator = Buffer.from('\r\n\r\n');
  let separatorIndex = part.indexOf(separator);
  if (separatorIndex === -1) {
    separator = Buffer.from('\n\n');
    separatorIndex = part.indexOf(separator);
  }
  if (separatorIndex === -1) {
    throw new MultipartUploadError(400, 'INVALID_MULTIPART', 'multipart part headers are invalid');
  }

  const headerText = part.subarray(0, separatorIndex).toString('utf8');
  const headers: Record<string, string> = {};
  for (const line of headerText.split(/\r?\n/)) {
    const colonIndex = line.indexOf(':');
    if (colonIndex <= 0) continue;
    headers[line.slice(0, colonIndex).trim().toLowerCase()] = line.slice(colonIndex + 1).trim();
  }
  return {
    headers,
    data: part.subarray(separatorIndex + separator.length),
  };
}

function readDispositionParam(disposition: string | undefined, key: string): string | undefined {
  if (!disposition) return undefined;
  const quoted = new RegExp(`(?:^|;)\\s*${key}="((?:\\\\.|[^"])*)"`, 'i').exec(disposition);
  if (quoted) {
    return quoted[1].replace(/\\"/g, '"').replace(/\\\\/g, '\\');
  }
  const bare = new RegExp(`(?:^|;)\\s*${key}=([^;]+)`, 'i').exec(disposition);
  return bare?.[1]?.trim();
}

function bufferToFieldValue(data: Buffer): string | undefined {
  const value = data.toString('utf8').trim();
  return value ? value : undefined;
}

export function parseMultipartUpload(contentType: string | string[] | undefined, body: Buffer): ParsedMultipartUpload {
  const boundary = readBoundary(contentType);
  let file: ParsedMultipartUpload['file'] | undefined;
  let fileName: string | undefined;
  let conversationId: string | undefined;

  for (const rawPart of splitMultipartBody(body, boundary)) {
    if (rawPart.length === 0) continue;
    const part = parsePart(rawPart);
    const disposition = part.headers['content-disposition'];
    const name = readDispositionParam(disposition, 'name');
    if (!name) continue;

    if (name === 'file') {
      file = {
        data: part.data,
        filename: readDispositionParam(disposition, 'filename'),
        contentType: part.headers['content-type'],
      };
    } else if (name === 'file_name') {
      fileName = bufferToFieldValue(part.data);
    } else if (name === 'conversation_id') {
      conversationId = bufferToFieldValue(part.data);
    }
  }

  if (!file) {
    throw new MultipartUploadError(400, 'UPLOAD_FILE_REQUIRED', 'multipart file field is required');
  }

  return { file, fileName, conversationId };
}

export function sanitizeUploadFileName(rawFileName: string | undefined): string {
  const basename = path.basename((rawFileName ?? '').replace(/\\/g, '/')).trim();
  let sanitized = stripAsciiControlCharacters(basename)
    .replace(/[<>:"/\\|?*]/g, '_')
    .replace(/\s+/g, ' ')
    .replace(/[ .]+$/g, '')
    .replace(/^[ .]+/g, '');

  if (!sanitized || sanitized === '.' || sanitized === '..') {
    return 'upload.bin';
  }

  const parsed = path.parse(sanitized);
  if (/^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i.test(parsed.name)) {
    sanitized = `upload-${sanitized}`;
  }

  if (sanitized.length <= 180) return sanitized;
  const extension = path.extname(sanitized);
  const stemLength = Math.max(1, 180 - extension.length);
  return `${sanitized.slice(0, stemLength)}${extension}`;
}

export function sanitizeUploadDirectorySegment(rawSegment: string | undefined): string {
  const sanitized = (rawSegment ?? '')
    .trim()
    .replace(/[^a-zA-Z0-9._-]/g, '_')
    .replace(/_+/g, '_')
    .replace(/^[._-]+|[._-]+$/g, '')
    .slice(0, 80);
  return sanitized || 'temp';
}

export function resolveUploadDirectory(uploadRoot: string, conversationId: string | undefined): string {
  if (!conversationId?.trim()) {
    return path.join(uploadRoot, 'temp');
  }
  return path.join(uploadRoot, 'conversations', sanitizeUploadDirectorySegment(conversationId));
}

export async function writeLocalUploadFile(uploadRoot: string, upload: ParsedMultipartUpload): Promise<string> {
  const uploadDirectory = resolveUploadDirectory(uploadRoot, upload.conversationId);
  await fs.promises.mkdir(uploadDirectory, { recursive: true });

  const fileName = sanitizeUploadFileName(upload.fileName || upload.file.filename);
  const parsed = path.parse(fileName);
  const stem = parsed.name || 'upload';
  const extension = parsed.ext;

  const writeWithSuffix = async (attempt: number): Promise<string> => {
    if (attempt >= 100) {
      const fallbackSuffix = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
      const targetPath = path.join(uploadDirectory, `${stem}-${fallbackSuffix}${extension}`);
      await fs.promises.writeFile(targetPath, upload.file.data, { flag: 'wx' });
      return targetPath;
    }
    const suffix = attempt === 0 ? '' : `-${attempt}`;
    const targetPath = path.join(uploadDirectory, `${stem}${suffix}${extension}`);
    try {
      await fs.promises.writeFile(targetPath, upload.file.data, { flag: 'wx' });
      return targetPath;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'EEXIST') return writeWithSuffix(attempt + 1);
      throw error;
    }
  };

  return writeWithSuffix(0);
}
