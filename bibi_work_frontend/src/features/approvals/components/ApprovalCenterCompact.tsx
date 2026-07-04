import { Check, X } from "lucide-react";
import { useI18n } from "../../../shared/i18n";
import { asRecord, stringFromJson } from "../../../shared/types/json";
import { Button, DangerActionButton, EmptyState, StatusPill } from "../../../shared/ui";
import { useApprovalDecisionMutation, useApprovalsQuery } from "../api/approval.queries";

export function ApprovalCenterCompact({ tenantId }: { tenantId: string }) {
  const { t } = useI18n();
  const approvals = useApprovalsQuery(tenantId);
  const decide = useApprovalDecisionMutation(tenantId);

  if (approvals.isLoading) {
    return <EmptyState title={t("approval.loading")} />;
  }
  if (!approvals.data?.length) {
    return <EmptyState title={t("approval.empty")} detail={t("approval.emptyDetail")} />;
  }

  return (
    <div className="inspector-list">
      {approvals.data.map((approval) => {
        const request = asRecord(approval.requestPayload);
        return (
          <article key={approval.id} className="approval-row">
            <div>
              <strong>{stringFromJson(request.tool_name, t("common.toolCall"))}</strong>
              <span>{stringFromJson(request.input_summary, t("common.noInputSummary"))}</span>
            </div>
            <StatusPill status={approval.status} />
            <div className="row-actions">
              <Button
                size="sm"
                variant="secondary"
                aria-label={t("common.approve")}
                title={t("common.approve")}
                icon={<Check size={15} />}
                onClick={() => decide.mutate({ approvalId: approval.id, decision: "approved" })}
              >
                {t("common.approve")}
              </Button>
              <DangerActionButton
                size="sm"
                aria-label={t("common.reject")}
                title={t("common.reject")}
                icon={<X size={15} />}
                onClick={() => decide.mutate({ approvalId: approval.id, decision: "rejected" })}
              >
                {t("common.reject")}
              </DangerActionButton>
            </div>
          </article>
        );
      })}
    </div>
  );
}
