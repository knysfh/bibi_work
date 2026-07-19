/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import type { IMessageAcpToolCall } from '@/common/chat/chatLib';
import { normalizeAcpToolCall } from '@/common/chat/normalizeToolCall';
import { describe, expect, it } from 'vitest';

describe('normalizeAcpToolCall', () => {
  it('preserves generated image paths for grouped tool summaries', () => {
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

    const normalized = normalizeAcpToolCall(message);

    expect((normalized as { imagePath?: string } | undefined)?.imagePath).toBe(
      '/Users/test/.codex/generated_images/session/ig_test_image.png'
    );
  });

  it('normalizes Rust tool result views and artifact refs', () => {
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
                  object_reference_id: '00000000-0000-0000-0000-000000000001',
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

    const normalized = normalizeAcpToolCall(message);

    expect(normalized?.output).toBe('returned 238 rows');
    expect(normalized?.views).toEqual([
      {
        kind: 'table',
        title: 'Sales rows',
        summary: '1 preview row, 1 column',
        artifactRef: {
          artifactId: 'artifact-1',
          objectReferenceId: '00000000-0000-0000-0000-000000000001',
          contentType: 'application/jsonl',
          contentHash: 'sha256:abc',
          sizeBytes: 128000,
        },
        tablePreview: {
          columns: [{ key: 'region', label: 'Region', type: undefined }],
          rows: [{ region: 'east' }],
        },
        chartPreview: undefined,
        mapPreview: undefined,
        previewText: undefined,
      },
    ]);
  });

  it('keeps chart specs and geojson previews for renderer-only previews', () => {
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
                  data: { values: [{ region: 'east', amount: 10 }] },
                  encoding: { x: { field: 'region' }, y: { field: 'amount' } },
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
                  type: 'Feature',
                  geometry: { type: 'Point', coordinates: [120, 30] },
                },
              },
            ],
          },
        },
      },
    };

    const normalized = normalizeAcpToolCall(message);

    expect(normalized?.views?.[0]?.chartPreview).toEqual({
      mark: 'bar',
      data: { values: [{ region: 'east', amount: 10 }] },
      encoding: { x: { field: 'region' }, y: { field: 'amount' } },
    });
    expect(normalized?.views?.[0]?.previewText).toBeUndefined();
    expect(normalized?.views?.[1]?.mapPreview).toEqual({
      type: 'Feature',
      geometry: { type: 'Point', coordinates: [120, 30] },
    });
    expect(normalized?.views?.[1]?.previewText).toBeUndefined();
  });
});
