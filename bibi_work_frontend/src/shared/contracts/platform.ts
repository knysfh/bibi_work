import { z } from "zod";
import type { JsonValue } from "../types/json";

export const jsonValueSchema: z.ZodType<JsonValue> = z.lazy(() =>
  z.union([
    z.string(),
    z.number(),
    z.boolean(),
    z.null(),
    z.array(jsonValueSchema),
    z.record(z.string(), jsonValueSchema)
  ])
);

const nullableString = z.string().nullable().optional();
const nullableUuid = z.string().uuid().nullable().optional();
const nullableDate = z.string().nullable().optional();

export const oidcConfigDtoSchema = z.object({
  issuer: z.string(),
  audience: z.string(),
  authorization_endpoint: nullableString,
  token_endpoint: nullableString,
  jwks_uri: nullableString
});

export const meDtoSchema = z.object({
  tenant_id: z.string().uuid(),
  user: z.object({
    id: z.string().uuid(),
    tenant_id: z.string().uuid(),
    ferriskey_subject: z.string(),
    username: nullableString,
    email: nullableString,
    display_name: nullableString,
    status: z.string(),
    created_at: z.string(),
    updated_at: z.string()
  }),
  tenants: z.array(
    z.object({
      id: z.string().uuid(),
      name: z.string(),
      slug: z.string(),
      membership_role: z.string(),
      metadata: jsonValueSchema
    })
  ),
  roles: z.array(z.string()),
  capabilities: z.array(z.string()),
  device: z.object({
    id: z.string().uuid(),
    tenant_id: z.string().uuid(),
    device_name: z.string(),
    platform: z.string(),
    trust_level: z.string(),
    last_seen_at: nullableDate,
    revoked_at: nullableDate
  }),
  session: z.object({
    id: z.string().uuid(),
    tenant_id: z.string().uuid(),
    device_id: z.string().uuid(),
    token_exp: z.string(),
    last_seen_at: nullableDate,
    source_ip: nullableString,
    user_agent: nullableString,
    revoked_at: nullableDate
  })
});

export const resourceDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  name: z.string(),
  description: nullableString,
  status: z.string(),
  metadata: jsonValueSchema,
  created_at: z.string(),
  updated_at: nullableDate
});

export const versionDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  parent_id: z.string().uuid(),
  version_label: z.string(),
  snapshot: jsonValueSchema,
  policy_version: nullableString,
  status: z.string(),
  created_at: z.string()
});

export const policyBindingDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  resource_type: z.string(),
  resource_id: z.string(),
  action: z.string(),
  subject_type: z.string(),
  subject_id: z.string(),
  effect: z.string(),
  risk_level: z.string(),
  obligations: jsonValueSchema,
  policy_version: z.string(),
  created_by_user_id: nullableUuid,
  created_at: z.string(),
  disabled_at: nullableDate
});

export const validationResponseDtoSchema = z.object({
  valid: z.boolean(),
  errors: z.array(z.string()),
  warnings: z.array(z.string())
});

export const capabilityResourceDtoSchema = z.object({
  resource_type: z.string(),
  resource_id: z.string().uuid(),
  version_id: nullableUuid,
  parent_id: nullableUuid,
  name: z.string(),
  description: nullableString,
  status: z.string(),
  snapshot: jsonValueSchema,
  schema_hash: nullableString,
  content_hash: nullableString,
  source_uri: nullableString
});

export const agentVersionCapabilitiesDtoSchema = z.object({
  agent_version_id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  agent_id: z.string().uuid(),
  version_label: z.string(),
  status: z.string(),
  policy_version: z.string(),
  config_snapshot: jsonValueSchema,
  skills: z.array(capabilityResourceDtoSchema),
  tools: z.array(capabilityResourceDtoSchema),
  mcp_tools: z.array(capabilityResourceDtoSchema)
});

export const conversationDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  workspace_id: nullableUuid,
  project_id: nullableUuid,
  agent_id: nullableUuid,
  title: z.string(),
  status: z.string(),
  metadata: jsonValueSchema,
  created_at: z.string(),
  updated_at: z.string()
});

export const runDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  conversation_id: z.string().uuid(),
  workspace_id: nullableUuid,
  agent_id: nullableUuid,
  agent_version_id: nullableUuid,
  project_id: nullableUuid,
  status: z.string(),
  trace_id: z.string(),
  thread_id: nullableString,
  policy_version: z.string(),
  run_scope_snapshot: jsonValueSchema.optional().default({}),
  queued_at: z.string(),
  updated_at: z.string()
});

