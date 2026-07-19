import readline from 'node:readline';

const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

lines.on('line', (line) => {
  const message = JSON.parse(line);
  if (message.id === undefined) return;
  let result = {};
  if (message.method === 'initialize') {
    result = {
      protocolVersion: message.params?.protocolVersion || '2025-06-18',
      capabilities: { tools: {} },
      serverInfo: { name: 'bibi-work-stdio-fixture', version: '1.0.0' },
    };
  } else if (message.method === 'tools/list') {
    result = {
      tools: [
        {
          name: 'stdio_fixture_health',
          description: 'Return deterministic stdio fixture health.',
          inputSchema: { type: 'object', properties: {}, additionalProperties: false },
          annotations: { readOnlyHint: true, destructiveHint: false },
        },
      ],
    };
  } else if (message.method === 'tools/call') {
    result = {
      content: [
        {
          type: 'text',
          text: JSON.stringify({ status: 'ok', arguments: message.params?.arguments || {} }),
        },
      ],
      structuredContent: { status: 'ok', arguments: message.params?.arguments || {} },
      isError: false,
    };
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id: message.id, result })}\n`);
});
