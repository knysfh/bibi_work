import type { WebHostOptions, WebHostHandle } from './types.js';

export type { AppMetadata, WebHostOptions, WebHostHandle } from './types.js';
export { startStaticServer, stopStaticServer } from './static-server.js';
export type { StaticServerOptions, StaticServerHandle } from './static-server.js';

/**
 * Start WebHost (main entry point).
 *
 * Starts the static server and proxies API traffic to an existing BiWork backend.
 * persistent configuration — callers (Electron main process, `bun run webui`
 * CLI) are responsible for resolving port / allowRemote from their own source
 * of truth (Electron ProcessConfig, CLI flags, env vars).
 */
export async function startWebHost(opts: WebHostOptions): Promise<WebHostHandle> {
  const { startStaticServer } = await import('./static-server.js');

  const staticHandle = await startStaticServer({
    staticDir: opts.staticDir,
    backendPort: opts.backend.port,
    port: opts.port,
    allowRemote: opts.allowRemote ?? false,
  });

  // 3. Return combined handle
  return {
    port: staticHandle.port,
    backendPort: opts.backend.port,
    url: staticHandle.url,
    localUrl: staticHandle.localUrl,
    networkUrl: staticHandle.networkUrl,
    lanIP: staticHandle.lanIP,
    async stop() {
      await staticHandle.stop();
    },
  };
}
