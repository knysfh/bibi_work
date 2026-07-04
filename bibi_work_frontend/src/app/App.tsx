import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { usePlatformApi } from "./providers";
import { AppShell } from "./app-shell/AppShell";
import { ErrorBoundary } from "./error-boundary/ErrorBoundary";
import { PlaceholderScreen } from "./PlaceholderScreen";
import { permittedNavigationItems, type RouteId } from "./navigation/navigation";
import { useLogoutMutation, useMeQuery } from "../features/auth/api/auth.queries";
import { LoginPanel } from "../features/auth/components/LoginPanel";
import { ApprovalCenterScreen } from "../features/approvals/components/ApprovalCenterScreen";
import { CatalogManagementScreen } from "../features/catalog/components/CatalogManagementScreen";
import { LlmManagementScreen } from "../features/llm/components/LlmManagementScreen";
import { MemoryGovernanceScreen } from "../features/memories/components/MemoryGovernanceScreen";
import { ProjectWorkspaceScreen } from "../features/projects/components/ProjectWorkspaceScreen";
import { SessionDevicePanel } from "../features/session-device/components/SessionDevicePanel";
import { WorkbenchScreen } from "../features/workbench/screens/WorkbenchScreen";
import { useI18n } from "../shared/i18n";
import { EmptyState } from "../shared/ui";

