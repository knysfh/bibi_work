import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";
import type { Workspace } from "../../../shared/contracts/platform";
import type { CreateLocalMountInput, CreateWorkspaceInput } from "./workspace.adapter";

export const workspaceQueryKeys = {
  list: (tenantId: string) => ["workspaces", tenantId] as const,
  localMounts: (tenantId: string, workspaceId: string) =>
    ["workspaces", tenantId, workspaceId, "local-mounts"] as const
};

export function useWorkspacesQuery(tenantId?: string) {
  const { workspaceApi } = usePlatformApi();
  return useQuery({
    queryKey: workspaceQueryKeys.list(tenantId ?? ""),
    queryFn: () => workspaceApi.listWorkspaces(tenantId ?? ""),
    enabled: Boolean(tenantId)
  });
}

export function useCreateWorkspaceMutation(tenantId: string) {
  const { workspaceApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: Omit<CreateWorkspaceInput, "tenantId">) =>
      workspaceApi.createWorkspace({ ...input, tenantId }),
    onSuccess: async (workspace) => {
      queryClient.setQueryData<Workspace[]>(workspaceQueryKeys.list(tenantId), (current) => [
        workspace,
        ...(current ?? []).filter((item) => item.id !== workspace.id)
      ]);
      await queryClient.invalidateQueries({ queryKey: workspaceQueryKeys.list(tenantId) });
    }
  });
}

export function useLocalMountsQuery(tenantId?: string, workspaceId?: string) {
  const { workspaceApi } = usePlatformApi();
  return useQuery({
    queryKey: workspaceQueryKeys.localMounts(tenantId ?? "", workspaceId ?? ""),
    queryFn: () => workspaceApi.listLocalMounts(tenantId ?? "", workspaceId ?? ""),
    enabled: Boolean(tenantId && workspaceId)
  });
}

export function useCreateLocalMountMutation(tenantId: string, workspaceId?: string) {
  const { workspaceApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: Omit<CreateLocalMountInput, "tenantId" | "workspaceId">) => {
      if (!workspaceId) {
        throw new Error("workspaceId is required");
      }
      return workspaceApi.createLocalMount({ ...input, tenantId, workspaceId });
    },
    onSuccess: async () => {
      if (workspaceId) {
        await queryClient.invalidateQueries({
          queryKey: workspaceQueryKeys.localMounts(tenantId, workspaceId)
        });
      }
    }
  });
}
