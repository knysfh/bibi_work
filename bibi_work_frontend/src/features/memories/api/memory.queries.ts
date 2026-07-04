import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";
import type {
  CreateMemoryInput,
  MemoryDecision,
  MemoryFilters,
  MemoryLayer,
  MemorySensitivity,
  MemoryStatus,
  MemoryVisibility
} from "./memory.adapter";

export const memoryQueryKeys = {
  list: (filters: MemoryFilters) =>
    [
      "memories",
      filters.tenantId,
      filters.userId ?? "",
      filters.agentId ?? "",
      filters.projectId ?? "",
      filters.layer ?? "",
      filters.status ?? "",
      filters.query ?? "",
      filters.runId ?? "",
      filters.limit ?? ""
    ] as const,
  search: (filters: MemoryFilters) =>
    [
      "memorySearch",
      filters.tenantId,
      filters.userId ?? "",
      filters.agentId ?? "",
      filters.projectId ?? "",
      filters.layer ?? "",
      filters.status ?? "",
      filters.query ?? "",
      filters.runId ?? "",
      filters.limit ?? ""
    ] as const
};

export interface MemoryScreenFilters {
  tenantId: string;
  layer?: MemoryLayer;
  status: MemoryStatus;
  visibility?: MemoryVisibility | "all";
  sensitivity?: MemorySensitivity | "all";
  query?: string;
  limit?: number;
}

export function useMemoriesQuery(filters: MemoryScreenFilters) {
  const { memoryApi } = usePlatformApi();
  const queryFilters = toMemoryFilters(filters);
  return useQuery({
    queryKey: memoryQueryKeys.list(queryFilters),
    queryFn: () => memoryApi.listMemories(queryFilters),
    enabled: Boolean(filters.tenantId),
    select: (memories) =>
      memories.filter((memory) => {
        const visibilityMatches =
          !filters.visibility ||
          filters.visibility === "all" ||
          memory.visibility === filters.visibility;
        const sensitivityMatches =
          !filters.sensitivity ||
          filters.sensitivity === "all" ||
          memory.sensitivity === filters.sensitivity;
        return visibilityMatches && sensitivityMatches;
      })
  });
}

export function useMemoryDecisionMutation(tenantId: string) {
  const { memoryApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { memoryId: string; decision: MemoryDecision }) =>
      memoryApi.decideMemory(input.memoryId, input.decision),
    onSuccess: async () => {
      await invalidateMemoryQueries(queryClient, tenantId);
    }
  });
}

export function useMemoryBatchDecisionMutation(tenantId: string) {
  const { memoryApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { decision: MemoryDecision; memoryIds: string[] }) =>
      memoryApi.batchDecideMemories({ ...input, tenantId }),
    onSuccess: async () => {
      await invalidateMemoryQueries(queryClient, tenantId);
    }
  });
}

export function useCreateMemoryMutation(tenantId: string) {
  const { memoryApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: Omit<CreateMemoryInput, "tenantId">) =>
      memoryApi.createMemory({ ...input, tenantId }),
    onSuccess: async () => {
      await invalidateMemoryQueries(queryClient, tenantId);
    }
  });
}

function toMemoryFilters(filters: MemoryScreenFilters): MemoryFilters {
  const query = filters.query?.trim();
  return {
    tenantId: filters.tenantId,
    layer: filters.layer,
    status: filters.status,
    query: query || undefined,
    limit: filters.limit ?? 100
  };
}

async function invalidateMemoryQueries(
  queryClient: ReturnType<typeof useQueryClient>,
  tenantId: string
) {
  await queryClient.invalidateQueries({ queryKey: ["memories", tenantId] });
  await queryClient.invalidateQueries({ queryKey: ["memorySearch", tenantId] });
}
