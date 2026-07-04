import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";
import type {
  AgentVersionTenantInput,
  CatalogListInput,
  CatalogVersionListInput,
  CreateMcpServerInput,
  CreatePolicyBindingInput,
  CreateCatalogResourceInput,
  DiscoverMcpToolsInput,
  DisableMcpToolInput,
  DisablePolicyBindingInput,
  DisableCatalogResourceInput,
  DisableCatalogVersionInput,
  McpToolListInput,
  PolicyBindingFilters,
  PublishCatalogVersionInput,
  UpdateMcpToolInput,
  UpdateMcpServerInput
} from "./catalog.adapter";

export const catalogQueryKeys = {
  resources: (input: CatalogListInput) => [
    "catalogResources",
    input.tenantId,
    input.kind,
    input.status ?? "all",
    input.limit ?? 100
  ],
  versions: (input: CatalogVersionListInput) => [
    "catalogVersions",
    input.tenantId,
    input.kind,
    input.resourceId,
    input.status ?? "all",
    input.limit ?? 100
  ],
  mcpTools: (input: McpToolListInput) => [
    "mcpTools",
    input.tenantId,
    input.mcpServerId,
    input.status ?? "all",
    input.limit ?? 100
  ],
  policyBindings: (filters: PolicyBindingFilters) => [
    "policyBindings",
    filters.tenantId,
    filters.resourceType ?? "all",
    filters.resourceId ?? "all",
    filters.action ?? "all",
    filters.includeDisabled ?? false,
    filters.limit ?? 100
  ],
  agentVersionCapabilities: (input: AgentVersionTenantInput) => [
    "agentVersionCapabilities",
    input.tenantId,
    input.agentVersionId
  ]
};

export function useCatalogResourcesQuery(input: CatalogListInput) {
  const { catalogApi } = usePlatformApi();
  return useQuery({
    queryKey: catalogQueryKeys.resources(input),
    queryFn: () => catalogApi.listResources(input),
    enabled: Boolean(input.tenantId)
  });
}

export function useCatalogVersionsQuery(input: CatalogVersionListInput, enabled: boolean) {
  const { catalogApi } = usePlatformApi();
  return useQuery({
    queryKey: catalogQueryKeys.versions(input),
    queryFn: () => catalogApi.listVersions(input),
    enabled: enabled && Boolean(input.tenantId && input.resourceId)
  });
}

export function useMcpToolsQuery(input: McpToolListInput, enabled: boolean) {
  const { catalogApi } = usePlatformApi();
  return useQuery({
    queryKey: catalogQueryKeys.mcpTools(input),
    queryFn: () => catalogApi.listMcpTools(input),
    enabled: enabled && Boolean(input.tenantId && input.mcpServerId)
  });
}

export function useCreateMcpServerMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateMcpServerInput) => catalogApi.createMcpServer(input),
    onSuccess: (_server, input) =>
      invalidateCatalogResourceQueries(queryClient, { tenantId: input.tenantId, kind: "mcpServers" })
  });
}

export function useUpdateMcpServerMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdateMcpServerInput) => catalogApi.updateMcpServer(input),
    onSuccess: (_server, input) =>
      invalidateCatalogResourceQueries(queryClient, { tenantId: input.tenantId, kind: "mcpServers" })
  });
}

export function useDiscoverMcpToolsMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DiscoverMcpToolsInput) => catalogApi.discoverMcpTools(input),
    onSuccess: (_tools, input) => {
      queryClient.invalidateQueries({
        queryKey: ["mcpTools", input.tenantId, input.mcpServerId]
      });
      invalidateCatalogResourceQueries(queryClient, { tenantId: input.tenantId, kind: "mcpServers" });
    }
  });
}

export function useUpdateMcpToolMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdateMcpToolInput) => catalogApi.updateMcpTool(input),
    onSuccess: (_tool, input) => invalidateMcpToolQueries(queryClient, input)
  });
}

