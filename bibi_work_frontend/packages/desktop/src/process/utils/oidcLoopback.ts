import http, { type Server } from 'http';
import { PROTOCOL_SCHEME } from './deepLink';

export const DESKTOP_OIDC_CALLBACK_PORT = 48123;
export const DESKTOP_OIDC_CALLBACK_PATH = '/callback';

export type OidcLoopbackCallback = {
  code?: string;
  error?: string;
  error_description?: string;
  state?: string;
};

export type OidcLoopbackHandle = {
  port: number;
  stop: () => Promise<void>;
};

export type OidcLoopbackOptions = {
  onCallback?: (callback: OidcLoopbackCallback) => Promise<void> | void;
  port?: number;
};

export function parseOidcCallbackFromUrl(callbackUrl: string): OidcLoopbackCallback | null {
  const url = new URL(callbackUrl, `http://127.0.0.1:${DESKTOP_OIDC_CALLBACK_PORT}`);
  if (url.pathname !== DESKTOP_OIDC_CALLBACK_PATH) return null;

  const callback: OidcLoopbackCallback = {};
  for (const key of ['code', 'state', 'error', 'error_description'] as const) {
    const value = url.searchParams.get(key);
    if (value) callback[key] = value;
  }
  if (!callback.code && !callback.error) return null;
  return callback;
}

export function buildOidcDeepLinkFromCallbackUrl(callbackUrl: string): string | null {
  const callback = parseOidcCallbackFromUrl(callbackUrl);
  if (!callback) return null;

  const target = new URL(`${PROTOCOL_SCHEME}://auth/callback`);
  for (const key of ['code', 'state', 'error', 'error_description']) {
    const value = callback[key as keyof OidcLoopbackCallback];
    if (value) target.searchParams.set(key, value);
  }
  return target.toString();
}

function legacyDeepLinkCallbackHtml(deepLink: string): string {
  const safeDeepLink = JSON.stringify(deepLink);
  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>BiWork Sign In</title>
  </head>
  <body>
    <p>Authentication received. You can return to BiWork.</p>
    <script>
      window.location.replace(${safeDeepLink});
    </script>
  </body>
</html>`;
}

function mainProcessCallbackHtml(ok: boolean, message: string): string {
  const safeMessage = message.replace(/[<>&"]/g, (char) => {
    switch (char) {
      case '<':
        return '&lt;';
      case '>':
        return '&gt;';
      case '&':
        return '&amp;';
      case '"':
        return '&quot;';
      default:
        return char;
    }
  });
  return `<!doctype html>
<html>
  <head>
    <meta charset="utf-8">
    <title>BiWork Sign In</title>
  </head>
  <body>
    <p>${safeMessage}</p>
    ${ok ? '<script>window.close();</script>' : ''}
  </body>
</html>`;
}

export async function startOidcLoopbackServer(options: OidcLoopbackOptions = {}): Promise<OidcLoopbackHandle> {
  const port = options.port ?? DESKTOP_OIDC_CALLBACK_PORT;
  const server: Server = http.createServer((req, res) => {
    void (async () => {
      const callback = parseOidcCallbackFromUrl(req.url ?? '/');
      if (!callback) {
        res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
        res.end('BiWork OIDC callback route not found');
        return;
      }

      if (options.onCallback) {
        try {
          await options.onCallback(callback);
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
          res.end(mainProcessCallbackHtml(true, 'Authentication complete. You can return to BiWork.'));
        } catch (error) {
          const message = error instanceof Error ? error.message : String(error);
          res.writeHead(400, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
          res.end(mainProcessCallbackHtml(false, `Authentication failed: ${message}`));
        }
        return;
      }

      const deepLink = buildOidcDeepLinkFromCallbackUrl(req.url ?? '/');
      if (!deepLink) {
        res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
        res.end('BiWork OIDC callback route not found');
        return;
      }

      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
      res.end(legacyDeepLinkCallbackHtml(deepLink));
    })().catch((error) => {
      const message = error instanceof Error ? error.message : String(error);
      if (!res.headersSent) {
        res.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' });
      }
      res.end(`BiWork OIDC callback failed: ${message}`);
    });
  });

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => {
      server.off('error', reject);
      resolve();
    });
  });

  const actualPort = (server.address() as { port: number } | null)?.port ?? port;
  return {
    port: actualPort,
    stop: () =>
      new Promise<void>((resolve) => {
        server.close(() => resolve());
      }),
  };
}
