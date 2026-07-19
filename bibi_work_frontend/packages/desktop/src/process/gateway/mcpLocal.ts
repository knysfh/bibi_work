import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport, getDefaultEnvironment } from '@modelcontextprotocol/sdk/client/stdio.js';

const DEFAULT_TIMEOUT_MS = 30_000;
const MAX_TIMEOUT_MS = 120_000;
const MAX_ARGS = 128;
const MAX_ENV = 128;

export class LocalMcpError extends Error {
  constructor(
    public readonly code: string,
    message: string,
    public readonly details: Record<string, unknown> = {}
  ) {
    super(message);
    this.name = 'LocalMcpError';
  }
}

type StdioTransportInput = {
  type: 'stdio';
  command: string;
  args?: string[];
  env?: Record<string, string>;
  timeout_ms?: number;
};

const requiredCommand = (value: unknown): string => {
  if (typeof value !== 'string' || !value.trim() || value.includes('\0')) {
    throw new LocalMcpError('MCP_STDIO_COMMAND_REQUIRED', 'MCP stdio command is required');
  }
  return value.trim();
};

const normalizedArgs = (value: unknown): string[] => {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > MAX_ARGS || value.some((entry) => typeof entry !== 'string')) {
    throw new LocalMcpError('MCP_STDIO_ARGS_INVALID', 'MCP stdio args must be a bounded string array');
  }
  return value as string[];
};

const resolvedEnvironment = (value: unknown): Record<string, string> => {
  if (value === undefined) return getDefaultEnvironment();
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new LocalMcpError('MCP_STDIO_ENV_INVALID', 'MCP stdio env must be an object');
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length > MAX_ENV) {
    throw new LocalMcpError('MCP_STDIO_ENV_INVALID', 'MCP stdio env exceeds the supported entry limit');
  }
  const env = getDefaultEnvironment();
  for (const [name, reference] of entries) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || typeof reference !== 'string') {
      throw new LocalMcpError('MCP_STDIO_ENV_INVALID', 'MCP stdio env names and references are invalid');
    }
    const match = /^env:\/\/([A-Za-z_][A-Za-z0-9_]*)$/.exec(reference.trim());
    if (!match) {
      throw new LocalMcpError(
        'MCP_STDIO_ENV_SECRET_REF_REQUIRED',
        `MCP stdio env ${name} must use env://NAME instead of an inline value`
      );
    }
    const resolved = process.env[match[1]];
    if (resolved === undefined) {
      throw new LocalMcpError('MCP_STDIO_ENV_UNAVAILABLE', `MCP stdio env reference is unavailable: ${match[1]}`);
    }
    env[name] = resolved;
  }
  return env;
};

const timeoutMilliseconds = (value: unknown): number => {
  if (value === undefined) return DEFAULT_TIMEOUT_MS;
  if (!Number.isInteger(value) || Number(value) < 1_000 || Number(value) > MAX_TIMEOUT_MS) {
    throw new LocalMcpError('MCP_TIMEOUT_INVALID', 'MCP timeout must be between 1000 and 120000 milliseconds');
  }
  return Number(value);
};

export async function discoverLocalStdioMcpTools(input: unknown): Promise<{
  tools: Array<{
    name: string;
    description?: string;
    input_schema: unknown;
    annotations?: unknown;
    _meta?: unknown;
  }>;
}> {
  return withLocalStdioClient(input, 'probe', async (client, timeout) => {
    const result = await client.listTools(undefined, { timeout, maxTotalTimeout: timeout });
    return {
      tools: result.tools.map((tool) => ({
        name: tool.name,
        description: tool.description,
        input_schema: tool.inputSchema,
        annotations: tool.annotations,
        _meta: tool._meta,
      })),
    };
  });
}

export async function callLocalStdioMcpTool(
  input: unknown,
  toolName: unknown,
  toolArguments: unknown
): Promise<Record<string, unknown>> {
  if (typeof toolName !== 'string' || !toolName.trim()) {
    throw new LocalMcpError('MCP_TOOL_NAME_REQUIRED', 'MCP tool name is required');
  }
  if (!toolArguments || typeof toolArguments !== 'object' || Array.isArray(toolArguments)) {
    throw new LocalMcpError('MCP_TOOL_ARGUMENTS_INVALID', 'MCP tool arguments must be an object');
  }
  return withLocalStdioClient(input, 'executor', async (client, timeout) => {
    return client.callTool({ name: toolName.trim(), arguments: toolArguments as Record<string, unknown> }, undefined, {
      timeout,
      maxTotalTimeout: timeout,
    });
  });
}

async function withLocalStdioClient<T>(
  input: unknown,
  clientRole: 'probe' | 'executor',
  operation: (client: Client, timeout: number) => Promise<T>
): Promise<T> {
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    throw new LocalMcpError('MCP_STDIO_CONFIG_INVALID', 'MCP stdio transport is required');
  }
  const transportInput = input as Partial<StdioTransportInput>;
  if (transportInput.type !== 'stdio') {
    throw new LocalMcpError('MCP_STDIO_CONFIG_INVALID', 'MCP stdio transport is required');
  }
  const command = requiredCommand(transportInput.command);
  const args = normalizedArgs(transportInput.args);
  const env = resolvedEnvironment(transportInput.env);
  const timeout = timeoutMilliseconds(transportInput.timeout_ms);
  const transport = new StdioClientTransport({ command, args, env, stderr: 'pipe' });
  const client = new Client({ name: `bibi-work-desktop-mcp-${clientRole}`, version: '1.0.0' }, { capabilities: {} });
  try {
    await Promise.race([
      client.connect(transport),
      new Promise<never>((_, reject) =>
        setTimeout(() => reject(new LocalMcpError('MCP_TIMEOUT', 'MCP stdio initialization timed out')), timeout)
      ),
    ]);
    return await operation(client, timeout);
  } catch (error) {
    if (error instanceof LocalMcpError) throw error;
    const candidate = error as NodeJS.ErrnoException;
    if (candidate.code === 'ENOENT') {
      throw new LocalMcpError('MCP_COMMAND_NOT_FOUND', `MCP command not found: ${command}`, { command });
    }
    if (candidate.code === 'EACCES') {
      throw new LocalMcpError('MCP_COMMAND_PERMISSION_DENIED', `MCP command is not executable: ${command}`, {
        command,
      });
    }
    throw new LocalMcpError('MCP_CONNECTION_FAILED', error instanceof Error ? error.message : String(error), {
      command,
    });
  } finally {
    await client.close().catch((_error: unknown): undefined => undefined);
  }
}
