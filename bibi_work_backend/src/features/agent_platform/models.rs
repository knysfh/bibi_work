use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ActorRef {
    pub user_id: Uuid,
    pub device_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResourceRef {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub id: String,
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct AuthzContext {
    pub project_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub workflow_run_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub tool_id: Option<Uuid>,
    pub mcp_server_id: Option<Uuid>,
    pub args_hash: Option<String>,
    pub risk_level: Option<String>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthzCheckRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub action: String,
    pub resource: ResourceRef,
    pub context: Option<AuthzContext>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AuthzBatchCheckRequest {
    pub checks: Vec<AuthzCheckRequest>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthzDecision {
    pub decision: String,
    pub policy_version: String,
    pub reason_code: Option<String>,
    pub obligations: Option<AuthzObligations>,
}

impl AuthzDecision {
    pub fn allow(policy_version: impl Into<String>) -> Self {
        Self {
            decision: "allow".to_string(),
            policy_version: policy_version.into(),
            reason_code: None,
            obligations: Some(AuthzObligations::standard()),
        }
    }

    pub fn review(
        policy_version: impl Into<String>,
        reason_code: impl Into<String>,
        approval_policy_id: Option<String>,
    ) -> Self {
        Self {
            decision: "review".to_string(),
            policy_version: policy_version.into(),
            reason_code: Some(reason_code.into()),
            obligations: Some(AuthzObligations {
                approval_policy_id,
                approval_timeout_sec: Some(3600),
                audit_level: Some("high".to_string()),
                redact_fields: Some(vec![
                    "authorization".to_string(),
                    "secret".to_string(),
                    "token".to_string(),
                    "password".to_string(),
                ]),
                max_output_bytes: Some(1_048_576),
                require_mfa: Some(false),
            }),
        }
    }

    pub fn deny(policy_version: impl Into<String>, reason_code: impl Into<String>) -> Self {
        Self {
            decision: "deny".to_string(),
            policy_version: policy_version.into(),
            reason_code: Some(reason_code.into()),
            obligations: Some(AuthzObligations {
                approval_policy_id: None,
                approval_timeout_sec: None,
                audit_level: Some("high".to_string()),
                redact_fields: Some(vec!["secret".to_string(), "token".to_string()]),
                max_output_bytes: Some(0),
                require_mfa: Some(false),
            }),
        }
    }

    pub fn is_allow(&self) -> bool {
        self.decision == "allow"
    }

    pub fn is_review(&self) -> bool {
        self.decision == "review"
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AuthzObligations {
    pub approval_policy_id: Option<String>,
    pub approval_timeout_sec: Option<i64>,
    pub audit_level: Option<String>,
    pub redact_fields: Option<Vec<String>>,
    pub max_output_bytes: Option<i64>,
    pub require_mfa: Option<bool>,
}

impl AuthzObligations {
    pub fn standard() -> Self {
        Self {
            approval_policy_id: None,
            approval_timeout_sec: None,
            audit_level: Some("normal".to_string()),
            redact_fields: Some(vec![
                "authorization".to_string(),
                "secret".to_string(),
                "token".to_string(),
                "password".to_string(),
            ]),
            max_output_bytes: Some(1_048_576),
            require_mfa: Some(false),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AuthzBatchCheckResponse {
    pub decisions: Vec<AuthzDecision>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MeUserResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub ferriskey_subject: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, Clone)]
pub struct MeTenantResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub membership_role: String,
    pub metadata: Value,
}

#[derive(Debug, Serialize, Clone)]
pub struct MeDeviceResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub device_name: String,
    pub platform: String,
    pub trust_level: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MeSessionResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub device_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub token_exp: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MeResponse {
    pub tenant_id: Uuid,
    pub user: MeUserResponse,
    pub tenants: Vec<MeTenantResponse>,
    pub roles: Vec<String>,
    pub capabilities: Vec<String>,
    pub device: MeDeviceResponse,
    pub session: MeSessionResponse,
}

#[derive(Debug, Deserialize)]
pub struct CreateTenantRequest {
    pub name: String,
    pub slug: String,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDeviceRequest {
    pub tenant_id: Uuid,
    pub device_name: String,
    pub platform: String,
    pub public_key: Option<String>,
    pub trust_level: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeDeviceRequest {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct RevokeSessionRequest {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub draft_config: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub tenant_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub draft_config: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSkillRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSkillRequest {
    pub tenant_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateToolRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub tool_type: Option<String>,
    pub schema: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateToolRequest {
    pub tenant_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub tool_type: Option<String>,
    pub schema: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMcpServerRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub transport: Option<String>,
    pub config: Option<Value>,
    pub secret_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMcpServerRequest {
    pub tenant_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub transport: Option<String>,
    pub config: Option<Value>,
    pub secret_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertMcpToolRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub schema: Option<Value>,
    pub schema_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMcpToolRequest {
    pub tenant_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub schema: Option<Value>,
    pub schema_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverMcpToolsRequest {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateLlmProviderRequest {
    pub tenant_id: Uuid,
    pub provider_key: String,
    pub display_name: String,
    pub base_url: Option<String>,
    pub auth_scheme: Option<String>,
    pub default_headers_template: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLlmProviderRequest {
    pub tenant_id: Uuid,
    pub display_name: Option<String>,
    pub base_url: Option<String>,
    pub auth_scheme: Option<String>,
    pub default_headers_template: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLlmCredentialRequest {
    pub tenant_id: Uuid,
    pub provider_id: Uuid,
    pub owner_scope: Option<String>,
    pub owner_resource_id: Option<String>,
    pub secret_ref: String,
    pub secret_hash: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct RotateLlmCredentialRequest {
    pub tenant_id: Uuid,
    pub secret_ref: String,
    pub secret_hash: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLlmCredentialRotationPolicyRequest {
    pub tenant_id: Uuid,
    pub enabled: bool,
    pub interval_seconds: Option<i64>,
    pub rotate_before_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct LlmCredentialRotationHealthQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct LlmCredentialRotationAttemptQuery {
    pub tenant_id: Uuid,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateLlmModelProfileRequest {
    pub tenant_id: Uuid,
    pub provider_id: Uuid,
    pub credential_id: Option<Uuid>,
    pub profile_name: String,
    pub model_name: String,
    pub context_window: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub response_format: Option<Value>,
    pub tool_choice_policy: Option<Value>,
    pub rate_limit_policy: Option<Value>,
    pub cost_policy: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLlmModelProfileRequest {
    pub tenant_id: Uuid,
    pub credential_id: Option<Uuid>,
    pub profile_name: Option<String>,
    pub model_name: Option<String>,
    pub context_window: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub response_format: Option<Value>,
    pub tool_choice_policy: Option<Value>,
    pub rate_limit_policy: Option<Value>,
    pub cost_policy: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct PublishVersionRequest {
    pub tenant_id: Uuid,
    pub version_label: String,
    pub snapshot: Option<Value>,
    pub schema_hash: Option<String>,
    pub content_hash: Option<String>,
    pub source_uri: Option<String>,
    pub policy_version: Option<String>,
    pub secret_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VersionListQuery {
    pub tenant_id: Uuid,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DisableCatalogResourceRequest {
    pub tenant_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct ValidationResponse {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CapabilityResourceResponse {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub version_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub snapshot: Value,
    pub schema_hash: Option<String>,
    pub content_hash: Option<String>,
    pub source_uri: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentVersionCapabilitiesResponse {
    pub agent_version_id: Uuid,
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub version_label: String,
    pub status: String,
    pub policy_version: String,
    pub config_snapshot: Value,
    pub skills: Vec<CapabilityResourceResponse>,
    pub tools: Vec<CapabilityResourceResponse>,
    pub sql_tools: Vec<CapabilityResourceResponse>,
    pub mcp_tools: Vec<CapabilityResourceResponse>,
}

#[derive(Debug, Deserialize)]
pub struct BindAgentVersionRequest {
    pub tenant_id: Uuid,
    pub skill_version_ids: Option<Vec<Uuid>>,
    pub tool_version_ids: Option<Vec<Uuid>>,
    pub sql_tool_version_ids: Option<Vec<Uuid>>,
    pub mcp_tool_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct PolicyBindingQuery {
    pub tenant_id: Option<Uuid>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub action: Option<String>,
    pub include_disabled: Option<bool>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditHashChainVerifyQuery {
    pub tenant_id: Uuid,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditHashChainSealRequest {
    pub tenant_id: Uuid,
    pub max_rows: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditLegalHoldQuery {
    pub tenant_id: Uuid,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAuditLegalHoldRequest {
    pub tenant_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub resource_type: Option<String>,
    pub reason: String,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseAuditLegalHoldRequest {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct AuditRetentionEligibilityQuery {
    pub tenant_id: Uuid,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditHashBackfillQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct AuditHashBackfillRequest {
    pub tenant_id: Uuid,
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

#[derive(Debug, Deserialize)]
pub struct AuditPartitionCleanupRequest {
    pub tenant_id: Uuid,
    pub partition_name: String,
    #[serde(default = "default_true")]
    pub dry_run: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct CreatePolicyBindingRequest {
    pub tenant_id: Uuid,
    pub resource_type: String,
    pub resource_id: String,
    pub action: String,
    pub subject_type: String,
    pub subject_id: String,
    pub effect: String,
    pub risk_level: Option<String>,
    pub obligations: Option<Value>,
    pub policy_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DisablePolicyBindingRequest {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectMountRequest {
    pub tenant_id: Uuid,
    pub virtual_path: String,
    pub backend_type: String,
    pub backend_ref: Option<String>,
    pub mount_config: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub remote_project_id: Option<Uuid>,
    pub default_agent_id: Option<Uuid>,
    pub default_agent_version_id: Option<Uuid>,
    pub default_model_profile_id: Option<Uuid>,
    pub tool_policy: Option<Value>,
    pub file_policy: Option<Value>,
    pub include_globs: Option<Value>,
    pub exclude_globs: Option<Value>,
    pub trust_state: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NullableUuidPatch {
    #[default]
    Missing,
    Clear,
    Set(Uuid),
}

impl<'de> Deserialize<'de> for NullableUuidPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<Uuid>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkspaceRequest {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub remote_project_id: NullableUuidPatch,
}

#[derive(Debug, Deserialize)]
pub struct CreateLocalMountRequest {
    pub tenant_id: Uuid,
    pub display_name: String,
    pub virtual_path: String,
    pub capabilities: Option<Value>,
    pub include_globs: Option<Value>,
    pub exclude_globs: Option<Value>,
    pub trust_state: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub tenant_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub title: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateConversationRequest {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub project_id: NullableUuidPatch,
}

#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    pub tenant_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub agent_version_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub idempotency_key: Option<String>,
    pub input: Option<Value>,
    pub run_config_snapshot: Option<Value>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentTeamListQuery {
    pub tenant_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentTeamRequest {
    pub tenant_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentTeamRequest {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub workspace_id: NullableUuidPatch,
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentTeamMemberRequest {
    pub tenant_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Option<Uuid>,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub slot_order: i32,
    pub policy_snapshot: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentTeamMemberRequest {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub agent_version_id: NullableUuidPatch,
    pub role: Option<String>,
    pub display_name: Option<String>,
    pub slot_order: Option<i32>,
    pub status: Option<String>,
    pub policy_snapshot: Option<Value>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct StartAgentTeamRunRequest {
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub project_id: Option<Uuid>,
    pub input: Option<Value>,
    pub run_config_snapshot: Option<Value>,
    pub thread_id: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CancelAgentTeamRunRequest {
    pub tenant_id: Uuid,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IngestRunEventsRequest {
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub run_id: Option<Uuid>,
    pub events: Vec<RunEventInput>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RunEventInput {
    pub event_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Option<Value>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEventKind {
    RunQueued,
    RunStarted,
    RunCompleted,
    RunFailed,
    RunCancelled,
    MessageStarted,
    MessageDelta,
    MessageCompleted,
    ThinkingStarted,
    ThinkingDelta,
    ThinkingCompleted,
    ToolCallRequested,
    ToolCallAuthorized,
    ToolCallApprovalRequired,
    ToolCallStarted,
    ToolCallDelta,
    ToolCallCompleted,
    ToolCallFailed,
    ApprovalRequested,
    ApprovalDecided,
    ArtifactDraftStarted,
    ArtifactDraftDelta,
    ArtifactDraftCompleted,
    ArtifactDraftFailed,
    FileChanged,
    TaskCreated,
    TaskUpdated,
    TaskCompleted,
    TaskFailed,
    SubagentStarted,
    SubagentMessage,
    SubagentToolCall,
    SubagentCompleted,
    SubagentFailed,
    WorkflowNodeStarted,
    WorkflowNodeCompleted,
    WorkflowNodeFailed,
    WorkflowNodeBlocked,
    TeamRunStarted,
    TeamRunUpdated,
    TeamRunCompleted,
    TeamRunFailed,
    TeamRunCancelled,
    TeamMemberQueued,
    TeamMemberStarted,
    TeamMemberUpdated,
    TeamMemberBlocked,
    TeamMemberCompleted,
    TeamMemberFailed,
    TeamMemberCancelled,
    ActivityRaw,
    LegacyInterruptRequested,
    LegacyApprovalCompleted,
    LocalExecCompleted,
    LocalExecFailed,
    LegacyToolCallUnknown,
}

impl RunEventKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "run.queued" => Some(Self::RunQueued),
            "run.started" => Some(Self::RunStarted),
            "run.completed" => Some(Self::RunCompleted),
            "run.failed" => Some(Self::RunFailed),
            "run.cancelled" => Some(Self::RunCancelled),
            "message.started" => Some(Self::MessageStarted),
            "message.delta" => Some(Self::MessageDelta),
            "message.completed" => Some(Self::MessageCompleted),
            "thinking.started" => Some(Self::ThinkingStarted),
            "thinking.delta" => Some(Self::ThinkingDelta),
            "thinking.completed" => Some(Self::ThinkingCompleted),
            "tool.call.requested" => Some(Self::ToolCallRequested),
            "tool.call.authorized" => Some(Self::ToolCallAuthorized),
            "tool.call.approval_required" => Some(Self::ToolCallApprovalRequired),
            "tool.call.started" => Some(Self::ToolCallStarted),
            "tool.call.delta" => Some(Self::ToolCallDelta),
            "tool.call.completed" => Some(Self::ToolCallCompleted),
            "tool.call.failed" => Some(Self::ToolCallFailed),
            "approval.requested" => Some(Self::ApprovalRequested),
            "approval.decided" => Some(Self::ApprovalDecided),
            "artifact.draft.started" => Some(Self::ArtifactDraftStarted),
            "artifact.draft.delta" => Some(Self::ArtifactDraftDelta),
            "artifact.draft.completed" => Some(Self::ArtifactDraftCompleted),
            "artifact.draft.failed" => Some(Self::ArtifactDraftFailed),
            "file.changed" => Some(Self::FileChanged),
            "task.created" => Some(Self::TaskCreated),
            "task.updated" => Some(Self::TaskUpdated),
            "task.completed" => Some(Self::TaskCompleted),
            "task.failed" => Some(Self::TaskFailed),
            "subagent.started" => Some(Self::SubagentStarted),
            "subagent.message" => Some(Self::SubagentMessage),
            "subagent.tool_call" => Some(Self::SubagentToolCall),
            "subagent.completed" => Some(Self::SubagentCompleted),
            "subagent.failed" => Some(Self::SubagentFailed),
            "workflow.node.started" => Some(Self::WorkflowNodeStarted),
            "workflow.node.completed" => Some(Self::WorkflowNodeCompleted),
            "workflow.node.failed" => Some(Self::WorkflowNodeFailed),
            "workflow.node.blocked" => Some(Self::WorkflowNodeBlocked),
            "team.run.started" => Some(Self::TeamRunStarted),
            "team.run.updated" => Some(Self::TeamRunUpdated),
            "team.run.completed" => Some(Self::TeamRunCompleted),
            "team.run.failed" => Some(Self::TeamRunFailed),
            "team.run.cancelled" => Some(Self::TeamRunCancelled),
            "team.member.queued" => Some(Self::TeamMemberQueued),
            "team.member.started" => Some(Self::TeamMemberStarted),
            "team.member.updated" => Some(Self::TeamMemberUpdated),
            "team.member.blocked" => Some(Self::TeamMemberBlocked),
            "team.member.completed" => Some(Self::TeamMemberCompleted),
            "team.member.failed" => Some(Self::TeamMemberFailed),
            "team.member.cancelled" => Some(Self::TeamMemberCancelled),
            "activity.raw" => Some(Self::ActivityRaw),
            "interrupt.requested" => Some(Self::LegacyInterruptRequested),
            "approval.completed" => Some(Self::LegacyApprovalCompleted),
            "local_exec.completed" => Some(Self::LocalExecCompleted),
            "local_exec.failed" => Some(Self::LocalExecFailed),
            "tool.call.unknown" => Some(Self::LegacyToolCallUnknown),
            _ => None,
        }
    }

    pub fn validate_payload(self, payload: &Value) -> Result<(), String> {
        let Some(object) = payload.as_object() else {
            return Err("event payload must be a JSON object".to_string());
        };
        let required = match self {
            Self::ToolCallRequested
            | Self::ToolCallAuthorized
            | Self::ToolCallApprovalRequired
            | Self::ToolCallStarted
            | Self::ToolCallDelta
            | Self::ToolCallCompleted
            | Self::ToolCallFailed => &["tool_call_id"][..],
            Self::ApprovalRequested | Self::ApprovalDecided | Self::LegacyApprovalCompleted => {
                &["approval_id"][..]
            }
            Self::ArtifactDraftStarted
            | Self::ArtifactDraftDelta
            | Self::ArtifactDraftCompleted
            | Self::ArtifactDraftFailed => &["draft_id"][..],
            Self::FileChanged => &["path"][..],
            Self::TeamRunStarted
            | Self::TeamRunUpdated
            | Self::TeamRunCompleted
            | Self::TeamRunFailed
            | Self::TeamRunCancelled => &["team_run_id"][..],
            Self::TeamMemberQueued
            | Self::TeamMemberStarted
            | Self::TeamMemberUpdated
            | Self::TeamMemberBlocked
            | Self::TeamMemberCompleted
            | Self::TeamMemberFailed
            | Self::TeamMemberCancelled => &["team_run_id", "team_member_id"][..],
            _ => &[][..],
        };
        for key in required {
            if !object.contains_key(*key) {
                return Err(format!("event payload missing required field `{key}`"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StreamEventResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub run_id: Option<Uuid>,
    pub seq: i64,
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: Value,
    pub trace_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchBootstrapResponse {
    pub me: MeResponse,
    pub navigation: WorkbenchNavigation,
    pub workspaces: Vec<WorkspaceSummary>,
    pub pinned_workspaces: Vec<WorkspaceSummary>,
    pub recent_conversations: Vec<ConversationSummary>,
    pub teams: Vec<AgentTeamSummary>,
    pub pending_approvals_count: i64,
    pub running_runs_count: i64,
    pub device: MeDeviceResponse,
    pub session: MeSessionResponse,
    pub feature_flags: WorkbenchFeatureFlags,
    pub ui_policy: WorkbenchUiPolicy,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchNavigation {
    pub primary: Vec<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchFeatureFlags {
    pub teams_enabled: bool,
    pub global_search_enabled: bool,
    pub preview_external_open_enabled: bool,
    pub auth: WorkbenchAuthFeatureFlags,
    pub runtime: WorkbenchRuntimeFeatureFlags,
    pub desktop: WorkbenchDesktopFeatureFlags,
    pub enterprise: WorkbenchEnterpriseFeatureFlags,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchAuthFeatureFlags {
    pub oidc_required: bool,
    pub password_login: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchRuntimeFeatureFlags {
    pub deepagents: bool,
    pub biwork_cli: bool,
    pub disabled: bool,
    pub remote_agent_direct: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchDesktopFeatureFlags {
    pub gateway_required_for_local_capabilities: bool,
    pub shell: bool,
    pub office_preview: bool,
    pub preview_history: bool,
    pub local_remote_control: bool,
    pub cdp_remote_control: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchEnterpriseFeatureFlags {
    pub assistants: bool,
    pub providers: bool,
    pub skills: bool,
    pub mcp: bool,
    pub conversations: bool,
    pub teams: bool,
    pub cron: bool,
    pub channel_governance: bool,
    pub extension_governance: bool,
}

impl WorkbenchFeatureFlags {
    pub fn biwork_enterprise_default() -> Self {
        Self {
            teams_enabled: true,
            global_search_enabled: true,
            preview_external_open_enabled: false,
            auth: WorkbenchAuthFeatureFlags {
                oidc_required: true,
                password_login: false,
            },
            runtime: WorkbenchRuntimeFeatureFlags {
                deepagents: true,
                biwork_cli: false,
                disabled: true,
                remote_agent_direct: false,
            },
            desktop: WorkbenchDesktopFeatureFlags {
                gateway_required_for_local_capabilities: true,
                shell: true,
                office_preview: true,
                preview_history: true,
                local_remote_control: false,
                cdp_remote_control: false,
            },
            enterprise: WorkbenchEnterpriseFeatureFlags {
                assistants: true,
                providers: true,
                skills: true,
                mcp: true,
                conversations: true,
                teams: true,
                cron: true,
                channel_governance: true,
                extension_governance: true,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WorkbenchUiPolicy {
    pub can_create_workspace: bool,
    pub can_mount_local_folder: bool,
    pub can_manage_catalog: bool,
    pub risk_auto_approval: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub status: String,
    pub trust_state: String,
    pub remote_project_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub available_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ConversationSummary {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub status: String,
    pub latest_run_status: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub unread_activity_count: i64,
}

#[derive(Debug, Serialize)]
pub struct AgentTeamSummary {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub member_count: i64,
    pub pending_approvals_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct AgentTeamResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub metadata: Value,
    pub members: Vec<AgentTeamMemberResponse>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub available_actions: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct AgentTeamMemberResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub team_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Option<Uuid>,
    pub role: String,
    pub display_name: String,
    pub slot_order: i32,
    pub policy_snapshot: Value,
    pub status: String,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct AgentTeamRunResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub team_id: Uuid,
    pub conversation_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub status: String,
    pub trace_id: String,
    pub thread_id: Option<String>,
    pub input_snapshot: Value,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub queued_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct AgentTeamRunMemberResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub team_run_id: Uuid,
    pub team_member_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub agent_version_id: Option<Uuid>,
    pub role: String,
    pub display_name: String,
    pub slot_order: i32,
    pub status: String,
    pub member_snapshot: Value,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub queued_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct AgentTeamRunDetailResponse {
    pub team_run: AgentTeamRunResponse,
    pub team: AgentTeamSummary,
    pub members: Vec<AgentTeamRunMemberResponse>,
    pub available_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchWorkspaceDetailResponse {
    pub workspace: WorkspaceSummary,
    pub local_mounts: Vec<LocalMountSummary>,
    pub project: Option<ResourceResponse>,
    pub conversations: Vec<ConversationSummary>,
    pub available_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalMountSummary {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub display_name: String,
    pub virtual_path: String,
    pub capabilities: Value,
    pub trust_state: String,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchConversationDetailResponse {
    pub conversation: ConversationSummary,
    pub workspace: Option<WorkspaceSummary>,
    pub project: Option<ResourceResponse>,
    pub latest_run: Option<RunResponse>,
    pub events: Vec<StreamEventResponse>,
    pub events_page: WorkbenchEventsPage,
    pub pending_approvals: Vec<ApprovalResponse>,
    pub artifacts: Vec<WorkbenchArtifactSummary>,
    pub file_changes: Vec<WorkbenchFileChangeSummary>,
    pub tasks: Vec<WorkbenchTaskSummary>,
    pub subagents: Vec<WorkbenchSubagentSummary>,
    pub memory_candidates: Vec<WorkbenchMemoryCandidateSummary>,
    pub available_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchEventsPage {
    pub after_seq: i64,
    pub last_seq: i64,
    pub has_more_before: bool,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchArtifactSummary {
    pub id: Uuid,
    pub run_id: Option<Uuid>,
    pub tool_call_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub project_id: Uuid,
    pub path: String,
    pub revision: i64,
    pub object_reference_id: Uuid,
    pub content_type: String,
    pub size_bytes: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchFileChangeSummary {
    pub event_id: String,
    pub seq: i64,
    pub project_id: Option<Uuid>,
    pub path: Option<String>,
    pub operation: Option<String>,
    pub revision: Option<i64>,
    pub reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchTaskSummary {
    pub task_id: String,
    pub title: String,
    pub status: String,
    pub summary: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchSubagentSummary {
    pub subagent_id: String,
    pub name: String,
    pub status: String,
    pub parent_tool_call_id: Option<String>,
    pub summary: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchMemoryCandidateSummary {
    pub id: Uuid,
    pub layer: String,
    pub content: String,
    pub confidence: f64,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchFileTreeResponse {
    pub project_id: Uuid,
    pub prefix: String,
    pub files: Vec<FileRevisionResponse>,
    pub entries: Vec<FileEntryResponse>,
}

#[derive(Debug, Serialize)]
pub struct PreviewDocument {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub content: Value,
    pub source: PreviewDocumentSource,
    pub actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewDocumentSource {
    pub project_id: Option<Uuid>,
    pub path: Option<String>,
    pub revision: Option<i64>,
    pub artifact_id: Option<Uuid>,
    pub object_reference_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchSearchResponse {
    pub items: Vec<WorkbenchSearchItem>,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchSearchItem {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub matched_text: Option<String>,
    pub target: WorkbenchSearchTarget,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkbenchSearchTarget {
    pub route: String,
    pub conversation_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub team_id: Option<Uuid>,
    pub artifact_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchBootstrapQuery {
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchWorkspaceDetailQuery {
    pub tenant_id: Option<Uuid>,
    pub conversation_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchConversationDetailQuery {
    pub tenant_id: Option<Uuid>,
    pub events_after_seq: Option<i64>,
    pub events_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchSearchQuery {
    pub tenant_id: Option<Uuid>,
    pub query: Option<String>,
    pub limit: Option<i64>,
    pub scope: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchFileTreeQuery {
    pub tenant_id: Option<Uuid>,
    pub project_id: Uuid,
    pub prefix: Option<String>,
    pub pattern: Option<String>,
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchFilePreviewQuery {
    pub tenant_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub path: Option<String>,
    pub revision: Option<i64>,
    pub artifact_id: Option<Uuid>,
    pub object_reference_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub offset_bytes: Option<i64>,
    pub limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchFileDiffQuery {
    pub tenant_id: Option<Uuid>,
    pub project_id: Uuid,
    pub path: String,
    pub from_revision: i64,
    pub to_revision: i64,
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct WorkbenchArtifactPreviewQuery {
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct EventStreamQuery {
    pub after_seq: Option<i64>,
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct IngestRunEventsResponse {
    pub events: Vec<StreamEventResponse>,
}

#[derive(Debug, Serialize)]
pub struct TenantResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ResourceResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct PolicyBindingResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub resource_type: String,
    pub resource_id: String,
    pub action: String,
    pub subject_type: String,
    pub subject_id: String,
    pub effect: String,
    pub risk_level: String,
    pub obligations: Value,
    pub policy_version: String,
    pub created_by_user_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
pub struct DeviceResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub device_name: String,
    pub platform: String,
    pub trust_level: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub ferriskey_subject: String,
    pub ferriskey_session_state: String,
    pub token_jti: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub token_exp: OffsetDateTime,
    pub roles_snapshot: Value,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub parent_id: Uuid,
    pub version_label: String,
    pub snapshot: Value,
    pub policy_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_secret_ref: Option<bool>,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ProjectMountResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub project_id: Uuid,
    pub virtual_path: String,
    pub backend_type: String,
    pub backend_ref: Option<String>,
    pub mount_config: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub name: String,
    pub remote_project_id: Option<Uuid>,
    pub default_agent_id: Option<Uuid>,
    pub default_agent_version_id: Option<Uuid>,
    pub default_model_profile_id: Option<Uuid>,
    pub tool_policy: Value,
    pub file_policy: Value,
    pub include_globs: Value,
    pub exclude_globs: Value,
    pub trust_state: String,
    pub metadata: Value,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct LocalMountResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub workspace_id: Uuid,
    pub display_name: String,
    pub virtual_path: String,
    pub capabilities: Value,
    pub include_globs: Value,
    pub exclude_globs: Value,
    pub trust_state: String,
    pub metadata: Value,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub title: String,
    pub status: String,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, Clone)]
pub struct RunResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub agent_version_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub status: String,
    pub trace_id: String,
    pub thread_id: Option<String>,
    pub policy_version: String,
    pub run_scope_snapshot: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub queued_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct ToolAuthorizeRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub trace_id: Option<String>,
    pub tool_id: Option<Uuid>,
    pub tool_name: String,
    pub resource: Option<ResourceRef>,
    pub args_hash: Option<String>,
    pub risk_level: Option<String>,
    pub input_summary: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ToolAuthorizeResponse {
    pub decision: AuthzDecision,
    pub tool_call_id: Uuid,
    pub approval_id: Option<Uuid>,
    pub interrupt_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ApprovalDecisionRequest {
    pub tenant_id: Uuid,
    pub decision: String,
    pub reason: Option<String>,
    pub payload: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ApprovalResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub tool_call_id: Option<Uuid>,
    pub status: String,
    pub approval_policy_id: Option<String>,
    pub request_payload: Value,
    pub decision_payload: Option<Value>,
    pub evidence_object_reference_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub decided_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct FileReadRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub path: String,
    pub revision: Option<i64>,
    pub version_id: Option<String>,
    pub run_id: Option<Uuid>,
    pub include_content: Option<bool>,
    pub allow_binary: Option<bool>,
    pub offset_bytes: Option<i64>,
    pub limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FileWriteRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub path: String,
    pub content_ref: Option<String>,
    pub inline_content: Option<String>,
    pub content_base64: Option<String>,
    pub content_type: Option<String>,
    pub expected_revision: i64,
    pub reason: String,
    pub run_id: Option<Uuid>,
    pub lock_token: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub args_hash: Option<String>,
    pub parent_tool_call_id: Option<String>,
    pub operation: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileEditRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub path: String,
    pub expected_revision: i64,
    pub find: String,
    pub replace: String,
    pub reason: String,
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct FileListQuery {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub run_id: Option<Uuid>,
    pub prefix: Option<String>,
    pub pattern: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileSearchRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub query: String,
    pub run_id: Option<Uuid>,
    pub prefix: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PublicFileReadQuery {
    pub tenant_id: Uuid,
    pub path: String,
    pub revision: Option<i64>,
    pub version_id: Option<String>,
    pub run_id: Option<Uuid>,
    pub include_content: Option<bool>,
    pub allow_binary: Option<bool>,
    pub offset_bytes: Option<i64>,
    pub limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PublicFileListQuery {
    pub tenant_id: Uuid,
    pub run_id: Option<Uuid>,
    pub prefix: Option<String>,
    pub pattern: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PublicFileSearchRequest {
    pub tenant_id: Uuid,
    pub query: String,
    pub run_id: Option<Uuid>,
    pub prefix: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PublicFileHistoryQuery {
    pub tenant_id: Uuid,
    pub path: String,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ProjectArtifactsQuery {
    pub tenant_id: Uuid,
    pub run_id: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ToolResultArtifactReadQuery {
    pub tenant_id: Uuid,
    pub object_reference_id: Uuid,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub offset_bytes: Option<i64>,
    pub limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ToolResultArtifactStreamQuery {
    pub tenant_id: Uuid,
    pub object_reference_id: Uuid,
    pub offset_bytes: Option<i64>,
    pub limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FileLockRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub path: String,
    pub run_id: Option<Uuid>,
    pub ttl_seconds: Option<i64>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FileUnlockRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub project_id: Uuid,
    pub path: String,
    pub run_id: Option<Uuid>,
    pub lock_token: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct FileLockResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub project_id: Uuid,
    pub path: String,
    pub path_hash: String,
    pub holder_run_id: Option<Uuid>,
    pub holder_user_id: Option<Uuid>,
    pub lock_token: String,
    pub reason: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, Clone)]
pub struct FileRevisionResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub project_id: Uuid,
    pub path: String,
    pub revision: i64,
    pub etag: String,
    pub content_hash: String,
    pub object_key: String,
    pub object_reference_id: Option<Uuid>,
    pub bucket: Option<String>,
    pub version_id: Option<String>,
    pub inline_content: Option<String>,
    pub content_base64: Option<String>,
    pub content_offset_bytes: Option<i64>,
    pub content_limit_bytes: Option<i64>,
    pub content_truncated: Option<bool>,
    pub size_bytes: i64,
    pub content_type: String,
    pub is_binary: bool,
    pub is_large: bool,
    pub reason: String,
    pub run_id: Option<Uuid>,
    pub metadata: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct FileListResponse {
    pub files: Vec<FileRevisionResponse>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<FileEntryResponse>,
}

#[derive(Debug, Serialize)]
pub struct ToolResultArtifactReadResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub run_id: Option<Uuid>,
    pub tool_call_id: Option<Uuid>,
    pub view_kind: String,
    pub ref_kind: String,
    pub project_id: Uuid,
    pub path: String,
    pub revision: i64,
    pub file_revision_id: Uuid,
    pub object_reference_id: Uuid,
    pub content_hash: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub content: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct FileEntryResponse {
    pub path: String,
    pub entry_type: String,
    pub depth: i32,
    pub children_count: i32,
    pub latest_revision: Option<i64>,
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMemoryRequest {
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub layer: String,
    pub content: String,
    pub source_run_id: Option<Uuid>,
    pub confidence: Option<f64>,
    pub status: Option<String>,
    pub visibility: Option<String>,
    pub retention_policy: Option<String>,
    pub sensitivity: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryQuery {
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub layer: Option<String>,
    pub status: Option<String>,
    pub query: Option<String>,
    pub run_id: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MemorySearchRequest {
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub layer: Option<String>,
    pub status: Option<String>,
    pub query: Option<String>,
    pub run_id: Option<Uuid>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryRetrieveForRunRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub run_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub layer: Option<String>,
    pub query: String,
    pub limit: Option<i64>,
    pub min_score: Option<f64>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MemoryContextResponse {
    pub memory_id: Uuid,
    pub layer: String,
    pub content: String,
    pub score: Option<f64>,
    pub confidence: f64,
    pub visibility: String,
    pub sensitivity: String,
    pub source: String,
    pub untrusted: bool,
}

#[derive(Debug, Serialize)]
pub struct MemoryRetrieveForRunResponse {
    pub memories: Vec<MemoryContextResponse>,
    pub source: String,
    pub vector_attempted: bool,
    pub vector_error: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MemoryCandidateInput {
    pub content: Option<String>,
    pub text: Option<String>,
    pub layer: Option<String>,
    pub confidence: Option<f64>,
    pub visibility: Option<String>,
    pub sensitivity: Option<String>,
    pub retention_policy: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryCandidatesRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub run_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub source_event_id: Option<Uuid>,
    #[serde(default)]
    pub candidates: Vec<MemoryCandidateInput>,
}

#[derive(Debug, Serialize)]
pub struct MemoryCandidatesResponse {
    pub memories: Vec<MemoryItemResponse>,
}

#[derive(Debug, Deserialize)]
pub struct MemoryAccessLogRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub memory_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub struct MemoryBatchDecisionRequest {
    pub tenant_id: Uuid,
    pub decision: String,
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub memory_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct MemoryBatchDecisionResponse {
    pub decision: String,
    pub target_status: String,
    pub succeeded: usize,
    pub failed: usize,
    pub results: Vec<MemoryBatchDecisionResult>,
}

#[derive(Debug, Serialize)]
pub struct MemoryBatchDecisionResult {
    pub memory_id: Uuid,
    pub status: String,
    pub memory: Option<MemoryItemResponse>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MemoryItemResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub source_run_id: Option<Uuid>,
    pub layer: String,
    pub content: String,
    pub confidence: f64,
    pub status: String,
    pub visibility: String,
    pub sensitivity: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkflowDesignRequest {
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub design: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowDesignListQuery {
    pub tenant_id: Uuid,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowDesignDetailQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkflowDesignRequest {
    pub tenant_id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    pub design: Option<Value>,
    pub status: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct WorkflowDesignResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub owner_user_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub design: Value,
    pub status: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct PublishWorkflowVersionRequest {
    pub tenant_id: Uuid,
    pub version_label: String,
    pub compiled_plan: Value,
    pub policy_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowVersionListQuery {
    pub tenant_id: Uuid,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowVersionDetailQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkflowRunRequest {
    pub tenant_id: Uuid,
    pub workflow_version_id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub input: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRunListQuery {
    pub tenant_id: Uuid,
    pub workflow_version_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowRunDetailQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workflow_version_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub status: String,
    pub trace_id: String,
    pub input: Value,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkflowNodeRunResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub workflow_run_id: Uuid,
    pub node_key: String,
    pub agent_run_id: Option<Uuid>,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub backoff_sec: i32,
    pub timeout_sec: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub not_before: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub completed_at: Option<OffsetDateTime>,
    pub input: Value,
    pub output: Option<Value>,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunDependencyResponse {
    pub from_node_key: String,
    pub to_node_key: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRunDetailResponse {
    pub run: WorkflowRunResponse,
    pub version: Option<VersionResponse>,
    pub design: Option<WorkflowDesignResponse>,
    pub node_runs: Vec<WorkflowNodeRunResponse>,
    pub dependencies: Vec<WorkflowRunDependencyResponse>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowTickResponse {
    pub workflow_run: WorkflowRunResponse,
    pub dispatched_runs: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct LocalExecRequest {
    pub tenant_id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_device_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    pub local_mount_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub operation: Option<String>,
    pub virtual_path: Option<String>,
    pub content: Option<String>,
    pub query: Option<String>,
    pub expected_revision: Option<i64>,
    pub reason: Option<String>,
    pub command: Option<Value>,
    pub timeout_ms: Option<i32>,
    pub max_output_bytes: Option<i32>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub args_hash: Option<String>,
    pub parent_tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalExecResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub device_id: Option<Uuid>,
    pub local_mount_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub status: String,
    pub command: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub timeout_ms: i32,
    pub max_output_bytes: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct LocalExecNextQuery {
    pub tenant_id: Uuid,
    pub kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalExecWorkItemResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub device_id: Option<Uuid>,
    pub command: Value,
    pub timeout_ms: i32,
    pub max_output_bytes: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct LocalExecCompleteRequest {
    pub tenant_id: Uuid,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LocalExecEventsRequest {
    pub tenant_id: Uuid,
    pub events: Vec<RunEventInput>,
}

#[derive(Debug, Serialize)]
pub struct LocalExecStatusResponse {
    pub id: Uuid,
    pub status: String,
    pub run_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct LocalExecPermissionRequest {
    pub tenant_id: Uuid,
    pub permission_id: String,
    pub title: String,
    pub options: Value,
    pub tool_call: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct LocalExecPermissionResponse {
    pub approval_id: Uuid,
    pub status: String,
    pub selected_option_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpToolCallRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub mcp_server_id: Option<Uuid>,
    pub mcp_tool_id: Option<Uuid>,
    pub tool_name: String,
    pub arguments: Value,
}

#[derive(Debug, Deserialize)]
pub struct SqlToolExecuteRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub sql_tool_id: Option<Uuid>,
    pub query_hash: Option<String>,
    pub parameters: Value,
}

#[derive(Debug, Deserialize)]
pub struct ThirdPartyToolCallRequest {
    pub tenant_id: Uuid,
    pub actor: ActorRef,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub tool_id: Option<Uuid>,
    pub tool_version_id: Option<Uuid>,
    pub tool_name: Option<String>,
    pub arguments: Value,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn resource_response_serializes_timestamps_as_rfc3339_strings() {
        let response = ResourceResponse {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            name: "provider".to_string(),
            description: Some("openai-compatible".to_string()),
            status: "active".to_string(),
            metadata: json!({}),
            created_at: OffsetDateTime::from_unix_timestamp(0).unwrap(),
            updated_at: None,
        };

        let value = serde_json::to_value(response).unwrap();

        assert_eq!(
            value["created_at"],
            Value::String("1970-01-01T00:00:00Z".to_string())
        );
        assert_eq!(value["updated_at"], Value::Null);
    }

    #[test]
    fn file_write_request_preserves_non_uuid_file_tool_context() {
        let request: FileWriteRequest = serde_json::from_value(json!({
            "tenant_id": Uuid::nil(),
            "actor_user_id": Uuid::nil(),
            "actor_device_id": null,
            "actor_session_id": null,
            "project_id": Uuid::nil(),
            "path": "/workspace/a.txt",
            "inline_content": "hello",
            "expected_revision": 1,
            "reason": "agent generated file",
            "run_id": null,
            "tool_call_id": "call-write",
            "tool_name": "write_file",
            "args_hash": "args-sha",
            "parent_tool_call_id": "call-task",
            "operation": "write_file"
        }))
        .unwrap();

        assert_eq!(request.tool_call_id.as_deref(), Some("call-write"));
        assert_eq!(request.tool_name.as_deref(), Some("write_file"));
        assert_eq!(request.args_hash.as_deref(), Some("args-sha"));
        assert_eq!(request.parent_tool_call_id.as_deref(), Some("call-task"));
        assert_eq!(request.operation.as_deref(), Some("write_file"));
    }

    #[test]
    fn local_exec_request_preserves_file_tool_context_and_reason() {
        let request: LocalExecRequest = serde_json::from_value(json!({
            "tenant_id": Uuid::nil(),
            "actor_user_id": Uuid::nil(),
            "actor_device_id": Uuid::nil(),
            "actor_session_id": null,
            "device_id": Uuid::nil(),
            "local_mount_id": Uuid::nil(),
            "project_id": Uuid::nil(),
            "run_id": Uuid::nil(),
            "operation": "write_text",
            "virtual_path": "/local/main/a.txt",
            "content": "hello",
            "expected_revision": 2,
            "reason": "agent local edit",
            "tool_call_id": "call-local-write",
            "tool_name": "write_file",
            "args_hash": "local-args-sha"
        }))
        .unwrap();

        assert_eq!(request.reason.as_deref(), Some("agent local edit"));
        assert_eq!(request.tool_call_id.as_deref(), Some("call-local-write"));
        assert_eq!(request.tool_name.as_deref(), Some("write_file"));
        assert_eq!(request.args_hash.as_deref(), Some("local-args-sha"));
    }

    #[test]
    fn tool_authorize_request_accepts_trace_id() {
        let request: ToolAuthorizeRequest = serde_json::from_value(json!({
            "tenant_id": Uuid::nil(),
            "actor": {
                "user_id": Uuid::nil(),
                "roles": ["tenant_member"]
            },
            "conversation_id": null,
            "run_id": Uuid::nil(),
            "trace_id": "trace-1",
            "tool_name": "read_file"
        }))
        .unwrap();

        assert_eq!(request.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(request.tool_name, "read_file");
    }

    #[test]
    fn workbench_feature_flags_explicitly_hide_out_of_scope_capabilities() {
        let value = serde_json::to_value(WorkbenchFeatureFlags::biwork_enterprise_default())
            .expect("workbench feature flags should serialize");

        assert_eq!(value["runtime"]["deepagents"], true);
        assert_eq!(value["runtime"]["biwork_cli"], false);
        assert_eq!(value["runtime"]["remote_agent_direct"], false);
        assert_eq!(
            value["desktop"]["gateway_required_for_local_capabilities"],
            true
        );
        assert_eq!(value["desktop"]["local_remote_control"], false);
        assert_eq!(value["desktop"]["cdp_remote_control"], false);
        assert_eq!(value["enterprise"]["conversations"], true);
        assert_eq!(value["enterprise"]["cron"], true);
        assert_eq!(value["teams_enabled"], true);
    }
}
