import type { Me } from "../../../shared/contracts/platform";
import { useI18n } from "../../../shared/i18n";
import { EmptyState, KeyValue, StatusPill } from "../../../shared/ui";
import { asRecord, stringFromJson } from "../../../shared/types/json";
import { ApprovalCenterCompact } from "./ApprovalCenterCompact";
import { useApprovalsQuery } from "../api/approval.queries";

export function ApprovalCenterScreen({ me }: { me: Me }) {
  const { t } = useI18n();
  const approvals = useApprovalsQuery(me.tenantId, "pending");
  return (
    <div className="page-grid two-columns">
      <section className="page-panel">
        <header className="panel-header">
          <div>
            <strong>{t("approval.pending")}</strong>
            <span>{t("common.itemCount", { count: approvals.data?.length ?? 0 })}</span>
          </div>
        </header>
        <ApprovalCenterCompact tenantId={me.tenantId} />
      </section>
      <section className="page-panel">
        <header className="panel-header">
          <div>
            <strong>{t("approval.detail")}</strong>
            <span>{t("approval.detailSubtitle")}</span>
          </div>
        </header>
        {approvals.data?.[0] ? (
          <div className="detail-stack">
            {(() => {
              const approval = approvals.data[0];
              const request = asRecord(approval.requestPayload);
              return (
                <>
                  <StatusPill status={approval.status} />
                  <KeyValue label={t("approval.tool")} value={stringFromJson(request.tool_name)} />
                  <KeyValue label={t("approval.risk")} value={stringFromJson(request.risk_level)} />
                  <KeyValue label={t("approval.policy")} value={approval.approvalPolicyId} />
                  <KeyValue label="Run" value={approval.runId} />
                  <pre className="json-preview">
                    {JSON.stringify(approval.requestPayload, null, 2)}
                  </pre>
                </>
              );
            })()}
          </div>
        ) : (
          <EmptyState title={t("approval.select")} detail={t("approval.selectDetail")} />
        )}
      </section>
    </div>
  );
}
