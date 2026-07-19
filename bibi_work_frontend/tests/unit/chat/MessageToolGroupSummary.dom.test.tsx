/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import React from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { IMessageAcpToolCall, IMessageToolCall } from '@/common/chat/chatLib';
import MessageToolGroupSummary from '@/renderer/pages/conversation/Messages/components/MessageToolGroupSummary';

const mockDownloadFileFromPath = vi.fn().mockResolvedValue(undefined);
const mockDownloadBlob = vi.fn();
const mockFetchToolResultArtifactRead = vi.fn();
const mockFetchToolResultArtifactStream = vi.fn();
const mockWorkbenchBootstrap = vi.fn();
const mockMessageSuccess = vi.fn();
const mockMessageError = vi.fn();
const runtimeMocks = vi.hoisted(() => ({
  vegaEmbed: vi.fn(),
  mapConstructor: vi.fn(),
  mapInstances: [] as Array<{
    on: ReturnType<typeof vi.fn>;
    fitBounds: ReturnType<typeof vi.fn>;
    remove: ReturnType<typeof vi.fn>;
  }>,
}));

vi.mock('vega-embed', () => ({
  __esModule: true,
  default: runtimeMocks.vegaEmbed,
}));

vi.mock('maplibre-gl', () => ({
  __esModule: true,
  default: {
    Map: runtimeMocks.mapConstructor,
  },
  Map: runtimeMocks.mapConstructor,
}));

vi.mock('@/common', () => ({
  ipcBridge: {
    database: {
      getConversationMessage: {
        invoke: vi.fn(),
      },
    },
    workbench: {
      bootstrap: {
        invoke: (...args: unknown[]) => mockWorkbenchBootstrap(...args),
      },
    },
  },
}));

vi.mock('@/renderer/components/media/LocalImageView', () => ({
  __esModule: true,
  default: ({ src, alt, className }: { src: string; alt: string; className?: string }) => (
    <img src={src} alt={alt} className={className} data-testid='local-image' />
  ),
}));

vi.mock('@/renderer/utils/file/download', () => ({
  downloadFileFromPath: (...args: unknown[]) => mockDownloadFileFromPath(...args),
  downloadBlob: (...args: unknown[]) => mockDownloadBlob(...args),
}));

