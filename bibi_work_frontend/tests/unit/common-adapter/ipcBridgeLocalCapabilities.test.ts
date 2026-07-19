/**
 * @vitest-environment node
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

type HttpCall = {
  method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  path: string;
  body?: unknown;
};

const httpBridgeMocks = vi.hoisted(() => {
  const calls: HttpCall[] = [];
  const provider =
    (method: HttpCall['method']) =>
    <Data, Params = undefined>(path: string | ((params: Params) => string), mapBody?: (params: Params) => unknown) => ({
      provider: vi.fn(),
      invoke: vi.fn(async (params?: Params) => {
        const resolvedPath = typeof path === 'function' ? path(params as Params) : path;
        const requestBody =
          mapBody && params !== undefined
            ? mapBody(params as Params)
            : method === 'GET' || method === 'DELETE'
              ? undefined
              : params;
        calls.push({
          method,
          path: resolvedPath,
          body: requestBody,
        });
        return undefined as Data;
      }),
    });
  const emitter = () => ({ on: vi.fn(() => vi.fn()), emit: vi.fn() });

  return {
    calls,
    httpGet: provider('GET'),
    httpPost: provider('POST'),
    httpPut: provider('PUT'),
    httpPatch: provider('PATCH'),
    httpDelete: provider('DELETE'),
    httpRequest: vi.fn(),
    stubProvider: vi.fn((name: string, defaultValue: unknown) => ({
      provider: vi.fn(),
      invoke: vi.fn(async () => defaultValue),
    })),
    withResponseMap: vi.fn(
      (
        inner: { provider: unknown; invoke: (params?: unknown) => Promise<unknown> },
        map: (raw: unknown) => unknown
      ) => ({
        provider: inner.provider,
        invoke: vi.fn(async (params?: unknown) => map(await inner.invoke(params))),
      })
    ),
    wsEmitter: vi.fn(emitter),
    wsMappedEmitter: vi.fn(emitter),
    stubEmitter: vi.fn(emitter),
  };
});

vi.mock('@/common/adapter/httpBridge', () => httpBridgeMocks);

vi.mock('@office-ai/platform', () => ({
  bridge: {
    buildProvider: vi.fn(() => ({
      provider: vi.fn(),
      invoke: vi.fn(),
    })),
    buildEmitter: vi.fn(() => ({
      on: vi.fn(() => vi.fn()),
      emit: vi.fn(),
    })),
  },
}));

describe('ipcBridge local desktop capability adapter', () => {
  beforeEach(() => {
    vi.resetModules();
    httpBridgeMocks.calls.length = 0;
    httpBridgeMocks.wsEmitter.mockClear();
  });

  it('keeps shell actions on the desktop local API contract', async () => {
    const { shell } = await import('@/common/adapter/ipcBridge');

    await shell.openFile.invoke('/workspace/report.pdf');
    await shell.showItemInFolder.invoke('/workspace/report.pdf');
    await shell.openExternal.invoke('https://example.com/docs');
    await shell.checkToolInstalled.invoke({ tool: 'officecli' });
    await shell.openFolderWith.invoke({ folder_path: '/workspace', tool: 'vscode' });

    expect(httpBridgeMocks.calls).toEqual([
      { method: 'POST', path: '/api/shell/open-file', body: { file_path: '/workspace/report.pdf' } },
      { method: 'POST', path: '/api/shell/show-item-in-folder', body: { file_path: '/workspace/report.pdf' } },
      { method: 'POST', path: '/api/shell/open-external', body: { url: 'https://example.com/docs' } },
      { method: 'POST', path: '/api/shell/check-tool-installed', body: { tool: 'officecli' } },
      {
        method: 'POST',
        path: '/api/shell/open-folder-with',
        body: { folder_path: '/workspace', tool: 'vscode' },
      },
    ]);
  });

  it('keeps document conversion and Office preview calls on local preview routes', async () => {
    const { document, pptPreview, wordPreview, excelPreview } = await import('@/common/adapter/ipcBridge');

    await document.convert.invoke({
      file_path: '/workspace/report.docx',
      to: 'markdown',
      workspace: '/workspace',
    });
    await pptPreview.start.invoke({ file_path: '/workspace/slides.pptx', workspace: '/workspace' });
    await pptPreview.stop.invoke({ file_path: '/workspace/slides.pptx' });
    await wordPreview.start.invoke({ file_path: '/workspace/report.docx', workspace: '/workspace' });
    await wordPreview.stop.invoke({ file_path: '/workspace/report.docx' });
    await excelPreview.start.invoke({ file_path: '/workspace/model.xlsx', workspace: '/workspace' });
    await excelPreview.stop.invoke({ file_path: '/workspace/model.xlsx' });

    expect(httpBridgeMocks.calls).toEqual([
      {
        method: 'POST',
        path: '/api/document/convert',
        body: { file_path: '/workspace/report.docx', to: 'markdown', workspace: '/workspace' },
      },
      {
        method: 'POST',
        path: '/api/ppt-preview/start',
        body: { file_path: '/workspace/slides.pptx', workspace: '/workspace' },
      },
      { method: 'POST', path: '/api/ppt-preview/stop', body: { file_path: '/workspace/slides.pptx' } },
      {
        method: 'POST',
        path: '/api/word-preview/start',
        body: { file_path: '/workspace/report.docx', workspace: '/workspace' },
      },
      { method: 'POST', path: '/api/word-preview/stop', body: { file_path: '/workspace/report.docx' } },
      {
        method: 'POST',
        path: '/api/excel-preview/start',
        body: { file_path: '/workspace/model.xlsx', workspace: '/workspace' },
      },
      { method: 'POST', path: '/api/excel-preview/stop', body: { file_path: '/workspace/model.xlsx' } },
    ]);
    expect(httpBridgeMocks.wsEmitter).toHaveBeenCalledWith('ppt-preview.status');
    expect(httpBridgeMocks.wsEmitter).toHaveBeenCalledWith('word-preview.status');
    expect(httpBridgeMocks.wsEmitter).toHaveBeenCalledWith('excel-preview.status');
  });
});