export const workspaceDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  owner_user_id: nullableUuid,
  name: z.string(),
  remote_project_id: nullableUuid,
  default_agent_id: nullableUuid,
  default_agent_version_id: nullableUuid,
  default_model_profile_id: nullableUuid,
  tool_policy: jsonValueSchema,
  file_policy: jsonValueSchema,
  include_globs: jsonValueSchema,
  exclude_globs: jsonValueSchema,
  trust_state: z.string(),
  metadata: jsonValueSchema,
  status: z.string(),
  created_at: z.string(),
  updated_at: z.string()
});

export const localMountDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  user_id: z.string().uuid(),
  device_id: z.string().uuid(),
  workspace_id: z.string().uuid(),
  display_name: z.string(),
  virtual_path: z.string(),
  capabilities: jsonValueSchema,
  include_globs: jsonValueSchema,
  exclude_globs: jsonValueSchema,
  trust_state: z.string(),
  metadata: jsonValueSchema,
  status: z.string(),
  created_at: z.string(),
  updated_at: z.string()
});

export const streamEventDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  conversation_id: z.string().uuid(),
  run_id: nullableUuid,
  seq: z.number(),
  event_id: z.string(),
  type: z.string(),
  payload: jsonValueSchema,
  trace_id: nullableString,
  created_at: z.string()
});

export const approvalDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  conversation_id: nullableUuid,
  run_id: nullableUuid,
  tool_call_id: nullableUuid,
  status: z.string(),
  approval_policy_id: nullableString,
  request_payload: jsonValueSchema,
  decision_payload: jsonValueSchema.nullable().optional(),
  evidence_object_reference_id: nullableUuid,
  created_at: z.string(),
  decided_at: nullableDate
});

export const deviceDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  user_id: z.string().uuid(),
  device_name: z.string(),
  platform: z.string(),
  trust_level: z.string(),
  last_seen_at: nullableDate,
  revoked_at: nullableDate,
  created_at: z.string(),
  updated_at: z.string()
});

export const sessionDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  user_id: z.string().uuid(),
  device_id: z.string().uuid(),
  ferriskey_subject: z.string(),
  ferriskey_session_state: z.string(),
  token_jti: nullableString,
  token_exp: z.string(),
  roles_snapshot: jsonValueSchema,
  last_seen_at: nullableDate,
  source_ip: nullableString,
  user_agent: nullableString,
  revoked_at: nullableDate,
  created_at: z.string(),
  updated_at: z.string()
});

export const fileRevisionDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  project_id: z.string().uuid(),
  path: z.string(),
  revision: z.number(),
  etag: z.string(),
  content_hash: z.string(),
  object_key: z.string(),
  object_reference_id: nullableUuid,
  bucket: nullableString,
  version_id: nullableString,
  inline_content: nullableString,
  content_base64: nullableString,
  size_bytes: z.number(),
  content_type: z.string(),
  is_binary: z.boolean(),
  is_large: z.boolean(),
  reason: z.string(),
  run_id: nullableUuid,
  metadata: jsonValueSchema,
  created_at: z.string()
});

export const fileEntryDtoSchema = z.object({
  path: z.string(),
  entry_type: z.string(),
  depth: z.number(),
  children_count: z.number(),
  latest_revision: z.number().nullable().optional(),
  size_bytes: z.number().nullable().optional()
});

export const fileListDtoSchema = z.object({
  files: z.array(fileRevisionDtoSchema),
  entries: z.array(fileEntryDtoSchema).default([])
});

export const toolResultArtifactReadDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  run_id: nullableUuid,
  tool_call_id: nullableUuid,
  view_kind: z.string(),
  ref_kind: z.string(),
  project_id: z.string().uuid(),
  path: z.string(),
  revision: z.number(),
  file_revision_id: z.string().uuid(),
  object_reference_id: z.string().uuid(),
  content_hash: z.string(),
  content_type: z.string(),
  size_bytes: z.number(),
  content: jsonValueSchema,
  created_at: z.string()
});

