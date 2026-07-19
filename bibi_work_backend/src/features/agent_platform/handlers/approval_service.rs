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
            run_snapshot,
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
    let event_tool_name = payload.tool_name.clone();
    let event_risk_level = payload
        .risk_level
        .clone()
        .unwrap_or_else(|| "low".to_string());
    let event_input_summary = payload.input_summary.clone();
    let resource = payload.resource.clone().unwrap_or(ResourceRef {
        resource_type: "tool".to_string(),
        id: payload
            .tool_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| payload.tool_name.clone()),
        path: None,
    });
    if let Some(response) = reuse_approved_tool_call(&state, &payload, &resource).await? {
        return Ok(Json(response));
    }

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
            trace_id: payload.trace_id.clone(),
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
    let mut approval_event = None;

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

        if let Some(conversation_id) = payload.conversation_id {
            approval_event = Some(
                event_store::insert_event_tx(
                    &mut tx,
                    authz_request.tenant_id,
                    conversation_id,
                    payload.run_id,
                    RunEventInput {
                        event_id: Some(format!("approval.requested.{approval}")),
                        event_type: "approval.requested".to_string(),
                        payload: Some(json!({
                            "approval_id": approval,
                            "tool_call_id": tool_call_id,
                            "tool_name": event_tool_name,
                            "risk_level": event_risk_level,
                            "input_summary": event_input_summary,
                            "reason": decision.reason_code,
                            "run_id": payload.run_id,
                        })),
                        trace_id: payload.trace_id.clone(),
                    },
                )
                .await?,
            );
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
    if let Some(event) = approval_event {
        event_store::publish_single_event(&state, &event).await;
    }

    Ok(Json(ToolAuthorizeResponse {
        decision,
        tool_call_id,
        approval_id,
        interrupt_id,
    }))
}

