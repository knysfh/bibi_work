use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::features::core::errors::AppError;

#[cfg(test)]
use super::biwork_agent_support::{
    biwork_agent_type, biwork_assistant_runtime_disabled_reason, normalize_biwork_agent_source,
};
#[cfg(test)]
use super::biwork_assistant_service::{
    biwork_agent_run_authz_request, biwork_assistant_documents, biwork_assistant_rule_content,
    normalize_biwork_assistant_source, set_biwork_assistant_rule_content,
};
#[cfg(test)]
use super::biwork_channel_service::{
    biwork_approve_channel_pairing, biwork_list_channel_sessions, biwork_revoke_channel_user,
};
#[cfg(test)]
use super::biwork_conversation_lifecycle_service::*;
#[cfg(test)]
use super::biwork_conversation_service::*;
#[cfg(test)]
use super::biwork_cron_service::biwork_run_cron_job;
#[cfg(test)]
use super::biwork_extension_service::extension_contribution_rows;
#[cfg(test)]
use super::biwork_skill_service::{
    biwork_builtin_skill_ref_candidates, biwork_external_skill_source_response,
    biwork_skill_detect_path_entries, build_biwork_skill_candidate, discover_biwork_skill_sources,
    normalize_biwork_skill_external_paths, parse_biwork_skill_markdown, path_to_string,
    scan_biwork_external_skill_source,
};
#[cfg(test)]
use super::biwork_team_service::*;

pub(super) fn ok(data: Value) -> Json<Value> {
    Json(json!({
        "success": true,
        "trace_id": response_trace_id(),
        "data": data,
    }))
}

pub(super) fn response_trace_id() -> String {
    Uuid::new_v4().to_string()
}

pub(super) fn biwork_failure(code: &str, message: impl Into<String>, details: Value) -> Value {
    let message = message.into();
    json!({
        "success": false,
        "trace_id": response_trace_id(),
        "code": code,
        "error": message,
        "message": message,
        "details": details,
    })
}

pub(super) fn epoch_ms(value: OffsetDateTime) -> i64 {
    value.unix_timestamp().saturating_mul(1_000) + i64::from(value.millisecond())
}

pub(super) fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn active_lease_payload(
    user_id: Uuid,
    session_id: Uuid,
    device_id: Uuid,
    leased_until: OffsetDateTime,
) -> Value {
    json!({
        "holder_user_id": user_id,
        "session_id": session_id,
        "device_id": device_id,
        "leased_until_ms": epoch_ms(leased_until),
    })
}

pub async fn biwork_local_runtime_required() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(biwork_failure(
            "FEATURE_NOT_AVAILABLE",
            "desktop local runtime is not attached to the Rust backend",
            json!({ "reason": "LOCAL_RUNTIME_UNAVAILABLE" }),
        )),
    )
}

pub async fn biwork_system_info() -> Result<Json<Value>, AppError> {
    let work_dir = std::env::current_dir()
        .map_err(|_| AppError::InvalidInput("failed to resolve working directory".to_string()))?;
    let cache_dir = std::env::temp_dir().join("bibi-work-cache");
    let log_dir = work_dir
        .parent()
        .map(|path| path.join("logs"))
        .unwrap_or_else(|| work_dir.join("logs"));
    Ok(ok(json!({
        "cache_dir": cache_dir.to_string_lossy(),
        "work_dir": work_dir.to_string_lossy(),
        "log_dir": log_dir.to_string_lossy(),
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })))
}

pub async fn biwork_ensure_node_runtime(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Ok(ok(json!({
        "ready": false,
        "code": "NODE_RUNTIME_LOCAL_REQUIRED",
        "message": "Node runtime preparation is owned by the desktop local runtime",
        "scope": payload.get("scope").cloned().unwrap_or(Value::Null),
    })))
}

pub async fn biwork_ensure_managed_acp_tool(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Ok(ok(json!({
        "ready": false,
        "code": "MANAGED_ACP_TOOL_LOCAL_REQUIRED",
        "message": "Managed ACP tool installation is owned by the desktop local runtime",
        "tool_id": payload.get("tool_id").cloned().unwrap_or(Value::Null),
        "scope": payload.get("scope").cloned().unwrap_or(Value::Null),
    })))
}

pub async fn biwork_google_subscription_status() -> Result<Json<Value>, AppError> {
    Ok(ok(json!({
        "isSubscriber": false,
        "tier": "enterprise",
        "lastChecked": epoch_ms(OffsetDateTime::now_utc()),
        "message": "Google subscription checks are disabled in the enterprise OIDC backend",
    })))
}

pub async fn biwork_test_bedrock_connection(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let region = payload
        .get("bedrock_config")
        .and_then(|config| value_string(config, "region"));
    Ok(ok(json!({
        "msg": "AWS Bedrock live connection testing is not executed by this compat endpoint; save the provider and use enterprise model profile tests for real runtime validation.",
        "code": "BEDROCK_TEST_NOT_EXECUTED",
        "region": region,
    })))
}

pub async fn biwork_empty_array() -> Result<Json<Value>, AppError> {
    Ok(ok(json!([])))
}

pub async fn biwork_empty_object() -> Result<Json<Value>, AppError> {
    Ok(ok(json!({})))
}