export const memoryItemDtoSchema = z.object({
  id: z.string().uuid(),
  tenant_id: z.string().uuid(),
  user_id: nullableUuid,
  agent_id: nullableUuid,
  project_id: nullableUuid,
  source_run_id: nullableUuid,
  layer: z.string(),
  content: z.string(),
  confidence: z.number(),
  status: z.string(),
  visibility: z.string(),
  sensitivity: z.string(),
  created_at: z.string(),
  updated_at: z.string()
});

export const memoryBatchDecisionResultDtoSchema = z.object({
  memory_id: z.string().uuid(),
  status: z.string(),
  memory: memoryItemDtoSchema.nullable().optional(),
  error_code: nullableString,
  error_message: nullableString
});

export const memoryBatchDecisionResponseDtoSchema = z.object({
  decision: z.string(),
  target_status: z.string(),
  succeeded: z.number(),
  failed: z.number(),
  results: z.array(memoryBatchDecisionResultDtoSchema)
});

export const genericResponseDtoSchema = z.object({
  code: z.string(),
  message: z.string()
});

export const dtoList = <T extends z.ZodTypeAny>(schema: T) => z.array(schema);

export type OidcConfigDto = z.infer<typeof oidcConfigDtoSchema>;
export type MeDto = z.infer<typeof meDtoSchema>;
export type ResourceDto = z.infer<typeof resourceDtoSchema>;
export type VersionDto = z.infer<typeof versionDtoSchema>;
export type PolicyBindingDto = z.infer<typeof policyBindingDtoSchema>;
export type ValidationResponseDto = z.infer<typeof validationResponseDtoSchema>;
export type CapabilityResourceDto = z.infer<typeof capabilityResourceDtoSchema>;
export type AgentVersionCapabilitiesDto = z.infer<typeof agentVersionCapabilitiesDtoSchema>;
export type ConversationDto = z.infer<typeof conversationDtoSchema>;
export type RunDto = z.infer<typeof runDtoSchema>;
export type WorkspaceDto = z.infer<typeof workspaceDtoSchema>;
export type LocalMountDto = z.infer<typeof localMountDtoSchema>;
export type StreamEventDto = z.infer<typeof streamEventDtoSchema>;
export type ApprovalDto = z.infer<typeof approvalDtoSchema>;
export type DeviceDto = z.infer<typeof deviceDtoSchema>;
export type SessionDto = z.infer<typeof sessionDtoSchema>;
export type FileRevisionDto = z.infer<typeof fileRevisionDtoSchema>;
export type FileEntryDto = z.infer<typeof fileEntryDtoSchema>;
export type FileListDto = z.infer<typeof fileListDtoSchema>;
export type ToolResultArtifactReadDto = z.infer<typeof toolResultArtifactReadDtoSchema>;
export type MemoryItemDto = z.infer<typeof memoryItemDtoSchema>;
export type MemoryBatchDecisionResultDto = z.infer<typeof memoryBatchDecisionResultDtoSchema>;
export type MemoryBatchDecisionResponseDto = z.infer<typeof memoryBatchDecisionResponseDtoSchema>;

export interface OidcConfig {
  issuer: string;
  audience: string;
  authorizationEndpoint?: string;
  tokenEndpoint?: string;
  jwksUri?: string;
}

export interface Me {
  tenantId: string;
  user: {
    id: string;
    tenantId: string;
    ferriskeySubject: string;
    username?: string;
    email?: string;
    displayName?: string;
    status: string;
    createdAt: string;
    updatedAt: string;
  };
  tenants: Array<{
    id: string;
    name: string;
    slug: string;
    membershipRole: string;
    metadata: JsonValue;
  }>;
  roles: string[];
  capabilities: string[];
  device: {
    id: string;
    tenantId: string;
    deviceName: string;
    platform: string;
    trustLevel: string;
    lastSeenAt?: string;
    revokedAt?: string;
  };
  session: {
    id: string;
    tenantId: string;
    deviceId: string;
    tokenExp: string;
    lastSeenAt?: string;
    sourceIp?: string;
    userAgent?: string;
    revokedAt?: string;
  };
}

export interface Resource {
  id: string;
  tenantId: string;
  name: string;
  description?: string;
  status: string;
  metadata: JsonValue;
  createdAt: string;
  updatedAt?: string;
}

export interface Version {
  id: string;
  tenantId: string;
  parentId: string;
  versionLabel: string;
  snapshot: JsonValue;
  policyVersion?: string;
  status: string;
  createdAt: string;
}

