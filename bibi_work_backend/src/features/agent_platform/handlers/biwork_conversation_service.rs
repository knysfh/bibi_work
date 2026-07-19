use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde_json::{Value, json};
use sqlx::Row;
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::{self, NewAuditLog},
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::{CreateRunRequest, RunEventInput},
            runtime::CancelRunRequest,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_compat_service::ok, biwork_conversation_runtime_service::conversation_runtime_summary,
    biwork_conversation_support::ensure_conversation_exists,
    run_service::create_and_dispatch_conversation_run,
};

pub async fn biwork_send_conversation_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let content = payload
        .get("content")
        .cloned()
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| json!(""));
    let loading_id = payload
        .get("loading_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    ensure_biwork_conversation_accepts_message(
        &state,
        ctx.tenant_id,
        conversation_id,
        loading_id.as_deref(),
    )
    .await?;
    let input = json!({
        "messages": [
            {
                "role": "user",
                "content": content,
            }
        ],
        "biwork": {
            "files": payload.get("files").cloned().unwrap_or_else(|| json!([])),
            "inject_skills": payload.get("inject_skills").cloned().unwrap_or_else(|| json!([])),
            "client": "biwork",
        },
    });
    let run = create_and_dispatch_conversation_run(
        &state,
        &ctx,
        conversation_id,
        CreateRunRequest {
            tenant_id: ctx.tenant_id,
            agent_id: None,
            agent_version_id: None,
            project_id: None,
            input: Some(input),
            run_config_snapshot: Some(json!({
                "ui": {
                    "client": "biwork",
                    "conversation_type": "acp",
                },
            })),
            idempotency_key: loading_id,
            thread_id: Some(conversation_id.to_string()),
        },
    )
    .await?;

    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "send_message",
            decision: "allow",
            reason_code: Some("conversation.send_message"),
            run_id: Some(run.id),
            output_summary: Some(conversation_audit_summary(&[
                ("run_id", &run.id.to_string()),
                ("client", "biwork"),
            ])),
        },
    )
    .await?;

    Ok(ok(json!({
        "msg_id": format!("user.{}", run.id),
        "turn_id": run.id.to_string(),
        "runtime": {
            "state": "running",
            "can_send_message": false,
            "has_task": true,
            "task_status": "running",
            "is_processing": true,
            "pending_confirmations": 0,
            "turn_id": run.id.to_string(),
        },
    })))
}

