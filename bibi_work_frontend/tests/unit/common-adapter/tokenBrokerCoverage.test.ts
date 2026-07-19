/**
 * @vitest-environment node
 */

import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const repoRoot = process.cwd();

type NetworkPrimitive = 'fetch' | 'XMLHttpRequest' | 'WebSocket' | 'EventSource';

type NetworkSite = {
  file: string;
  line: number;
  column: number;
  primitive: NetworkPrimitive;
};

const sourceRoots = ['packages/desktop/src/common', 'packages/desktop/src/renderer'];

const allowedNetworkPrimitiveFiles: Record<NetworkPrimitive, ReadonlySet<string>> = {
  fetch: new Set([
    'packages/desktop/src/common/adapter/httpBridge.ts',
    'packages/desktop/src/common/config/configService.ts',
    'packages/desktop/src/common/platform/ElectronPlatformServices.ts',
    'packages/desktop/src/common/platform/NodePlatformServices.ts',
    'packages/desktop/src/common/platform/index.ts',
    'packages/desktop/src/renderer/api/client.ts',
    'packages/desktop/src/renderer/pages/conversation/Messages/components/MessageToolGroup.tsx',
    'packages/desktop/src/renderer/pages/conversation/Preview/components/PreviewPanel/PreviewPanel.tsx',
  ]),
  XMLHttpRequest: new Set([
    'packages/desktop/src/renderer/services/FileService.ts',
    'packages/desktop/src/renderer/services/SpeechToTextService.ts',
  ]),
  WebSocket: new Set([
    'packages/desktop/src/common/adapter/browser.ts',
    'packages/desktop/src/common/adapter/httpBridge.ts',
    'packages/desktop/src/renderer/services/speech/SpeechStreamClient.ts',
  ]),
  EventSource: new Set(),
};

const authAwareBackendFiles: Record<string, string[]> = {
  'packages/desktop/src/common/adapter/browser.ts': ['getAccessToken', 'subscribeAccessToken', 'access_token'],
  'packages/desktop/src/common/adapter/httpBridge.ts': ['getAuthorizationHeaders', 'access_token'],
  'packages/desktop/src/common/config/configService.ts': ['getAccessToken', 'getAuthorizationHeaders'],
  'packages/desktop/src/renderer/api/client.ts': ['getAuthorizationHeaders'],
  'packages/desktop/src/renderer/services/FileService.ts': ['getAuthorizationHeaders'],
  'packages/desktop/src/renderer/services/SpeechToTextService.ts': ['getAuthorizationHeaders'],
  'packages/desktop/src/renderer/services/speech/SpeechStreamClient.ts': ['peekAccessToken', 'accessToken'],
};

function readSource(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), 'utf8');
}

function toRepoPath(path: string): string {
  return relative(repoRoot, path).split('\\').join('/');
}

function listSourceFiles(dir: string): string[] {
  const entries = readdirSync(dir)
    .map((name) => join(dir, name))
    .sort((a, b) => a.localeCompare(b));

  return entries.flatMap((entry) => {
    const stat = statSync(entry);
    if (stat.isDirectory()) {
      return listSourceFiles(entry);
    }
    return /\.(?:ts|tsx)$/.test(entry) && !entry.endsWith('.d.ts') ? [entry] : [];
  });
}

function expressionName(expression: ts.Expression): string | null {
  if (ts.isIdentifier(expression)) {
    return expression.text;
  }
  if (ts.isPropertyAccessExpression(expression)) {
    return expression.name.text;
  }
  return null;
}

function scriptKindFor(path: string): ts.ScriptKind {
  return path.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
}

function extractNetworkSites(): NetworkSite[] {
  const files = sourceRoots.flatMap((root) => listSourceFiles(resolve(repoRoot, root)));
  const sites: NetworkSite[] = [];

  for (const filePath of files) {
    const source = readFileSync(filePath, 'utf8');
    const sourceFile = ts.createSourceFile(filePath, source, ts.ScriptTarget.Latest, true, scriptKindFor(filePath));
    const file = toRepoPath(filePath);

    function record(node: ts.Node, primitive: NetworkPrimitive): void {
      const position = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
      sites.push({
        file,
        line: position.line + 1,
        column: position.character + 1,
        primitive,
      });
    }

    function visit(node: ts.Node): void {
      if (ts.isCallExpression(node)) {
        const name = expressionName(node.expression);
        if (name === 'fetch' || name === 'WebSocket' || name === 'EventSource') {
          record(node, name);
        }
      }
      if (ts.isNewExpression(node)) {
        const name = expressionName(node.expression);
        if (name === 'XMLHttpRequest' || name === 'WebSocket' || name === 'EventSource') {
          record(node, name);
        }
      }
      ts.forEachChild(node, visit);
    }

    visit(sourceFile);
  }

  return sites.sort((a, b) =>
    `${a.primitive} ${a.file}:${a.line}:${a.column}`.localeCompare(`${b.primitive} ${b.file}:${b.line}:${b.column}`)
  );
}

describe('token broker coverage', () => {
  it('does not expose the legacy WeChat SSE login path from the renderer', () => {
    const source = readSource(
      'packages/desktop/src/renderer/components/settings/SettingsModal/contents/channels/WeixinConfigForm.tsx'
    );

    expect(source).not.toContain('new EventSource');
    expect(source).not.toContain('/api/channel/weixin/login');
  });

  it('keeps renderer/common network primitives behind explicit token-aware boundaries', () => {
    const unexpected = extractNetworkSites()
      .filter((site) => !allowedNetworkPrimitiveFiles[site.primitive].has(site.file))
      .map((site) => `${site.primitive} ${site.file}:${site.line}:${site.column}`);

    expect(unexpected).toEqual([]);
  });

  it('keeps direct backend network boundary files wired to the auth token broker', () => {
    const missing = Object.entries(authAwareBackendFiles).flatMap(([file, requiredTokens]) => {
      const source = readSource(file);
      return requiredTokens.filter((token) => !source.includes(token)).map((token) => `${file} missing ${token}`);
    });

    expect(missing).toEqual([]);
  });
});