export interface PolicyBinding {
  id: string;
  tenantId: string;
  resourceType: string;
  resourceId: string;
  action: string;
  subjectType: string;
  subjectId: string;
  effect: string;
  riskLevel: string;
  obligations: JsonValue;
  policyVersion: string;
  createdByUserId?: string;
  createdAt: string;
  disabledAt?: string;
}

export interface ValidationResponse {
  valid: boolean;
  errors: string[];
  warnings: string[];
}

export interface CapabilityResource {
  resourceType: string;
  resourceId: string;
  versionId?: string;
  parentId?: string;
  name: string;
  description?: string;
  status: string;
  snapshot: JsonValue;
  schemaHash?: string;
  contentHash?: string;
  sourceUri?: string;
}

export interface AgentVersionCapabilities {
  agentVersionId: string;
  tenantId: string;
  agentId: string;
  versionLabel: string;
  status: string;
  policyVersion: string;
  configSnapshot: JsonValue;
  skills: CapabilityResource[];
  tools: CapabilityResource[];
  mcpTools: CapabilityResource[];
}

export interface Conversation {
  id: string;
  tenantId: string;
  workspaceId?: string;
  projectId?: string;
  agentId?: string;
  title: string;
  status: string;
  metadata: JsonValue;
  createdAt: string;
  updatedAt: string;
}

export interface Run {
  id: string;
  tenantId: string;
  conversationId: string;
  workspaceId?: string;
  agentId?: string;
  agentVersionId?: string;
  projectId?: string;
  status: string;
  traceId: string;
  threadId?: string;
  policyVersion: string;
  runScopeSnapshot: JsonValue;
  queuedAt: string;
  updatedAt: string;
}

