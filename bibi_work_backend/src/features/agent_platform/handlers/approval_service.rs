use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};
use sqlx::Row;
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::{self, ApprovalEvidenceInput, NewAuditLog},
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::*,
            runtime::ResumeRunRequest,
            secret_resolver,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn tool_call_authorize(
    State(state): State<AppState>,
    Json(payload): Json<ToolAuthorizeRequest>,
) -> Result<Json<ToolAuthorizeResponse>, AppError> {
    let resource = payload.resource.clone().unwrap_or(ResourceRef {
        resource_type: "tool".to_string(),
        id: payload
            .tool_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| payload.tool_name.clone()),
        path: None,
    });

    let authz_request = AuthzCheckRequest {
        tenant_id: payload.tenant_id,
        actor: payload.actor,
        action: "execute".to_string(),
        resource,
        context: Some(AuthzContext {
            conversation_id: payload.conversation_id,
            run_id: payload.run_id,
            tool_id: payload.tool_id,
            args_hash: payload.args_hash.clone(),
            risk_level: payload.risk_level.clone(),
            ..Default::default()
        }),
    };

    let decision = state.authz_service.check(&authz_request).await;
    let status = match decision.decision.as_str() {
        "allow" => "authorized",
        "review" => "waiting_approval",
        _ => "denied",
    };

    let mut tx = state.connect_pool.begin().await?;
    let tool_call_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tool_calls (
            id, tenant_id, conversation_id, run_id, tool_id, tool_name,
            resource_type, resource_id, args_hash, risk_level, status, decision,
            policy_version, input_summary
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(tool_call_id)
    .bind(authz_request.tenant_id)
    .bind(payload.conversation_id)
    .bind(payload.run_id)
    .bind(payload.tool_id)
    .bind(payload.tool_name)
    .bind(&authz_request.resource.resource_type)
    .bind(&authz_request.resource.id)
    .bind(payload.args_hash)
    .bind(payload.risk_level.unwrap_or_else(|| "low".to_string()))
    .bind(status)
    .bind(&decision.decision)
    .bind(&decision.policy_version)
    .bind(payload.input_summary)
    .execute(&mut *tx)
    .await?;

    let mut approval_id = None;
    let mut interrupt_id = None;

    if decision.is_review() {
        let approval = Uuid::new_v4();
        let interrupt = Uuid::new_v4();
        approval_id = Some(approval);
        interrupt_id = Some(interrupt);

        sqlx::query(
            r#"
            INSERT INTO approvals (
                id, tenant_id, conversation_id, run_id, tool_call_id,
                approval_policy_id, request_payload
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(approval)
        .bind(authz_request.tenant_id)
        .bind(payload.conversation_id)
        .bind(payload.run_id)
        .bind(tool_call_id)
        .bind(
            decision
                .obligations
                .as_ref()
                .and_then(|obligation| obligation.approval_policy_id.clone()),
        )
        .bind(json!({ "authz": decision, "tool_call_id": tool_call_id }))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO interrupts (
                id, tenant_id, conversation_id, run_id, approval_id, type, payload
            )
            VALUES ($1, $2, $3, $4, $5, 'approval', $6)
            "#,
        )
        .bind(interrupt)
        .bind(authz_request.tenant_id)
        .bind(payload.conversation_id)
        .bind(payload.run_id)
        .bind(approval)
        .bind(json!({ "approval_id": approval, "tool_call_id": tool_call_id }))
        .execute(&mut *tx)
        .await?;

        if let Some(run_id) = payload.run_id {
            sqlx::query(
                "UPDATE runs SET status = 'waiting_approval', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
            )
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    write_authz_audit_tx(
        &mut tx,
        &authz_request,
        &decision,
        Some(tool_call_id),
        approval_id,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Json(ToolAuthorizeResponse {
        decision,
        tool_call_id,
        approval_id,
        interrupt_id,
    }))
}

pub async fn list_approvals(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<ApprovalResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, run_id, tool_call_id, status,
               approval_policy_id, request_payload, decision_payload,
               evidence_object_reference_id, created_at, decided_at
        FROM approvals
        WHERE tenant_id = $1 AND ($2::text IS NULL OR status = $2)
        ORDER BY created_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let approvals = rows
        .into_iter()
        .map(approval_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(approvals))
}

pub async fn decide_approval(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(approval_id): Path<Uuid>,
    Json(payload): Json<ApprovalDecisionRequest>,
) -> Result<Json<ApprovalResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "approve",
        "approval",
        approval_id.to_string(),
        None,
    )
    .await?;

    let normalized = match payload.decision.as_str() {
        "approved" | "approve" | "allow" => "approved",
        "rejected" | "reject" | "deny" => "rejected",
        other => {
            return Err(AppError::InvalidInput(format!(
                "unsupported decision: {other}"
            )));
        }
    };

    let mut tx = state.connect_pool.begin().await?;
    let decision_payload = json!({
        "decision": normalized,
        "reason": payload.reason,
        "payload": payload.payload
    });
    let decision_payload_for_resume = decision_payload.clone();

    let row = sqlx::query(
        r#"
        UPDATE approvals
        SET status = $1,
            approver_user_id = $2,
            decision_payload = $3,
            decided_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $4 AND tenant_id = $5 AND status = 'pending'
        RETURNING id, tenant_id, conversation_id, run_id, tool_call_id, status,
                  approval_policy_id, request_payload, decision_payload,
                  evidence_object_reference_id, created_at, decided_at
        "#,
    )
    .bind(normalized)
    .bind(ctx.platform_user_id)
    .bind(decision_payload)
    .bind(approval_id)
    .bind(payload.tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::Conflict("approval is not pending or does not exist".to_string()))?;

    let mut approval = approval_from_row(row)?;

    if normalized == "rejected"
        && let Some(run_id) = approval.run_id
    {
        sqlx::query("UPDATE runs SET status = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2")
            .bind("failed")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
    }

    insert_approval_decision_audit_tx(&mut tx, &ctx, &approval, normalized).await?;

    if let Some(conversation_id) = approval.conversation_id {
        let event = event_store::insert_event_tx(
            &mut tx,
            approval.tenant_id,
            conversation_id,
            approval.run_id,
            RunEventInput {
                event_id: Some(format!("approval.completed.{}", approval.id)),
                event_type: "approval.completed".to_string(),
                payload: Some(json!({
                    "approval_id": approval.id,
                    "status": approval.status,
                    "run_id": approval.run_id
                })),
                trace_id: None,
            },
        )
        .await?;
        let archived_evidence =
            archive_decided_approval_evidence(&mut tx, &state, &ctx, &approval).await?;
        approval.evidence_object_reference_id = archived_evidence.object_reference_id;
        if let Err(err) = tx.commit().await {
            if let Some(object_key) = archived_evidence.object_key {
                let _ = state.rustfs_client.delete_audit_object(&object_key).await;
            }
            return Err(err.into());
        }
        event_store::publish_single_event(&state, &event).await;
    } else {
        let archived_evidence =
            archive_decided_approval_evidence(&mut tx, &state, &ctx, &approval).await?;
        approval.evidence_object_reference_id = archived_evidence.object_reference_id;
        if let Err(err) = tx.commit().await {
            if let Some(object_key) = archived_evidence.object_key {
                let _ = state.rustfs_client.delete_audit_object(&object_key).await;
            }
            return Err(err.into());
        }
    }

    if normalized == "approved"
        && let Some(run_id) = approval.run_id
    {
        let mut resume_payload =
            load_resume_payload(&state, run_id, &approval, decision_payload_for_resume).await?;
        if let Err(err) = secret_resolver::attach_llm_runtime_credential(
            &state,
            resume_payload.tenant_id,
            run_id,
            &mut resume_payload.run_config_snapshot,
        )
        .await
        {
            warn!(
                approval_id = %approval.id,
                run_id = %run_id,
                "approval was accepted but runtime credential resolution failed: {}",
                err
            );
            mark_approval_resume_failed(&state, &approval, &err.to_string()).await?;
            return Ok(Json(approval));
        }
        if let Err(err) = state
            .agent_runtime_client
            .resume_run(run_id, &resume_payload)
            .await
        {
            warn!(
                approval_id = %approval.id,
                run_id = %run_id,
                "approval was accepted but runtime resume failed: {}",
                err
            );
            mark_approval_resume_failed(&state, &approval, &err.to_string()).await?;
        }
    }

    Ok(Json(approval))
}

async fn insert_approval_decision_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ctx: &PlatformRequestContext,
    approval: &ApprovalResponse,
    normalized: &str,
) -> Result<(), AppError> {
    let approval_resource_id = approval.id.to_string();
    audit::insert_audit_log_tx(
        tx,
        NewAuditLog {
            tenant_id: approval.tenant_id,
            actor_user_id: Some(ctx.platform_user_id),
            actor_device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            resource_type: "approval",
            resource_id: &approval_resource_id,
            action: "approval.completed",
            decision: normalized,
            policy_version: "local-policy-v1",
            reason_code: None,
            run_id: approval.run_id,
            conversation_id: approval.conversation_id,
            workflow_run_id: None,
            tool_call_id: approval.tool_call_id,
            approval_id: Some(approval.id),
            args_hash: None,
            input_summary: None,
            output_summary: None,
            risk_level: Some("high"),
            ip: None,
            user_agent: None,
            trace_id: None,
        },
    )
    .await?;
    Ok(())
}

