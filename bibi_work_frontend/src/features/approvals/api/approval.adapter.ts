import {
  approvalDtoSchema,
  dtoList,
  mapApproval,
  type Approval
} from "../../../shared/contracts/platform";
import type { JsonValue } from "../../../shared/types/json";
import type { HttpClient } from "../../../shared/api/http-client";

export interface ApprovalApi {
  listApprovals(tenantId: string, status?: string, limit?: number): Promise<Approval[]>;
  decideApproval(input: {
    tenantId: string;
    approvalId: string;
    decision: "approved" | "rejected";
    reason?: string;
    payload?: JsonValue;
  }): Promise<Approval>;
}

export function createApprovalApi(http: HttpClient): ApprovalApi {
  return {
    async listApprovals(tenantId, status = "pending", limit = 100) {
      return (
        await http.get("/approvals", dtoList(approvalDtoSchema), {
          query: { tenant_id: tenantId, status, limit }
        })
      ).map(mapApproval);
    },
    async decideApproval(input) {
      return mapApproval(
        await http.post(
          `/approvals/${input.approvalId}/decision`,
          {
            tenant_id: input.tenantId,
            decision: input.decision,
            reason: input.reason,
            payload: input.payload
          },
          approvalDtoSchema
        )
      );
    }
  };
}
