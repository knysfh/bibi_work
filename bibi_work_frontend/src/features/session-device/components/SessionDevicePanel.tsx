import { Laptop, ShieldOff } from "lucide-react";
import { useMemo, useState } from "react";
import type { Device, Me, Session } from "../../../shared/contracts/platform";
import { useI18n, type I18nKey } from "../../../shared/i18n";
import {
  ConfirmDialog,
  DangerActionButton,
  EmptyState,
  KeyValue,
  Panel,
  ResourceList,
  SegmentedControl,
  StatusPill
} from "../../../shared/ui";
import {
  useDevicesQuery,
  useRevokeDeviceMutation,
  useRevokeSessionMutation,
  useSessionsQuery
} from "../api/session-device.queries";

type ConfirmAction =
  | { type: "device"; id: string; name: string }
  | { type: "session"; id: string };
type SessionFilter = "current" | "active" | "revoked" | "all";

export function SessionDevicePanel({ me }: { me: Me }) {
  const { t } = useI18n();
  const [confirmAction, setConfirmAction] = useState<ConfirmAction | null>(null);
  const [sessionFilter, setSessionFilter] = useState<SessionFilter>("current");
  const devices = useDevicesQuery(me.tenantId);
  const sessions = useSessionsQuery(me.tenantId);
  const revokeSession = useRevokeSessionMutation(me.tenantId);
  const revokeDevice = useRevokeDeviceMutation(me.tenantId);
  const devicesById = useMemo(
    () => new Map((devices.data ?? []).map((device) => [device.id, device])),
    [devices.data]
  );
  const groupedSessions = useMemo(
    () => groupSessions(sessions.data ?? [], me.device.id, sessionFilter),
    [me.device.id, sessionFilter, sessions.data]
  );
  const filteredSessionCount = groupedSessions.reduce(
    (sum, group) => sum + group.sessions.length,
    0
  );

  async function confirmRevoke() {
    if (!confirmAction) {
      return;
    }
    if (confirmAction.type === "device") {
      await revokeDevice.mutateAsync(confirmAction.id);
    } else {
      await revokeSession.mutateAsync(confirmAction.id);
    }
    setConfirmAction(null);
  }

  return (
    <>
      <div className="page-grid two-columns">
        <Panel
          title={t("session.devices")}
          subtitle={t("common.deviceCount", { count: devices.data?.length ?? 0 })}
        >
          <ResourceList>
            <p className="config-help">{t("session.securityHint")}</p>
            {devices.data?.length ? (
              devices.data.map((device) => (
                <article key={device.id} className="resource-row">
                  <Laptop size={18} />
                  <div>
                    <strong title={device.deviceName}>
                      {displayDeviceName(device.deviceName)}
                    </strong>
                    <span>
                      {device.platform} · {device.trustLevel}
                    </span>
                  </div>
                  <StatusPill status={device.revokedAt ? "revoked" : "active"} />
                  <DangerActionButton
                    size="sm"
                    icon={<ShieldOff size={15} />}
                    onClick={() =>
                      setConfirmAction({
                        type: "device",
                        id: device.id,
                        name: device.deviceName
                      })
                    }
                    disabled={Boolean(device.revokedAt)}
                  >
                    {t("session.revokeSelectedDevice")}
                  </DangerActionButton>
                </article>
              ))
            ) : (
              <EmptyState title={t("session.noDevices")} />
            )}
          </ResourceList>
        </Panel>
        <Panel
          title={t("session.sessions")}
          subtitle={t("common.itemCount", { count: filteredSessionCount })}
        >
          <ResourceList>
            <SessionFilterControl active={sessionFilter} onChange={setSessionFilter} />
            {sessionFilter === "current" ? (
              <p className="config-help">{t("session.recentOnly")}</p>
            ) : null}
            {filteredSessionCount ? (
              groupedSessions.map((group) =>
                group.sessions.length ? (
                  <section className="session-group" key={group.id}>
                    <div className="session-group-title">
                      <strong>{t(group.labelKey)}</strong>
                      <span>{t("common.itemCount", { count: group.sessions.length })}</span>
                    </div>
                    {group.sessions.map((session) => (
                      <SessionRow
                        key={session.id}
                        currentDeviceId={me.device.id}
                        device={devicesById.get(session.deviceId)}
                        session={session}
                        onRevoke={() => setConfirmAction({ type: "session", id: session.id })}
                      />
                    ))}
                  </section>
                ) : null
              )
            ) : (
              <EmptyState title={t("session.noSessions")} />
            )}
          </ResourceList>
        </Panel>
      </div>
      {confirmAction ? (
        <ConfirmDialog
          title={t("common.confirmAction")}
          message={confirmMessage(confirmAction, t)}
          confirmLabel={t("common.revoke")}
          cancelLabel={t("common.cancel")}
          pending={revokeDevice.isPending || revokeSession.isPending}
          onCancel={() => setConfirmAction(null)}
          onConfirm={confirmRevoke}
        />
      ) : null}
    </>
  );
}

