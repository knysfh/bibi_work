import { Badge } from "./Badge";
import { useI18n, type I18nKey } from "../i18n";

const statusTone: Record<string, "neutral" | "info" | "success" | "warning" | "danger"> = {
  queued: "neutral",
  idle: "neutral",
  running: "info",
  started: "info",
  waiting_approval: "warning",
  pending: "warning",
  candidate: "warning",
  completed: "success",
  approved: "success",
  failed: "danger",
  rejected: "danger",
  cancelled: "neutral",
  archived: "neutral",
  revoked: "danger",
  active: "success",
  binary: "neutral",
  draft: "neutral",
  published: "success",
  disabled: "neutral"
};

export function StatusPill({ status }: { status: string }) {
  const { t } = useI18n();
  const labelKey = `status.${status}` as I18nKey;
  return (
    <Badge tone={statusTone[status] ?? "neutral"}>
      {statusTone[status] ? t(labelKey) : status}
    </Badge>
  );
}
