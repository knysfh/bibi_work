use serde::{Deserialize, Serialize};
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
pub struct MeTenantResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub membership_role: String,
    pub metadata: Value,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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
    pub mcp_tools: Vec<CapabilityResourceResponse>,
}

#[derive(Debug, Deserialize)]
pub struct BindAgentVersionRequest {
    pub tenant_id: Uuid,
    pub skill_version_ids: Option<Vec<Uuid>>,
    pub tool_version_ids: Option<Vec<Uuid>>,
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
    pub command: Option<Value>,
    pub timeout_ms: Option<i32>,
    pub max_output_bytes: Option<i32>,
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
}
