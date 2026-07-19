/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { getBaseUrl, httpRawRequest, httpRequest } from '@/common/adapter/httpBridge';
import { getAuthorizationHeaders, invalidateAuthSession, refreshAccessToken } from '@/common/auth/authTokenBroker';
import { trackUpload, type UploadSource } from '@/renderer/hooks/file/useUploadState';

/** Sentinel error message used when an upload is cancelled by the caller. */
export const UPLOAD_ABORTED_ERROR = 'Upload aborted';

export interface UploadFileOptions {
  /** Cancel the upload from the outside. Closing the XHR also frees the backend connection. */
  signal?: AbortSignal;
}

export interface ToolResultArtifactStreamOptions {
  signal?: AbortSignal;
  offset_bytes?: number;
  limit_bytes?: number;
  range?: string;
}

export interface ToolResultArtifactReadOptions {
  signal?: AbortSignal;
  offset?: number;
  limit?: number;
  offset_bytes?: number;
  limit_bytes?: number;
}

export type ToolResultArtifactReadContent =
  | {
      kind: 'json_rows';
      offset: number;
      limit: number;
      total_rows: number;
      rows: unknown[];
    }
  | {
      kind: 'json_value';
      value: unknown;
    }
  | {
      kind: 'text';
      offset: number;
      limit: number;
      total_chars: number;
      text: string;
      truncated: boolean;
    }
  | {
      kind: 'text_byte_range';
      offset_bytes: number;
      limit_bytes: number;
      total_bytes: number;
      text: string;
      truncated: boolean;
    }
  | {
      kind: 'binary_metadata';
      content_type: string;
      size_bytes: number;
    };

export interface ToolResultArtifactReadResponse {
  id: string;
  tenant_id: string;
  run_id?: string | null;
  tool_call_id?: string | null;
  view_kind: string;
  ref_kind: string;
  project_id: string;
  path: string;
  revision: number;
  file_revision_id: string;
  object_reference_id: string;
  content_hash: string;
  content_type: string;
  size_bytes: number;
  content: ToolResultArtifactReadContent;
  created_at: string;
}

export async function fetchToolResultArtifactRead(
  tenant_id: string,
  object_reference_id: string,
  options?: ToolResultArtifactReadOptions
): Promise<ToolResultArtifactReadResponse> {
  if (
    (options?.offset !== undefined || options?.limit !== undefined) &&
    (options.offset_bytes !== undefined || options.limit_bytes !== undefined)
  ) {
    throw new Error('offset/limit and offset_bytes/limit_bytes are mutually exclusive');
  }
  const params = new URLSearchParams({ tenant_id, object_reference_id });
  if (options?.offset !== undefined) params.set('offset', String(options.offset));
  if (options?.limit !== undefined) params.set('limit', String(options.limit));
  if (options?.offset_bytes !== undefined) params.set('offset_bytes', String(options.offset_bytes));
  if (options?.limit_bytes !== undefined) params.set('limit_bytes', String(options.limit_bytes));
  return httpRequest<ToolResultArtifactReadResponse>(
    'GET',
    `/api/v1/tool-result-artifacts/read?${params.toString()}`,
    undefined,
    {
      signal: options?.signal,
    }
  );
}

export async function fetchToolResultArtifactStream(
  tenant_id: string,
  object_reference_id: string,
  options?: ToolResultArtifactStreamOptions
): Promise<Response> {
  if (options?.range && (options.offset_bytes !== undefined || options.limit_bytes !== undefined)) {
    throw new Error('range and offset_bytes/limit_bytes are mutually exclusive');
  }
  const params = new URLSearchParams({ tenant_id, object_reference_id });
  if (options?.offset_bytes !== undefined) params.set('offset_bytes', String(options.offset_bytes));
  if (options?.limit_bytes !== undefined) params.set('limit_bytes', String(options.limit_bytes));
  const headers = options?.range ? { Range: options.range } : undefined;
  return httpRawRequest('GET', `/api/v1/tool-result-artifacts/stream?${params.toString()}`, {
    headers,
    signal: options?.signal,
  });
}

