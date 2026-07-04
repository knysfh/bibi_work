import { ChevronDown, Wrench } from "lucide-react";
import { useState } from "react";
import type { ToolCallProjection } from "../domain/run.types";
import { StatusPill } from "../../../shared/ui";
import { useI18n } from "../../../shared/i18n";
import { ToolResultRenderer } from "./ToolResultRenderer";

export function ToolCallCard({ toolCall }: { toolCall: ToolCallProjection }) {
  const { t } = useI18n();
  const [isOpen, setIsOpen] = useState(false);

  return (
    <details className="tool-card" open={isOpen}>
      <summary
        onClick={(event) => {
          event.preventDefault();
          setIsOpen((current) => !current);
        }}
      >
        <span>
          <Wrench size={15} />
          {toolCall.name}
        </span>
        <span className="summary-right">
          {toolCall.riskLevel ? <em>{toolCall.riskLevel}</em> : null}
          <StatusPill status={toolCall.status} />
          <ChevronDown size={15} />
        </span>
      </summary>
      <dl className="compact-dl">
        <dt>{t("common.inputSummary")}</dt>
        <dd>{toolCall.inputSummary || "-"}</dd>
        <dt>{t("common.outputSummary")}</dt>
        <dd>{toolCall.outputSummary || toolCall.errorSummary || "-"}</dd>
      </dl>
      <ToolResultRenderer views={toolCall.views} />
    </details>
  );
}
