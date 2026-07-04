import type { ReactNode } from "react";

export function EmptyState({
  title,
  detail,
  action
}: {
  title: ReactNode;
  detail?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty-state">
      <strong>{title}</strong>
      {detail ? <span>{detail}</span> : null}
      {action ? <div className="empty-state-action">{action}</div> : null}
    </div>
  );
}