/**
 * Upload a file to the backend via HTTP multipart.
 *
 * Works in both Electron (via `http://127.0.0.1:<backendPort>`) and WebUI
 * (same-origin reverse-proxied). Conversation-bound uploads go to the
 * workspace uploads directory; pre-conversation uploads go to temp storage.
 *
 * Field names match the backend contract exactly (snake_case): `file`,
 * `file_name` (optional), `conversation_id` (optional). The response is
 * `ApiResponse<String>` where `data` is the absolute file path on disk.
 *
 * @param onProgress Optional callback receiving upload percentage (0-100).
 * @param options    Optional bag — currently supports an `AbortSignal` so callers can cancel.
 */
export async function uploadFileViaHttp(
  file: File,
  conversation_id?: string,
  onProgress?: (percent: number) => void,
  file_name?: string,
  options?: UploadFileOptions
): Promise<string> {
  const formData = new FormData();
  formData.append('file', file);
  if (file_name) {
    formData.append('file_name', file_name);
  }
  if (conversation_id) {
    formData.append('conversation_id', conversation_id);
  }

  return new Promise<string>((resolve, reject) => {
    const signal = options?.signal;
    const send = async (attempt: 0 | 1): Promise<void> => {
      if (signal?.aborted) {
        reject(new Error(UPLOAD_ABORTED_ERROR));
        return;
      }
      const authHeaders = await getAuthorizationHeaders();
      const xhr = new XMLHttpRequest();
      xhr.open('POST', `${getBaseUrl()}/api/fs/upload`);
      for (const [key, value] of Object.entries(authHeaders)) xhr.setRequestHeader(key, value);

      let onSignalAbort: (() => void) | null = null;
      if (signal) {
        onSignalAbort = () => {
          try {
            xhr.abort();
          } catch {
            /* ignore */
          }
        };
        signal.addEventListener('abort', onSignalAbort);
      }
      const detachSignal = (): void => {
        if (signal && onSignalAbort) signal.removeEventListener('abort', onSignalAbort);
        onSignalAbort = null;
      };

      if (onProgress) {
        xhr.upload.addEventListener('progress', (event) => {
          if (event.lengthComputable) onProgress(Math.round((event.loaded / event.total) * 100));
        });
      }

      xhr.addEventListener('load', () => {
        detachSignal();
        if (xhr.status === 401 && attempt === 0) {
          void refreshAccessToken()
            .then((token) => (token ? send(1) : Promise.reject(new Error('Upload failed: 401 Unauthorized'))))
            .catch(reject);
          return;
        }
        if (xhr.status === 401 && attempt === 1) void invalidateAuthSession();
        if (xhr.status === 413) {
          reject(new Error('FILE_TOO_LARGE'));
          return;
        }
        if (xhr.status < 200 || xhr.status >= 300) {
          reject(new Error(`Upload failed: ${xhr.status} ${xhr.statusText}`));
          return;
        }
        try {
          const result = JSON.parse(xhr.responseText) as { success: boolean; data?: string };
          if (!result.success || typeof result.data !== 'string' || !result.data) {
            reject(new Error('Upload failed: server returned unsuccessful response'));
          } else {
            resolve(result.data);
          }
        } catch {
          reject(new Error('Upload failed: invalid server response'));
        }
      });
      xhr.addEventListener('error', () => {
        detachSignal();
        reject(new Error('Upload failed: network error'));
      });
      xhr.addEventListener('abort', () => {
        detachSignal();
        reject(new Error(UPLOAD_ABORTED_ERROR));
      });
      xhr.send(formData);
    };
    void send(0).catch(reject);
  });
}
// Simple formatBytes implementation moved from deleted updateConfig
function formatBytes(bytes: number, decimals = 2): string {
  if (bytes === 0) return '0 Bytes';
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(dm)) + ' ' + sizes[i];
}