export interface Workspace {
  id: string;
  tenantId: string;
  ownerUserId?: string;
  name: string;
  remoteProjectId?: string;
  defaultAgentId?: string;
  defaultAgentVersionId?: string;
  defaultModelProfileId?: string;
  toolPolicy: JsonValue;
  filePolicy: JsonValue;
  includeGlobs: JsonValue;
  excludeGlobs: JsonValue;
  trustState: string;
  metadata: JsonValue;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface LocalMount {
  id: string;
  tenantId: string;
  userId: string;
  deviceId: string;
  workspaceId: string;
  displayName: string;
  virtualPath: string;
  capabilities: JsonValue;
  includeGlobs: JsonValue;
  excludeGlobs: JsonValue;
  trustState: string;
  metadata: JsonValue;
  status: string;
  createdAt: string;
  updatedAt: string;
}

export interface RunEvent {
  id: string;
  tenantId: string;
  conversationId: string;
  runId?: string;
  seq: number;
  eventId: string;
  type: string;
  payload: JsonValue;
  traceId?: string;
  createdAt: string;
}

export interface Approval {
  id: string;
  tenantId: string;
  conversationId?: string;
  runId?: string;
  toolCallId?: string;
  status: string;
  approvalPolicyId?: string;
  requestPayload: JsonValue;
  decisionPayload?: JsonValue | null;
  evidenceObjectReferenceId?: string;
  createdAt: string;
  decidedAt?: string;
}

export interface Device {
  id: string;
  tenantId: string;
  userId: string;
  deviceName: string;
  platform: string;
  trustLevel: string;
  lastSeenAt?: string;
  revokedAt?: string;
  createdAt: string;
  updatedAt: string;
}

export interface Session {
  id: string;
  tenantId: string;
  userId: string;
  deviceId: string;
  ferriskeySubject: string;
  ferriskeySessionState: string;
  tokenJti?: string;
  tokenExp: string;
  rolesSnapshot: JsonValue;
  lastSeenAt?: string;
  sourceIp?: string;
  userAgent?: string;
  revokedAt?: string;
  createdAt: string;
  updatedAt: string;
}

export interface FileRevision {
  id: string;
  tenantId: string;
  projectId: string;
  path: string;
  revision: number;
  etag: string;
  contentHash: string;
  objectKey: string;
  objectReferenceId?: string;
  bucket?: string;
  versionId?: string;
  inlineContent?: string;
  contentBase64?: string;
  sizeBytes: number;
  contentType: string;
  isBinary: boolean;
  isLarge: boolean;
  reason: string;
  runId?: string;
  metadata: JsonValue;
  createdAt: string;
}

export interface FileEntry {
  path: string;
  entryType: string;
  depth: number;
  childrenCount: number;
  latestRevision?: number;
  sizeBytes?: number;
}

export interface FileList {
  files: FileRevision[];
  entries: FileEntry[];
}

export interface ToolResultArtifactRead {
  id: string;
  tenantId: string;
  runId?: string;
  toolCallId?: string;
  viewKind: string;
  refKind: string;
  projectId: string;
  path: string;
  revision: number;
  fileRevisionId: string;
  objectReferenceId: string;
  contentHash: string;
  contentType: string;
  sizeBytes: number;
  content: JsonValue;
  createdAt: string;
}

export interface MemoryItem {
  id: string;
  tenantId: string;
  userId?: string;
  agentId?: string;
  projectId?: string;
  sourceRunId?: string;
  layer: string;
  content: string;
  confidence: number;
  status: string;
  visibility: string;
  sensitivity: string;
  createdAt: string;
  updatedAt: string;
}

export interface MemoryBatchDecisionResult {
  memoryId: string;
  status: string;
  memory?: MemoryItem;
  errorCode?: string;
  errorMessage?: string;
}

export interface MemoryBatchDecisionResponse {
  decision: string;
  targetStatus: string;
  succeeded: number;
  failed: number;
  results: MemoryBatchDecisionResult[];
}

export function mapOidcConfig(dto: OidcConfigDto): OidcConfig {
  return {
    issuer: dto.issuer,
    audience: dto.audience,
    authorizationEndpoint: dto.authorization_endpoint ?? undefined,
    tokenEndpoint: dto.token_endpoint ?? undefined,
    jwksUri: dto.jwks_uri ?? undefined
  };
}

export function mapMe(dto: MeDto): Me {
  return {
    tenantId: dto.tenant_id,
    user: {
      id: dto.user.id,
      tenantId: dto.user.tenant_id,
      ferriskeySubject: dto.user.ferriskey_subject,
      username: dto.user.username ?? undefined,
      email: dto.user.email ?? undefined,
      displayName: dto.user.display_name ?? undefined,
      status: dto.user.status,
      createdAt: dto.user.created_at,
      updatedAt: dto.user.updated_at
    },
    tenants: dto.tenants.map((tenant) => ({
      id: tenant.id,
      name: tenant.name,
      slug: tenant.slug,
      membershipRole: tenant.membership_role,
      metadata: tenant.metadata
    })),
    roles: dto.roles,
    capabilities: dto.capabilities,
    device: {
      id: dto.device.id,
      tenantId: dto.device.tenant_id,
      deviceName: dto.device.device_name,
      platform: dto.device.platform,
      trustLevel: dto.device.trust_level,
      lastSeenAt: dto.device.last_seen_at ?? undefined,
      revokedAt: dto.device.revoked_at ?? undefined
    },
    session: {
      id: dto.session.id,
      tenantId: dto.session.tenant_id,
      deviceId: dto.session.device_id,
      tokenExp: dto.session.token_exp,
      lastSeenAt: dto.session.last_seen_at ?? undefined,
      sourceIp: dto.session.source_ip ?? undefined,
      userAgent: dto.session.user_agent ?? undefined,
      revokedAt: dto.session.revoked_at ?? undefined
    }
  };
}

export function mapResource(dto: ResourceDto): Resource {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    name: dto.name,
    description: dto.description ?? undefined,
    status: dto.status,
    metadata: dto.metadata,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at ?? undefined
  };
}

export function mapVersion(dto: VersionDto): Version {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    parentId: dto.parent_id,
    versionLabel: dto.version_label,
    snapshot: dto.snapshot,
    policyVersion: dto.policy_version ?? undefined,
    status: dto.status,
    createdAt: dto.created_at
  };
}

export function mapPolicyBinding(dto: PolicyBindingDto): PolicyBinding {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    resourceType: dto.resource_type,
    resourceId: dto.resource_id,
    action: dto.action,
    subjectType: dto.subject_type,
    subjectId: dto.subject_id,
    effect: dto.effect,
    riskLevel: dto.risk_level,
    obligations: dto.obligations,
    policyVersion: dto.policy_version,
    createdByUserId: dto.created_by_user_id ?? undefined,
    createdAt: dto.created_at,
    disabledAt: dto.disabled_at ?? undefined
  };
}

export function mapValidationResponse(dto: ValidationResponseDto): ValidationResponse {
  return {
    valid: dto.valid,
    errors: dto.errors,
    warnings: dto.warnings
  };
}

