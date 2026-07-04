import type { JsonValue } from "../../../shared/types/json";
import {
  dtoList,
  localMountDtoSchema,
  mapLocalMount,
  mapWorkspace,
  type LocalMount,
  type Workspace,
  workspaceDtoSchema
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";

export interface CreateWorkspaceInput {
  tenantId: string;
  name: string;
  remoteProjectId?: string;
  defaultAgentId?: string;
  defaultAgentVersionId?: string;
  defaultModelProfileId?: string;
  toolPolicy?: JsonValue;
  filePolicy?: JsonValue;
  includeGlobs?: JsonValue;
  excludeGlobs?: JsonValue;
  trustState?: string;
  metadata?: JsonValue;
}

export interface CreateLocalMountInput {
  tenantId: string;
  workspaceId: string;
  displayName: string;
  virtualPath: string;
  capabilities?: JsonValue;
  includeGlobs?: JsonValue;
  excludeGlobs?: JsonValue;
  trustState?: string;
  metadata?: JsonValue;
}

export interface WorkspaceApi {
  listWorkspaces(tenantId: string, limit?: number): Promise<Workspace[]>;
  createWorkspace(input: CreateWorkspaceInput): Promise<Workspace>;
  listLocalMounts(tenantId: string, workspaceId: string, limit?: number): Promise<LocalMount[]>;
  createLocalMount(input: CreateLocalMountInput): Promise<LocalMount>;
}

export function createWorkspaceApi(http: HttpClient): WorkspaceApi {
  return {
    async listWorkspaces(tenantId, limit = 100) {
      return (
        await http.get("/workspaces", dtoList(workspaceDtoSchema), {
          query: { tenant_id: tenantId, limit }
        })
      ).map(mapWorkspace);
    },
    async createWorkspace(input) {
      return mapWorkspace(
        await http.post(
          "/workspaces",
          {
            tenant_id: input.tenantId,
            name: input.name,
            remote_project_id: input.remoteProjectId,
            default_agent_id: input.defaultAgentId,
            default_agent_version_id: input.defaultAgentVersionId,
            default_model_profile_id: input.defaultModelProfileId,
            tool_policy: input.toolPolicy,
            file_policy: input.filePolicy,
            include_globs: input.includeGlobs,
            exclude_globs: input.excludeGlobs,
            trust_state: input.trustState,
            metadata: input.metadata
          },
          workspaceDtoSchema
        )
      );
    },
    async listLocalMounts(tenantId, workspaceId, limit = 100) {
      return (
        await http.get(`/workspaces/${workspaceId}/local-mounts`, dtoList(localMountDtoSchema), {
          query: { tenant_id: tenantId, limit }
        })
      ).map(mapLocalMount);
    },
    async createLocalMount(input) {
      return mapLocalMount(
        await http.post(
          `/workspaces/${input.workspaceId}/local-mounts`,
          {
            tenant_id: input.tenantId,
            display_name: input.displayName,
            virtual_path: input.virtualPath,
            capabilities: input.capabilities,
            include_globs: input.includeGlobs,
            exclude_globs: input.excludeGlobs,
            trust_state: input.trustState,
            metadata: input.metadata
          },
          localMountDtoSchema
        )
      );
    }
  };
}
