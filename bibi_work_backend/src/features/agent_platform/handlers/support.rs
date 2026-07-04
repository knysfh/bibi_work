use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::NewAuditLog, ferriskey_oidc::PlatformRequestContext, models::*, workflow_mapping,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

pub(super) const LOCAL_POLICY_VERSION: &str = "local-policy-v1";
pub(super) const LOCAL_RISK_POLICY_VERSION: &str = "local-risk-v1";

#[derive(Debug, Deserialize)]
pub struct TenantListQuery {
    pub tenant_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

pub(super) async fn require_tenant_action(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: Uuid,
    action: &str,
    resource_type: &str,
) -> Result<AuthzDecision, AppError> {
    require_ferriskey_allow(
        state,
        ctx,
        tenant_id,
        action,
        resource_type,
        format!("{resource_type}:new"),
        None,
    )
    .await
}

pub(super) fn normalize_request_actor(
    request: &mut AuthzCheckRequest,
    ctx: &PlatformRequestContext,
) {
    request.actor.device_id = Some(ctx.device_id);
    request.actor.session_id = Some(ctx.session_id);
    request.actor.roles = ctx.roles.clone();
}

pub(super) async fn require_ferriskey_allow(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: Uuid,
    action: &str,
    resource_type: &str,
    resource_id: String,
    context: Option<AuthzContext>,
) -> Result<AuthzDecision, AppError> {
    require_ferriskey_allow_for_actor(
        state,
        tenant_id,
        ActorRef {
            user_id: ctx.platform_user_id,
            device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            roles: ctx.roles.clone(),
        },
        action,
        resource_type,
        resource_id,
        context,
    )
    .await
}

pub(super) async fn require_ferriskey_allow_for_actor(
    state: &AppState,
    tenant_id: Uuid,
    actor: ActorRef,
    action: &str,
    resource_type: &str,
    resource_id: String,
    context: Option<AuthzContext>,
) -> Result<AuthzDecision, AppError> {
    let request = AuthzCheckRequest {
        tenant_id,
        actor,
        action: action.to_string(),
        resource: ResourceRef {
            resource_type: resource_type.to_string(),
            id: resource_id,
            path: None,
        },
        context,
    };
    let decision = state.authz_service.check(&request).await;
    write_authz_audit(&state.connect_pool, &request, &decision).await?;

    if decision.is_allow() {
        Ok(decision)
    } else {
        Err(AppError::PermissionDenied(format!(
            "authz decision={} resource={}:{} reason={}",
            decision.decision,
            request.resource.resource_type,
            request.resource.id,
            decision.reason_code.as_deref().unwrap_or("unspecified")
        )))
    }
}

pub(super) async fn ensure_tenant_member(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM user_tenant_memberships
            WHERE tenant_id = $1 AND user_id = $2
        ) AS exists
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .try_get("exists")?;

    if exists {
        Ok(())
    } else {
        Err(AppError::PermissionDenied(
            "user is not a member of the tenant".to_string(),
        ))
    }
}

pub(super) async fn write_authz_audit(
    pool: &PgPool,
    request: &AuthzCheckRequest,
    decision: &AuthzDecision,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    write_authz_audit_tx(&mut tx, request, decision, None, None).await?;
    tx.commit().await.map_err(|_| AppError::DatabaseTransaction)
}