export function App() {
  const { apiBaseUrl, desktopAuthApi, tokenProvider } = usePlatformApi();
  const { t } = useI18n();
  const queryClient = useQueryClient();
  const [hasToken, setHasToken] = useState<boolean | null>(null);
  const [activeRoute, setActiveRoute] = useState<RouteId>(() =>
    routeFromHash(window.location.hash)
  );

  useEffect(() => {
    if (!window.location.hash) {
      window.history.replaceState(null, "", routeToHash(activeRoute));
    }
    const syncRouteFromHash = () => setActiveRoute(routeFromHash(window.location.hash));
    window.addEventListener("popstate", syncRouteFromHash);
    window.addEventListener("hashchange", syncRouteFromHash);
    return () => {
      window.removeEventListener("popstate", syncRouteFromHash);
      window.removeEventListener("hashchange", syncRouteFromHash);
    };
  }, [activeRoute]);

  const changeRoute = useCallback((route: RouteId) => {
    setActiveRoute(route);
    const nextHash = routeToHash(route);
    if (window.location.hash !== nextHash) {
      window.history.pushState(null, "", nextHash);
    }
  }, []);

  useEffect(() => {
    let mounted = true;
    desktopAuthApi.loadTokenSet().then((tokenSet) => {
      if (mounted) {
        setHasToken(Boolean(tokenSet?.accessToken));
      }
    });
    return () => {
      mounted = false;
    };
  }, [desktopAuthApi]);

  const meQuery = useMeQuery(Boolean(hasToken));
  const logout = useLogoutMutation(async () => {
    await desktopAuthApi.clearTokenSet();
    setHasToken(false);
  });

  useEffect(() => {
    if (meQuery.error && "status" in meQuery.error && meQuery.error.status === 401) {
      desktopAuthApi.clearTokenSet().finally(() => {
        queryClient.clear();
        setHasToken(false);
      });
    }
  }, [desktopAuthApi, meQuery.error, queryClient]);

  const tenantId = meQuery.data?.tenantId;
  const deviceId = meQuery.data?.device.id;
  const bridgeUserAgent = meQuery.data?.session.userAgent ?? navigator.userAgent;

  useEffect(() => {
    if (!hasToken || !tenantId || !deviceId) {
      return;
    }
    let active = true;
    void tokenProvider.getAccessToken().then((accessToken) => {
      if (!active || !accessToken) {
        return;
      }
      void desktopAuthApi.localExecStartBridge({
        apiBaseUrl,
        accessToken,
        tenantId,
        deviceId,
        userAgent: bridgeUserAgent
      });
    });
    return () => {
      active = false;
      void desktopAuthApi.localExecStopBridge();
    };
  }, [apiBaseUrl, bridgeUserAgent, desktopAuthApi, deviceId, hasToken, tenantId, tokenProvider]);

  if (hasToken === null) {
    return <EmptyState title={t("app.loading")} detail={t("app.loadingDetail")} />;
  }

  if (!hasToken) {
    return (
      <LoginPanel
        onTokenSaved={() => {
          queryClient.clear();
          setHasToken(true);
        }}
      />
    );
  }

  if (meQuery.isLoading) {
    return <EmptyState title={t("app.loadingMe")} detail={t("app.loadingMeDetail")} />;
  }

  if (meQuery.error || !meQuery.data) {
    return (
      <LoginPanel
        onTokenSaved={() => {
          queryClient.clear();
          setHasToken(true);
        }}
      />
    );
  }

  const me = meQuery.data;
  const permittedRoutes = new Set(permittedNavigationItems(me).map((item) => item.id));
  const safeRoute = permittedRoutes.has(activeRoute) ? activeRoute : "workbench";
  const catalogInitialTab =
    safeRoute === "agents" || safeRoute === "skills" || safeRoute === "tools" || safeRoute === "mcp"
      ? safeRoute
      : "agents";

  return (
    <AppShell
      me={me}
      activeRoute={safeRoute}
      onRouteChange={changeRoute}
      onLogout={() => logout.mutate()}
    >
      <ErrorBoundary>
        {safeRoute === "workbench" ? <WorkbenchScreen me={me} /> : null}
        {safeRoute === "projects" ? <ProjectWorkspaceScreen me={me} /> : null}
        {safeRoute === "memories" ? <MemoryGovernanceScreen me={me} /> : null}
        {safeRoute === "approvals" ? <ApprovalCenterScreen me={me} /> : null}
        {safeRoute === "sessions" ? <SessionDevicePanel me={me} /> : null}
        {safeRoute === "catalog" ||
        safeRoute === "agents" ||
        safeRoute === "skills" ||
        safeRoute === "tools" ||
        safeRoute === "mcp" ? (
          <CatalogManagementScreen
            me={me}
            initialTab={catalogInitialTab}
            onTabRouteChange={changeRoute}
          />
        ) : null}
        {safeRoute === "llm" ? <LlmManagementScreen me={me} /> : null}
        {safeRoute === "workflows" ? <PlaceholderScreen titleKey="placeholder.workflows" /> : null}
        {safeRoute === "audit" ? <PlaceholderScreen titleKey="placeholder.audit" /> : null}
        {safeRoute === "settings" ? <PlaceholderScreen titleKey="placeholder.settings" /> : null}
      </ErrorBoundary>
    </AppShell>
  );
}

function routeToHash(route: RouteId): string {
  if (route === "catalog") {
    return "#/catalog";
  }
  if (["agents", "skills", "tools", "mcp"].includes(route)) {
    return `#/catalog/${route}`;
  }
  return `#/${route}`;
}

function routeFromHash(hash: string): RouteId {
  const path = hash.replace(/^#\/?/, "").replace(/\/+$/, "");
  if (!path) {
    return "workbench";
  }
  if (path === "catalog") {
    return "catalog";
  }
  if (path.startsWith("catalog/")) {
    const nested = path.slice("catalog/".length);
    if (nested === "llm") {
      return "llm";
    }
    if (isRouteId(nested) && ["agents", "skills", "tools", "mcp", "llm"].includes(nested)) {
      return nested;
    }
    return "catalog";
  }
  return isRouteId(path) ? path : "workbench";
}

function isRouteId(value: string): value is RouteId {
  return [
    "workbench",
    "projects",
    "catalog",
    "agents",
    "skills",
    "tools",
    "mcp",
    "llm",
    "workflows",
    "memories",
    "approvals",
    "audit",
    "sessions",
    "settings"
  ].includes(value);
}
