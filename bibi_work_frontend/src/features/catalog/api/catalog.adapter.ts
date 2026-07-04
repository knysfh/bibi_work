import {
  agentVersionCapabilitiesDtoSchema,
  dtoList,
  mapAgentVersionCapabilities,
  mapPolicyBinding,
  mapResource,
  mapValidationResponse,
  mapVersion,
  policyBindingDtoSchema,
  resourceDtoSchema,
  validationResponseDtoSchema,
  versionDtoSchema,
  type AgentVersionCapabilities,
  type PolicyBinding,
  type Resource,
  type ValidationResponse,
  type Version
} from "../../../shared/contracts/platform";
import type { HttpClient } from "../../../shared/api/http-client";
import type { JsonValue } from "../../../shared/types/json";

export type CatalogResourceKind =
  | "agents"
  | "skills"
  | "tools"
  | "mcpServers"
  | "llmProviders"
  | "llmModelProfiles";

export type VersionedCatalogKind = "agents" | "skills" | "tools";
export type EditableCatalogKind = VersionedCatalogKind;

export interface CatalogListInput {
  tenantId: string;
  kind: CatalogResourceKind;
  status?: string;
  limit?: number;
}

export interface CatalogVersionListInput {
  tenantId: string;
  kind: VersionedCatalogKind;
  resourceId: string;
  status?: string;
  limit?: number;
}

export interface McpToolListInput {
  tenantId: string;
  mcpServerId: string;
  status?: string;
  limit?: number;
}

export interface PolicyBindingFilters {
  tenantId: string;
  resourceType?: string;
  resourceId?: string;
  action?: string;
  includeDisabled?: boolean;
  limit?: number;
}

export interface CreatePolicyBindingInput {
  tenantId: string;
  resourceType: string;
  resourceId: string;
  action: string;
  subjectType: string;
  subjectId: string;
  effect: string;
  riskLevel?: string;
  obligations?: JsonValue;
  policyVersion?: string;
}

export interface DisablePolicyBindingInput {
  tenantId: string;
  bindingId: string;
  resourceType?: string;
  resourceId?: string;
}

export interface CreateCatalogResourceInput {
  tenantId: string;
  kind: EditableCatalogKind;
  name: string;
  description?: string;
  metadata?: JsonValue;
  draftConfig?: JsonValue;
  toolType?: string;
  schema?: JsonValue;
}

export interface DisableCatalogResourceInput {
  tenantId: string;
  kind: EditableCatalogKind;
  resourceId: string;
}

export interface PublishCatalogVersionInput {
  tenantId: string;
  kind: VersionedCatalogKind;
  resourceId: string;
  versionLabel: string;
  snapshot?: JsonValue;
  schemaHash?: string;
  contentHash?: string;
  sourceUri?: string;
  policyVersion?: string;
}

export interface DisableCatalogVersionInput {
  tenantId: string;
  kind: VersionedCatalogKind;
  versionId: string;
  resourceId?: string;
}

export interface AgentVersionTenantInput {
  tenantId: string;
  agentVersionId: string;
}

export interface CreateMcpServerInput {
  tenantId: string;
  name: string;
  description?: string;
  transport?: string;
  config?: JsonValue;
  secretRef?: string;
}

export interface UpdateMcpServerInput extends CreateMcpServerInput {
  mcpServerId: string;
}

export interface DiscoverMcpToolsInput {
  tenantId: string;
  mcpServerId: string;
}

export interface UpdateMcpToolInput {
  tenantId: string;
  mcpToolId: string;
  mcpServerId?: string;
  name?: string;
  description?: string;
  schema?: JsonValue;
  schemaHash?: string;
}

export interface DisableMcpToolInput {
  tenantId: string;
  mcpToolId: string;
  mcpServerId?: string;
}