vi.mock('@/renderer/services/FileService', () => ({
  fetchToolResultArtifactRead: (...args: unknown[]) => mockFetchToolResultArtifactRead(...args),
  fetchToolResultArtifactStream: (...args: unknown[]) => mockFetchToolResultArtifactStream(...args),
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock('@arco-design/web-react', async () => {
  const actual = await vi.importActual<typeof import('@arco-design/web-react')>('@arco-design/web-react');

  return {
    ...actual,
    Message: {
      useMessage: () => [{ success: mockMessageSuccess, error: mockMessageError }, null],
    },
  };
});

describe('MessageToolGroupSummary ACP image output', () => {
  beforeEach(() => {
    mockDownloadFileFromPath.mockReset();
    mockDownloadFileFromPath.mockResolvedValue(undefined);
    mockDownloadBlob.mockReset();
    mockFetchToolResultArtifactRead.mockReset();
    mockFetchToolResultArtifactStream.mockReset();
    mockWorkbenchBootstrap.mockReset();
    mockWorkbenchBootstrap.mockResolvedValue({ auth: { tenant_id: 'tenant-1' } });
    mockMessageSuccess.mockClear();
    mockMessageError.mockClear();
    runtimeMocks.vegaEmbed.mockReset();
    runtimeMocks.vegaEmbed.mockResolvedValue({ view: { finalize: vi.fn() } });
    runtimeMocks.mapConstructor.mockReset();
    runtimeMocks.mapInstances.length = 0;
    runtimeMocks.mapConstructor.mockImplementation(function MapMock() {
      const instance = {
        on: vi.fn((event: string, handler: () => void) => {
          if (event === 'load') queueMicrotask(handler);
          return instance;
        }),
        fitBounds: vi.fn(),
        remove: vi.fn(),
      };
      runtimeMocks.mapInstances.push(instance);
      return instance;
    });
  });

  it('renders generated image preview when an ACP image tool call is expanded', () => {
    const message: IMessageAcpToolCall = {
      id: 'ig_test_image',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call_update',
          tool_call_id: 'ig_test_image',
          status: 'completed',
          title: 'Image generation',
          kind: 'execute',
          raw_output: {
            image: {
              path: '/Users/test/.codex/generated_images/session/ig_test_image.png',
            },
          },
          content: [
            {
              type: 'content',
              content: {
                type: 'text',
                text: 'Revised prompt: 一张小猫照片',
              },
            },
          ],
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));

    const image = screen.getByTestId('local-image');
    expect(image).toHaveAttribute('src', '/Users/test/.codex/generated_images/session/ig_test_image.png');
    expect(image).toHaveAttribute('alt', 'ig_test_image.png');
  });

  it('downloads the generated image from its local path', () => {
    const imagePath = '/Users/test/.codex/generated_images/session/ig_test_image.png';
    const message: IMessageAcpToolCall = {
      id: 'ig_test_image',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call_update',
          tool_call_id: 'ig_test_image',
          status: 'completed',
          title: 'Image generation',
          kind: 'execute',
          raw_output: {
            image: {
              path: imagePath,
            },
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByLabelText('acp.image.download_aria'));

    expect(mockDownloadFileFromPath).toHaveBeenCalledWith(imagePath, 'ig_test_image.png');
  });

  it('shows an error when generated image download fails', async () => {
    const imagePath = '/Users/test/.codex/generated_images/session/ig_test_image.png';
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockDownloadFileFromPath.mockRejectedValueOnce(new Error('denied'));
    const message: IMessageAcpToolCall = {
      id: 'ig_test_image',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call_update',
          tool_call_id: 'ig_test_image',
          status: 'completed',
          title: 'Image generation',
          kind: 'execute',
          raw_output: {
            image: {
              path: imagePath,
            },
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByLabelText('acp.image.download_aria'));

    await waitFor(() => {
      expect(mockMessageError).toHaveBeenCalledWith('acp.image.download_error');
    });
    expect(consoleError).toHaveBeenCalledWith('[MessageToolGroupSummary] Failed to download image:', expect.any(Error));
    expect(mockMessageSuccess).not.toHaveBeenCalled();
    consoleError.mockRestore();
  });

  it('uses i18n keys for the image download control', () => {
    const message: IMessageAcpToolCall = {
      id: 'ig_test_image',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call_update',
          tool_call_id: 'ig_test_image',
          status: 'completed',
          title: 'Image generation',
          kind: 'execute',
          raw_output: {
            image: {
              path: '/Users/test/.codex/generated_images/session/ig_test_image.png',
            },
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));

    expect(screen.getByLabelText('acp.image.download_aria')).toBeInTheDocument();
  });

  it('does not render image controls for tool calls without image output', () => {
    const message: IMessageToolCall = {
      id: 'tool-1',
      conversation_id: 'conv-1',
      type: 'tool_call',
      content: {
        call_id: 'tool-1',
        name: 'Shell Command',
        args: {},
        status: 'completed',
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));

    expect(screen.queryByTestId('local-image')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('acp.image.download_aria')).not.toBeInTheDocument();
  });

  it('shows friendly fields first and keeps raw JSON behind technical details', () => {
    const message: IMessageToolCall = {
      id: 'tool-friendly',
      conversation_id: 'conv-1',
      type: 'tool_call',
      content: {
        call_id: 'tool-friendly',
        name: 'search_places',
        args: { query: 'coffee nearby', api_key: 'secret-value' },
        output: '{"output_summary":"3 places found","internal_id":"abc"}',
        status: 'completed',
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByText('search_places'));

    expect(screen.getByText('Request')).toBeInTheDocument();
    expect(screen.getAllByText('coffee nearby').length).toBeGreaterThan(0);
    expect(screen.getByText('Hidden')).toBeInTheDocument();
    expect(screen.getByText('3 places found')).toBeInTheDocument();
    expect(screen.queryByText(/internal_id/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByText('Technical details'));
    expect(screen.getByText(/internal_id/)).toBeInTheDocument();
  });

  it('renders Rust tool result artifact views and downloads raw artifact bytes', async () => {
    const objectReferenceId = '00000000-0000-0000-0000-000000000001';
    const response = new Response('row-1\n', {
      status: 200,
      headers: { 'content-type': 'application/jsonl' },
    });
    mockFetchToolResultArtifactStream.mockResolvedValue(response);
    const message: IMessageAcpToolCall = {
      id: 'tool-result-table',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call',
          tool_call_id: 'tool-result-table',
          status: 'completed',
          title: 'Query sales',
          kind: 'execute',
          rawOutput: {
            output_summary: 'returned 238 rows',
            views: [
              {
                kind: 'table',
                title: 'Sales rows',
                columns: [{ key: 'region', label: 'Region' }],
                rows_preview: [{ region: 'east' }],
                data_ref: {
                  artifact_id: 'artifact-1',
                  object_reference_id: objectReferenceId,
                  content_type: 'application/jsonl',
                  content_hash: 'sha256:abc',
                  size_bytes: 128000,
                },
              },
            ],
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByText('Query sales'));

    expect(screen.getByTestId('tool-result-view')).toHaveTextContent('Sales rows');
    expect(screen.getByTestId('tool-result-view')).toHaveTextContent('125 KB');
    expect(screen.getByRole('columnheader', { name: 'Region' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: 'east' })).toBeInTheDocument();

    fireEvent.click(screen.getByTestId(`tool-result-artifact-download-${objectReferenceId}`));

    await waitFor(() => {
      expect(mockFetchToolResultArtifactStream).toHaveBeenCalledWith('tenant-1', objectReferenceId);
    });
    expect(mockDownloadBlob).toHaveBeenCalledWith(expect.any(Blob), 'Sales-rows.jsonl');
  });

  it('loads paged artifact rows through the read endpoint on demand', async () => {
    const objectReferenceId = '00000000-0000-0000-0000-000000000002';
    mockFetchToolResultArtifactRead.mockResolvedValue({
      id: 'artifact-row',
      tenant_id: 'tenant-1',
      run_id: null,
      tool_call_id: null,
      view_kind: 'table',
      ref_kind: 'data_ref',
      project_id: 'project-1',
      path: '/artifacts/tool-results/table.jsonl',
      revision: 3,
      file_revision_id: 'revision-1',
      object_reference_id: objectReferenceId,
      content_hash: 'sha256:def',
      content_type: 'application/jsonl',
      size_bytes: 256000,
      created_at: '2026-07-10T00:00:00Z',
      content: {
        kind: 'json_rows',
        offset: 0,
        limit: 500,
        total_rows: 2,
        rows: [
          { region: 'north', amount: 42 },
          { region: 'south', amount: 84 },
        ],
      },
    });
    const message: IMessageAcpToolCall = {
      id: 'tool-result-table-read',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call',
          tool_call_id: 'tool-result-table-read',
          status: 'completed',
          title: 'Query sales',
          kind: 'execute',
          rawOutput: {
            views: [
              {
                kind: 'table',
                title: 'Sales rows',
                columns: [
                  { key: 'region', label: 'Region' },
                  { key: 'amount', label: 'Amount' },
                ],
                rows_preview: [{ region: 'east', amount: 1 }],
                data_ref: {
                  artifact_id: 'artifact-2',
                  object_reference_id: objectReferenceId,
                  content_type: 'application/jsonl',
                  content_hash: 'sha256:def',
                  size_bytes: 256000,
                },
              },
            ],
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByText('Query sales'));
    fireEvent.click(screen.getByTestId(`tool-result-artifact-preview-${objectReferenceId}`));

    await waitFor(() => {
      expect(mockFetchToolResultArtifactRead).toHaveBeenCalledWith('tenant-1', objectReferenceId, {
        offset: 0,
        limit: 500,
      });
    });
    expect(screen.getByText('Loaded rows 1-2 of 2')).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: 'north' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: '84' })).toBeInTheDocument();
  });

  it('renders static previews for JSON and file diff views without executing markup', () => {
    const message: IMessageAcpToolCall = {
      id: 'tool-result-json',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call',
          tool_call_id: 'tool-result-json',
          status: 'completed',
          title: 'Inspect result',
          kind: 'execute',
          rawOutput: {
            views: [
              {
                kind: 'json',
                title: 'JSON preview',
                value_preview: { status: 'ok', nested: { count: 2 } },
              },
              {
                kind: 'file_diff',
                title: 'Patch',
                files: [
                  {
                    path: '/workspace/report.md',
                    file_diff: '--- a/report.md\n+++ b/report.md\n@@\n-old\n+new',
                  },
                ],
              },
            ],
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByText('Inspect result'));

    expect(screen.getByText(/"status": "ok"/)).toBeInTheDocument();
    expect(screen.getByText(/# \/workspace\/report.md/)).toBeInTheDocument();
    expect(screen.getByText(/-old/)).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /workspace/ })).not.toBeInTheDocument();
  });

  it('renders chart and map runtime previews with static fallback', async () => {
    const message: IMessageAcpToolCall = {
      id: 'tool-result-visual',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call',
          tool_call_id: 'tool-result-visual',
          status: 'completed',
          title: 'Visualize',
          kind: 'execute',
          rawOutput: {
            views: [
              {
                kind: 'chart',
                title: 'Sales chart',
                spec_kind: 'vega_lite',
                spec: {
                  mark: 'bar',
                  data: {
                    values: [
                      { region: 'east', amount: 10 },
                      { region: 'west', amount: 20 },
                    ],
                  },
                  encoding: {
                    x: { field: 'region' },
                    y: { field: 'amount' },
                  },
                },
              },
              {
                kind: 'map',
                title: 'Route',
                format: 'geojson',
                data_ref: {
                  artifact_id: 'map-artifact',
                  content_type: 'application/geo+json',
                  content_hash: 'sha256:def',
                  size_bytes: 512,
                },
                data_preview: {
                  type: 'FeatureCollection',
                  features: [
                    { type: 'Feature', geometry: { type: 'Point', coordinates: [120, 30] } },
                    { type: 'Feature', geometry: { type: 'Point', coordinates: [121, 31] } },
                  ],
                },
              },
            ],
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByText('Visualize'));

    expect(screen.getByTestId('tool-result-chart-preview')).toHaveTextContent('2 points · x: region · y: amount');
    expect(screen.getByTestId('tool-result-map-preview')).toHaveTextContent('2 coordinates');
    expect(screen.queryByText(/"mark": "bar"/)).not.toBeInTheDocument();

    await waitFor(() => {
      expect(runtimeMocks.vegaEmbed).toHaveBeenCalledWith(
        expect.any(HTMLDivElement),
        expect.objectContaining({ mark: 'bar' }),
        expect.objectContaining({ actions: false, renderer: 'svg' })
      );
    });
    await waitFor(() => {
      expect(runtimeMocks.mapConstructor).toHaveBeenCalledWith(
        expect.objectContaining({
          container: expect.any(HTMLDivElement),
          attributionControl: false,
          interactive: true,
        })
      );
    });
    expect(screen.getByTestId('tool-result-vega-runtime')).toHaveAttribute('data-runtime-state', 'ready');
    expect(screen.getByTestId('tool-result-maplibre-runtime')).toHaveAttribute('data-runtime-state', 'ready');
    const mapOptions = runtimeMocks.mapConstructor.mock.calls[0][0] as {
      style: { sources: Record<string, { data: unknown }> };
    };
    expect(mapOptions.style.sources['tool-result'].data).toEqual(
      expect.objectContaining({
        type: 'FeatureCollection',
      })
    );
    expect(runtimeMocks.mapInstances[0].fitBounds).toHaveBeenCalledWith(
      [
        [120, 30],
        [121, 31],
      ],
      expect.objectContaining({ duration: 0 })
    );
  });

  it('keeps Vega-Lite specs with external data URLs on static fallback', async () => {
    const message: IMessageAcpToolCall = {
      id: 'tool-result-external-chart',
      conversation_id: 'conv-1',
      type: 'acp_tool_call',
      content: {
        sessionId: 'sess-1',
        update: {
          sessionUpdate: 'tool_call',
          tool_call_id: 'tool-result-external-chart',
          status: 'completed',
          title: 'External chart',
          kind: 'execute',
          rawOutput: {
            views: [
              {
                kind: 'chart',
                title: 'External data chart',
                spec_kind: 'vega_lite',
                spec: {
                  mark: 'bar',
                  data: { url: 'https://example.test/data.json' },
                  encoding: {
                    x: { field: 'region' },
                    y: { field: 'amount' },
                  },
                },
              },
            ],
          },
        },
      },
    };

    render(<MessageToolGroupSummary messages={[message]} />);
    fireEvent.click(screen.getByText('View Steps · 1'));
    fireEvent.click(screen.getByText('External chart'));

    expect(screen.getByText(/example\.test\/data\.json/)).toBeInTheDocument();
    expect(screen.getByTestId('tool-result-vega-runtime')).toHaveAttribute('data-runtime-state', 'fallback');
    await Promise.resolve();
    expect(runtimeMocks.vegaEmbed).not.toHaveBeenCalled();
  });
});
