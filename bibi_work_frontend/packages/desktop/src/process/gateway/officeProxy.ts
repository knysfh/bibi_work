/**
 * @license
 * Copyright 2025 AionUi (aionui.com)
 * SPDX-License-Identifier: Apache-2.0
 */

import type { IncomingMessage, ServerResponse } from 'http';
import { OfficeLocalRouteError } from './officeLocal';

const HOP_BY_HOP_HEADERS = new Set([
  'connection',
  'content-length',
  'host',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
]);

export function resolveOfficeWatchProxyTarget(url: URL): URL {
  const match = url.pathname.match(/^\/api\/(?:ppt-proxy|office-watch-proxy)\/(\d{1,5})(\/.*)?$/);
  if (!match) {
    throw new OfficeLocalRouteError(404, 'OFFICE_PROXY_ROUTE_NOT_FOUND', 'desktop office proxy route not found');
  }
  const port = Number(match[1]);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new OfficeLocalRouteError(400, 'INVALID_OFFICE_PROXY_PORT', 'office proxy port is invalid');
  }
  const suffix = match[2] && match[2] !== '/' ? match[2] : '/';
  return new URL(`http://127.0.0.1:${port}${suffix}${url.search}`);
}

export async function proxyOfficeWatchRequest(req: IncomingMessage, res: ServerResponse, url: URL): Promise<void> {
  const target = resolveOfficeWatchProxyTarget(url);
  const proxyResponse = await fetch(target, { method: req.method === 'HEAD' ? 'HEAD' : 'GET' });
  const headers: Record<string, string> = {};
  proxyResponse.headers.forEach((value, key) => {
    if (!HOP_BY_HOP_HEADERS.has(key.toLowerCase())) {
      headers[key] = value;
    }
  });
  res.writeHead(proxyResponse.status, headers);
  if (req.method === 'HEAD') {
    res.end();
    return;
  }
  res.end(Buffer.from(await proxyResponse.arrayBuffer()));
}