async fn reuse_approved_tool_call(
    state: &AppState,
    payload: &ToolAuthorizeRequest,
    resource: &ResourceRef,
) -> Result<Option<ToolAuthorizeResponse>, AppError> {
    let (Some(run_id), Some(conversation_id)) = (payload.run_id, payload.conversation_id) else {
        return Ok(None);
    };
    let mut tx = state.connect_pool.begin().await?;
    let row = sqlx::query(
        r#"
        SELECT tc.id AS tool_call_id,
               tc.policy_version,
               a.id AS approval_id,
               i.id AS interrupt_id
        FROM tool_calls tc
        JOIN approvals a
          ON a.tool_call_id = tc.id
         AND a.tenant_id = tc.tenant_id
         AND a.status = 'approved'
        LEFT JOIN interrupts i
          ON i.approval_id = a.id
         AND i.tenant_id = a.tenant_id
        WHERE tc.tenant_id = $1
          AND tc.conversation_id = $2
          AND tc.run_id = $3
          AND tc.tool_name = $4
          AND tc.resource_type = $5
          AND tc.resource_id = $6
          AND tc.args_hash IS NOT DISTINCT FROM $7
          AND tc.status = 'waiting_approval'
          AND tc.decision = 'review'
        ORDER BY a.decided_at DESC, a.id DESC
        LIMIT 1
        FOR UPDATE OF tc
        "#,
    )
    .bind(payload.tenant_id)
    .bind(conversation_id)
    .bind(run_id)
    .bind(&payload.tool_name)
    .bind(&resource.resource_type)
    .bind(&resource.id)
    .bind(&payload.args_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };

    let tool_call_id: Uuid = row.try_get("tool_call_id")?;
    let updated = sqlx::query(
        "UPDATE tool_calls SET status = 'authorized' WHERE id = $1 AND status = 'waiting_approval'",
    )
    .bind(tool_call_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Ok(None);
    }
    let policy_version: String = row.try_get("policy_version")?;
    let approval_id: Uuid = row.try_get("approval_id")?;
    let interrupt_id: Option<Uuid> = row.try_get("interrupt_id")?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Some(ToolAuthorizeResponse {
        decision: AuthzDecision::allow(policy_version),
        tool_call_id,
        approval_id: Some(approval_id),
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
    let desktop_local_run = if let Some(run_id) = approval.run_id {
        sqlx::query_scalar::<_, bool>(
            r#"
            SELECT COALESCE(run_config_snapshot#>>'{runtime,kind}', '') = $3
            FROM runs
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(run_id)
        .bind(approval.tenant_id)
        .bind(run_snapshot::DESKTOP_ACP_RUNTIME_KIND)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(false)
    } else {
        false
    };

    sqlx::query(
        r#"
        UPDATE interrupts
        SET status = 'resolved',
            resolved_at = COALESCE(resolved_at, CURRENT_TIMESTAMP)
        WHERE tenant_id = $1
          AND approval_id = $2
          AND status = 'open'
        "#,
    )
    .bind(approval.tenant_id)
    .bind(approval.id)
    .execute(&mut *tx)
    .await?;

    if normalized == "rejected"
        && !desktop_local_run
        && let Some(run_id) = approval.run_id
    {
        sqlx::query("UPDATE runs SET status = $1, updated_at = CURRENT_TIMESTAMP WHERE id = $2")
            .bind("failed")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
    }
    if desktop_local_run {
        if let Some(run_id) = approval.run_id {
            sqlx::query(
                "UPDATE runs SET status = 'running', updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND status = 'waiting_approval'",
            )
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(tool_call_id) = approval.tool_call_id {
            sqlx::query("UPDATE tool_calls SET status = $1, decision = $2 WHERE id = $3")
                .bind(if normalized == "approved" {
                    "authorized"
                } else {
                    "failed"
                })
                .bind(if normalized == "approved" {
                    "allow"
                } else {
                    "deny"
                })
                .bind(tool_call_id)
                .execute(&mut *tx)
                .await?;
        }
    }

    insert_approval_decision_audit_tx(&mut tx, &ctx, &approval, normalized).await?;

    if let Some(conversation_id) = approval.conversation_id {
        let event = event_store::insert_event_tx(
            &mut tx,
            approval.tenant_id,
            conversation_id,
            approval.run_id,
            RunEventInput {
                event_id: Some(format!("approval.decided.{}", approval.id)),
                event_type: "approval.decided".to_string(),
                payload: Some(json!({
                    "approval_id": approval.id,
                    "decision": approval.status,
                    "decided_by_user_id": ctx.platform_user_id,
                    "decided_at": approval.decided_at,
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
        if desktop_local_run {
            return Ok(Json(approval));
        }
        let mut resume_payload =
            load_resume_payload(&state, run_id, &approval, decision_payload_for_resume).await?;
        if let Err(err) =
            run_snapshot::ensure_python_dispatch_runtime(&resume_payload.run_config_snapshot)
        {
            warn!(
                approval_id = %approval.id,
                run_id = %run_id,
                "approval was accepted but runtime resume is not handled by Python: {}",
                err
            );
            mark_approval_resume_failed(&state, &approval, &err.to_string()).await?;
            return Ok(Json(approval));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::json;
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};
    use uuid::Uuid;

    use crate::{
        configuration::{AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings},
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
    };

    struct ToolAuthorizeTestContext {
        tenant_id: Uuid,
        user_id: Uuid,
        device_id: Uuid,
        session_id: Uuid,
        conversation_id: Uuid,
        run_id: Uuid,
    }

    #[tokio::test]
    #[ignore]
    async fn tool_authorize_denies_critical_local_exec_without_explicit_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_tool_authorize_context(&state.connect_pool).await?;

        let Json(response) = tool_call_authorize(
            State(state.clone()),
            Json(tool_authorize_request(
                &context,
                "local_exec",
                "critical",
                ResourceRef {
                    resource_type: "local_exec".to_string(),
                    id: context.device_id.to_string(),
                    path: None,
                },
            )),
        )
        .await?;

        assert_eq!(response.decision.decision, "deny");
        assert_eq!(
            response.decision.reason_code.as_deref(),
            Some("critical_risk_requires_explicit_policy")
        );
        assert!(response.approval_id.is_none());
        assert!(response.interrupt_id.is_none());

        let tool_call = load_tool_call(&state.connect_pool, response.tool_call_id).await?;
        assert_eq!(tool_call.try_get::<String, _>("status")?, "denied");
        assert_eq!(tool_call.try_get::<String, _>("decision")?, "deny");
        assert_eq!(
            tool_call.try_get::<String, _>("resource_type")?,
            "local_exec"
        );
        assert_eq!(
            tool_call.try_get::<String, _>("resource_id")?,
            context.device_id.to_string()
        );
        assert_eq!(tool_call.try_get::<String, _>("risk_level")?, "critical");
        assert_eq!(
            count_approvals(&state.connect_pool, context.tenant_id).await?,
            0
        );
        assert_eq!(
            count_interrupts(&state.connect_pool, context.tenant_id).await?,
            0
        );
        assert_eq!(
            run_status(&state.connect_pool, context.run_id).await?,
            "running"
        );
        assert_latest_authz_decision(
            &state.connect_pool,
            context.tenant_id,
            response.tool_call_id,
            "deny",
            "critical_risk_requires_explicit_policy",
        )
        .await?;

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn tool_authorize_reviews_high_risk_tool_with_execute_relation()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_tool_authorize_context(&state.connect_pool).await?;
        let resource_id = Uuid::new_v4().to_string();
        seed_resource_relation(
            &state.connect_pool,
            context.tenant_id,
            "tool",
            &resource_id,
            "runner",
            context.user_id,
        )
        .await?;

        let Json(response) = tool_call_authorize(
            State(state.clone()),
            Json(tool_authorize_request(
                &context,
                "deploy_site",
                "high",
                ResourceRef {
                    resource_type: "tool".to_string(),
                    id: resource_id.clone(),
                    path: None,
                },
            )),
        )
        .await?;

        assert_eq!(response.decision.decision, "review");
        assert_eq!(
            response.decision.reason_code.as_deref(),
            Some("risk_requires_review")
        );
        let approval_id = response.approval_id.expect("approval id");
        let interrupt_id = response.interrupt_id.expect("interrupt id");

        let tool_call = load_tool_call(&state.connect_pool, response.tool_call_id).await?;
        assert_eq!(
            tool_call.try_get::<String, _>("status")?,
            "waiting_approval"
        );
        assert_eq!(tool_call.try_get::<String, _>("decision")?, "review");
        assert_eq!(tool_call.try_get::<String, _>("resource_type")?, "tool");
        assert_eq!(tool_call.try_get::<String, _>("resource_id")?, resource_id);
        assert_eq!(tool_call.try_get::<String, _>("risk_level")?, "high");
        assert_eq!(
            run_status(&state.connect_pool, context.run_id).await?,
            "waiting_approval"
        );

        let approval_status: String =
            sqlx::query_scalar("SELECT status FROM approvals WHERE id = $1")
                .bind(approval_id)
                .fetch_one(&state.connect_pool)
                .await?;
        assert_eq!(approval_status, "pending");
        let interrupt_status: String =
            sqlx::query_scalar("SELECT status FROM interrupts WHERE id = $1")
                .bind(interrupt_id)
                .fetch_one(&state.connect_pool)
                .await?;
        assert_eq!(interrupt_status, "open");
        let approval_event_type: String = sqlx::query_scalar(
            "SELECT type FROM run_events WHERE conversation_id = $1 AND type = 'approval.requested' ORDER BY seq DESC LIMIT 1",
        )
        .bind(context.conversation_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(approval_event_type, "approval.requested");
        assert_latest_authz_decision(
            &state.connect_pool,
            context.tenant_id,
            response.tool_call_id,
            "review",
            "risk_requires_review",
        )
        .await?;

        let Json(decided) = decide_approval(
            State(state.clone()),
            Extension(PlatformRequestContext {
                tenant_id: context.tenant_id,
                platform_user_id: context.user_id,
                ferriskey_subject: "tool-authz-subject".to_string(),
                preferred_username: Some("tool-authz-user".to_string()),
                email: None,
                roles: vec!["tenant_admin".to_string()],
                session_id: context.session_id,
                device_id: context.device_id,
                trace_id: "trace-tool-approval".to_string(),
                token_jti: None,
                token_exp: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
            }),
            Path(approval_id),
            Json(ApprovalDecisionRequest {
                tenant_id: context.tenant_id,
                decision: "approved".to_string(),
                reason: Some("integration test".to_string()),
                payload: None,
            }),
        )
        .await?;
        assert_eq!(decided.status, "approved");
        let resolved_interrupt_status: String =
            sqlx::query_scalar("SELECT status FROM interrupts WHERE id = $1")
                .bind(interrupt_id)
                .fetch_one(&state.connect_pool)
                .await?;
        assert_eq!(resolved_interrupt_status, "resolved");
        let Json(reused) = tool_call_authorize(
            State(state.clone()),
            Json(tool_authorize_request(
                &context,
                "deploy_site",
                "high",
                ResourceRef {
                    resource_type: "tool".to_string(),
                    id: resource_id,
                    path: None,
                },
            )),
        )
        .await?;
        assert_eq!(reused.decision.decision, "allow");
        assert_eq!(reused.tool_call_id, response.tool_call_id);
        assert_eq!(reused.approval_id, Some(approval_id));
        assert_eq!(
            load_tool_call(&state.connect_pool, response.tool_call_id)
                .await?
                .try_get::<String, _>("status")?,
            "authorized"
        );
        let approval_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM approvals WHERE run_id = $1")
                .bind(context.run_id)
                .fetch_one(&state.connect_pool)
                .await?;
        assert_eq!(approval_count, 1);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    fn tool_authorize_request(
        context: &ToolAuthorizeTestContext,
        tool_name: &str,
        risk_level: &str,
        resource: ResourceRef,
    ) -> ToolAuthorizeRequest {
        ToolAuthorizeRequest {
            tenant_id: context.tenant_id,
            actor: ActorRef {
                user_id: context.user_id,
                device_id: Some(context.device_id),
                session_id: Some(context.session_id),
                roles: Vec::new(),
            },
            conversation_id: Some(context.conversation_id),
            run_id: Some(context.run_id),
            trace_id: Some("trace-tool-authz".to_string()),
            tool_id: None,
            tool_name: tool_name.to_string(),
            resource: Some(resource),
            args_hash: Some("args-hash".to_string()),
            risk_level: Some(risk_level.to_string()),
            input_summary: Some("{\"args\":[]}".to_string()),
        }
    }

    async fn seed_tool_authorize_context(
        pool: &PgPool,
    ) -> Result<ToolAuthorizeTestContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Tool Authz Test', $2)")
            .bind(tenant_id)
            .bind(format!("tool-authz-test-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, 'tool-authz-subject', 'tool-authz-user', 'active')
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
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
                id, tenant_id, user_id, device_fingerprint, device_name, platform, trust_level
            )
            VALUES ($1, $2, $3, 'tool-authz-device', 'Tool Authz Device', 'oidc', 'standard')
            "#,
        )
        .bind(device_id)
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_sessions (
                id, tenant_id, user_id, device_id, ferriskey_subject, ferriskey_session_state,
                token_exp, roles_snapshot, token_hash
            )
            VALUES (
                $1, $2, $3, $4, 'tool-authz-subject', 'tool-authz-session',
                CURRENT_TIMESTAMP + INTERVAL '1 hour', $5, 'token-hash'
            )
            "#,
        )
        .bind(session_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(device_id)
        .bind(json!(["tenant_member"]))
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversations (id, tenant_id, created_by_user_id, title)
            VALUES ($1, $2, $3, 'Tool authz conversation')
            "#,
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO runs (
                id, tenant_id, conversation_id, created_by_user_id, status,
                input, run_config_snapshot, trace_id
            )
            VALUES ($1, $2, $3, $4, 'running', '{}'::jsonb, '{}'::jsonb, 'trace-tool-authz')
            "#,
        )
        .bind(run_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(user_id)
        .execute(pool)
        .await?;

        Ok(ToolAuthorizeTestContext {
            tenant_id,
            user_id,
            device_id,
            session_id,
            conversation_id,
            run_id,
        })
    }

    async fn seed_resource_relation(
        pool: &PgPool,
        tenant_id: Uuid,
        resource_type: &str,
        resource_id: &str,
        relation: &str,
        user_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO resource_relations (
                tenant_id, resource_type, resource_id, relation, subject_type, subject_id
            )
            VALUES ($1, $2, $3, $4, 'user', $5)
            "#,
        )
        .bind(tenant_id)
        .bind(resource_type)
        .bind(resource_id)
        .bind(relation)
        .bind(user_id.to_string())
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn load_tool_call(
        pool: &PgPool,
        tool_call_id: Uuid,
    ) -> Result<sqlx::postgres::PgRow, sqlx::Error> {
        sqlx::query(
            r#"
            SELECT status, decision, resource_type, resource_id, risk_level
            FROM tool_calls
            WHERE id = $1
            "#,
        )
        .bind(tool_call_id)
        .fetch_one(pool)
        .await
    }

    async fn count_approvals(pool: &PgPool, tenant_id: Uuid) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM approvals WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
    }

    async fn count_interrupts(pool: &PgPool, tenant_id: Uuid) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM interrupts WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(pool)
            .await
    }

    async fn run_status(pool: &PgPool, run_id: Uuid) -> Result<String, sqlx::Error> {
        sqlx::query_scalar("SELECT status FROM runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(pool)
            .await
    }

    async fn assert_latest_authz_decision(
        pool: &PgPool,
        tenant_id: Uuid,
        tool_call_id: Uuid,
        decision: &str,
        reason_code: &str,
    ) -> Result<(), sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT decision, reason_code, tool_call_id
            FROM audit_logs
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .fetch_one(pool)
        .await?;
        assert_eq!(row.try_get::<String, _>("decision")?, decision);
        assert_eq!(row.try_get::<String, _>("reason_code")?, reason_code);
        assert_eq!(row.try_get::<Uuid, _>("tool_call_id")?, tool_call_id);
        Ok(())
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
