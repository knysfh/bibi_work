import { Network } from "lucide-react";
import { useI18n } from "../../../shared/i18n";
import { EmptyState, StatusPill } from "../../../shared/ui";
import type { SubagentProjection } from "../domain/run.types";

export function SubagentList({ subagents }: { subagents: SubagentProjection[] }) {
  const { t } = useI18n();
  if (subagents.length === 0) {
    return <EmptyState title={t("run.noSubagents")} />;
  }
  return (
    <div className="inspector-list">
      {subagents.map((subagent) => (
        <div key={subagent.id} className="inspector-row">
          <Network size={15} />
          <div>
            <strong>{subagent.name}</strong>
            {subagent.summary ? <span>{subagent.summary}</span> : null}
          </div>
          <StatusPill status={subagent.status} />
        </div>
      ))}
    </div>
  );
}