function SessionFilterControl({
  active,
  onChange
}: {
  active: SessionFilter;
  onChange: (filter: SessionFilter) => void;
}) {
  const { t } = useI18n();
  const filters: Array<{ id: SessionFilter; labelKey: I18nKey }> = [
    { id: "current", labelKey: "session.filter.current" },
    { id: "active", labelKey: "session.filter.active" },
    { id: "revoked", labelKey: "session.filter.revoked" },
    { id: "all", labelKey: "session.filter.all" }
  ];
  return (
    <SegmentedControl
      label={t("session.filterLabel")}
      items={filters.map((filter) => ({ id: filter.id, label: t(filter.labelKey) }))}
      active={active}
      onChange={onChange}
    />
  );
}

function SessionRow({
  session,
  device,
  currentDeviceId,
  onRevoke
}: {
  session: Session;
  device?: Device;
  currentDeviceId: string;
  onRevoke: () => void;
}) {
  const { t } = useI18n();
  return (
    <article className="session-row">
      <div className="session-main">
        <div className="session-row-heading">
          <StatusPill status={session.revokedAt ? "revoked" : "active"} />
          {session.deviceId === currentDeviceId ? (
            <span>{t("session.currentDevice")}</span>
          ) : (
            <span title={device?.deviceName}>
              {device ? displayDeviceName(device.deviceName) : t("session.unknownDevice")}
            </span>
          )}
        </div>
        <KeyValue label="Session" value={shortId(session.id)} />
        <KeyValue label={t("session.tokenExp")} value={session.tokenExp} />
        <KeyValue label={t("session.lastSeen")} value={session.lastSeenAt} />
      </div>
      <DangerActionButton
        size="sm"
        icon={<ShieldOff size={15} />}
        onClick={onRevoke}
        disabled={Boolean(session.revokedAt)}
      >
        {t("common.revoke")}
      </DangerActionButton>
    </article>
  );
}

function groupSessions(sessions: Session[], currentDeviceId: string, filter: SessionFilter) {
  const active = sessions.filter((session) => !session.revokedAt);
  const current = active.filter((session) => session.deviceId === currentDeviceId);
  const other = active.filter((session) => session.deviceId !== currentDeviceId);
  const revoked = sessions.filter((session) => session.revokedAt);

  if (filter === "current") {
    return [
      {
        id: "current",
        labelKey: "session.group.current" as const,
        sessions: recentSessions(current, 10)
      }
    ];
  }
  if (filter === "active") {
    return [
      { id: "current", labelKey: "session.group.current" as const, sessions: current },
      { id: "other", labelKey: "session.group.other" as const, sessions: other }
    ];
  }
  if (filter === "revoked") {
    return [{ id: "revoked", labelKey: "session.group.revoked" as const, sessions: revoked }];
  }
  return [
    { id: "current", labelKey: "session.group.current" as const, sessions: current },
    { id: "other", labelKey: "session.group.other" as const, sessions: other },
    { id: "revoked", labelKey: "session.group.revoked" as const, sessions: revoked }
  ];
}

function shortId(id: string): string {
  return id.length > 13 ? `${id.slice(0, 8)}...${id.slice(-4)}` : id;
}

function displayDeviceName(rawName: string): string {
  const name = rawName.replace(/^oidc:/, "").trim();
  if (!name || name === "unknown") {
    return "Unknown client";
  }
  if (/^curl\//i.test(name)) {
    return "curl client";
  }
  if (/^Python-urllib\//i.test(name)) {
    return "Python client";
  }
  const browser = browserName(name);
  const platform = platformName(name);
  if (browser && platform) {
    return `${browser} on ${platform}`;
  }
  return name.length > 48 ? `${name.slice(0, 45)}...` : name;
}

function browserName(userAgent: string): string | null {
  if (/HeadlessChrome|Chrome|Chromium/i.test(userAgent)) {
    return "Chrome";
  }
  if (/Firefox/i.test(userAgent)) {
    return "Firefox";
  }
  if (/Safari/i.test(userAgent)) {
    return "Safari";
  }
  return null;
}

function platformName(userAgent: string): string | null {
  if (/Windows NT/i.test(userAgent)) {
    return "Windows";
  }
  if (/Mac OS X|Macintosh/i.test(userAgent)) {
    return "macOS";
  }
  if (/Android/i.test(userAgent)) {
    return "Android";
  }
  if (/iPhone|iPad/i.test(userAgent)) {
    return "iOS";
  }
  if (/Linux|X11/i.test(userAgent)) {
    return "Linux";
  }
  return null;
}

function recentSessions(sessions: Session[], limit: number): Session[] {
  return [...sessions]
    .sort((left, right) => timestamp(right) - timestamp(left))
    .slice(0, limit);
}

function timestamp(session: Session): number {
  return Date.parse(session.lastSeenAt ?? session.updatedAt ?? session.createdAt) || 0;
}

function confirmMessage(
  action: ConfirmAction,
  t: (key: I18nKey, values?: Record<string, string | number>) => string
) {
  if (action.type === "device") {
    return t("session.confirmRevokeDevice", { name: action.name });
  }
  return t("session.confirmRevokeSession", { id: action.id });
}
