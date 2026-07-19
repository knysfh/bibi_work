import { startWebHost, type WebHostHandle } from '@biwork/web-host';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { openBrowserUrl, shouldAutoOpenBrowser } from './browser.js';

const DEFAULT_PORT = 25808;
let currentHandle: WebHostHandle | null = null;

function resolveCliRoot(): string {
  const executableName = path.basename(process.execPath).toLowerCase();
  if (executableName === 'biwork-web' || executableName === 'biwork-web.exe') return path.dirname(process.execPath);
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
}

function parseArgs(argv: string[]): Map<string, string | true> {
  const flags = new Map<string, string | true>();
  for (let index = argv[0] === 'start' ? 1 : 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (!token.startsWith('--')) continue;
    const next = argv[index + 1];
    if (next && !next.startsWith('--')) {
      flags.set(token.slice(2), next);
      index += 1;
    } else {
      flags.set(token.slice(2), true);
    }
  }
  return flags;
}

function resolveBackendPort(flags: Map<string, string | true>): number {
  const flag = flags.get('backend-url');
  const raw = typeof flag === 'string' ? flag : (process.env.BIWORK_ENTERPRISE_BACKEND_URL ?? 'http://127.0.0.1:8361');
  const url = new URL(raw);
  if (url.protocol !== 'http:' || !['127.0.0.1', 'localhost', '::1', '[::1]'].includes(url.hostname)) {
    throw new Error('BiWork backend URL must be a loopback http:// URL');
  }
  return url.port ? Number(url.port) : 80;
}

async function main(): Promise<void> {
  const flags = parseArgs(process.argv.slice(2));
  const cliRoot = resolveCliRoot();
  const staticFlag = flags.get('static-dir');
  const staticDir = typeof staticFlag === 'string' ? path.resolve(staticFlag) : path.join(cliRoot, 'static');
  if (!fs.existsSync(staticDir)) throw new Error(`Static assets not found: ${staticDir}`);
  const portFlag = flags.get('port');
  const port = Number(typeof portFlag === 'string' ? portFlag : (process.env.BIWORK_PORT ?? DEFAULT_PORT));
  const allowRemote = flags.has('remote') || ['1', 'true'].includes(process.env.BIWORK_ALLOW_REMOTE ?? '');
  currentHandle = await startWebHost({
    app: { version: '0.0.0', isPackaged: true, resourcesPath: cliRoot, userDataPath: cliRoot },
    staticDir,
    port,
    allowRemote,
    backend: { kind: 'useExistingBackend', port: resolveBackendPort(flags) },
  });
  console.log(`BiWork WebUI ready: ${currentHandle.localUrl}`);
  if (
    shouldAutoOpenBrowser({
      allowRemote,
      env: process.env,
      openFlag: flags.has('open'),
      noOpenFlag: flags.has('no-open'),
    })
  ) {
    const result = openBrowserUrl(currentHandle.localUrl);
    if (!result.ok) console.warn(`[biwork-web] could not open browser: ${result.reason}`);
  }
}

async function shutdown(): Promise<void> {
  await currentHandle?.stop();
  process.exit(0);
}

process.on('SIGINT', () => void shutdown());
process.on('SIGTERM', () => void shutdown());
void main().catch((error) => {
  console.error('[biwork-web] startup failed:', error);
  process.exit(1);
});