pub(super) async fn write_authz_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    request: &AuthzCheckRequest,
    decision: &AuthzDecision,
    tool_call_id: Option<Uuid>,
    approval_id: Option<Uuid>,
) -> Result<(), AppError> {
    let context = request.context.clone().unwrap_or_default();
    let context_json = serde_json::to_value(&context)
        .map_err(|_| AppError::InvalidInput("failed to encode authz context".to_string()))?;
    let obligations_json = serde_json::to_value(&decision.obligations)
        .map_err(|_| AppError::InvalidInput("failed to encode authz obligations".to_string()))?;

    sqlx::query(
        r#"
        INSERT INTO authz_decisions (
            tenant_id, actor_user_id, actor_device_id, session_id,
            resource_type, resource_id, action, decision, policy_version, reason_code,
            obligations, context
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(request.tenant_id)
    .bind(request.actor.user_id)
    .bind(request.actor.device_id)
    .bind(request.actor.session_id)
    .bind(&request.resource.resource_type)
    .bind(&request.resource.id)
    .bind(&request.action)
    .bind(&decision.decision)
    .bind(&decision.policy_version)
    .bind(&decision.reason_code)
    .bind(obligations_json)
    .bind(context_json)
    .execute(&mut **tx)
    .await?;

    crate::features::agent_platform::audit::insert_audit_log_tx(
        tx,
        NewAuditLog {
            tenant_id: request.tenant_id,
            actor_user_id: Some(request.actor.user_id),
            actor_device_id: request.actor.device_id,
            session_id: request.actor.session_id,
            resource_type: &request.resource.resource_type,
            resource_id: &request.resource.id,
            action: &request.action,
            decision: &decision.decision,
            policy_version: &decision.policy_version,
            reason_code: decision.reason_code.as_deref(),
            run_id: context.run_id,
            conversation_id: context.conversation_id,
            workflow_run_id: context.workflow_run_id,
            tool_call_id,
            approval_id,
            args_hash: context.args_hash.as_deref(),
            input_summary: None,
            output_summary: None,
            risk_level: context.risk_level.as_deref(),
            ip: context.source_ip.as_deref(),
            user_agent: context.user_agent.as_deref(),
            trace_id: None,
        },
    )
    .await?;
    Ok(())
}

pub(super) async fn find_run_by_idempotency(
    pool: &PgPool,
    tenant_id: Uuid,
    idempotency_key: &str,
) -> Result<Option<RunResponse>, AppError> {
    let maybe_row = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
               project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
               queued_at, updated_at
        FROM runs
        WHERE tenant_id = $1 AND idempotency_key = $2
        "#,
    )
    .bind(tenant_id)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await?;

    maybe_row.map(run_from_row).transpose()
}

pub(super) async fn load_run(pool: &PgPool, run_id: Uuid) -> Result<RunResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
               project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
               queued_at, updated_at
        FROM runs
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("run not found".to_string()))?;

    run_from_row(row)
}

pub(super) async fn load_device(
    pool: &PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
) -> Result<DeviceResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, device_name, platform, trust_level,
               last_seen_at, revoked_at, created_at, updated_at
        FROM devices
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(device_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("device not found".to_string()))?;

    device_from_row(row)
}

pub(super) async fn load_session(
    pool: &PgPool,
    tenant_id: Uuid,
    session_id: Uuid,
) -> Result<SessionResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, device_id, ferriskey_subject,
               ferriskey_session_state, token_jti, token_exp, roles_snapshot,
               last_seen_at, source_ip, user_agent, revoked_at, created_at, updated_at
        FROM platform_sessions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(session_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("session not found".to_string()))?;

    session_from_row(row)
}

pub(super) async fn update_workflow_node_status_from_run_event(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Option<Uuid>,
    event_type: &str,
    event_payload: &Value,
) -> Result<Option<Uuid>, AppError> {
    let Some(run_id) = run_id else {
        return Ok(None);
    };

    let Some(status) = (match event_type {
        "run.started" => Some("running"),
        "approval.requested" | "interrupt.requested" => Some("waiting_approval"),
        "run.completed" => Some("completed"),
        "run.failed" => Some("failed"),
        "run.cancelled" => Some("cancelled"),
        _ => None,
    }) else {
        return Ok(None);
    };

    let output_payload = if matches!(status, "completed" | "failed" | "cancelled") {
        mapped_workflow_node_output(tx, run_id, event_payload).await?
    } else {
        event_payload.clone()
    };

    let row = sqlx::query(
        r#"
        UPDATE workflow_node_runs
        SET status = CASE
                WHEN $1 = 'failed' AND attempts < max_attempts THEN 'pending'
                ELSE $1
            END,
            agent_run_id = CASE
                WHEN $1 = 'failed' AND attempts < max_attempts THEN NULL
                ELSE agent_run_id
            END,
            not_before = CASE
                WHEN $1 = 'failed' AND attempts < max_attempts THEN CURRENT_TIMESTAMP + (backoff_sec * INTERVAL '1 second')
                ELSE not_before
            END,
            started_at = CASE
                WHEN $1 = 'running' THEN COALESCE(started_at, CURRENT_TIMESTAMP)
                ELSE started_at
            END,
            completed_at = CASE
                WHEN $1 IN ('completed', 'failed', 'cancelled')
                 AND NOT ($1 = 'failed' AND attempts < max_attempts) THEN CURRENT_TIMESTAMP
                ELSE completed_at
            END,
            output = CASE
                WHEN $1 IN ('completed', 'failed', 'cancelled')
                 AND NOT ($1 = 'failed' AND attempts < max_attempts) THEN $3
                ELSE output
            END,
            last_error = CASE
                WHEN $1 = 'failed' THEN COALESCE($4, 'run failed')
                WHEN $1 = 'running' THEN NULL
                ELSE last_error
            END,
            updated_at = CURRENT_TIMESTAMP
        WHERE agent_run_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled', 'blocked', 'skipped')
        RETURNING workflow_run_id
        "#,
    )
    .bind(status)
    .bind(run_id)
    .bind(output_payload)
    .bind(run_event_error_summary(event_payload))
    .fetch_optional(&mut **tx)
    .await?;

    if matches!(status, "completed" | "failed" | "cancelled") {
        Ok(row.map(|row| row.try_get("workflow_run_id")).transpose()?)
    } else {
        Ok(None)
    }
}

