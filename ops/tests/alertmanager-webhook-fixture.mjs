import { writeFileSync } from 'node:fs';
import { createServer } from 'node:http';

const port = Number(process.argv[2]);
const outputPath = process.argv[3];

if (!Number.isInteger(port) || !outputPath) {
  throw new Error('usage: node alertmanager-webhook-fixture.mjs <port> <output-path>');
}

createServer((request, response) => {
  const chunks = [];
  request.on('data', (chunk) => chunks.push(chunk));
  request.on('end', () => {
    if (request.method !== 'POST' || request.url !== '/alerts') {
      response.writeHead(404).end();
      return;
    }
    writeFileSync(outputPath, Buffer.concat(chunks));
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end('{}');
  });
}).listen(port, '0.0.0.0');