export interface CatalogApi {
  listResources(input: CatalogListInput): Promise<Resource[]>;
  listVersions(input: CatalogVersionListInput): Promise<Version[]>;
  listMcpTools(input: McpToolListInput): Promise<Resource[]>;
  createMcpServer(input: CreateMcpServerInput): Promise<Resource>;
  updateMcpServer(input: UpdateMcpServerInput): Promise<Resource>;
  discoverMcpTools(input: DiscoverMcpToolsInput): Promise<Resource[]>;
  updateMcpTool(input: UpdateMcpToolInput): Promise<Resource>;
  disableMcpTool(input: DisableMcpToolInput): Promise<Resource>;
  listPolicyBindings(filters: PolicyBindingFilters): Promise<PolicyBinding[]>;
  createPolicyBinding(input: CreatePolicyBindingInput): Promise<PolicyBinding>;
  disablePolicyBinding(input: DisablePolicyBindingInput): Promise<PolicyBinding>;
  createResource(input: CreateCatalogResourceInput): Promise<Resource>;
  disableResource(input: DisableCatalogResourceInput): Promise<Resource>;
  publishVersion(input: PublishCatalogVersionInput): Promise<Version>;
  disableVersion(input: DisableCatalogVersionInput): Promise<Version>;
  getAgentVersionCapabilities(input: AgentVersionTenantInput): Promise<AgentVersionCapabilities>;
  validateAgentVersion(input: AgentVersionTenantInput): Promise<ValidationResponse>;
}

const resourcePaths: Record<CatalogResourceKind, string> = {
  agents: "/agents",
  skills: "/skills",
  tools: "/tools",
  mcpServers: "/mcp-servers",
  llmProviders: "/llm-providers",
  llmModelProfiles: "/llm-model-profiles"
};

const versionPaths: Record<VersionedCatalogKind, (resourceId: string) => string> = {
  agents: (resourceId) => `/agents/${resourceId}/versions`,
  skills: (resourceId) => `/skills/${resourceId}/versions`,
  tools: (resourceId) => `/tools/${resourceId}/versions`
};

const versionDisablePaths: Record<VersionedCatalogKind, (versionId: string) => string> = {
  agents: (versionId) => `/agent-versions/${versionId}/disable`,
  skills: (versionId) => `/skill-versions/${versionId}/disable`,
  tools: (versionId) => `/tool-versions/${versionId}/disable`
};

