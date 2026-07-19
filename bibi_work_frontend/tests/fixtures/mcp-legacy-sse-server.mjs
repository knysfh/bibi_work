import { createServer } from 'node:http';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { SSEServerTransport } from '@modelcontextprotocol/sdk/server/sse.js';

const port = Number(process.env.MCP_LEGACY_SSE_PORT ?? '39173');
const transports = new Map();

function createMcpServer() {
  const server = new McpServer({ name: 'bibi-legacy-sse-fixture', version: '1.0.0' });
  server.tool('legacy_echo', 'Official SDK legacy SSE fixture', async () => ({
    content: [{ type: 'text', text: 'legacy-ok' }],
  }));
  return server;
}

const httpServer = createServer(async (request, response) => {
  const url = new URL(request.url ?? '/', `http://127.0.0.1:${port}`);
  if (request.method === 'GET' && url.pathname === '/sse') {
    const transport = new SSEServerTransport('/messages', response, {
      enableDnsRebindingProtection: true,
      allowedHosts: [`127.0.0.1:${port}`, `localhost:${port}`],
    });
    transports.set(transport.sessionId, transport);
    response.on('close', () => transports.delete(transport.sessionId));
    await createMcpServer().connect(transport);
    return;
  }
  if (request.method === 'POST' && url.pathname === '/messages') {
    const transport = transports.get(url.searchParams.get('sessionId'));
    if (!transport) {
      response.writeHead(404).end();
      return;
    }
    await transport.handlePostMessage(request, response);
    return;
  }
  response.writeHead(404).end();
});

httpServer.listen(port, '127.0.0.1', () => {
  process.stdout.write(`legacy-sse-ready:${port}\n`);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => httpServer.close(() => process.exit(0)));
}
