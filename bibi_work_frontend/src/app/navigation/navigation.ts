import {
  ClipboardCheck,
  Database,
  FileSearch,
  GitBranch,
  KeyRound,
  Layers3,
  Settings,
  ShieldCheck,
  TerminalSquare
} from "lucide-react";
import type { ComponentType } from "react";
import type { Me } from "../../shared/contracts/platform";
import type { I18nKey } from "../../shared/i18n";

export type RouteId =
  | "workbench"
  | "projects"
  | "catalog"
  | "agents"
  | "skills"
  | "tools"
  | "mcp"
  | "llm"
  | "workflows"
  | "memories"
  | "approvals"
  | "audit"
  | "sessions"
  | "settings";

export interface NavigationItem {
  id: RouteId;
  labelKey: I18nKey;
  icon: ComponentType<{ size?: number; strokeWidth?: number }>;
  groupKey?: I18nKey;
  capability?: string;
  adminOnly?: boolean;
  hiddenFromNav?: boolean;
}

export const navigationItems: NavigationItem[] = [
  { id: "workbench", labelKey: "nav.workbench", icon: TerminalSquare, groupKey: "nav.group.work" },
  {
    id: "projects",
    labelKey: "nav.projects",
    icon: FileSearch,
    groupKey: "nav.group.work",
    capability: "project:read"
  },
  {
    id: "catalog",
    labelKey: "nav.catalog",
    icon: Layers3,
    groupKey: "nav.group.resources",
    capability: "catalog:manage",
    adminOnly: true
  },
  {
    id: "agents",
    labelKey: "nav.agents",
    icon: Layers3,
    groupKey: "nav.group.resources",
    capability: "catalog:manage",
    adminOnly: true,
    hiddenFromNav: true
  },
  {
    id: "skills",
    labelKey: "nav.skills",
    icon: Layers3,
    groupKey: "nav.group.resources",
    capability: "catalog:manage",
    adminOnly: true,
    hiddenFromNav: true
  },
  {
    id: "tools",
    labelKey: "nav.tools",
    icon: Layers3,
    groupKey: "nav.group.resources",
    capability: "catalog:manage",
    adminOnly: true,
    hiddenFromNav: true
  },
  {
    id: "mcp",
    labelKey: "nav.mcp",
    icon: Layers3,
    groupKey: "nav.group.resources",
    capability: "catalog:manage",
    adminOnly: true,
    hiddenFromNav: true
  },
  {
    id: "llm",
    labelKey: "nav.llm",
    icon: Layers3,
    groupKey: "nav.group.resources",
    capability: "catalog:manage",
    adminOnly: true
  },
  {
    id: "workflows",
    labelKey: "nav.workflows",
    icon: GitBranch,
    groupKey: "nav.group.work",
    capability: "workflow:manage"
  },
  {
    id: "memories",
    labelKey: "nav.memories",
    icon: Database,
    groupKey: "nav.group.governance",
    capability: "memory:govern"
  },
  {
    id: "approvals",
    labelKey: "nav.approvals",
    icon: ClipboardCheck,
    groupKey: "nav.group.governance",
    capability: "approval:decide"
  },
  {
    id: "audit",
    labelKey: "nav.audit",
    icon: ShieldCheck,
    groupKey: "nav.group.governance",
    capability: "audit:read",
    adminOnly: true
  },
  { id: "sessions", labelKey: "nav.sessions", icon: KeyRound, groupKey: "nav.group.system" },
  { id: "settings", labelKey: "nav.settings", icon: Settings, groupKey: "nav.group.system" }
];

export function visibleNavigationItems(me: Me): NavigationItem[] {
  return permittedNavigationItems(me).filter((item) => !item.hiddenFromNav);
}

export function permittedNavigationItems(me: Me): NavigationItem[] {
  const capabilities = new Set(me.capabilities);
  const roles = new Set(me.roles);
  const isAdmin = roles.has("platform_admin") || roles.has("tenant_admin");
  return navigationItems.filter((item) => {
    if (item.adminOnly && !isAdmin && !capabilities.has(item.capability ?? "")) {
      return false;
    }
    return !item.capability || capabilities.has(item.capability);
  });
}

export function routeTitle(routeId: RouteId): I18nKey {
  return navigationItems.find((item) => item.id === routeId)?.labelKey ?? "nav.workbench";
}

export function routeNavId(routeId: RouteId): RouteId {
  return ["agents", "skills", "tools", "mcp"].includes(routeId) ? "catalog" : routeId;
}
