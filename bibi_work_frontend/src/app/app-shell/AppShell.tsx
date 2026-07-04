import { ChevronDown, Globe2, LogOut, PanelLeftClose, PanelLeftOpen } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Me } from "../../shared/contracts/platform";
import { availableLanguages, languageLabel, useI18n, type LanguageCode } from "../../shared/i18n";
import { Button, StatusPill } from "../../shared/ui";
import {
  routeNavId,
  routeTitle,
  type NavigationItem,
  type RouteId,
  visibleNavigationItems
} from "../navigation/navigation";

export function AppShell({
  me,
  activeRoute,
  onRouteChange,
  onLogout,
  children
}: {
  me: Me;
  activeRoute: RouteId;
  onRouteChange: (route: RouteId) => void;
  onLogout: () => void;
  children: ReactNode;
}) {
  const items = visibleNavigationItems(me);
  const { language, setLanguage, t } = useI18n();
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const accountMenuRef = useRef<HTMLDivElement | null>(null);
  const activeNavId = routeNavId(activeRoute);
  const groupedItems = groupNavigationItems(items);
  const displayName = me.user.displayName ?? me.user.username ?? t("app.currentUser");

  useEffect(() => {
    if (!accountMenuOpen) {
      return;
    }
    function closeOnOutsidePointer(event: PointerEvent) {
      const target = event.target;
      if (target instanceof Node && !accountMenuRef.current?.contains(target)) {
        setAccountMenuOpen(false);
      }
    }
    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setAccountMenuOpen(false);
      }
    }
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [accountMenuOpen]);

  return (
    <div className={`app-shell ${sidebarCollapsed ? "sidebar-collapsed" : ""}`}>
      <aside className="sidebar">
        <div className="sidebar-toolbar">
          <Button
            size="icon"
            variant="ghost"
            className="sidebar-toggle"
            aria-label={t(sidebarCollapsed ? "app.sidebar.expand" : "app.sidebar.collapse")}
            title={t(sidebarCollapsed ? "app.sidebar.expand" : "app.sidebar.collapse")}
            aria-expanded={!sidebarCollapsed}
            icon={sidebarCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
            onClick={() => setSidebarCollapsed((value) => !value)}
          />
        </div>
        <div className="brand-block">
          <strong>Bibi Work</strong>
          <span>{me.tenants[0]?.name ?? "tenant"}</span>
        </div>
        <nav className="nav-list" aria-label={t("app.navLabel")}>
          {groupedItems.map((group) => (
            <div className="nav-group" key={group.key}>
              <span className="nav-group-label">{t(group.key)}</span>
              {group.items.map((item) => {
                const Icon = item.icon;
                return (
                  <button
                    key={item.id}
                    className={item.id === activeNavId ? "active" : ""}
                    title={t(item.labelKey)}
                    onClick={() => onRouteChange(item.id)}
                  >
                    <Icon size={17} strokeWidth={2} />
                    <span>{t(item.labelKey)}</span>
                  </button>
                );
              })}
            </div>
          ))}
        </nav>
        <div className="sidebar-footer">
          <div className="user-chip">
            <span className="avatar">
              {(me.user.displayName ?? me.user.username ?? "U").slice(0, 1)}
            </span>
            <div>
              <strong>{me.user.displayName ?? me.user.username ?? t("app.currentUser")}</strong>
              <span>{me.roles.slice(0, 2).join(", ") || me.user.status}</span>
            </div>
          </div>
        </div>
      </aside>
      <main className="main-frame">
        <header className="topbar">
          <div>
            <h1>{t(routeTitle(activeRoute))}</h1>
          </div>
          <div className="topbar-actions">
            <div className="account-menu" ref={accountMenuRef}>
              <button
                type="button"
                className="account-menu-trigger"
                aria-haspopup="menu"
                aria-expanded={accountMenuOpen}
                onClick={() => setAccountMenuOpen((open) => !open)}
              >
                <span className="avatar compact-avatar">{displayName.slice(0, 1)}</span>
                <span>{displayName}</span>
                <ChevronDown size={14} />
              </button>
              {accountMenuOpen ? (
                <div className="account-menu-popover" role="menu">
                  <div className="account-menu-section">
                    <strong>{displayName}</strong>
                    <span>{me.roles.slice(0, 2).join(", ") || me.user.status}</span>
                    <StatusPill status={me.session.revokedAt ? "revoked" : "active"} />
                  </div>
                  <div className="account-menu-section">
                    <span>{t("app.device")}</span>
                    <strong>{deviceSummary(me, t)}</strong>
                  </div>
                  <label className="account-menu-language">
                    <Globe2 size={15} />
                    <span>{t("app.language")}</span>
                    <select
                      aria-label={t("app.language")}
                      value={language}
                      onChange={(event) => setLanguage(event.target.value as LanguageCode)}
                    >
                      {availableLanguages().map((item) => (
                        <option key={item} value={item}>
                          {languageLabel(item)}
                        </option>
                      ))}
                    </select>
                  </label>
                  <Button
                    variant="ghost"
                    icon={<LogOut size={16} />}
                    onClick={() => {
                      setAccountMenuOpen(false);
                      onLogout();
                    }}
                  >
                    {t("app.logout")}
                  </Button>
                </div>
              ) : null}
            </div>
          </div>
        </header>
        <section className="route-frame">{children}</section>
      </main>
    </div>
  );
}

function groupNavigationItems(items: NavigationItem[]) {
  const order = [
    "nav.group.work",
    "nav.group.resources",
    "nav.group.governance",
    "nav.group.system"
  ] as const;
  return order
    .map((key) => ({ key, items: items.filter((item) => item.groupKey === key) }))
    .filter((group) => group.items.length);
}

function deviceSummary(me: Me, t: ReturnType<typeof useI18n>["t"]) {
  if (me.device.deviceName.startsWith("oidc:")) {
    return `${t("app.authenticatedDevice")} · OIDC`;
  }
  return `${me.device.deviceName} · ${me.device.platform}`;
}