// ===== 文件类型支持配置 =====
// 注意：当前为预先设计的架构，支持所有文件类型
// 以下常量为将来可能的文件类型过滤功能预留

/** 支持的图片文件扩展名 */
export const imageExts = ['.jpg', '.jpeg', '.png', '.gif', '.bmp', '.webp', '.svg'];

/** 支持的文档文件扩展名 */
export const documentExts = ['.pdf', '.doc', '.docx', '.pptx', '.xlsx', '.odt', '.odp', '.ods'];

/** 支持的文本文件扩展名 */
export const textExts = [
  '.txt',
  '.md',
  '.json',
  '.xml',
  '.csv',
  '.log',
  '.js',
  '.ts',
  '.jsx',
  '.tsx',
  '.html',
  '.css',
  '.scss',
  '.py',
  '.java',
  '.cpp',
  '.c',
  '.h',
  '.go',
  '.rs',
  '.yml',
  '.yaml',
  '.toml',
  '.ini',
  '.conf',
  '.config',
];

/** 所有支持的文件扩展名（预先设计，当前实际接受所有文件类型） */
export const allSupportedExts = [...imageExts, ...documentExts, ...textExts];

// 文件元数据接口
export interface FileMetadata {
  name: string;
  path: string;
  size: number;
  type: string;
  lastModified: number;
}

/**
 * 检查文件是否被支持
 * 注意：当前实现为预先设计的架构，支持所有文件类型
 * supportedExts 参数预留给将来的文件类型过滤功能
 *
 * @param _file_name 文件名（预留参数）
 * @param _supportedExts 支持的文件扩展名数组（预留参数）
 * @returns 总是返回 true，表示支持所有文件类型
 */
export function isSupportedFile(_file_name: string, _supportedExts: string[]): boolean {
  return true; // 预先设计：当前支持所有文件类型
}

// 获取文件扩展名
export function getFileExtension(file_name: string): string {
  const lastDotIndex = file_name.lastIndexOf('.');
  return lastDotIndex > -1 ? file_name.substring(lastDotIndex).toLowerCase() : '';
}

import { BIWORK_TIMESTAMP_REGEX } from '@/common/config/constants';

// 清理BiWork时间戳后缀，返回原始文件名
export function cleanBiWorkTimestamp(file_name: string): string {
  return file_name.replace(BIWORK_TIMESTAMP_REGEX, '$1');
}

// 从文件路径获取清理后的文件名（用于UI显示）
export function getCleanFileName(file_path: string): string {
  const file_name = file_path.split(/[\\/]/).pop() || '';
  return cleanBiWorkTimestamp(file_name);
}

// 从文件路径数组获取清理后的文件名数组（用于消息格式化）
export function getCleanFileNames(file_paths: string[]): string[] {
  return file_paths.map(getCleanFileName);
}

/**
 * 过滤支持的文件
 * 注意：由于 isSupportedFile 当前总是返回 true，此函数实际不会过滤任何文件
 * 这是预先设计的架构，为将来的文件类型过滤功能预留
 *
 * @param files 文件元数据数组
 * @param supportedExts 支持的文件扩展名数组（预留参数）
 * @returns 当前返回所有文件，未进行过滤
 */
export function filterSupportedFiles(files: FileMetadata[], supportedExts: string[]): FileMetadata[] {
  return files.filter((file) => isSupportedFile(file.name, supportedExts));
}

// 从拖拽事件中提取文件 (纯工具函数，不处理业务逻辑)
export function getFilesFromDropEvent(event: DragEvent): FileMetadata[] {
  const files: FileMetadata[] = [];

  if (!event.dataTransfer?.files) {
    return files;
  }

  for (let i = 0; i < event.dataTransfer.files.length; i++) {
    const file = event.dataTransfer.files[i];
    // 在 Electron 环境中，拖拽文件会有额外的 path 属性
    const electronFile = file as File & { path?: string };

    files.push({
      name: file.name,
      path: electronFile.path || '', // 原始路径，可能为空
      size: file.size,
      type: file.type,
      lastModified: file.lastModified,
    });
  }

  return files;
}