pub async fn biwork_cancel_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let turn_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let turn_uuid = turn_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok());
    let mut tx = state.connect_pool.begin().await?;
    let rows = sqlx::query(
        r#"
        UPDATE runs
        SET status = 'cancelled',
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND ($4::text IS NULL OR ($3::uuid IS NOT NULL AND id = $3))
          AND status IN ('queued', 'pending', 'running', 'waiting_approval')
        RETURNING id, trace_id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(turn_uuid)
    .bind(&turn_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut cancelled_runs = Vec::with_capacity(rows.len());
    let mut events_to_publish = Vec::with_capacity(rows.len());
    for row in rows {
        let run_id: Uuid = row.try_get("id")?;
        let trace_id: String = row.try_get("trace_id")?;
        events_to_publish.push(
            event_store::insert_event_tx(
                &mut tx,
                ctx.tenant_id,
                conversation_id,
                Some(run_id),
                RunEventInput {
                    event_id: Some(format!("run.cancelled.biwork.{run_id}")),
                    event_type: "run.cancelled".to_string(),
                    payload: Some(json!({
                        "run_id": run_id,
                        "status": "cancelled",
                        "reason": "conversation_cancelled",
                        "client": "biwork"
                    })),
                    trace_id: Some(trace_id.clone()),
                },
            )
            .await?,
        );
        cancelled_runs.push((run_id, trace_id));
    }

    let cancelled_approvals = sqlx::query(
        r#"
        UPDATE approvals
        SET status = 'cancelled',
            decision_payload = jsonb_build_object(
                'decision', 'cancelled',
                'reason', 'conversation_cancelled'
            ),
            decided_at = COALESCE(decided_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND ($4::text IS NULL OR ($3::uuid IS NOT NULL AND run_id = $3))
          AND status = 'pending'
        RETURNING id, run_id, tool_call_id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(turn_uuid)
    .bind(&turn_id)
    .fetch_all(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE interrupts
        SET status = 'resolved',
            resolved_at = COALESCE(resolved_at, CURRENT_TIMESTAMP)
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND ($4::text IS NULL OR ($3::uuid IS NOT NULL AND run_id = $3))
          AND status = 'open'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(turn_uuid)
    .bind(&turn_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE local_exec_requests ler
        SET status = 'cancelled', updated_at = CURRENT_TIMESTAMP
        WHERE ler.tenant_id = $1
          AND ler.run_id IN (
              SELECT r.id
              FROM runs r
              WHERE r.tenant_id = $1
                AND r.conversation_id = $2
                AND ($4::text IS NULL OR ($3::uuid IS NOT NULL AND r.id = $3))
                AND r.status = 'cancelled'
          )
          AND ler.status IN ('queued', 'dispatching')
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(turn_uuid)
    .bind(&turn_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE tool_calls
        SET status = 'cancelled',
            decision = 'deny',
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP)
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND ($4::text IS NULL OR ($3::uuid IS NOT NULL AND run_id = $3))
          AND status = 'waiting_approval'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(turn_uuid)
    .bind(&turn_id)
    .execute(&mut *tx)
    .await?;

    for approval in cancelled_approvals {
        let approval_id: Uuid = approval.try_get("id")?;
        let run_id: Option<Uuid> = approval.try_get("run_id")?;
        let tool_call_id: Option<Uuid> = approval.try_get("tool_call_id")?;
        events_to_publish.push(
            event_store::insert_event_tx(
                &mut tx,
                ctx.tenant_id,
                conversation_id,
                run_id,
                RunEventInput {
                    event_id: Some(format!("approval.decided.cancelled.{approval_id}")),
                    event_type: "approval.decided".to_string(),
                    payload: Some(json!({
                        "approval_id": approval_id,
                        "tool_call_id": tool_call_id,
                        "run_id": run_id,
                        "decision": "cancelled",
                        "status": "cancelled",
                        "reason": "conversation_cancelled"
                    })),
                    trace_id: Some(ctx.trace_id.clone()),
                },
            )
            .await?,
        );
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    for event in &events_to_publish {
        event_store::publish_single_event(&state, event).await;
    }
    for (run_id, trace_id) in &cancelled_runs {
        if let Err(err) = state
            .agent_runtime_client
            .cancel_run(
                *run_id,
                &CancelRunRequest {
                    tenant_id: ctx.tenant_id,
                    conversation_id,
                    trace_id: Some(trace_id.clone()),
                    reason: "conversation_cancelled".to_string(),
                },
            )
            .await
        {
            warn!(
                "failed to propagate BiWork conversation cancel for run {}: {}",
                run_id, err
            );
        }
    }

    let cancelled_run_count = cancelled_runs.len().to_string();
    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "cancel",
            decision: "allow",
            reason_code: Some("conversation.cancel"),
            run_id: turn_uuid,
            output_summary: Some(conversation_audit_summary(&[
                (
                    "turn_id",
                    payload.get("turn_id").and_then(Value::as_str).unwrap_or(""),
                ),
                ("cancelled_runs", &cancelled_run_count),
            ])),
        },
    )
    .await?;

    Ok(ok(json!({
        "runtime": conversation_runtime_summary(),
    })))
}

pub async fn biwork_side_question(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let question = payload
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if question.is_empty() {
        return Ok(ok(json!({
            "status": "invalid",
            "reason": "emptyQuestion",
        })));
    }
    Ok(ok(json!({ "status": "unsupported" })))
}

pub(super) struct ConversationAudit<'a> {
    pub(super) conversation_id: Uuid,
    pub(super) action: &'a str,
    pub(super) decision: &'a str,
    pub(super) reason_code: Option<&'a str>,
    pub(super) run_id: Option<Uuid>,
    pub(super) output_summary: Option<String>,
}

pub(super) async fn write_conversation_audit(
    state: &AppState,
    ctx: &PlatformRequestContext,
    entry: ConversationAudit<'_>,
) -> Result<(), AppError> {
    let ConversationAudit {
        conversation_id,
        action,
        decision,
        reason_code,
        run_id,
        output_summary,
    } = entry;
    let resource_id = conversation_id.to_string();
    let mut tx = state.connect_pool.begin().await?;
    audit::insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: ctx.tenant_id,
            actor_user_id: Some(ctx.platform_user_id),
            actor_device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            resource_type: "conversation",
            resource_id: &resource_id,
            action,
            decision,
            policy_version: "biwork-conversation-v1",
            reason_code,
            run_id,
            conversation_id: Some(conversation_id),
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some(&resource_id),
            output_summary: output_summary.as_deref(),
            risk_level: Some("medium"),
            ip: None,
            user_agent: None,
            trace_id: Some(ctx.trace_id.as_str()),
        },
    )
    .await?;
    tx.commit().await.map_err(|_| AppError::DatabaseTransaction)
}

pub(super) fn conversation_audit_summary(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) fn biwork_cron_job_id_from_conversation_metadata(metadata: &Value) -> Option<Uuid> {
    metadata
        .pointer("/extra/cron_job_id")
        .or_else(|| metadata.pointer("/extra/cronJobId"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

async fn ensure_biwork_conversation_accepts_message(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    loading_id: Option<&str>,
) -> Result<(), AppError> {
    let active_run_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM runs
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled')
          AND ($3::text IS NULL OR idempotency_key IS DISTINCT FROM $3)
        ORDER BY updated_at DESC, started_at DESC NULLS LAST, queued_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(loading_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    if let Some(active_run_id) = active_run_id {
        return Err(AppError::Conflict(biwork_conversation_busy_message(
            active_run_id,
        )));
    }
    Ok(())
}

pub(super) fn biwork_conversation_busy_message(active_run_id: Uuid) -> String {
    format!("conversation is already processing run {active_run_id}")
}
