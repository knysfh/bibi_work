import { CheckSquare } from "lucide-react";
import { useI18n } from "../../../shared/i18n";
import { EmptyState, StatusPill } from "../../../shared/ui";
import type { TaskProjection } from "../domain/run.types";

export function TaskList({ tasks }: { tasks: TaskProjection[] }) {
  const { t } = useI18n();
  if (tasks.length === 0) {
    return <EmptyState title={t("run.noTasks")} />;
  }
  return (
    <div className="inspector-list">
      {tasks.map((task) => (
        <div key={task.id} className="inspector-row">
          <CheckSquare size={15} />
          <div>
            <strong>{task.title}</strong>
            {task.summary ? <span>{task.summary}</span> : null}
          </div>
          <StatusPill status={task.status} />
        </div>
      ))}
    </div>
  );
}