async fn archive_decided_approval_evidence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &AppState,
    ctx: &PlatformRequestContext,
    approval: &ApprovalResponse,
) -> Result<audit::ArchivedAuditEvidence, AppError> {
    audit::archive_approval_evidence_tx(
        tx,
        &state.rustfs_client,
        ApprovalEvidenceInput {
            tenant_id: approval.tenant_id,
            approval_id: approval.id,
            actor_user_id: Some(ctx.platform_user_id),
            conversation_id: approval.conversation_id,
            run_id: approval.run_id,
            tool_call_id: approval.tool_call_id,
            status: approval.status.clone(),
            request_payload: approval.request_payload.clone(),
            decision_payload: approval
                .decision_payload
                .clone()
                .unwrap_or_else(|| json!({ "decision": approval.status })),
            decided_at: approval.decided_at,
        },
    )
    .await
}

async fn load_resume_payload(
    state: &AppState,
    run_id: Uuid,
    approval: &ApprovalResponse,
    decision_payload: Value,
) -> Result<ResumeRunRequest, AppError> {
    let row = sqlx::query(
        r#"
        SELECT input, run_config_snapshot, trace_id, thread_id, checkpoint_id
        FROM runs
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(run_id)
    .bind(approval.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("run not found for approval resume".to_string()))?;

    Ok(ResumeRunRequest {
        tenant_id: approval.tenant_id,
        conversation_id: approval.conversation_id,
        approval_id: approval.id,
        trace_id: Some(row.try_get("trace_id")?),
        input: row.try_get("input")?,
        run_config_snapshot: row.try_get("run_config_snapshot")?,
        thread_id: row.try_get("thread_id")?,
        checkpoint_id: row.try_get("checkpoint_id")?,
        decision_payload,
    })
}

async fn mark_approval_resume_failed(
    state: &AppState,
    approval: &ApprovalResponse,
    error: &str,
) -> Result<(), AppError> {
    let Some(run_id) = approval.run_id else {
        return Ok(());
    };

    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'failed',
            completed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;

    let event = if let Some(conversation_id) = approval.conversation_id {
        Some(
            event_store::insert_event_tx(
                &mut tx,
                approval.tenant_id,
                conversation_id,
                Some(run_id),
                RunEventInput {
                    event_id: Some(format!("run.resume.failed.{}", approval.id)),
                    event_type: "run.failed".to_string(),
                    payload: Some(json!({
                        "run_id": run_id,
                        "approval_id": approval.id,
                        "error_type": "approval_resume_failed",
                        "error": error
                    })),
                    trace_id: None,
                },
            )
            .await?,
        )
    } else {
        None
    };

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    if let Some(event) = event {
        event_store::publish_single_event(state, &event).await;
    }

    Ok(())
}