export function mapCapabilityResource(dto: CapabilityResourceDto): CapabilityResource {
  return {
    resourceType: dto.resource_type,
    resourceId: dto.resource_id,
    versionId: dto.version_id ?? undefined,
    parentId: dto.parent_id ?? undefined,
    name: dto.name,
    description: dto.description ?? undefined,
    status: dto.status,
    snapshot: dto.snapshot,
    schemaHash: dto.schema_hash ?? undefined,
    contentHash: dto.content_hash ?? undefined,
    sourceUri: dto.source_uri ?? undefined
  };
}

export function mapAgentVersionCapabilities(
  dto: AgentVersionCapabilitiesDto
): AgentVersionCapabilities {
  return {
    agentVersionId: dto.agent_version_id,
    tenantId: dto.tenant_id,
    agentId: dto.agent_id,
    versionLabel: dto.version_label,
    status: dto.status,
    policyVersion: dto.policy_version,
    configSnapshot: dto.config_snapshot,
    skills: dto.skills.map(mapCapabilityResource),
    tools: dto.tools.map(mapCapabilityResource),
    mcpTools: dto.mcp_tools.map(mapCapabilityResource)
  };
}

export function mapConversation(dto: ConversationDto): Conversation {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    workspaceId: dto.workspace_id ?? undefined,
    projectId: dto.project_id ?? undefined,
    agentId: dto.agent_id ?? undefined,
    title: dto.title,
    status: dto.status,
    metadata: dto.metadata,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at
  };
}

export function mapRun(dto: RunDto): Run {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    conversationId: dto.conversation_id,
    workspaceId: dto.workspace_id ?? undefined,
    agentId: dto.agent_id ?? undefined,
    agentVersionId: dto.agent_version_id ?? undefined,
    projectId: dto.project_id ?? undefined,
    status: dto.status,
    traceId: dto.trace_id,
    threadId: dto.thread_id ?? undefined,
    policyVersion: dto.policy_version,
    runScopeSnapshot: dto.run_scope_snapshot,
    queuedAt: dto.queued_at,
    updatedAt: dto.updated_at
  };
}

export function mapWorkspace(dto: WorkspaceDto): Workspace {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    ownerUserId: dto.owner_user_id ?? undefined,
    name: dto.name,
    remoteProjectId: dto.remote_project_id ?? undefined,
    defaultAgentId: dto.default_agent_id ?? undefined,
    defaultAgentVersionId: dto.default_agent_version_id ?? undefined,
    defaultModelProfileId: dto.default_model_profile_id ?? undefined,
    toolPolicy: dto.tool_policy,
    filePolicy: dto.file_policy,
    includeGlobs: dto.include_globs,
    excludeGlobs: dto.exclude_globs,
    trustState: dto.trust_state,
    metadata: dto.metadata,
    status: dto.status,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at
  };
}

export function mapLocalMount(dto: LocalMountDto): LocalMount {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    userId: dto.user_id,
    deviceId: dto.device_id,
    workspaceId: dto.workspace_id,
    displayName: dto.display_name,
    virtualPath: dto.virtual_path,
    capabilities: dto.capabilities,
    includeGlobs: dto.include_globs,
    excludeGlobs: dto.exclude_globs,
    trustState: dto.trust_state,
    metadata: dto.metadata,
    status: dto.status,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at
  };
}

export function mapRunEvent(dto: StreamEventDto): RunEvent {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    conversationId: dto.conversation_id,
    runId: dto.run_id ?? undefined,
    seq: dto.seq,
    eventId: dto.event_id,
    type: dto.type,
    payload: dto.payload,
    traceId: dto.trace_id ?? undefined,
    createdAt: dto.created_at
  };
}

export function mapApproval(dto: ApprovalDto): Approval {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    conversationId: dto.conversation_id ?? undefined,
    runId: dto.run_id ?? undefined,
    toolCallId: dto.tool_call_id ?? undefined,
    status: dto.status,
    approvalPolicyId: dto.approval_policy_id ?? undefined,
    requestPayload: dto.request_payload,
    decisionPayload: dto.decision_payload,
    evidenceObjectReferenceId: dto.evidence_object_reference_id ?? undefined,
    createdAt: dto.created_at,
    decidedAt: dto.decided_at ?? undefined
  };
}