async fn mapped_workflow_node_output(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
    event_payload: &Value,
) -> Result<Value, AppError> {
    let node_run_input: Option<Value> = sqlx::query_scalar(
        r#"
        SELECT input
        FROM workflow_node_runs
        WHERE agent_run_id = $1
          AND status NOT IN ('completed', 'failed', 'cancelled', 'blocked', 'skipped')
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(node_run_input) = node_run_input else {
        return Ok(event_payload.clone());
    };

    workflow_mapping::map_terminal_output(&node_run_input, event_payload)
}

pub(super) fn run_event_error_summary(event_payload: &Value) -> Option<String> {
    event_payload
        .get("error")
        .or_else(|| event_payload.get("message"))
        .and_then(Value::as_str)
        .map(|message| message.chars().take(512).collect())
}

pub(super) async fn validate_agent_version_model_profile(
    pool: &PgPool,
    tenant_id: Uuid,
    snapshot: &Value,
) -> Result<(), AppError> {
    let model_profile_id = snapshot
        .get("model_profile_id")
        .or_else(|| {
            snapshot
                .get("agent")
                .and_then(|agent| agent.get("model_profile_id"))
        })
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::InvalidInput(
                "agent version snapshot must include model_profile_id".to_string(),
            )
        })?;

    let model_profile_id = Uuid::parse_str(model_profile_id)
        .map_err(|_| AppError::InvalidInput("model_profile_id must be a uuid".to_string()))?;

    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM llm_model_profiles
            WHERE id = $1
              AND tenant_id = $2
              AND status = 'active'
        ) AS exists
        "#,
    )
    .bind(model_profile_id)
    .bind(tenant_id)
    .fetch_one(pool)
    .await?
    .try_get("exists")?;

    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "model_profile_id does not reference an active LLM model profile".to_string(),
        ))
    }
}

pub(super) fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

pub(super) fn resource_from_row(row: PgRow) -> Result<ResourceResponse, AppError> {
    Ok(ResourceResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at").ok(),
    })
}

pub(super) fn policy_binding_from_row(row: PgRow) -> Result<PolicyBindingResponse, AppError> {
    Ok(PolicyBindingResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        action: row.try_get("action")?,
        subject_type: row.try_get("subject_type")?,
        subject_id: row.try_get("subject_id")?,
        effect: row.try_get("effect")?,
        risk_level: row.try_get("risk_level")?,
        obligations: row.try_get("obligations")?,
        policy_version: row.try_get("policy_version")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        created_at: row.try_get("created_at")?,
        disabled_at: row.try_get("disabled_at")?,
    })
}