pub(super) fn trimmed_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn required_string(value: &Value, key: &str) -> Result<String, AppError> {
    trimmed_string(value, key).ok_or_else(|| AppError::InvalidInput(format!("{key} is required")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Extension, body::to_bytes, extract::Path, extract::State, response::IntoResponse};
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};
    use std::fs;
    use time::Duration;

    use crate::{
        configuration::{AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings},
        features::agent_platform::{
            authz::ResourceAuthzService,
            ferriskey_oidc::{FerrisKeyOidcVerifier, PlatformRequestContext},
            memory_vector::MemoryVectorClient,
            runtime::AgentRuntimeClient,
            rustfs::RustFsClient,
        },
        startup::AppState,
    };

    #[test]
    fn assistant_visibility_uses_the_authenticated_agent_run_scope() {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let ctx = test_platform_context(tenant_id, user_id);

        let request = biwork_agent_run_authz_request(&ctx, agent_id);

        assert_eq!(request.tenant_id, tenant_id);
        assert_eq!(request.actor.user_id, user_id);
        assert_eq!(request.actor.device_id, Some(ctx.device_id));
        assert_eq!(request.actor.session_id, Some(ctx.session_id));
        assert_eq!(request.action, "run");
        assert_eq!(request.resource.resource_type, "agent");
        assert_eq!(request.resource.id, agent_id.to_string());
        assert_eq!(request.context.unwrap().agent_id, Some(agent_id));
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn deleting_a_conversation_cancels_active_runs_before_archiving()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let tool_call_id = Uuid::new_v4();
        let approval_id = Uuid::new_v4();
        let interrupt_id = Uuid::new_v4();
        let completed_run_id = Uuid::new_v4();
        let stale_tool_call_id = Uuid::new_v4();
        let stale_approval_id = Uuid::new_v4();
        let stale_interrupt_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("BiWork delete cancellation test")
            .bind(format!("biwork-delete-cancel-{tenant_id}"))
            .execute(&state.connect_pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, $3, $4, 'active')
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("biwork-delete-cancel-subject-{user_id}"))
        .bind(format!("biwork-delete-cancel-user-{user_id}"))
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            "INSERT INTO user_tenant_memberships (tenant_id, user_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO devices (
                id, tenant_id, user_id, device_fingerprint, device_name, platform, trust_level
            )
            VALUES ($1, $2, $3, $4, 'Delete cancellation test', 'oidc', 'standard')
            "#,
        )
        .bind(device_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("biwork-delete-cancel-device-{device_id}"))
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_sessions (
                id, tenant_id, user_id, device_id, ferriskey_subject,
                ferriskey_session_state, token_exp, roles_snapshot, token_hash
            )
            VALUES ($1, $2, $3, $4, $5, $6, CURRENT_TIMESTAMP + INTERVAL '1 hour', $7, $8)
            "#,
        )
        .bind(session_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(device_id)
        .bind(format!("biwork-delete-cancel-subject-{user_id}"))
        .bind(format!("biwork-delete-cancel-session-{session_id}"))
        .bind(json!(["tenant_member"]))
        .bind(format!("biwork-delete-cancel-token-{session_id}"))
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversations (id, tenant_id, created_by_user_id, title)
            VALUES ($1, $2, $3, 'Delete active conversation')
            "#,
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .bind(user_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO runs (
                id, tenant_id, conversation_id, created_by_user_id, status,
                input, run_config_snapshot, trace_id
            )
            VALUES ($1, $2, $3, $4, 'running', '{}'::jsonb, '{}'::jsonb, $5)
            "#,
        )
        .bind(run_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(user_id)
        .bind(format!("trace-{run_id}"))
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO tool_calls (
                id, tenant_id, conversation_id, run_id, tool_name,
                resource_type, resource_id, risk_level, status, decision, policy_version
            )
            VALUES (
                $1, $2, $3, $4, 'dangerous_tool',
                'tool', 'dangerous_tool', 'high', 'waiting_approval', 'review', 'test-policy'
            )
            "#,
        )
        .bind(tool_call_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(run_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO approvals (
                id, tenant_id, conversation_id, run_id, tool_call_id, status
            )
            VALUES ($1, $2, $3, $4, $5, 'pending')
            "#,
        )
        .bind(approval_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(run_id)
        .bind(tool_call_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO interrupts (
                id, tenant_id, conversation_id, run_id, approval_id, type, status
            )
            VALUES ($1, $2, $3, $4, $5, 'approval', 'open')
            "#,
        )
        .bind(interrupt_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(run_id)
        .bind(approval_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO runs (
                id, tenant_id, conversation_id, created_by_user_id, status,
                input, run_config_snapshot, trace_id, completed_at
            )
            VALUES ($1, $2, $3, $4, 'completed', '{}'::jsonb, '{}'::jsonb, $5, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(completed_run_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(user_id)
        .bind(format!("trace-{completed_run_id}"))
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO tool_calls (
                id, tenant_id, conversation_id, run_id, tool_name,
                resource_type, resource_id, risk_level, status, decision, policy_version
            )
            VALUES (
                $1, $2, $3, $4, 'stale_dangerous_tool',
                'tool', 'stale_dangerous_tool', 'high', 'waiting_approval', 'review', 'test-policy'
            )
            "#,
        )
        .bind(stale_tool_call_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(completed_run_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO approvals (
                id, tenant_id, conversation_id, run_id, tool_call_id, status
            )
            VALUES ($1, $2, $3, $4, $5, 'pending')
            "#,
        )
        .bind(stale_approval_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(completed_run_id)
        .bind(stale_tool_call_id)
        .execute(&state.connect_pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO interrupts (
                id, tenant_id, conversation_id, run_id, approval_id, type, status
            )
            VALUES ($1, $2, $3, $4, $5, 'approval', 'open')
            "#,
        )
        .bind(stale_interrupt_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(completed_run_id)
        .bind(stale_approval_id)
        .execute(&state.connect_pool)
        .await?;

        let mut ctx = test_platform_context(tenant_id, user_id);
        ctx.device_id = device_id;
        ctx.session_id = session_id;
        let _ =
            biwork_delete_conversation(State(state.clone()), Extension(ctx), Path(conversation_id))
                .await?;

        let run_status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&state.connect_pool)
            .await?;
        let conversation_archived: bool = sqlx::query_scalar(
            "SELECT deleted_at IS NOT NULL AND status = 'archived' FROM conversations WHERE id = $1",
        )
        .bind(conversation_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let cancelled_event_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM run_events WHERE run_id = $1 AND type = 'run.cancelled')",
        )
        .bind(run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let approval_status: String =
            sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
                .bind(approval_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let interrupt_status: String =
            sqlx::query_scalar("SELECT status FROM interrupts WHERE id = $1")
                .bind(interrupt_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let tool_call_status: String =
            sqlx::query_scalar("SELECT status FROM tool_calls WHERE id = $1")
                .bind(tool_call_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let completed_run_status: String =
            sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
                .bind(completed_run_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let stale_approval_status: String =
            sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
                .bind(stale_approval_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let stale_interrupt_status: String =
            sqlx::query_scalar("SELECT status FROM interrupts WHERE id = $1")
                .bind(stale_interrupt_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let stale_tool_call_status: String =
            sqlx::query_scalar("SELECT status FROM tool_calls WHERE id = $1")
                .bind(stale_tool_call_id)
                .fetch_one(&state.connect_pool)
                .await?;
        let approval_remove_event_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM run_events WHERE run_id = $1 AND type = 'approval.decided')",
        )
        .bind(run_id)
        .fetch_one(&state.connect_pool)
        .await?;

        assert_eq!(run_status, "cancelled");
        assert!(conversation_archived);
        assert!(cancelled_event_exists);
        assert_eq!(approval_status, "cancelled");
        assert_eq!(interrupt_status, "resolved");
        assert_eq!(tool_call_status, "cancelled");
        assert_eq!(completed_run_status, "completed");
        assert_eq!(stale_approval_status, "cancelled");
        assert_eq!(stale_interrupt_status, "resolved");
        assert_eq!(stale_tool_call_status, "cancelled");
        assert!(approval_remove_event_exists);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn cron_run_denies_agent_version_capability_before_writing_run()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_cron_run_capability_context(&state.connect_pool).await?;

        let result = biwork_run_cron_job(
            State(state.clone()),
            Extension(test_platform_context(context.tenant_id, context.user_id)),
            Path(context.job_id),
        )
        .await;

        match result {
            Err(AppError::PermissionDenied(message)) => {
                assert!(message.contains("resource=skill:"));
                assert!(message.contains(&context.skill_id.to_string()));
                assert!(message.contains("policy_explicit_deny"));
            }
            other => panic!("expected skill capability denial, got {other:?}"),
        }

        let run_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM runs
            WHERE tenant_id = $1 AND conversation_id = $2
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.conversation_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let run_event_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM run_events
            WHERE tenant_id = $1
              AND conversation_id = $2
              AND run_id IS NOT NULL
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.conversation_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let scheduled_run_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM scheduled_job_runs
            WHERE tenant_id = $1
              AND scheduled_job_id = $2
              AND run_id IS NOT NULL
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.job_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let artifact_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM scheduled_job_artifacts
            WHERE tenant_id = $1 AND scheduled_job_id = $2
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.job_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let failed_attempt_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM scheduled_job_runs
            WHERE tenant_id = $1
              AND scheduled_job_id = $2
              AND status = 'failed'
              AND run_id IS NULL
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.job_id)
        .fetch_one(&state.connect_pool)
        .await?;

        assert_eq!(run_count, 0);
        assert_eq!(run_event_count, 0);
        assert_eq!(scheduled_run_count, 0);
        assert_eq!(artifact_count, 0);
        assert_eq!(failed_attempt_count, 1);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn channel_pairing_approve_denies_before_authorizing_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_channel_route_authz_context(&state.connect_pool).await?;

        let result = biwork_approve_channel_pairing(
            State(state.clone()),
            Extension(test_platform_context(context.tenant_id, context.user_id)),
            Json(json!({ "code": context.pairing_code.clone() })),
        )
        .await;

        match result {
            Err(AppError::PermissionDenied(message)) => {
                assert!(message.contains("resource=channel_pairing:PAIR-DENY"));
                assert!(message.contains("policy_explicit_deny"));
            }
            other => panic!("expected channel pairing authz denial, got {other:?}"),
        }

        let pairing_status: String = sqlx::query_scalar(
            r#"
            SELECT status
            FROM channel_pairing_requests
            WHERE tenant_id = $1 AND code = $2
            "#,
        )
        .bind(context.tenant_id)
        .bind(&context.pairing_code)
        .fetch_one(&state.connect_pool)
        .await?;
        let authorized_user_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM channel_authorized_users
            WHERE tenant_id = $1
              AND platform = 'telegram'
              AND platform_user_id = 'pairing-user'
            "#,
        )
        .bind(context.tenant_id)
        .fetch_one(&state.connect_pool)
        .await?;

        assert_eq!(pairing_status, "pending");
        assert_eq!(authorized_user_count, 0);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn channel_user_revoke_denies_before_revoking_user_or_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_channel_route_authz_context(&state.connect_pool).await?;

        let result = biwork_revoke_channel_user(
            State(state.clone()),
            Extension(test_platform_context(context.tenant_id, context.user_id)),
            Json(json!({ "user_id": context.channel_user_id })),
        )
        .await;

        match result {
            Err(AppError::PermissionDenied(message)) => {
                assert!(message.contains(&format!(
                    "resource=channel_user:{}",
                    context.channel_user_id
                )));
                assert!(message.contains("policy_explicit_deny"));
            }
            other => panic!("expected channel user revoke authz denial, got {other:?}"),
        }

        let user_status: String = sqlx::query_scalar(
            r#"
            SELECT status
            FROM channel_authorized_users
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(context.channel_user_id)
        .bind(context.tenant_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let active_session_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM channel_sessions
            WHERE id = $1 AND tenant_id = $2 AND ended_at IS NULL
            "#,
        )
        .bind(context.session_id)
        .bind(context.tenant_id)
        .fetch_one(&state.connect_pool)
        .await?;

        assert_eq!(user_status, "active");
        assert_eq!(active_session_count, 1);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn channel_session_list_requires_channel_read_authorization()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_channel_route_authz_context(&state.connect_pool).await?;

        let result = biwork_list_channel_sessions(
            State(state.clone()),
            Extension(test_platform_context(context.tenant_id, context.user_id)),
        )
        .await;

        match result {
            Err(AppError::PermissionDenied(message)) => {
                assert!(message.contains("resource=channel:sessions"));
                assert!(message.contains("relation_missing"));
            }
            other => panic!("expected channel sessions authz denial, got {other:?}"),
        }

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn extension_contribution_rows_apply_enterprise_governance_filters()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_extension_governance_context(&state.connect_pool).await?;

        let assistant_rows =
            extension_contribution_rows(&state, context.tenant_id, context.device_id, "assistant")
                .await?;
        let assistant_keys = assistant_rows
            .iter()
            .map(|row| row.try_get::<String, _>("contribution_key"))
            .collect::<Result<Vec<_>, _>>()?;

        let webui_rows =
            extension_contribution_rows(&state, context.tenant_id, context.device_id, "webui")
                .await?;
        let webui_keys = webui_rows
            .iter()
            .map(|row| row.try_get::<String, _>("contribution_key"))
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(assistant_keys, vec!["allowed-assistant".to_string()]);
        assert_eq!(webui_keys, vec!["approved-webui".to_string()]);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[test]
    fn biwork_success_and_failure_envelopes_include_trace_id() {
        let success = ok(json!({ "value": true })).0;
        assert_eq!(success["success"], true);
        assert!(Uuid::parse_str(success["trace_id"].as_str().unwrap()).is_ok());

        let failure = biwork_failure("TEST_FAILURE", "failed", json!({}));
        assert_eq!(failure["success"], false);
        assert!(Uuid::parse_str(failure["trace_id"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn skill_settings_routes_require_user_settings_authz_before_write() {
        let source = include_str!("biwork_skill_service.rs");
        let save_external_paths = source
            .find("async fn save_biwork_skill_external_paths")
            .expect("external paths save helper exists");
        let external_source = &source[save_external_paths..];
        let external_authz = external_source
            .find("require_biwork_user_settings_update")
            .expect("external paths save requires settings authz");
        let external_write = external_source
            .find("set_biwork_client_setting")
            .expect("external paths save writes settings");

        assert!(external_authz < external_write);

        let skills_market = source
            .find("async fn set_biwork_skills_market_enabled")
            .expect("skills market helper exists");
        let market_source = &source[skills_market..];
        let market_authz = market_source
            .find("require_biwork_user_settings_update")
            .expect("skills market toggle requires settings authz");
        let market_write = market_source
            .find("set_biwork_client_setting")
            .expect("skills market toggle writes settings");

        assert!(market_authz < market_write);
    }

    #[test]
    fn biwork_team_status_preserves_cancelling_contract() {
        assert_eq!(biwork_team_status("accepted"), "accepted");
        assert_eq!(biwork_team_status("queued"), "running");
        assert_eq!(biwork_team_status("waiting_approval"), "running");
        assert_eq!(biwork_team_status("cancelling"), "cancelling");
        assert_eq!(biwork_team_status("canceling"), "cancelling");
        assert_eq!(biwork_team_status("cancelled"), "cancelled");
    }

    #[test]
    fn biwork_team_run_state_status_derives_terminal_after_cancelling() {
        assert_eq!(
            biwork_team_run_state_status("cancelling", &["running".to_string()]),
            "cancelling"
        );
        assert_eq!(
            biwork_team_run_state_status("cancelling", &["cancelling".to_string()]),
            "cancelling"
        );
        assert_eq!(
            biwork_team_run_state_status(
                "cancelling",
                &["completed".to_string(), "cancelled".to_string()]
            ),
            "cancelled"
        );
        assert_eq!(
            biwork_team_run_state_status(
                "cancelling",
                &["cancelled".to_string(), "failed".to_string()]
            ),
            "failed"
        );
    }

    #[test]
    fn biwork_skill_markdown_parser_slugifies_frontmatter_name() {
        let markdown = r#"---
name: "Summarize Docs"
description: "Turns long docs into concise summaries"
---
# Ignored Heading

Body text.
"#;
        let (name, description) = parse_biwork_skill_markdown(markdown, "fallback").unwrap();

        assert_eq!(name, "summarize-docs");
        assert_eq!(description, "Turns long docs into concise summaries");
    }

    #[test]
    fn biwork_skill_discovery_scans_parent_directory_children() {
        let root = std::env::temp_dir().join(format!("biwork-skill-test-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("alpha")).unwrap();
        fs::create_dir_all(root.join("beta")).unwrap();
        fs::write(
            root.join("alpha").join("SKILL.md"),
            "# Alpha Skill\n\nAlpha description",
        )
        .unwrap();
        fs::write(
            root.join("beta").join("SKILL.md"),
            "# Beta Skill\n\nBeta description",
        )
        .unwrap();

        let sources = discover_biwork_skill_sources(&root).unwrap();
        let mut names = sources
            .iter()
            .map(build_biwork_skill_candidate)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|candidate| candidate.name)
            .collect::<Vec<_>>();
        names.sort();

        assert_eq!(names, vec!["alpha-skill", "beta-skill"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn biwork_builtin_skill_ref_candidates_normalize_markdown_paths() {
        assert_eq!(
            biwork_builtin_skill_ref_candidates("auto-inject/cron/SKILL.md"),
            vec!["auto-inject/cron", "cron", "auto-inject/cron/skill.md"]
        );
        assert_eq!(
            biwork_builtin_skill_ref_candidates("cron.md"),
            vec!["cron", "cron.md"]
        );
        assert_eq!(
            biwork_builtin_skill_ref_candidates(r"auto-inject\officecli\SKILL.md"),
            vec![
                "auto-inject/officecli",
                "officecli",
                "auto-inject/officecli/skill.md"
            ]
        );
    }

    #[test]
    fn biwork_assistant_documents_preserve_editor_contract_fields() {
        let agent_id = Uuid::new_v4();
        let payload = json!({
            "id": "writer-assistant",
            "name": "Writer",
            "description": "Writes drafts",
            "avatar": "pen",
            "agent_id": "managed-agent-1",
            "enabled_skills": ["summarize"],
            "custom_skill_names": ["draft"],
            "disabled_builtin_skills": ["web"],
            "recommended_prompts": ["Write a brief"],
            "recommended_prompts_i18n": {"en-US": ["Write a brief"]},
            "defaults": {
                "model": {"mode": "fixed", "value": "model-a"},
                "permission": {"mode": "auto"},
                "thought_level": {"mode": "fixed", "value": "high"},
                "skills": {"mode": "fixed", "value": ["summarize"]},
                "mcps": {"mode": "fixed", "value": ["mcp-a"]}
            }
        });

        let (config, metadata) =
            biwork_assistant_documents(agent_id, None, None, &payload, true).unwrap();

        assert_eq!(metadata["assistant_source"], "user");
        assert_eq!(metadata["biwork_id"], "writer-assistant");
        assert_eq!(metadata["avatar"], "pen");
        assert_eq!(config["engine_agent_id"], agent_id.to_string());
        assert_eq!(config["skills"], json!(["summarize"]));
        assert_eq!(config["custom_skill_names"], json!(["draft"]));
        assert_eq!(config["disabled_builtin_skills"], json!(["web"]));
        assert_eq!(config["prompts"], json!(["Write a brief"]));
        assert_eq!(config["prompts_i18n"]["en-US"], json!(["Write a brief"]));
        assert_eq!(config["defaults"]["mcps"]["value"], json!(["mcp-a"]));
    }

    #[test]
    fn conversation_associated_parses_cron_job_id_aliases() {
        let cron_job_id = Uuid::new_v4();

        assert_eq!(
            biwork_cron_job_id_from_conversation_metadata(&json!({
                "extra": {
                    "cron_job_id": cron_job_id.to_string(),
                }
            })),
            Some(cron_job_id)
        );
        assert_eq!(
            biwork_cron_job_id_from_conversation_metadata(&json!({
                "extra": {
                    "cronJobId": cron_job_id.to_string(),
                }
            })),
            Some(cron_job_id)
        );
        assert_eq!(
            biwork_cron_job_id_from_conversation_metadata(&json!({
                "extra": {
                    "cron_job_id": "not-a-uuid",
                }
            })),
            None
        );
    }

    #[test]
    fn biwork_assistant_source_matches_frontend_contract() {
        assert_eq!(normalize_biwork_assistant_source("builtin"), "builtin");
        assert_eq!(normalize_biwork_assistant_source("generated"), "generated");
        assert_eq!(normalize_biwork_assistant_source("cli"), "generated");
        assert_eq!(normalize_biwork_assistant_source("user"), "user");
        assert_eq!(normalize_biwork_assistant_source("custom"), "user");
        assert_eq!(normalize_biwork_assistant_source("remote"), "user");
        assert_eq!(normalize_biwork_assistant_source("extension"), "user");
    }

    #[test]
    fn biwork_agent_source_matches_frontend_contract() {
        assert_eq!(normalize_biwork_agent_source(None), "internal");
        assert_eq!(normalize_biwork_agent_source(Some("internal")), "internal");
        assert_eq!(normalize_biwork_agent_source(Some("builtin")), "builtin");
        assert_eq!(
            normalize_biwork_agent_source(Some("extension")),
            "extension"
        );
        assert_eq!(normalize_biwork_agent_source(Some("custom")), "custom");
        assert_eq!(normalize_biwork_agent_source(Some("remote")), "custom");
        assert_eq!(normalize_biwork_agent_source(Some("user")), "custom");
    }

    #[test]
    fn biwork_agent_type_preserves_non_acp_runtime_discriminants() {
        assert_eq!(biwork_agent_type("deepagents", &json!({})), "acp");
        assert_eq!(biwork_agent_type("remote", &json!({})), "remote");
        assert_eq!(
            biwork_agent_type("acp", &json!({ "source": "remote" })),
            "remote"
        );
    }

    #[test]
    fn biwork_assistant_runtime_availability_is_fail_closed() {
        assert!(biwork_assistant_runtime_disabled_reason("deepagents", "acp").is_none());
        assert!(biwork_assistant_runtime_disabled_reason("biwork_cli", "acp").is_none());
        assert!(
            biwork_assistant_runtime_disabled_reason("acp", "acp")
                .unwrap()
                .contains("obsolete")
        );
        assert!(
            biwork_assistant_runtime_disabled_reason("deepagents", "remote")
                .unwrap()
                .contains("remote agent runtime")
        );
        assert!(
            biwork_assistant_runtime_disabled_reason("disabled", "acp")
                .unwrap()
                .contains("catalog-visible")
        );
    }

    #[test]
    fn biwork_team_member_block_reason_matches_assistant_selectability() {
        assert!(
            biwork_team_member_block_reason(
                "active",
                &json!({ "runtime": { "kind": "deepagents" } }),
                &json!({})
            )
            .is_none()
        );

        assert_eq!(
            biwork_team_member_block_reason(
                "disabled",
                &json!({ "runtime": { "kind": "deepagents" } }),
                &json!({})
            ),
            Some("assistant is disabled".to_string())
        );

        assert!(
            biwork_team_member_block_reason(
                "active",
                &json!({ "runtime": { "kind": "biwork_cli" } }),
                &json!({})
            )
            .unwrap()
            .contains("team execution")
        );

        assert!(
            biwork_team_member_block_reason(
                "active",
                &json!({ "runtime": { "kind": "deepagents" } }),
                &json!({ "source": "remote" })
            )
            .unwrap()
            .contains("remote agent runtime")
        );
    }

    #[test]
    fn biwork_assistant_rule_content_prefers_locale_then_default() {
        let config = json!({
            "system_prompt": "Default rules",
            "context_i18n": {
                "zh-CN": "中文规则"
            }
        });

        assert_eq!(
            biwork_assistant_rule_content(&config, Some("zh-CN")),
            "中文规则"
        );
        assert_eq!(
            biwork_assistant_rule_content(&config, Some("en-US")),
            "Default rules"
        );
        assert_eq!(
            biwork_assistant_rule_content(&config, None),
            "Default rules"
        );
    }

    #[test]
    fn set_biwork_assistant_rule_content_preserves_non_default_locale_prompt() {
        let mut config = json!({
            "system_prompt": "English rules",
            "context_i18n": {
                "en-US": "English rules",
                "zh-CN": "旧中文规则"
            }
        });

        set_biwork_assistant_rule_content(&mut config, Some("zh-CN"), Some("新中文规则")).unwrap();

        assert_eq!(config["system_prompt"], "English rules");
        assert_eq!(config["context_i18n"]["zh-CN"], "新中文规则");
    }

    #[test]
    fn set_biwork_assistant_rule_content_updates_default_for_default_locale() {
        let mut config = json!({
            "system_prompt": "Old English rules",
            "context_i18n": {
                "en-US": "Old English rules"
            }
        });

        set_biwork_assistant_rule_content(&mut config, Some("en-US"), Some("New English rules"))
            .unwrap();

        assert_eq!(config["system_prompt"], "New English rules");
        assert_eq!(config["context_i18n"]["en-US"], "New English rules");
    }

    #[test]
    fn set_biwork_assistant_rule_content_delete_clears_all_rules() {
        let mut config = json!({
            "system_prompt": "Default rules",
            "context_i18n": {
                "zh-CN": "中文规则"
            }
        });

        set_biwork_assistant_rule_content(&mut config, None, None).unwrap();

        assert!(config.get("system_prompt").is_none());
        assert!(config.get("context_i18n").is_none());
    }

    #[test]
    fn normalize_biwork_skill_external_paths_filters_invalid_entries() {
        let paths = normalize_biwork_skill_external_paths(&json!([
            { "name": "Docs", "path": "/workspace/skills/docs" },
            { "name": "Duplicate", "path": "/workspace/skills/docs" },
            { "name": "", "path": "C:\\Users\\me\\skills\\office" },
            { "name": "Missing path" },
            { "name": "Blank path", "path": " " }
        ]));

        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0],
            json!({ "name": "Docs", "path": "/workspace/skills/docs" })
        );
        assert_eq!(
            paths[1],
            json!({ "name": "office", "path": "C:\\Users\\me\\skills\\office" })
        );
    }

    #[test]
    fn skill_detect_paths_include_enterprise_and_external_entries() {
        let paths = biwork_skill_detect_path_entries(&[json!({
            "name": "Docs",
            "path": "/workspace/skills/docs"
        })]);

        assert_eq!(
            paths,
            vec![
                json!({
                    "name": "Enterprise Custom Skills",
                    "path": "enterprise://skills/custom"
                }),
                json!({
                    "name": "Enterprise Builtin Skills",
                    "path": "enterprise://skills/builtin"
                }),
                json!({
                    "name": "Docs",
                    "path": "/workspace/skills/docs"
                })
            ]
        );
    }

    #[test]
    fn external_skill_source_response_matches_biwork_contract() {
        let source = biwork_external_skill_source_response(
            "Docs",
            "/workspace/skills/docs",
            vec![json!({
                "name": "summary",
                "description": "Summarize docs",
                "path": "/workspace/skills/docs/summary"
            })],
        );

        assert_eq!(source["name"], "Docs");
        assert_eq!(source["path"], "/workspace/skills/docs");
        assert_eq!(source["source"], "custom-/workspace/skills/docs");
        assert_eq!(source["skills"].as_array().unwrap().len(), 1);
        assert_eq!(source["skills"][0]["name"], "summary");
    }

    #[test]
    fn scan_biwork_external_skill_source_returns_importable_skills() {
        let root = std::env::temp_dir().join(format!(
            "biwork-external-skill-source-test-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("summary")).unwrap();
        fs::write(
            root.join("summary").join("SKILL.md"),
            "# Summary\n\nSummarize long documents",
        )
        .unwrap();

        let skills = scan_biwork_external_skill_source(&path_to_string(&root));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0]["name"], "summary");
        assert_eq!(skills[0]["description"], "Summarize long documents");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_biwork_external_skill_source_is_fail_soft() {
        let missing = std::env::temp_dir().join(format!(
            "biwork-missing-external-skill-source-{}",
            Uuid::new_v4()
        ));

        assert!(scan_biwork_external_skill_source(&path_to_string(&missing)).is_empty());
    }

    #[test]
    fn biwork_failure_contains_backend_http_error_fields() {
        let failure = biwork_failure(
            "FEATURE_NOT_AVAILABLE",
            "desktop local runtime is not attached",
            json!({ "reason": "LOCAL_RUNTIME_UNAVAILABLE", "scope": "fs" }),
        );

        assert_eq!(failure["success"], false);
        assert_eq!(failure["code"], "FEATURE_NOT_AVAILABLE");
        assert_eq!(failure["error"], "desktop local runtime is not attached");
        assert_eq!(failure["message"], failure["error"]);
        assert_eq!(failure["details"]["reason"], "LOCAL_RUNTIME_UNAVAILABLE");
        assert_eq!(failure["details"]["scope"], "fs");
    }

    #[tokio::test]
    async fn local_runtime_required_uses_webui_fallback_error_contract() {
        let response = biwork_local_runtime_required().await.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(payload["success"], false);
        assert_eq!(payload["code"], "FEATURE_NOT_AVAILABLE");
        assert_eq!(payload["details"]["reason"], "LOCAL_RUNTIME_UNAVAILABLE");
    }

    #[test]
    fn active_lease_payload_records_holder_and_deadline() {
        let user_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let leased_until = OffsetDateTime::UNIX_EPOCH + Duration::seconds(90);

        let payload = active_lease_payload(user_id, session_id, device_id, leased_until);

        assert_eq!(payload["holder_user_id"], user_id.to_string());
        assert_eq!(payload["session_id"], session_id.to_string());
        assert_eq!(payload["device_id"], device_id.to_string());
        assert_eq!(payload["leased_until_ms"], 90_000);
    }

    #[test]
    fn team_session_payload_records_holder_state_and_timestamp() {
        let user_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let timestamp = OffsetDateTime::UNIX_EPOCH + Duration::seconds(42);

        let active = team_session_payload("active", user_id, session_id, device_id, timestamp);
        assert_eq!(active["state"], "active");
        assert_eq!(active["holder_user_id"], user_id.to_string());
        assert_eq!(active["session_id"], session_id.to_string());
        assert_eq!(active["device_id"], device_id.to_string());
        assert_eq!(active["updated_at_ms"], 42_000);
        assert_eq!(active["ensured_at_ms"], 42_000);
        assert!(active.get("stopped_at_ms").is_none());

        let stopped = team_session_payload("stopped", user_id, session_id, device_id, timestamp);
        assert_eq!(stopped["state"], "stopped");
        assert_eq!(stopped["updated_at_ms"], 42_000);
        assert_eq!(stopped["stopped_at_ms"], 42_000);
        assert!(stopped.get("ensured_at_ms").is_none());
    }

    #[test]
    fn team_session_response_exposes_top_level_state() {
        let session = json!({
            "state": "active",
            "holder_user_id": Uuid::new_v4(),
        });

        let response = team_session_response(session.clone());

        assert_eq!(response["state"], "active");
        assert_eq!(response["session"], session);
    }

    #[test]
    fn biwork_busy_conversation_message_matches_frontend_conflict_detector() {
        let run_id = Uuid::new_v4();
        let message = biwork_conversation_busy_message(run_id);

        assert!(message.contains("already processing"));
        assert!(message.contains(&run_id.to_string()));
    }

    #[test]
    fn conversation_audit_summary_uses_stable_non_empty_fields() {
        let summary = conversation_audit_summary(&[
            ("title", "Weekly planning"),
            ("run_id", "00000000-0000-0000-0000-000000000333"),
            ("prompt", ""),
        ]);

        assert_eq!(
            summary,
            "title=Weekly planning; run_id=00000000-0000-0000-0000-000000000333"
        );
    }

    struct CronRunCapabilityContext {
        tenant_id: Uuid,
        user_id: Uuid,
        conversation_id: Uuid,
        job_id: Uuid,
        skill_id: Uuid,
    }

    struct ChannelRouteAuthzContext {
        tenant_id: Uuid,
        user_id: Uuid,
        pairing_code: String,
        channel_user_id: Uuid,
        session_id: Uuid,
    }

    struct ExtensionGovernanceContext {
        tenant_id: Uuid,
        device_id: Uuid,
    }

    async fn test_pool() -> Result<PgPool, Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(pool)
    }

    async fn test_state() -> Result<AppState, Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());

        Ok(AppState {
            connect_pool: pool.clone(),
            redis_client: RedisClient::open(redis_url)?,
            ferriskey_oidc: FerrisKeyOidcVerifier::new(FerrisKeySettings {
                issuer: "http://localhost:3333/realms/bibi-work".to_string(),
                audience: "bibi-work-backend".to_string(),
                trusted_authorized_parties: Vec::new(),
                discovery_url:
                    "http://localhost:3333/realms/bibi-work/.well-known/openid-configuration"
                        .to_string(),
                jwks_uri: None,
                default_tenant_slug: "bibi-work".to_string(),
                timeout_milliseconds: 1000,
            })?,
            authz_service: ResourceAuthzService::new(pool),
            agent_runtime_client: AgentRuntimeClient::new(AgentRuntimeSettings {
                base_url: None,
                shared_token: secret("test-internal-token"),
                timeout_milliseconds: 1000,
            })?,
            rustfs_client: RustFsClient::disabled_for_tests(),
            memory_vector_client: MemoryVectorClient::new(MemoryVectorSettings {
                enabled: false,
                embedding_endpoint: None,
                qdrant_rest_url: None,
                qdrant_collection: "test_memories".to_string(),
                timeout_milliseconds: 1000,
                max_context_chars: 1200,
                worker_interval_milliseconds: 1000,
                worker_batch_size: 1,
                worker_max_attempts: 1,
            })?,
            internal_shared_token: "test-internal-token".to_string(),
            audit_partition_cleanup_enabled: false,
            secret_resolver:
                crate::features::agent_platform::secret_resolver::SecretResolver::env_only_for_tests(
                ),
            credential_rotation_worker_enabled: false,
        })
    }

    async fn seed_cron_run_capability_context(
        pool: &PgPool,
    ) -> Result<CronRunCapabilityContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Cron run capability test")
            .bind(format!("cron-run-capability-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("cron-run-capability-subject-{user_id}"))
        .bind(format!("cron-run-capability-user-{user_id}"))
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'member')
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;

        let model_profile_id = seed_cron_model_profile(pool, tenant_id).await?;
        let agent_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agents (tenant_id, owner_user_id, name, status)
            VALUES ($1, $2, $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("cron-run-agent-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        let agent_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agent_versions (
                tenant_id, agent_id, version_label, config_snapshot, status
            )
            VALUES ($1, $2, $3, $4, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(agent_id)
        .bind(format!("v-{}", Uuid::new_v4()))
        .bind(json!({
            "runtime": { "kind": "deepagents" },
            "model_profile_id": model_profile_id,
            "agent": { "system_prompt": "capability gated cron run" }
        }))
        .fetch_one(pool)
        .await?;
        let skill_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO skills (tenant_id, name, status)
            VALUES ($1, $2, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("cron-run-skill-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        let skill_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO skill_versions (tenant_id, skill_id, version_label, status)
            VALUES ($1, $2, $3, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(skill_id)
        .bind(format!("v-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_version_skill_bindings (agent_version_id, skill_version_id)
            VALUES ($1, $2)
            "#,
        )
        .bind(agent_version_id)
        .bind(skill_version_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversations (id, tenant_id, created_by_user_id, agent_id, title)
            VALUES ($1, $2, $3, $4, 'Cron run capability test')
            "#,
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(agent_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO scheduled_jobs (
                id, tenant_id, name, source_conversation_id, target_mode,
                target_conversation_id, assistant_profile_id, agent_snapshot,
                prompt_template, model_profile_id, schedule_kind, schedule_expr,
                created_by_user_id, created_from, metadata
            )
            VALUES (
                $1, $2, 'Cron run capability test', $3, 'existing',
                $3, $4, $5, 'must not dispatch', $6, 'every', '60000',
                $7, 'user', '{}'::jsonb
            )
            "#,
        )
        .bind(job_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(agent_id)
        .bind(json!({
            "name": "Cron capability assistant",
            "assistant_id": agent_id.to_string()
        }))
        .bind(model_profile_id)
        .bind(user_id)
        .execute(pool)
        .await?;

        seed_test_policy_binding(
            pool,
            tenant_id,
            user_id,
            "agent",
            &agent_id.to_string(),
            "run",
            "allow",
        )
        .await?;
        seed_test_policy_binding(
            pool,
            tenant_id,
            user_id,
            "skill",
            &skill_id.to_string(),
            "use",
            "deny",
        )
        .await?;

        Ok(CronRunCapabilityContext {
            tenant_id,
            user_id,
            conversation_id,
            job_id,
            skill_id,
        })
    }

    async fn seed_channel_route_authz_context(
        pool: &PgPool,
    ) -> Result<ChannelRouteAuthzContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let channel_user_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let pairing_code = "PAIR-DENY".to_string();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Channel route authz test")
            .bind(format!("channel-route-authz-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("channel-route-authz-subject-{user_id}"))
        .bind(format!("channel-route-authz-user-{user_id}"))
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'member')
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO channel_connectors (
                tenant_id, connector_key, runtime_kind, status, enabled, connected
            )
            VALUES ($1, 'telegram', 'builtin', 'connected', TRUE, TRUE)
            "#,
        )
        .bind(tenant_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO channel_pairing_requests (
                tenant_id, platform, code, platform_user_id, display_name, expires_at
            )
            VALUES ($1, 'telegram', $2, 'pairing-user', 'Pairing User', $3)
            "#,
        )
        .bind(tenant_id)
        .bind(&pairing_code)
        .bind(OffsetDateTime::now_utc() + Duration::hours(1))
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO channel_authorized_users (
                id, tenant_id, platform, platform_user_id, display_name, status
            )
            VALUES ($1, $2, 'telegram', 'authorized-user', 'Authorized User', 'active')
            "#,
        )
        .bind(channel_user_id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO channel_sessions (
                id, tenant_id, platform, channel_user_id, agent_type, workspace, chat_id
            )
            VALUES ($1, $2, 'telegram', $3, 'acp', '/workspace', 'chat-1')
            "#,
        )
        .bind(session_id)
        .bind(tenant_id)
        .bind(channel_user_id)
        .execute(pool)
        .await?;

        seed_test_policy_binding(
            pool,
            tenant_id,
            user_id,
            "channel_pairing",
            &pairing_code,
            "approve",
            "deny",
        )
        .await?;
        seed_test_policy_binding(
            pool,
            tenant_id,
            user_id,
            "channel_user",
            &channel_user_id.to_string(),
            "revoke",
            "deny",
        )
        .await?;

        Ok(ChannelRouteAuthzContext {
            tenant_id,
            user_id,
            pairing_code,
            channel_user_id,
            session_id,
        })
    }

    async fn seed_extension_governance_context(
        pool: &PgPool,
    ) -> Result<ExtensionGovernanceContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Extension governance test")
            .bind(format!("extension-governance-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("extension-governance-subject-{user_id}"))
        .bind(format!("extension-governance-user-{user_id}"))
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'member')
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO devices (
                id, tenant_id, user_id, device_fingerprint, device_name, platform
            )
            VALUES ($1, $2, $3, $4, 'Extension Governance Device', 'desktop')
            "#,
        )
        .bind(device_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("extension-governance-device-{device_id}"))
        .execute(pool)
        .await?;

        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "allowed-assistant-package",
            "discovered",
            "assistant",
            "allowed-assistant",
            true,
            true,
            true,
            "installed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "disabled-device-package",
            "discovered",
            "assistant",
            "disabled-device",
            true,
            true,
            false,
            "installed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "uninstalled-package",
            "discovered",
            "assistant",
            "uninstalled",
            true,
            false,
            true,
            "installed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "install-failed-package",
            "discovered",
            "assistant",
            "install-failed",
            true,
            true,
            true,
            "install_failed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "blocked-assistant-package",
            "blocked",
            "assistant",
            "blocked-assistant",
            true,
            true,
            true,
            "installed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "disabled-contribution-package",
            "discovered",
            "assistant",
            "disabled-contribution",
            false,
            true,
            true,
            "installed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "discovered-webui-package",
            "discovered",
            "webui",
            "discovered-webui",
            true,
            true,
            true,
            "installed",
        )
        .await?;
        seed_extension_contribution(
            pool,
            tenant_id,
            device_id,
            "approved-webui-package",
            "approved",
            "webui",
            "approved-webui",
            true,
            true,
            true,
            "installed",
        )
        .await?;

        Ok(ExtensionGovernanceContext {
            tenant_id,
            device_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn seed_extension_contribution(
        pool: &PgPool,
        tenant_id: Uuid,
        device_id: Uuid,
        extension_name: &str,
        package_status: &str,
        contribution_type: &str,
        contribution_key: &str,
        contribution_enabled: bool,
        device_installed: bool,
        device_enabled: bool,
        install_status: &str,
    ) -> Result<(), sqlx::Error> {
        let package_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO extension_packages (
                id, tenant_id, extension_name, source, manifest, risk_level, status
            )
            VALUES ($1, $2, $3, 'local', $4, 'moderate', $5)
            "#,
        )
        .bind(package_id)
        .bind(tenant_id)
        .bind(extension_name)
        .bind(json!({
            "display_name": extension_name,
        }))
        .bind(package_status)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO device_extension_states (
                tenant_id, device_id, extension_package_id, installed, enabled, install_status
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(tenant_id)
        .bind(device_id)
        .bind(package_id)
        .bind(device_installed)
        .bind(device_enabled)
        .bind(install_status)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO extension_contributions (
                tenant_id, extension_package_id, contribution_type,
                contribution_key, manifest, enabled
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(tenant_id)
        .bind(package_id)
        .bind(contribution_type)
        .bind(contribution_key)
        .bind(json!({
            "label": contribution_key,
        }))
        .bind(contribution_enabled)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn seed_cron_model_profile(pool: &PgPool, tenant_id: Uuid) -> Result<Uuid, sqlx::Error> {
        let suffix = Uuid::new_v4();
        let provider_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO llm_providers (tenant_id, provider_key, display_name, base_url)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind("test")
        .bind(format!("Cron Run Test Provider {suffix}"))
        .bind("http://localhost:1/v1")
        .fetch_one(pool)
        .await?;

        sqlx::query_scalar(
            r#"
            INSERT INTO llm_model_profiles (
                tenant_id, provider_id, profile_name, model_name,
                max_output_tokens, temperature
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(provider_id)
        .bind(format!("cron-run-profile-{suffix}"))
        .bind("fake-model")
        .bind(1024_i64)
        .bind(0.0_f64)
        .fetch_one(pool)
        .await
    }

    async fn seed_test_policy_binding(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        resource_type: &str,
        resource_id: &str,
        action: &str,
        effect: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO resource_policy_bindings (
                tenant_id, resource_type, resource_id, action,
                subject_type, subject_id, effect, created_by_user_id
            )
            VALUES ($1, $2, $3, $4, 'user', $5, $6, $7)
            "#,
        )
        .bind(tenant_id)
        .bind(resource_type)
        .bind(resource_id)
        .bind(action)
        .bind(user_id.to_string())
        .bind(effect)
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    fn test_platform_context(tenant_id: Uuid, user_id: Uuid) -> PlatformRequestContext {
        PlatformRequestContext {
            tenant_id,
            platform_user_id: user_id,
            ferriskey_subject: format!("test-subject-{user_id}"),
            preferred_username: Some(format!("test-user-{user_id}")),
            email: None,
            roles: vec!["tenant_member".to_string()],
            session_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            trace_id: format!("trace-{}", Uuid::new_v4()),
            token_jti: None,
            token_exp: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        }
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