export function mapDevice(dto: DeviceDto): Device {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    userId: dto.user_id,
    deviceName: dto.device_name,
    platform: dto.platform,
    trustLevel: dto.trust_level,
    lastSeenAt: dto.last_seen_at ?? undefined,
    revokedAt: dto.revoked_at ?? undefined,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at
  };
}

export function mapSession(dto: SessionDto): Session {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    userId: dto.user_id,
    deviceId: dto.device_id,
    ferriskeySubject: dto.ferriskey_subject,
    ferriskeySessionState: dto.ferriskey_session_state,
    tokenJti: dto.token_jti ?? undefined,
    tokenExp: dto.token_exp,
    rolesSnapshot: dto.roles_snapshot,
    lastSeenAt: dto.last_seen_at ?? undefined,
    sourceIp: dto.source_ip ?? undefined,
    userAgent: dto.user_agent ?? undefined,
    revokedAt: dto.revoked_at ?? undefined,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at
  };
}

export function mapFileRevision(dto: FileRevisionDto): FileRevision {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    projectId: dto.project_id,
    path: dto.path,
    revision: dto.revision,
    etag: dto.etag,
    contentHash: dto.content_hash,
    objectKey: dto.object_key,
    objectReferenceId: dto.object_reference_id ?? undefined,
    bucket: dto.bucket ?? undefined,
    versionId: dto.version_id ?? undefined,
    inlineContent: dto.inline_content ?? undefined,
    contentBase64: dto.content_base64 ?? undefined,
    sizeBytes: dto.size_bytes,
    contentType: dto.content_type,
    isBinary: dto.is_binary,
    isLarge: dto.is_large,
    reason: dto.reason,
    runId: dto.run_id ?? undefined,
    metadata: dto.metadata,
    createdAt: dto.created_at
  };
}

export function mapFileEntry(dto: FileEntryDto): FileEntry {
  return {
    path: dto.path,
    entryType: dto.entry_type,
    depth: dto.depth,
    childrenCount: dto.children_count,
    latestRevision: dto.latest_revision ?? undefined,
    sizeBytes: dto.size_bytes ?? undefined
  };
}

export function mapFileList(dto: FileListDto): FileList {
  return {
    files: dto.files.map(mapFileRevision),
    entries: dto.entries.map(mapFileEntry)
  };
}

export function mapToolResultArtifactRead(
  dto: ToolResultArtifactReadDto
): ToolResultArtifactRead {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    runId: dto.run_id ?? undefined,
    toolCallId: dto.tool_call_id ?? undefined,
    viewKind: dto.view_kind,
    refKind: dto.ref_kind,
    projectId: dto.project_id,
    path: dto.path,
    revision: dto.revision,
    fileRevisionId: dto.file_revision_id,
    objectReferenceId: dto.object_reference_id,
    contentHash: dto.content_hash,
    contentType: dto.content_type,
    sizeBytes: dto.size_bytes,
    content: dto.content,
    createdAt: dto.created_at
  };
}

export function mapMemoryItem(dto: MemoryItemDto): MemoryItem {
  return {
    id: dto.id,
    tenantId: dto.tenant_id,
    userId: dto.user_id ?? undefined,
    agentId: dto.agent_id ?? undefined,
    projectId: dto.project_id ?? undefined,
    sourceRunId: dto.source_run_id ?? undefined,
    layer: dto.layer,
    content: dto.content,
    confidence: dto.confidence,
    status: dto.status,
    visibility: dto.visibility,
    sensitivity: dto.sensitivity,
    createdAt: dto.created_at,
    updatedAt: dto.updated_at
  };
}

export function mapMemoryBatchDecisionResult(
  dto: MemoryBatchDecisionResultDto
): MemoryBatchDecisionResult {
  return {
    memoryId: dto.memory_id,
    status: dto.status,
    memory: dto.memory ? mapMemoryItem(dto.memory) : undefined,
    errorCode: dto.error_code ?? undefined,
    errorMessage: dto.error_message ?? undefined
  };
}

export function mapMemoryBatchDecisionResponse(
  dto: MemoryBatchDecisionResponseDto
): MemoryBatchDecisionResponse {
  return {
    decision: dto.decision,
    targetStatus: dto.target_status,
    succeeded: dto.succeeded,
    failed: dto.failed,
    results: dto.results.map(mapMemoryBatchDecisionResult)
  };
}