// 从拖拽事件中提取文本
export function getTextFromDropEvent(event: DragEvent): string {
  return event.dataTransfer?.getData('text/plain') || '';
}

// 格式化文件大小（使用统一的formatBytes实现）
export function formatFileSize(bytes: number): string {
  return formatBytes(bytes, 2); // 保持2位精度以兼容之前的行为
}

/**
 * 检查是否为图片文件
 * 注意：由于 isSupportedFile 当前总是返回 true，此函数实际总是返回 true
 * 预先设计的架构，为将来的文件类型判断功能预留
 * 当前未被使用，保留供将来扩展
 */
export function isImageFile(file_name: string): boolean {
  return isSupportedFile(file_name, imageExts);
}

/**
 * 检查是否为文档文件
 * 注意：由于 isSupportedFile 当前总是返回 true，此函数实际总是返回 true
 * 预先设计的架构，为将来的文件类型判断功能预留
 * 当前未被使用，保留供将来扩展
 */
export function isDocumentFile(file_name: string): boolean {
  return isSupportedFile(file_name, documentExts);
}

/**
 * 检查是否为文本文件
 * 注意：由于 isSupportedFile 当前总是返回 true，此函数实际总是返回 true
 * 预先设计的架构，为将来的文件类型判断功能预留
 * 当前未被使用，保留供将来扩展
 */
export function isTextFile(file_name: string): boolean {
  return isSupportedFile(file_name, textExts);
}

class FileServiceClass {
  /**
   * Process files from drag and drop events, uploading any file that lacks a
   * native disk path via HTTP multipart.
   *
   * In Electron, files dragged from the OS file manager already expose an
   * absolute `path`, so we skip upload for those. Anything without a path
   * (WebUI, synthetic File objects, browser-sourced drags) is uploaded to the
   * backend, which returns the absolute stored path.
   */
  async processDroppedFiles(
    files: FileList,
    conversation_id?: string,
    source: UploadSource = 'sendbox'
  ): Promise<FileMetadata[]> {
    const processedFiles: FileMetadata[] = [];

    for (let i = 0; i < files.length; i++) {
      const file = files[i];
      // In Electron environment, dragged files have additional path property
      const electronFile = file as File & { path?: string };

      let file_path = electronFile.path || '';

      // If no valid path (WebUI or some dragged files may not have paths), upload via HTTP multipart
      if (!file_path) {
        // Each upload owns its own AbortController; the tracker exposes an `abort()`
        // that triggers the signal so user-driven cancel and conversation-switch
        // bulk-abort go through the same path.
        const controller = new AbortController();
        const tracker = trackUpload(file.size, {
          source,
          name: file.name,
          conversationId: conversation_id || undefined,
          onAbort: () => controller.abort(),
        });
        try {
          file_path = await uploadFileViaHttp(file, conversation_id || '', tracker.onProgress, undefined, {
            signal: controller.signal,
          });
        } catch (error) {
          // Re-throw size errors so caller can show user-facing toast
          if (error instanceof Error && error.message === 'FILE_TOO_LARGE') {
            throw error;
          }
          if (error instanceof Error && error.message === UPLOAD_ABORTED_ERROR) {
            // User-initiated abort: drop this file silently (the UI already reflects it).
            continue;
          }
          console.error('Failed to upload dragged file:', error);
          continue;
        } finally {
          tracker.finish();
        }
      }

      processedFiles.push({
        name: file.name,
        path: file_path,
        size: file.size,
        type: file.type,
        lastModified: file.lastModified,
      });
    }

    return processedFiles;
  }
}

export const FileService = new FileServiceClass();
