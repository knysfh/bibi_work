import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";

export const approvalQueryKeys = {
  list: (tenantId: string, status = "pending") => ["approvals", tenantId, status] as const
};

export function useApprovalsQuery(tenantId?: string, status = "pending") {
  const { approvalApi } = usePlatformApi();
  return useQuery({
    queryKey: approvalQueryKeys.list(tenantId ?? "", status),
    queryFn: () => approvalApi.listApprovals(tenantId ?? "", status),
    enabled: Boolean(tenantId)
  });
}

export function useApprovalDecisionMutation(tenantId: string) {
  const { approvalApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: {
      approvalId: string;
      decision: "approved" | "rejected";
      reason?: string;
    }) => approvalApi.decideApproval({ ...input, tenantId }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: approvalQueryKeys.list(tenantId) });
    }
  });
}