export function createCatalogApi(http: HttpClient): CatalogApi {
  return {
    async listResources(input) {
      return (
        await http.get(resourcePaths[input.kind], dtoList(resourceDtoSchema), {
          query: toTenantListQuery(input)
        })
      ).map(mapResource);
    },
    async listVersions(input) {
      return (
        await http.get(versionPaths[input.kind](input.resourceId), dtoList(versionDtoSchema), {
          query: toTenantListQuery(input)
        })
      ).map(mapVersion);
    },
    async listMcpTools(input) {
      return (
        await http.get(`/mcp-servers/${input.mcpServerId}/tools`, dtoList(resourceDtoSchema), {
          query: toTenantListQuery(input)
        })
      ).map(mapResource);
    },
    async createMcpServer(input) {
      return mapResource(
        await http.post("/mcp-servers", toMcpServerBody(input), resourceDtoSchema)
      );
    },
    async updateMcpServer(input) {
      return mapResource(
        await http.patch(
          `/mcp-servers/${input.mcpServerId}`,
          toMcpServerBody(input),
          resourceDtoSchema
        )
      );
    },
    async discoverMcpTools(input) {
      return (
        await http.post(
          `/mcp-servers/${input.mcpServerId}/tools:discover`,
          { tenant_id: input.tenantId },
          dtoList(resourceDtoSchema)
        )
      ).map(mapResource);
    },
    async updateMcpTool(input) {
      return mapResource(
        await http.patch(
          `/mcp-tools/${input.mcpToolId}`,
          {
            tenant_id: input.tenantId,
            name: input.name,
            description: input.description,
            schema: input.schema,
            schema_hash: input.schemaHash
          },
          resourceDtoSchema
        )
      );
    },
    async disableMcpTool(input) {
      return mapResource(
        await http.post(
          `/mcp-tools/${input.mcpToolId}/disable`,
          { tenant_id: input.tenantId },
          resourceDtoSchema
        )
      );
    },
    async listPolicyBindings(filters) {
      return (
        await http.get("/policy-bindings", dtoList(policyBindingDtoSchema), {
          query: {
            tenant_id: filters.tenantId,
            resource_type: filters.resourceType,
            resource_id: filters.resourceId,
            action: filters.action,
            include_disabled: filters.includeDisabled,
            limit: filters.limit ?? 100
          }
        })
      ).map(mapPolicyBinding);
    },
    async createPolicyBinding(input) {
      return mapPolicyBinding(
        await http.post(
          "/policy-bindings",
          {
            tenant_id: input.tenantId,
            resource_type: input.resourceType,
            resource_id: input.resourceId,
            action: input.action,
            subject_type: input.subjectType,
            subject_id: input.subjectId,
            effect: input.effect,
            risk_level: input.riskLevel,
            obligations: input.obligations,
            policy_version: input.policyVersion
          },
          policyBindingDtoSchema
        )
      );
    },
    async disablePolicyBinding(input) {
      return mapPolicyBinding(
        await http.post(
          `/policy-bindings/${input.bindingId}/disable`,
          { tenant_id: input.tenantId },
          policyBindingDtoSchema
        )
      );
    },
    async createResource(input) {
      return mapResource(
        await http.post(resourcePaths[input.kind], toCreateResourceBody(input), resourceDtoSchema)
      );
    },
    async disableResource(input) {
      return mapResource(
        await http.post(
          `${resourcePaths[input.kind]}/${input.resourceId}/disable`,
          { tenant_id: input.tenantId },
          resourceDtoSchema
        )
      );
    },
    async publishVersion(input) {
      return mapVersion(
        await http.post(
          versionPaths[input.kind](input.resourceId),
          {
            tenant_id: input.tenantId,
            version_label: input.versionLabel,
            snapshot: input.snapshot,
            schema_hash: input.schemaHash,
            content_hash: input.contentHash,
            source_uri: input.sourceUri,
            policy_version: input.policyVersion
          },
          versionDtoSchema
        )
      );
    },
    async disableVersion(input) {
      return mapVersion(
        await http.post(
          versionDisablePaths[input.kind](input.versionId),
          { tenant_id: input.tenantId },
          versionDtoSchema
        )
      );
    },
    async getAgentVersionCapabilities(input) {
      return mapAgentVersionCapabilities(
        await http.get(
          `/agent-versions/${input.agentVersionId}/effective-capabilities`,
          agentVersionCapabilitiesDtoSchema,
          { query: { tenant_id: input.tenantId } }
        )
      );
    },
    async validateAgentVersion(input) {
      return mapValidationResponse(
        await http.post(
          `/agent-versions/${input.agentVersionId}/validate`,
          { tenant_id: input.tenantId },
          validationResponseDtoSchema
        )
      );
    }
  };
}

function toTenantListQuery(input: { tenantId: string; status?: string; limit?: number }) {
  return {
    tenant_id: input.tenantId,
    status: input.status,
    limit: input.limit ?? 100
  };
}

function toCreateResourceBody(input: CreateCatalogResourceInput) {
  const common = {
    tenant_id: input.tenantId,
    name: input.name,
    description: input.description,
    metadata: input.metadata
  };
  if (input.kind === "agents") {
    return { ...common, draft_config: input.draftConfig };
  }
  if (input.kind === "tools") {
    return { ...common, tool_type: input.toolType, schema: input.schema };
  }
  return common;
}

function toMcpServerBody(input: CreateMcpServerInput) {
  return {
    tenant_id: input.tenantId,
    name: input.name,
    description: input.description,
    transport: input.transport,
    config: input.config,
    secret_ref: input.secretRef
  };
}