export function useDisableMcpToolMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DisableMcpToolInput) => catalogApi.disableMcpTool(input),
    onSuccess: (_tool, input) => invalidateMcpToolQueries(queryClient, input)
  });
}

export function usePolicyBindingsQuery(filters: PolicyBindingFilters, enabled: boolean) {
  const { catalogApi } = usePlatformApi();
  return useQuery({
    queryKey: catalogQueryKeys.policyBindings(filters),
    queryFn: () => catalogApi.listPolicyBindings(filters),
    enabled: enabled && Boolean(filters.tenantId)
  });
}

function invalidateMcpToolQueries(
  queryClient: ReturnType<typeof useQueryClient>,
  input: { tenantId: string; mcpServerId?: string }
) {
  return queryClient.invalidateQueries({
    queryKey: input.mcpServerId ? ["mcpTools", input.tenantId, input.mcpServerId] : ["mcpTools", input.tenantId]
  });
}

export function useCreatePolicyBindingMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreatePolicyBindingInput) => catalogApi.createPolicyBinding(input),
    onSuccess: (_binding, input) => invalidatePolicyBindingQueries(queryClient, input)
  });
}

export function useDisablePolicyBindingMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DisablePolicyBindingInput) => catalogApi.disablePolicyBinding(input),
    onSuccess: (_binding, input) => invalidatePolicyBindingQueries(queryClient, input)
  });
}

export function useCreateCatalogResourceMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: CreateCatalogResourceInput) => catalogApi.createResource(input),
    onSuccess: (_resource, input) => invalidateCatalogResourceQueries(queryClient, input)
  });
}

export function useDisableCatalogResourceMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DisableCatalogResourceInput) => catalogApi.disableResource(input),
    onSuccess: (_resource, input) => invalidateCatalogResourceQueries(queryClient, input)
  });
}

export function usePublishCatalogVersionMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: PublishCatalogVersionInput) => catalogApi.publishVersion(input),
    onSuccess: (_version, input) => invalidateCatalogVersionQueries(queryClient, input)
  });
}

export function useDisableCatalogVersionMutation() {
  const { catalogApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: DisableCatalogVersionInput) => catalogApi.disableVersion(input),
    onSuccess: (_version, input) => invalidateCatalogVersionQueries(queryClient, input)
  });
}

export function useAgentVersionCapabilitiesMutation() {
  const { catalogApi } = usePlatformApi();
  return useMutation({
    mutationFn: (input: AgentVersionTenantInput) => catalogApi.getAgentVersionCapabilities(input)
  });
}

export function useValidateAgentVersionMutation() {
  const { catalogApi } = usePlatformApi();
  return useMutation({
    mutationFn: (input: AgentVersionTenantInput) => catalogApi.validateAgentVersion(input)
  });
}

function invalidateCatalogResourceQueries(
  queryClient: ReturnType<typeof useQueryClient>,
  input: { tenantId: string; kind: string }
) {
  return queryClient.invalidateQueries({
    queryKey: ["catalogResources", input.tenantId, input.kind]
  });
}

function invalidatePolicyBindingQueries(
  queryClient: ReturnType<typeof useQueryClient>,
  input: { tenantId: string; resourceType?: string; resourceId?: string }
) {
  return queryClient.invalidateQueries({
    queryKey:
      input.resourceType && input.resourceId
        ? ["policyBindings", input.tenantId, input.resourceType, input.resourceId]
        : ["policyBindings", input.tenantId]
  });
}

function invalidateCatalogVersionQueries(
  queryClient: ReturnType<typeof useQueryClient>,
  input: { tenantId: string; kind: string; resourceId?: string }
) {
  return queryClient.invalidateQueries({
    queryKey: input.resourceId
      ? ["catalogVersions", input.tenantId, input.kind, input.resourceId]
      : ["catalogVersions", input.tenantId, input.kind]
  });
}
