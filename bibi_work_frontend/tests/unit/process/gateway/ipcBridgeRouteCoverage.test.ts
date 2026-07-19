/**
 * @vitest-environment node
 */

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';
import { classifyDesktopGatewayRoute } from '@process/gateway/routeOwnership';

const repoRoot = process.cwd();
const IPC_BRIDGE_SOURCE = 'packages/desktop/src/common/adapter/ipcBridge.ts';
const RUST_ROUTE_OWNERSHIP_SOURCE =
  '../bibi_work_backend/src/features/agent_platform/handlers/biwork_route_ownership_service.rs';

const helperMethods: Record<string, string> = {
  httpGet: 'GET',
  httpPost: 'POST',
  httpPut: 'PUT',
  httpPatch: 'PATCH',
  httpDelete: 'DELETE',
};

type BridgeRoute = {
  method: string;
  path: string;
};

type ManifestRoute = {
  method: string;
  path: string;
  ownership: string;
  authority: string;
  pattern: RegExp;
};

function pathFromNode(node: ts.Node | undefined): string | null {
  if (!node) return null;
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (ts.isTemplateExpression(node)) {
    return node.templateSpans.reduce((path, span) => `${path}\${}${span.literal.text}`, node.head.text);
  }
  if (ts.isArrowFunction(node)) return pathFromNode(node.body);
  if (ts.isParenthesizedExpression(node)) return pathFromNode(node.expression);
  return null;
}

function concretePath(path: string): string {
  return path
    .split('?')[0]
    .replace(/\$\{\}/g, 'route-param')
    .replace(/\/route-param(\/|$)/g, '/00000000-0000-0000-0000-000000000001$1');
}

function extractBridgeRoutes(): BridgeRoute[] {
  const sourcePath = resolve(repoRoot, IPC_BRIDGE_SOURCE);
  const source = readFileSync(sourcePath, 'utf8');
  const sourceFile = ts.createSourceFile(sourcePath, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
  const routes = new Map<string, BridgeRoute>();

  function visit(node: ts.Node): void {
    if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
      const method = helperMethods[node.expression.text];
      if (method) {
        const path = pathFromNode(node.arguments[0]);
        if (path?.startsWith('/api/')) {
          const key = `${method} ${path}`;
          routes.set(key, { method, path });
        }
      }
    }
    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
  return Array.from(routes.values()).sort((a, b) => `${a.method} ${a.path}`.localeCompare(`${b.method} ${b.path}`));
}

function manifestPattern(path: string): RegExp {
  let pattern = '^';
  for (let index = 0; index < path.length; index += 1) {
    if (path[index] === '{') {
      const end = path.indexOf('}', index);
      const name = path.slice(index + 1, end);
      pattern += name.startsWith('*') ? '.*' : '[^/]+';
      index = end;
      continue;
    }
    pattern += path[index]!.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`${pattern}$`);
}

function extractRouteOwnershipManifestRoutes(): ManifestRoute[] {
  const source = sourceFromFile(RUST_ROUTE_OWNERSHIP_SOURCE);
  const start = source.indexOf('fn route_ownership_manifest()');
  const end = source.indexOf('pub async fn biwork_list_route_ownership', start);
  const manifestSource = source.slice(start, end);
  const routes: ManifestRoute[] = [];
  const routePattern =
    /\{\s*"method":\s*"([^"]+)",\s*"path":\s*"([^"]+)",\s*"ownership":\s*"([^"]+)",\s*"authority":\s*"([^"]+)",\s*"auth":\s*"[^"]+"\s*\}/g;
  for (const match of manifestSource.matchAll(routePattern)) {
    const [, method, path, ownership, authority] = match;
    routes.push({
      method,
      path,
      ownership,
      authority,
      pattern: manifestPattern(path),
    });
  }
  return routes;
}

function sourceFromFile(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), 'utf8');
}

describe('ipcBridge route ownership coverage', () => {
  it('classifies every renderer-facing HTTP bridge route', () => {
    const routes = extractBridgeRoutes();

    expect(routes.length).toBeGreaterThan(150);

    const unknown = routes
      .map((route) => ({
        ...route,
        concretePath: concretePath(route.path),
        ownership: classifyDesktopGatewayRoute(route.method, concretePath(route.path)).ownership,
      }))
      .filter((route) => route.ownership === 'UNKNOWN');

    expect(unknown).toEqual([]);
  });

  it('keeps renderer-facing route classifier aligned with the Rust ownership manifest', () => {
    const manifestRoutes = extractRouteOwnershipManifestRoutes();
    expect(manifestRoutes.length).toBeGreaterThan(150);

    const mismatches = extractBridgeRoutes().flatMap((route) => {
      const concrete = concretePath(route.path);
      const manifestRoute = manifestRoutes.find(
        (candidate) => candidate.method === route.method && candidate.pattern.test(concrete)
      );
      const classified = classifyDesktopGatewayRoute(route.method, concrete);
      if (!manifestRoute) {
        return [`${route.method} ${route.path}: missing manifest route`];
      }
      if (classified.ownership !== manifestRoute.ownership || classified.authority !== manifestRoute.authority) {
        return [
          `${route.method} ${route.path}: classifier=${classified.ownership}/${classified.authority}, manifest=${manifestRoute.ownership}/${manifestRoute.authority}`,
        ];
      }
      return [];
    });

    expect(mismatches).toEqual([]);
  });
});
