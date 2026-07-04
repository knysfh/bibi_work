import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { usePlatformApi } from "../../../app/providers";

export const projectQueryKeys = {
  list: (tenantId: string) => ["projects", tenantId] as const,
  files: (tenantId: string, projectId: string, prefix = "", pattern = "") =>
    ["projectFiles", tenantId, projectId, prefix, pattern] as const,
  file: (
    tenantId: string,
    projectId: string,
    path: string,
    revision?: number,
    versionId?: string,
    allowBinary?: boolean
  ) =>
    [
      "projectFile",
      tenantId,
      projectId,
      path,
      revision ?? "latest",
      versionId ?? "default-version",
      allowBinary ? "allow-binary" : "metadata-only"
    ] as const,
  search: (tenantId: string, projectId: string, query: string, prefix = "") =>
    ["projectFileSearch", tenantId, projectId, query, prefix] as const,
  history: (tenantId: string, projectId: string, path: string) =>
    ["projectFileHistory", tenantId, projectId, path] as const,
  artifacts: (tenantId: string, projectId: string, runId = "") =>
    ["projectArtifacts", tenantId, projectId, runId] as const,
  toolResultArtifact: (tenantId: string, objectReferenceId: string, offset = 0, limit = 50) =>
    ["toolResultArtifact", tenantId, objectReferenceId, offset, limit] as const
};

export function useProjectsQuery(tenantId?: string) {
  const { projectApi } = usePlatformApi();
  return useQuery({
    queryKey: projectQueryKeys.list(tenantId ?? ""),
    queryFn: () => projectApi.listProjects(tenantId ?? ""),
    enabled: Boolean(tenantId)
  });
}

export function useProjectFilesQuery(
  tenantId?: string,
  projectId?: string,
  prefix = "",
  pattern = ""
) {
  const { projectApi } = usePlatformApi();
  return useQuery({
    queryKey: projectQueryKeys.files(tenantId ?? "", projectId ?? "", prefix, pattern),
    queryFn: () =>
      projectApi.listFiles({
        tenantId: tenantId ?? "",
        projectId: projectId ?? "",
        prefix,
        pattern
      }),
    enabled: Boolean(tenantId && projectId)
  });
}

export function useProjectFileQuery(
  tenantId?: string,
  projectId?: string,
  path?: string,
  revision?: number,
  versionId?: string,
  allowBinary?: boolean
) {
  const { projectApi } = usePlatformApi();
  return useQuery({
    queryKey: projectQueryKeys.file(
      tenantId ?? "",
      projectId ?? "",
      path ?? "",
      revision,
      versionId,
      allowBinary
    ),
    queryFn: () =>
      projectApi.readFile({
        tenantId: tenantId ?? "",
        projectId: projectId ?? "",
        path: path ?? "",
        revision,
        versionId,
        allowBinary
      }),
    enabled: Boolean(tenantId && projectId && path)
  });
}

export function useProjectFileSearchQuery(
  tenantId?: string,
  projectId?: string,
  query = "",
  prefix = ""
) {
  const { projectApi } = usePlatformApi();
  const trimmedQuery = query.trim();
  return useQuery({
    queryKey: projectQueryKeys.search(tenantId ?? "", projectId ?? "", trimmedQuery, prefix),
    queryFn: () =>
      projectApi.searchFiles({
        tenantId: tenantId ?? "",
        projectId: projectId ?? "",
        query: trimmedQuery,
        prefix,
        limit: 50
      }),
    enabled: Boolean(tenantId && projectId && trimmedQuery)
  });
}

export function useProjectFileHistoryQuery(tenantId?: string, projectId?: string, path?: string) {
  const { projectApi } = usePlatformApi();
  return useQuery({
    queryKey: projectQueryKeys.history(tenantId ?? "", projectId ?? "", path ?? ""),
    queryFn: () =>
      projectApi.listFileHistory({
        tenantId: tenantId ?? "",
        projectId: projectId ?? "",
        path: path ?? "",
        limit: 50
      }),
    enabled: Boolean(tenantId && projectId && path)
  });
}

export function useProjectArtifactsQuery(tenantId?: string, projectId?: string, runId = "") {
  const { projectApi } = usePlatformApi();
  return useQuery({
    queryKey: projectQueryKeys.artifacts(tenantId ?? "", projectId ?? "", runId),
    queryFn: () =>
      projectApi.listArtifacts({
        tenantId: tenantId ?? "",
        projectId: projectId ?? "",
        runId: runId.trim() || undefined
      }),
    enabled: Boolean(tenantId && projectId)
  });
}

export function useToolResultArtifactQuery(
  tenantId?: string,
  objectReferenceId?: string,
  offset = 0,
  limit = 50,
  enabled = true
) {
  const { projectApi } = usePlatformApi();
  return useQuery({
    queryKey: projectQueryKeys.toolResultArtifact(
      tenantId ?? "",
      objectReferenceId ?? "",
      offset,
      limit
    ),
    queryFn: () =>
      projectApi.readToolResultArtifact({
        tenantId: tenantId ?? "",
        objectReferenceId: objectReferenceId ?? "",
        offset,
        limit
      }),
    enabled: Boolean(enabled && tenantId && objectReferenceId)
  });
}

export async function invalidateProjectFileQueries(
  queryClient: QueryClient,
  tenantId: string,
  projectId?: string
) {
  const keyPrefix = projectId ? [tenantId, projectId] : [tenantId];
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["projectFiles", ...keyPrefix] }),
    queryClient.invalidateQueries({ queryKey: ["projectFile", ...keyPrefix] }),
    queryClient.invalidateQueries({ queryKey: ["projectFileSearch", ...keyPrefix] }),
    queryClient.invalidateQueries({ queryKey: ["projectFileHistory", ...keyPrefix] }),
    queryClient.invalidateQueries({ queryKey: ["projectArtifacts", ...keyPrefix] }),
    queryClient.invalidateQueries({ queryKey: ["toolResultArtifact", tenantId] })
  ]);
}

export function useCreateProjectMutation(tenantId: string) {
  const { projectApi } = usePlatformApi();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: { name: string; description?: string }) =>
      projectApi.createProject({ ...input, tenantId }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: projectQueryKeys.list(tenantId) });
    }
  });
}