pub(super) fn device_from_row(row: PgRow) -> Result<DeviceResponse, AppError> {
    Ok(DeviceResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        device_name: row.try_get("device_name")?,
        platform: row.try_get("platform")?,
        trust_level: row.try_get("trust_level")?,
        last_seen_at: row.try_get("last_seen_at")?,
        revoked_at: row.try_get("revoked_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn session_from_row(row: PgRow) -> Result<SessionResponse, AppError> {
    Ok(SessionResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        device_id: row.try_get("device_id")?,
        ferriskey_subject: row.try_get("ferriskey_subject")?,
        ferriskey_session_state: row.try_get("ferriskey_session_state")?,
        token_jti: row.try_get("token_jti")?,
        token_exp: row.try_get("token_exp")?,
        roles_snapshot: row.try_get("roles_snapshot")?,
        last_seen_at: row.try_get("last_seen_at")?,
        source_ip: row.try_get("source_ip")?,
        user_agent: row.try_get("user_agent")?,
        revoked_at: row.try_get("revoked_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn version_from_row(row: PgRow) -> Result<VersionResponse, AppError> {
    Ok(VersionResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        parent_id: row.try_get("parent_id")?,
        version_label: row.try_get("version_label")?,
        snapshot: row.try_get("snapshot")?,
        policy_version: row.try_get("policy_version")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
    })
}

pub(super) fn workspace_from_row(row: PgRow) -> Result<WorkspaceResponse, AppError> {
    Ok(WorkspaceResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        name: row.try_get("name")?,
        remote_project_id: row.try_get("remote_project_id")?,
        default_agent_id: row.try_get("default_agent_id")?,
        default_agent_version_id: row.try_get("default_agent_version_id")?,
        default_model_profile_id: row.try_get("default_model_profile_id")?,
        tool_policy: row.try_get("tool_policy")?,
        file_policy: row.try_get("file_policy")?,
        include_globs: row.try_get("include_globs")?,
        exclude_globs: row.try_get("exclude_globs")?,
        trust_state: row.try_get("trust_state")?,
        metadata: row.try_get("metadata")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn local_mount_from_row(row: PgRow) -> Result<LocalMountResponse, AppError> {
    Ok(LocalMountResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        device_id: row.try_get("device_id")?,
        workspace_id: row.try_get("workspace_id")?,
        display_name: row.try_get("display_name")?,
        virtual_path: row.try_get("virtual_path")?,
        capabilities: row.try_get("capabilities")?,
        include_globs: row.try_get("include_globs")?,
        exclude_globs: row.try_get("exclude_globs")?,
        trust_state: row.try_get("trust_state")?,
        metadata: row.try_get("metadata")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn workflow_run_from_row(row: PgRow) -> Result<WorkflowRunResponse, AppError> {
    Ok(WorkflowRunResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        workflow_version_id: row.try_get("workflow_version_id")?,
        conversation_id: row.try_get("conversation_id")?,
        project_id: row.try_get("project_id")?,
        status: row.try_get("status")?,
        trace_id: row.try_get("trace_id")?,
        input: row.try_get("input")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn conversation_from_row(row: PgRow) -> Result<ConversationResponse, AppError> {
    Ok(ConversationResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        workspace_id: row.try_get("workspace_id")?,
        project_id: row.try_get("project_id")?,
        agent_id: row.try_get("agent_id")?,
        title: row.try_get("title")?,
        status: row.try_get("status")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn run_from_row(row: PgRow) -> Result<RunResponse, AppError> {
    Ok(RunResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        conversation_id: row.try_get("conversation_id")?,
        workspace_id: row.try_get("workspace_id")?,
        agent_id: row.try_get("agent_id")?,
        agent_version_id: row.try_get("agent_version_id")?,
        project_id: row.try_get("project_id")?,
        status: row.try_get("status")?,
        trace_id: row.try_get("trace_id")?,
        thread_id: row.try_get("thread_id")?,
        policy_version: row.try_get("policy_version")?,
        run_scope_snapshot: row.try_get("run_scope_snapshot")?,
        queued_at: row.try_get("queued_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) fn approval_from_row(row: PgRow) -> Result<ApprovalResponse, AppError> {
    Ok(ApprovalResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        conversation_id: row.try_get("conversation_id")?,
        run_id: row.try_get("run_id")?,
        tool_call_id: row.try_get("tool_call_id")?,
        status: row.try_get("status")?,
        approval_policy_id: row.try_get("approval_policy_id")?,
        request_payload: row.try_get("request_payload")?,
        decision_payload: row.try_get("decision_payload")?,
        evidence_object_reference_id: row.try_get("evidence_object_reference_id")?,
        created_at: row.try_get("created_at")?,
        decided_at: row.try_get("decided_at")?,
    })
}

pub(super) fn memory_from_row(row: PgRow) -> Result<MemoryItemResponse, AppError> {
    Ok(MemoryItemResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        agent_id: row.try_get("agent_id")?,
        project_id: row.try_get("project_id")?,
        source_run_id: row.try_get("source_run_id")?,
        layer: row.try_get("layer")?,
        content: row.try_get("content")?,
        confidence: row.try_get("confidence")?,
        status: row.try_get("status")?,
        visibility: row.try_get("visibility")?,
        sensitivity: row.try_get("sensitivity")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}
