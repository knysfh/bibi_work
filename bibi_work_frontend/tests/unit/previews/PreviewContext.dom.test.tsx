/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import { renderHook, act, cleanup } from '@testing-library/react';
import React, { type ReactNode } from 'react';
import { ipcBridge } from '@/common';
import { PreviewProvider, usePreviewContext } from '@/renderer/pages/conversation/Preview/context/PreviewContext';

type FileStreamContentUpdate = {
  file_path: string;
  content: string;
  workspace: string;
  relative_path: string;
  operation: 'write' | 'delete';
};

vi.mock('@/common', () => ({
  ipcBridge: {
    fileStream: {
      contentUpdate: { on: vi.fn(() => vi.fn()) },
    },
    preview: {
      open: { on: vi.fn(() => vi.fn()) },
    },
    fs: {
      writeFile: { invoke: vi.fn() },
      getFileMetadata: { invoke: vi.fn() },
      readFile: { invoke: vi.fn() },
      getImageBase64: { invoke: vi.fn() },
    },
  },
}));

vi.mock('@/renderer/utils/emitter', () => ({
  emitter: {
    on: vi.fn(),
    off: vi.fn(),
  },
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (k: string) => k,
    i18n: { language: 'en' },
  }),
}));

describe('PreviewContext', () => {
  const wrapper = ({ children }: { children: ReactNode }) => <PreviewProvider>{children}</PreviewProvider>;
  let fileStreamHandler: ((payload: FileStreamContentUpdate) => void) | undefined;

  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    fileStreamHandler = undefined;
    vi.mocked(ipcBridge.fileStream.contentUpdate.on).mockImplementation((handler) => {
      fileStreamHandler = handler as (payload: FileStreamContentUpdate) => void;
      return vi.fn();
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    cleanup();
  });

  it('initializes with closed state', () => {
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    expect(result.current.isOpen).toBe(false);
    expect(result.current.tabs).toEqual([]);
    expect(result.current.activeTabId).toBe(null);
  });

  it('opens preview and creates tab', () => {
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    act(() => {
      result.current.openPreview('# Hello', 'markdown', { title: 'test.md' });
    });
    expect(result.current.isOpen).toBe(true);
    expect(result.current.tabs).toHaveLength(1);
    expect(result.current.tabs[0].content).toBe('# Hello');
    expect(result.current.tabs[0].content_type).toBe('markdown');
  });

  it('closes preview and clears all tabs', () => {
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    act(() => {
      result.current.openPreview('content', 'code');
    });
    act(() => {
      result.current.closePreview();
    });
    expect(result.current.isOpen).toBe(false);
    expect(result.current.tabs).toEqual([]);
  });

  it('provides all context API methods', () => {
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    expect(typeof result.current.openPreview).toBe('function');
    expect(typeof result.current.closePreview).toBe('function');
    expect(typeof result.current.updateContent).toBe('function');
    expect(typeof result.current.findPreviewTab).toBe('function');
  });

  it('updates content and marks tab as dirty', () => {
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    act(() => {
      result.current.openPreview('original', 'code');
    });
    expect(result.current.activeTab?.isDirty).toBe(false);
    act(() => {
      result.current.updateContent('modified');
    });
    expect(result.current.activeTab?.content).toBe('modified');
    expect(result.current.activeTab?.isDirty).toBe(true);
  });

  it('saves Rust-backed preview tabs with expected_revision and refreshes revision metadata', async () => {
    vi.mocked(ipcBridge.fs.writeFile.invoke).mockResolvedValue(true);
    vi.mocked(ipcBridge.fs.getFileMetadata.invoke).mockResolvedValue({
      name: 'main.ts',
      path: '/workspace/src/main.ts',
      size: 11,
      type: 'text/plain; charset=utf-8',
      lastModified: 1,
      isDirectory: false,
      revision: 8,
      etag: 'rev-8',
    });

    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    act(() => {
      result.current.openPreview('old', 'code', {
        file_name: 'main.ts',
        file_path: '/workspace/src/main.ts',
        workspace: '/workspace',
        revision: 7,
      });
    });
    act(() => {
      result.current.updateContent('new content');
    });

    let saved = false;
    await act(async () => {
      saved = await result.current.saveContent();
    });

    expect(saved).toBe(true);
    expect(ipcBridge.fs.writeFile.invoke).toHaveBeenCalledWith({
      path: '/workspace/src/main.ts',
      data: 'new content',
      workspace: '/workspace',
      expected_revision: 7,
    });
    expect(ipcBridge.fs.getFileMetadata.invoke).toHaveBeenCalledWith({
      path: '/workspace/src/main.ts',
      workspace: '/workspace',
    });
    expect(result.current.activeTab?.isDirty).toBe(false);
    expect(result.current.activeTab?.metadata?.revision).toBe(8);
  });

  it('applies debounced fileStream content updates to matching clean tabs', () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    act(() => {
      result.current.openPreview('old content', 'code', {
        file_name: 'main.ts',
        file_path: '/workspace/src/main.ts',
        workspace: '/workspace',
      });
    });

    act(() => {
      fileStreamHandler?.({
        file_path: '/workspace/src/main.ts',
        content: 'agent content',
        workspace: '/workspace',
        relative_path: 'src/main.ts',
        operation: 'write',
      });
      vi.advanceTimersByTime(500);
    });

    expect(result.current.activeTab?.content).toBe('agent content');
    expect(result.current.activeTab?.originalContent).toBe('agent content');
    expect(result.current.activeTab?.isDirty).toBe(false);
  });

  it('does not overwrite dirty tabs when fileStream content arrives', () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => usePreviewContext(), { wrapper });
    act(() => {
      result.current.openPreview('old content', 'code', {
        file_name: 'main.ts',
        file_path: '/workspace/src/main.ts',
        workspace: '/workspace',
      });
    });
    act(() => {
      result.current.updateContent('local unsaved edit');
    });

    act(() => {
      fileStreamHandler?.({
        file_path: '/workspace/src/main.ts',
        content: 'agent content',
        workspace: '/workspace',
        relative_path: 'src/main.ts',
        operation: 'write',
      });
      vi.advanceTimersByTime(500);
    });

    expect(result.current.activeTab?.content).toBe('local unsaved edit');
    expect(result.current.activeTab?.isDirty).toBe(true);
  });
});
