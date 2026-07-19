import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { callLocalStdioMcpTool, discoverLocalStdioMcpTools } from '@process/gateway/mcpLocal';
import type { LocalMcpError } from '@process/gateway/mcpLocal';
import { executeLocalMcpWorkItem } from '@process/gateway/mcpLocalWorker';

describe('desktop stdio MCP coordinator', () => {
  it('discovers tools through the official MCP stdio transport without a shell', async () => {
    const result = await discoverLocalStdioMcpTools({
      type: 'stdio',
      command: process.execPath,
      args: [resolve(process.cwd(), 'tests/fixtures/mcp/stdio-server.mjs')],
      timeout_ms: 5_000,
    });

    expect(result.tools).toEqual([
      expect.objectContaining({
        name: 'stdio_fixture_health',
        input_schema: expect.objectContaining({ type: 'object' }),
      }),
    ]);
  });

  it('rejects inline environment values before spawning the server', async () => {
    await expect(
      discoverLocalStdioMcpTools({
        type: 'stdio',
        command: process.execPath,
        env: { API_TOKEN: 'inline-secret' },
      })
    ).rejects.toMatchObject<Partial<LocalMcpError>>({ code: 'MCP_STDIO_ENV_SECRET_REF_REQUIRED' });
  });

  it('calls tools through the official MCP stdio transport', async () => {
    const result = await callLocalStdioMcpTool(
      {
        type: 'stdio',
        command: process.execPath,
        args: [resolve(process.cwd(), 'tests/fixtures/mcp/stdio-server.mjs')],
        timeout_ms: 5_000,
      },
      'stdio_fixture_health',
      { probe: true }
    );

    expect(result).toMatchObject({
      isError: false,
      structuredContent: { status: 'ok', arguments: { probe: true } },
    });
  });

  it('executes a device-queue stdio work item without exposing a shell', async () => {
    const result = await executeLocalMcpWorkItem({
      id: 'work-1',
      tenant_id: 'tenant-1',
      command: {
        protocol: 'local_runtime.v1',
        kind: 'mcp_stdio',
        transport: {
          type: 'stdio',
          command: process.execPath,
          args: [resolve(process.cwd(), 'tests/fixtures/mcp/stdio-server.mjs')],
          timeout_ms: 5_000,
        },
        tool: { name: 'stdio_fixture_health', arguments: { queued: true } },
      },
    });

    expect(result).toMatchObject({
      structuredContent: { status: 'ok', arguments: { queued: true } },
    });
  });
});
