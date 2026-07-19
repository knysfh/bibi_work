use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::{self, NewAuditLog},
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::{CreateRunRequest, RunEventInput},
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    agent_catalog_service::latest_published_agent_version_id,
    biwork_compat_service::{epoch_ms, ok, required_string, trimmed_string, value_string},
    biwork_conversation_projection::conversations_from_rows,
    biwork_conversation_support::ensure_conversation_exists,
    run_service::create_and_dispatch_conversation_run,
};

#[derive(Debug, Deserialize)]
pub struct CronJobsQuery {
    conversation_id: Option<String>,
}

pub async fn biwork_list_cron_jobs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<CronJobsQuery>,
) -> Result<Json<Value>, AppError> {
    let conversation_id = query
        .conversation_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok());
    let rows = sqlx::query(
        r#"
        SELECT *
        FROM scheduled_jobs
        WHERE tenant_id = $1
          AND created_by_user_id = $2
          AND deleted_at IS NULL
          AND (
              $3::uuid IS NULL
              OR source_conversation_id = $3
              OR target_conversation_id = $3
          )
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(conversation_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let jobs = rows
        .iter()
        .map(cron_job_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ok(Value::Array(jobs)))
}

pub async fn biwork_get_cron_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = load_cron_job(&state, &ctx, job_id).await?;
    Ok(ok(cron_job_from_row(&row)?))
}

pub async fn biwork_create_cron_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = trimmed_string(&payload, "name").unwrap_or_else(|| "Scheduled task".to_string());
    let description = trimmed_string(&payload, "description");
    let created_from = payload
        .get("created_by")
        .and_then(Value::as_str)
        .unwrap_or("user")
        .to_string();
    let (schedule_kind, schedule_expr, timezone, schedule_description) =
        schedule_parts(payload.get("schedule"))?;
    let next_run_at = initial_cron_next_run_at(&schedule_kind, &schedule_expr)?;
    let prompt_template = cron_prompt_template(&payload)?;
    let target_mode = payload
        .get("execution_mode")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .pointer("/target/execution_mode")
                .and_then(Value::as_str)
        })
        .unwrap_or("existing")
        .to_string();
    let source_conversation_id = payload
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let agent_snapshot = payload
        .get("agent_config")
        .cloned()
        .or_else(|| payload.pointer("/metadata/agent_config").cloned())
        .unwrap_or_else(|| json!({}));
    let assistant_profile_id = agent_snapshot
        .get("assistant_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let model_profile_id = agent_snapshot
        .get("model_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .or_else(|| {
            agent_snapshot
                .pointer("/model/id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
        });
    let metadata = json!({
        "conversation_title": payload.get("conversation_title").cloned().unwrap_or(Value::Null),
        "schedule_description": schedule_description,
        "original_payload": payload,
    });

    let row = sqlx::query(
        r#"
        INSERT INTO scheduled_jobs (
            tenant_id, name, source_conversation_id, target_mode, target_conversation_id,
            assistant_profile_id, agent_snapshot, prompt_template, model_profile_id,
            schedule_kind, schedule_expr, timezone, enabled, created_by_user_id,
            created_from, description, metadata, max_retries, next_run_at
        )
        VALUES (
            $1, $2, $3, $4, $3, $5, $6, $7, $8, $9, $10, $11, TRUE,
            $12, $13, $14, $15, 3, $16
        )
        RETURNING *
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(name)
    .bind(source_conversation_id)
    .bind(target_mode)
    .bind(assistant_profile_id)
    .bind(agent_snapshot)
    .bind(prompt_template)
    .bind(model_profile_id)
    .bind(schedule_kind)
    .bind(schedule_expr)
    .bind(timezone)
    .bind(ctx.platform_user_id)
    .bind(created_from)
    .bind(description)
    .bind(metadata)
    .bind(next_run_at)
    .fetch_one(&state.connect_pool)
    .await?;

    let job_id: Uuid = row.try_get("id")?;
    write_cron_audit(
        &state,
        &ctx,
        CronAudit {
            job_id,
            action: "create",
            decision: "allow",
            reason_code: Some("cron.create"),
            conversation_id: None,
            run_id: None,
            output_summary: Some(cron_audit_summary(
                Some("cron.create"),
                None,
                None,
                next_run_at,
            )),
        },
    )
    .await?;

    let job = cron_job_from_row(&row)?;
    emit_cron_ws_event(
        &state,
        &ctx,
        cron_event_conversation_id_from_row(&row)?,
        None,
        "cron.job-created",
        cron_job_ws_payload(&job),
    )
    .await?;

    Ok(ok(job))
}

pub async fn biwork_update_cron_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let schedule_value = payload.get("schedule");
    let schedule_supplied = schedule_value.is_some_and(|value| !value.is_null());
    let schedule = optional_schedule_parts(schedule_value)?;
    let next_run_at = if schedule_supplied {
        let kind = schedule
            .kind
            .as_deref()
            .ok_or_else(|| AppError::InvalidInput("cron schedule kind is required".to_string()))?;
        let expr = schedule.expr.as_deref().ok_or_else(|| {
            AppError::InvalidInput("cron schedule expression is required".to_string())
        })?;
        initial_cron_next_run_at(kind, expr)?
    } else {
        None
    };
    let agent_snapshot = payload
        .get("agent_config")
        .cloned()
        .or_else(|| payload.pointer("/metadata/agent_config").cloned());
    let assistant_profile_id = agent_snapshot
        .as_ref()
        .and_then(|value| value.get("assistant_id"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let model_profile_id = agent_snapshot
        .as_ref()
        .and_then(|value| value.get("model_id"))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let metadata_patch = json!({
        "conversation_title": payload.get("conversation_title").cloned().unwrap_or(Value::Null),
        "schedule_description": schedule.description.clone().map(Value::String).unwrap_or(Value::Null),
        "last_update_payload": payload,
    });

    let row = sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            enabled = COALESCE($5, enabled),
            schedule_kind = COALESCE($6, schedule_kind),
            schedule_expr = COALESCE($7, schedule_expr),
            timezone = COALESCE($8, timezone),
            prompt_template = COALESCE($9, prompt_template),
            target_mode = COALESCE($10, target_mode),
            agent_snapshot = COALESCE($11, agent_snapshot),
            assistant_profile_id = COALESCE($12, assistant_profile_id),
            model_profile_id = COALESCE($13, model_profile_id),
            max_retries = COALESCE($14, max_retries),
            metadata = metadata || $15,
            next_run_at = CASE WHEN $17 THEN $18::timestamptz ELSE next_run_at END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND created_by_user_id = $16
          AND deleted_at IS NULL
        RETURNING *
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(trimmed_string(&payload, "name"))
    .bind(trimmed_string(&payload, "description"))
    .bind(payload.get("enabled").and_then(Value::as_bool))
    .bind(schedule.kind)
    .bind(schedule.expr)
    .bind(schedule.tz)
    .bind(optional_cron_prompt_template(&payload))
    .bind(
        payload
            .get("execution_mode")
            .and_then(Value::as_str)
            .or_else(|| {
                payload
                    .pointer("/target/execution_mode")
                    .and_then(Value::as_str)
            })
            .map(str::to_string),
    )
    .bind(agent_snapshot)
    .bind(assistant_profile_id)
    .bind(model_profile_id)
    .bind(payload.get("max_retries").and_then(Value::as_i64))
    .bind(metadata_patch)
    .bind(ctx.platform_user_id)
    .bind(schedule_supplied)
    .bind(next_run_at)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("cron job not found".to_string()))?;

    write_cron_audit(
        &state,
        &ctx,
        CronAudit {
            job_id,
            action: "update",
            decision: "allow",
            reason_code: Some("cron.update"),
            conversation_id: None,
            run_id: None,
            output_summary: Some(cron_audit_summary(
                Some("cron.update"),
                None,
                None,
                next_run_at,
            )),
        },
    )
    .await?;

    let job = cron_job_from_row(&row)?;
    emit_cron_ws_event(
        &state,
        &ctx,
        cron_event_conversation_id_from_row(&row)?,
        None,
        "cron.job-updated",
        cron_job_ws_payload(&job),
    )
    .await?;

    Ok(ok(job))
}

pub async fn biwork_delete_cron_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET deleted_at = CURRENT_TIMESTAMP,
            enabled = FALSE,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND created_by_user_id = $3
          AND deleted_at IS NULL
        RETURNING source_conversation_id, target_conversation_id
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?;
    let row = row.ok_or_else(|| AppError::NotFound("cron job not found".to_string()))?;

    write_cron_audit(
        &state,
        &ctx,
        CronAudit {
            job_id,
            action: "delete",
            decision: "allow",
            reason_code: Some("cron.delete"),
            conversation_id: None,
            run_id: None,
            output_summary: None,
        },
    )
    .await?;

    emit_cron_ws_event(
        &state,
        &ctx,
        cron_event_conversation_id_from_row(&row)?,
        None,
        "cron.job-removed",
        json!({ "job_id": job_id.to_string() }),
    )
    .await?;

    Ok(ok(Value::Null))
}

pub async fn biwork_run_cron_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let job = load_cron_job(&state, &ctx, job_id).await?;
    let dispatched = match dispatch_cron_job(&state, &ctx, &job, "cron.manual", None).await {
        Ok(dispatched) => dispatched,
        Err(error) => {
            let message = error.to_string();
            mark_cron_job_run_failure(&state, ctx.tenant_id, job_id, "cron.manual", &message)
                .await?;
            write_cron_audit(
                &state,
                &ctx,
                CronAudit {
                    job_id,
                    action: "run",
                    decision: "error",
                    reason_code: Some("cron.manual"),
                    conversation_id: None,
                    run_id: None,
                    output_summary: Some(cron_failure_audit_summary("cron.manual", &message)),
                },
            )
            .await?;
            emit_cron_ws_event(
                &state,
                &ctx,
                cron_event_conversation_id_from_row(&job)?,
                None,
                "cron.job-executed",
                json!({
                    "job_id": job_id.to_string(),
                    "cron_job_id": job_id.to_string(),
                    "status": "error",
                    "error": message,
                }),
            )
            .await?;
            return Err(error);
        }
    };
    mark_cron_job_run_success(&state, ctx.tenant_id, job_id, false, None, false).await?;
    write_cron_audit(
        &state,
        &ctx,
        CronAudit {
            job_id,
            action: "run",
            decision: "allow",
            reason_code: Some("cron.manual"),
            conversation_id: Some(dispatched.conversation_id),
            run_id: Some(dispatched.run_id),
            output_summary: Some(cron_audit_summary(
                Some("cron.manual"),
                Some(dispatched.conversation_id),
                Some(dispatched.run_id),
                None,
            )),
        },
    )
    .await?;

    emit_cron_ws_event(
        &state,
        &ctx,
        Some(dispatched.conversation_id),
        Some(dispatched.run_id),
        "cron.job-executed",
        json!({
            "job_id": job_id.to_string(),
            "cron_job_id": job_id.to_string(),
            "cron_job_name": dispatched.job_name,
            "status": "ok",
            "conversation_id": dispatched.conversation_id.to_string(),
            "run_id": dispatched.run_id.to_string(),
            "triggered_at": epoch_ms(dispatched.triggered_at),
        }),
    )
    .await?;

    Ok(ok(json!({
        "conversation_id": dispatched.conversation_id.to_string(),
        "run_id": dispatched.run_id.to_string(),
    })))
}

pub async fn biwork_cron_system_resume(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let checked_at = OffsetDateTime::now_utc();
    let rows = sqlx::query(
        r#"
        SELECT *
        FROM scheduled_jobs
        WHERE tenant_id = $1
          AND created_by_user_id = $2
          AND enabled IS TRUE
          AND deleted_at IS NULL
          AND next_run_at IS NOT NULL
          AND next_run_at <= CURRENT_TIMESTAMP
        ORDER BY next_run_at ASC, created_at ASC
        LIMIT 20
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut dispatched = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();

    for row in rows {
        let job_id: Uuid = row.try_get("id")?;
        let job_name: String = row.try_get("name")?;
        let schedule_kind: String = row.try_get("schedule_kind")?;
        let schedule_expr: String = row.try_get("schedule_expr")?;
        let due_at: OffsetDateTime = row.try_get("next_run_at")?;

        let Some(next_state) =
            next_cron_state_after_resume(&schedule_kind, &schedule_expr, due_at, checked_at)?
        else {
            skipped.push(json!({
                "job_id": job_id.to_string(),
                "name": job_name,
                "schedule_kind": schedule_kind,
                "reason": "schedule_kind_requires_scheduler",
            }));
            continue;
        };

        let idempotency_key = format!("cron:{job_id}:resume:{}", epoch_ms(due_at));
        match dispatch_cron_job(
            &state,
            &ctx,
            &row,
            "cron.system_resume",
            Some(idempotency_key),
        )
        .await
        {
            Ok(result) => {
                mark_cron_job_run_success(
                    &state,
                    ctx.tenant_id,
                    job_id,
                    next_state.disable_job,
                    next_state.next_run_at,
                    true,
                )
                .await?;
                write_cron_audit(
                    &state,
                    &ctx,
                    CronAudit {
                        job_id,
                        action: "run",
                        decision: "allow",
                        reason_code: Some("cron.system_resume"),
                        conversation_id: Some(result.conversation_id),
                        run_id: Some(result.run_id),
                        output_summary: Some(cron_audit_summary(
                            Some("cron.system_resume"),
                            Some(result.conversation_id),
                            Some(result.run_id),
                            next_state.next_run_at,
                        )),
                    },
                )
                .await?;
                emit_cron_ws_event(
                    &state,
                    &ctx,
                    Some(result.conversation_id),
                    Some(result.run_id),
                    "cron.job-executed",
                    json!({
                        "job_id": job_id.to_string(),
                        "cron_job_id": job_id.to_string(),
                        "cron_job_name": result.job_name,
                        "status": "ok",
                        "conversation_id": result.conversation_id.to_string(),
                        "run_id": result.run_id.to_string(),
                        "triggered_at": epoch_ms(result.triggered_at),
                    }),
                )
                .await?;
                dispatched.push(json!({
                    "job_id": job_id.to_string(),
                    "name": job_name,
                    "conversation_id": result.conversation_id.to_string(),
                    "run_id": result.run_id.to_string(),
                    "next_run_at_ms": next_state.next_run_at.map(epoch_ms),
                    "disabled": next_state.disable_job,
                }));
            }
            Err(error) => {
                let message = error.to_string();
                mark_cron_job_run_failure(
                    &state,
                    ctx.tenant_id,
                    job_id,
                    "cron.system_resume",
                    &message,
                )
                .await?;
                write_cron_audit(
                    &state,
                    &ctx,
                    CronAudit {
                        job_id,
                        action: "run",
                        decision: "error",
                        reason_code: Some("cron.system_resume"),
                        conversation_id: None,
                        run_id: None,
                        output_summary: Some(format!(
                            "trigger=cron.system_resume; error={message}"
                        )),
                    },
                )
                .await?;
                emit_cron_ws_event(
                    &state,
                    &ctx,
                    cron_event_conversation_id_from_row(&row)?,
                    None,
                    "cron.job-executed",
                    json!({
                        "job_id": job_id.to_string(),
                        "cron_job_id": job_id.to_string(),
                        "cron_job_name": job_name,
                        "status": "error",
                        "error": message,
                    }),
                )
                .await?;
                failed.push(json!({
                    "job_id": job_id.to_string(),
                    "name": job_name,
                    "error": message,
                }));
            }
        }
    }

    Ok(ok(json!({
        "checked_at": epoch_ms(checked_at),
        "source": payload.get("source").cloned().unwrap_or(Value::Null),
        "dispatched": dispatched,
        "skipped": skipped,
        "failed": failed,
    })))
}

pub async fn biwork_list_cron_job_conversations(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let rows = associated_cron_conversations(&state, &ctx, job_id).await?;

    let conversations = conversations_from_rows(&state, ctx.tenant_id, rows).await?;
    Ok(ok(Value::Array(conversations)))
}

pub(super) async fn associated_cron_conversations(
    state: &AppState,
    ctx: &PlatformRequestContext,
    job_id: Uuid,
) -> Result<Vec<sqlx::postgres::PgRow>, AppError> {
    let _ = load_cron_job(state, ctx, job_id).await?;
    associated_conversations_for_cron_job(state, ctx, job_id).await
}

async fn associated_conversations_for_cron_job(
    state: &AppState,
    ctx: &PlatformRequestContext,
    job_id: Uuid,
) -> Result<Vec<sqlx::postgres::PgRow>, AppError> {
    Ok(sqlx::query(
        r#"
        WITH job AS (
          SELECT id, source_conversation_id, target_conversation_id
          FROM scheduled_jobs
          WHERE id = $3
            AND tenant_id = $1
            AND created_by_user_id = $2
            AND deleted_at IS NULL
        )
        SELECT DISTINCT c.id,
               c.title,
               c.status,
               c.metadata,
               c.workspace_id,
               c.project_id,
               c.agent_id,
               c.created_at,
               c.updated_at
        FROM conversations c
        JOIN job j ON TRUE
        WHERE c.tenant_id = $1
          AND c.created_by_user_id = $2
          AND c.deleted_at IS NULL
          AND (
            c.id = j.source_conversation_id
            OR c.id = j.target_conversation_id
            OR c.metadata #>> '{extra,cron_job_id}' = j.id::text
            OR c.metadata #>> '{extra,cronJobId}' = j.id::text
          )
        ORDER BY c.updated_at DESC
        LIMIT 100
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(job_id)
    .fetch_all(&state.connect_pool)
    .await?)
}

pub async fn biwork_save_cron_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let content = required_string(&payload, "content")?;
    let result = sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET skill_content = $4,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND created_by_user_id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(content)
    .execute(&state.connect_pool)
    .await?;
    ensure_cron_skill_job_updated(result.rows_affected())?;
    sqlx::query(
        r#"
        UPDATE scheduled_job_artifacts
        SET status = 'saved',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND scheduled_job_id = $2
          AND artifact_kind = 'skill_suggest'
          AND status = 'pending'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(job_id)
    .execute(&state.connect_pool)
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_get_cron_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let has_skill: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM scheduled_jobs
            WHERE id = $1
              AND tenant_id = $2
              AND created_by_user_id = $3
              AND deleted_at IS NULL
              AND COALESCE(NULLIF(skill_content, ''), '') <> ''
        )
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_one(&state.connect_pool)
    .await?;
    Ok(ok(json!({ "has_skill": has_skill })))
}

pub async fn biwork_delete_cron_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET skill_content = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND created_by_user_id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;
    ensure_cron_skill_job_updated(result.rows_affected())?;
    Ok(ok(Value::Null))
}

fn optional_cron_prompt_template(payload: &Value) -> Option<String> {
    payload
        .get("prompt")
        .and_then(Value::as_str)
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .or_else(|| {
            payload
                .pointer("/target/payload/text")
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn cron_prompt_template(payload: &Value) -> Result<String, AppError> {
    optional_cron_prompt_template(payload)
        .ok_or_else(|| AppError::InvalidInput("cron message is required".to_string()))
}

fn ensure_cron_skill_job_updated(rows_affected: u64) -> Result<(), AppError> {
    if rows_affected == 0 {
        return Err(AppError::NotFound("cron job not found".to_string()));
    }
    Ok(())
}

struct OptionalScheduleParts {
    kind: Option<String>,
    expr: Option<String>,
    tz: Option<String>,
    description: Option<String>,
}

fn schedule_parts(
    schedule: Option<&Value>,
) -> Result<(String, String, Option<String>, String), AppError> {
    let Some(schedule) = schedule.and_then(Value::as_object) else {
        return Ok((
            "cron".to_string(),
            "0 9 * * *".to_string(),
            None,
            "Every day at 09:00".to_string(),
        ));
    };
    let kind = schedule
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("cron")
        .to_string();
    let expr = match kind.as_str() {
        "at" => schedule
            .get("atMs")
            .and_then(Value::as_i64)
            .map(|value| value.to_string())
            .or_else(|| {
                schedule
                    .get("at")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        "every" => schedule
            .get("everyMs")
            .and_then(Value::as_i64)
            .map(|value| value.to_string()),
        _ => Some(
            schedule
                .get("expr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
    }
    .filter(|value| kind == "cron" || !value.trim().is_empty())
    .ok_or_else(|| AppError::InvalidInput("cron schedule expression is required".to_string()))?;
    let timezone = schedule
        .get("tz")
        .and_then(Value::as_str)
        .map(str::to_string);
    let description = schedule
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| expr.clone());
    Ok((kind, expr, timezone, description))
}

fn optional_schedule_parts(schedule: Option<&Value>) -> Result<OptionalScheduleParts, AppError> {
    if schedule.is_none() || schedule.is_some_and(Value::is_null) {
        return Ok(OptionalScheduleParts {
            kind: None,
            expr: None,
            tz: None,
            description: None,
        });
    }
    let (kind, expr, tz, description) = schedule_parts(schedule)?;
    Ok(OptionalScheduleParts {
        kind: Some(kind),
        expr: Some(expr),
        tz,
        description: Some(description),
    })
}

fn cron_schedule_json(kind: &str, expr: &str, tz: Option<String>, metadata: &Value) -> Value {
    let description =
        value_string(metadata, "schedule_description").unwrap_or_else(|| expr.to_string());
    match kind {
        "at" => json!({
            "kind": "at",
            "atMs": expr.parse::<i64>().unwrap_or(0),
            "description": description,
        }),
        "every" => json!({
            "kind": "every",
            "everyMs": expr.parse::<i64>().unwrap_or(0),
            "description": description,
        }),
        _ => json!({
            "kind": "cron",
            "expr": expr,
            "tz": tz,
            "description": description,
        }),
    }
}

fn cron_job_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let id: Uuid = row.try_get("id")?;
    let source_conversation_id: Option<Uuid> = row.try_get("source_conversation_id")?;
    let target_conversation_id: Option<Uuid> = row.try_get("target_conversation_id")?;
    let metadata: Value = row.try_get("metadata")?;
    let agent_snapshot: Value = row.try_get("agent_snapshot")?;
    let schedule_kind: String = row.try_get("schedule_kind")?;
    let schedule_expr: String = row.try_get("schedule_expr")?;
    let timezone: Option<String> = row.try_get("timezone")?;
    let next_run_at: Option<OffsetDateTime> = row.try_get("next_run_at")?;
    let last_run_at: Option<OffsetDateTime> = row.try_get("last_run_at")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    let created_from: String = row.try_get("created_from")?;
    let target_mode: String = row.try_get("target_mode")?;
    let conversation_id = target_conversation_id.or(source_conversation_id);

    Ok(json!({
        "id": id.to_string(),
        "job_id": id.to_string(),
        "name": row.try_get::<String, _>("name")?,
        "description": row.try_get::<Option<String>, _>("description")?,
        "enabled": row.try_get::<bool, _>("enabled")?,
        "schedule": cron_schedule_json(&schedule_kind, &schedule_expr, timezone, &metadata),
        "target": {
            "payload": {
                "kind": "message",
                "text": row.try_get::<String, _>("prompt_template")?,
            },
            "execution_mode": target_mode,
        },
        "metadata": {
            "conversation_id": conversation_id.map(|id| id.to_string()).unwrap_or_default(),
            "conversation_title": metadata.get("conversation_title").cloned().unwrap_or(Value::Null),
            "agent_type": value_string(&agent_snapshot, "agent_type")
                .or_else(|| value_string(&agent_snapshot, "mode"))
                .unwrap_or_else(|| "acp".to_string()),
            "created_by": created_from,
            "created_at": epoch_ms(created_at),
            "updated_at": epoch_ms(updated_at),
            "agent_config": agent_snapshot,
        },
        "state": {
            "next_run_at_ms": next_run_at.map(epoch_ms),
            "last_run_at_ms": last_run_at.map(epoch_ms),
            "last_status": row.try_get::<Option<String>, _>("last_status")?,
            "last_error": row.try_get::<Option<String>, _>("last_error")?,
            "run_count": row.try_get::<i32, _>("run_count")?,
            "retry_count": row.try_get::<i32, _>("retry_count")?,
            "max_retries": row.try_get::<i32, _>("max_retries")?,
        },
    }))
}

fn cron_job_ws_payload(job: &Value) -> Value {
    let mut payload = job.clone();
    if let Some(object) = payload.as_object_mut()
        && !object.contains_key("job_id")
        && let Some(id) = object.get("id").cloned()
    {
        object.insert("job_id".to_string(), id);
    }
    payload
}

struct CronDispatchResult {
    conversation_id: Uuid,
    run_id: Uuid,
    job_name: String,
    triggered_at: OffsetDateTime,
}

struct CronNextState {
    next_run_at: Option<OffsetDateTime>,
    disable_job: bool,
}

async fn dispatch_cron_job(
    state: &AppState,
    ctx: &PlatformRequestContext,
    job: &sqlx::postgres::PgRow,
    trigger: &str,
    idempotency_key: Option<String>,
) -> Result<CronDispatchResult, AppError> {
    let job_id: Uuid = job.try_get("id")?;
    let conversation_id = ensure_cron_target_conversation(state, ctx, job).await?;
    let prompt_template: String = job.try_get("prompt_template")?;
    let agent_id: Option<Uuid> = job.try_get("assistant_profile_id")?;
    let agent_snapshot: Value = job.try_get("agent_snapshot")?;
    let agent_version_id =
        resolve_cron_agent_version_id(state, ctx.tenant_id, agent_id, &agent_snapshot).await?;
    let model_profile_id: Option<Uuid> = job.try_get("model_profile_id")?;
    let job_name: String = job.try_get("name")?;

    let run = create_and_dispatch_conversation_run(
        state,
        ctx,
        conversation_id,
        CreateRunRequest {
            tenant_id: ctx.tenant_id,
            agent_id,
            agent_version_id,
            project_id: None,
            input: Some(json!({
                "messages": [
                    { "role": "user", "content": prompt_template }
                ],
                "biwork": {
                    "client": "biwork",
                    "trigger": trigger,
                    "cron_job_id": job_id.to_string(),
                    "cron_job_name": job_name.clone(),
                },
            })),
            run_config_snapshot: Some(json!({
                "runtime": { "kind": "deepagents" },
                "model_profile_id": model_profile_id,
                "ui": { "client": "biwork", "conversation_type": "acp" },
                "cron": {
                    "job_id": job_id.to_string(),
                    "job_name": job_name.clone(),
                    "agent_snapshot": agent_snapshot,
                },
            })),
            idempotency_key,
            thread_id: Some(conversation_id.to_string()),
        },
    )
    .await?;

    sqlx::query(
        r#"
        INSERT INTO scheduled_job_runs (tenant_id, scheduled_job_id, run_id, status, summary)
        VALUES ($1, $2, $3, 'queued', $4)
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(job_id)
    .bind(run.id)
    .bind(json!({
        "conversation_id": conversation_id.to_string(),
        "trigger": trigger,
    }))
    .execute(&state.connect_pool)
    .await?;

    insert_cron_trigger_artifact(
        state,
        ctx.tenant_id,
        job_id,
        &job_name,
        conversation_id,
        run.id,
        run.queued_at,
    )
    .await?;

    Ok(CronDispatchResult {
        conversation_id,
        run_id: run.id,
        job_name,
        triggered_at: run.queued_at,
    })
}

async fn resolve_cron_agent_version_id(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Option<Uuid>,
    agent_snapshot: &Value,
) -> Result<Option<Uuid>, AppError> {
    if let Some(agent_version_id) = cron_agent_snapshot_version_id(agent_snapshot)? {
        return Ok(Some(agent_version_id));
    }
    let Some(agent_id) = agent_id else {
        return Ok(None);
    };

    latest_published_agent_version_id(&state.connect_pool, tenant_id, Some(agent_id)).await
}

fn cron_agent_snapshot_version_id(agent_snapshot: &Value) -> Result<Option<Uuid>, AppError> {
    for path in [
        "/agent_version_id",
        "/agentVersionId",
        "/version_id",
        "/versionId",
        "/agent/agent_version_id",
        "/agent/agentVersionId",
        "/agent/version_id",
        "/agent/versionId",
    ] {
        let Some(value) = agent_snapshot.pointer(path) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        let Some(text) = value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Err(AppError::InvalidInput(
                "cron agent_version_id must be a UUID".to_string(),
            ));
        };
        return Uuid::parse_str(text).map(Some).map_err(|_| {
            AppError::InvalidInput("cron agent_version_id must be a UUID".to_string())
        });
    }
    Ok(None)
}

async fn insert_cron_trigger_artifact(
    state: &AppState,
    tenant_id: Uuid,
    job_id: Uuid,
    job_name: &str,
    conversation_id: Uuid,
    run_id: Uuid,
    triggered_at: OffsetDateTime,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO scheduled_job_artifacts (
            id, tenant_id, scheduled_job_id, conversation_id, artifact_key,
            artifact_kind, status, payload, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, 'cron_trigger', 'active', $6, $7, CURRENT_TIMESTAMP)
        ON CONFLICT (id)
        DO UPDATE
        SET conversation_id = EXCLUDED.conversation_id,
            payload = EXCLUDED.payload,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(run_id)
    .bind(tenant_id)
    .bind(job_id)
    .bind(conversation_id)
    .bind(format!("cron_trigger.{run_id}"))
    .bind(cron_trigger_artifact_payload(
        job_id,
        job_name,
        triggered_at,
    ))
    .bind(triggered_at)
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

async fn mark_cron_job_run_success(
    state: &AppState,
    tenant_id: Uuid,
    job_id: Uuid,
    disable_job: bool,
    next_run_at: Option<OffsetDateTime>,
    update_next_run_at: bool,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET last_run_at = CURRENT_TIMESTAMP,
            last_status = 'ok',
            last_error = NULL,
            run_count = run_count + 1,
            retry_count = 0,
            enabled = CASE WHEN $3 THEN FALSE ELSE enabled END,
            next_run_at = CASE WHEN $5 THEN $4::timestamptz ELSE next_run_at END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(job_id)
    .bind(tenant_id)
    .bind(disable_job)
    .bind(next_run_at)
    .bind(update_next_run_at)
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

async fn mark_cron_job_run_failure(
    state: &AppState,
    tenant_id: Uuid,
    job_id: Uuid,
    trigger: &str,
    error: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET last_run_at = CURRENT_TIMESTAMP,
            last_status = 'error',
            last_error = $3,
            retry_count = retry_count + 1,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(job_id)
    .bind(tenant_id)
    .bind(error)
    .execute(&state.connect_pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO scheduled_job_runs (tenant_id, scheduled_job_id, status, summary)
        VALUES ($1, $2, 'failed', $3)
        "#,
    )
    .bind(tenant_id)
    .bind(job_id)
    .bind(cron_failure_run_summary(trigger, error))
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

fn cron_event_conversation_id_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<Option<Uuid>, AppError> {
    let target_conversation_id: Option<Uuid> = row.try_get("target_conversation_id")?;
    let source_conversation_id: Option<Uuid> = row.try_get("source_conversation_id")?;
    Ok(target_conversation_id.or(source_conversation_id))
}

async fn emit_cron_ws_event(
    state: &AppState,
    ctx: &PlatformRequestContext,
    conversation_id: Option<Uuid>,
    run_id: Option<Uuid>,
    event_type: &str,
    payload: Value,
) -> Result<(), AppError> {
    let Some(conversation_id) = conversation_id else {
        return Ok(());
    };
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        ctx.tenant_id,
        conversation_id,
        run_id,
        RunEventInput {
            event_id: Some(format!("{event_type}.{}", Uuid::new_v4())),
            event_type: event_type.to_string(),
            payload: Some(payload),
            trace_id: Some(ctx.trace_id.clone()),
        },
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(state, &event).await;
    Ok(())
}

struct CronAudit<'a> {
    job_id: Uuid,
    action: &'a str,
    decision: &'a str,
    reason_code: Option<&'a str>,
    conversation_id: Option<Uuid>,
    run_id: Option<Uuid>,
    output_summary: Option<String>,
}

async fn write_cron_audit(
    state: &AppState,
    ctx: &PlatformRequestContext,
    entry: CronAudit<'_>,
) -> Result<(), AppError> {
    let CronAudit {
        job_id,
        action,
        decision,
        reason_code,
        conversation_id,
        run_id,
        output_summary,
    } = entry;
    let mut tx = state.connect_pool.begin().await?;
    let resource_id = job_id.to_string();
    audit::insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: ctx.tenant_id,
            actor_user_id: Some(ctx.platform_user_id),
            actor_device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            resource_type: "scheduled_job",
            resource_id: &resource_id,
            action,
            decision,
            policy_version: "biwork-cron-v1",
            reason_code,
            run_id,
            conversation_id,
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

fn cron_audit_summary(
    trigger: Option<&str>,
    conversation_id: Option<Uuid>,
    run_id: Option<Uuid>,
    next_run_at: Option<OffsetDateTime>,
) -> String {
    let mut parts = Vec::new();
    if let Some(trigger) = trigger {
        parts.push(format!("trigger={trigger}"));
    }
    if let Some(conversation_id) = conversation_id {
        parts.push(format!("conversation_id={conversation_id}"));
    }
    if let Some(run_id) = run_id {
        parts.push(format!("run_id={run_id}"));
    }
    if let Some(next_run_at) = next_run_at {
        parts.push(format!("next_run_at_ms={}", epoch_ms(next_run_at)));
    }
    parts.join("; ")
}

fn cron_failure_run_summary(trigger: &str, error: &str) -> Value {
    json!({
        "trigger": trigger,
        "error": error,
    })
}

fn cron_failure_audit_summary(trigger: &str, error: &str) -> String {
    let mut parts = Vec::new();
    if !trigger.trim().is_empty() {
        parts.push(format!("trigger={trigger}"));
    }
    if !error.trim().is_empty() {
        parts.push(format!("error={error}"));
    }
    parts.join("; ")
}

fn initial_cron_next_run_at(
    schedule_kind: &str,
    schedule_expr: &str,
) -> Result<Option<OffsetDateTime>, AppError> {
    match schedule_kind {
        "at" => Ok(Some(offset_datetime_from_epoch_ms(parse_epoch_ms(
            schedule_expr,
        )?)?)),
        "every" => {
            let interval_ms = parse_positive_interval_ms(schedule_expr)?;
            offset_datetime_from_epoch_ms(
                epoch_ms(OffsetDateTime::now_utc()).saturating_add(interval_ms),
            )
            .map(Some)
        }
        _ => Ok(None),
    }
}

fn next_cron_state_after_resume(
    schedule_kind: &str,
    schedule_expr: &str,
    due_at: OffsetDateTime,
    checked_at: OffsetDateTime,
) -> Result<Option<CronNextState>, AppError> {
    match schedule_kind {
        "at" => Ok(Some(CronNextState {
            next_run_at: None,
            disable_job: true,
        })),
        "every" => {
            let interval_ms = parse_positive_interval_ms(schedule_expr)?;
            let due_ms = epoch_ms(due_at);
            let checked_ms = epoch_ms(checked_at);
            let elapsed_ms = checked_ms.saturating_sub(due_ms);
            let steps = elapsed_ms / interval_ms + 1;
            let next_ms = due_ms.saturating_add(steps.saturating_mul(interval_ms));
            Ok(Some(CronNextState {
                next_run_at: Some(offset_datetime_from_epoch_ms(next_ms)?),
                disable_job: false,
            }))
        }
        _ => Ok(None),
    }
}

fn parse_epoch_ms(value: &str) -> Result<i64, AppError> {
    value.trim().parse::<i64>().map_err(|_| {
        AppError::InvalidInput("cron at schedule must use epoch milliseconds".to_string())
    })
}

fn parse_positive_interval_ms(value: &str) -> Result<i64, AppError> {
    let interval_ms = value
        .trim()
        .parse::<i64>()
        .map_err(|_| AppError::InvalidInput("cron interval must use milliseconds".to_string()))?;
    if interval_ms <= 0 {
        return Err(AppError::InvalidInput(
            "cron interval must be positive".to_string(),
        ));
    }
    Ok(interval_ms)
}

fn offset_datetime_from_epoch_ms(epoch_ms_value: i64) -> Result<OffsetDateTime, AppError> {
    let seconds = epoch_ms_value.div_euclid(1_000);
    let milliseconds = epoch_ms_value.rem_euclid(1_000);
    OffsetDateTime::from_unix_timestamp(seconds)
        .map(|value| value + Duration::milliseconds(milliseconds))
        .map_err(|_| AppError::InvalidInput("cron timestamp is out of range".to_string()))
}

async fn load_cron_job(
    state: &AppState,
    ctx: &PlatformRequestContext,
    job_id: Uuid,
) -> Result<sqlx::postgres::PgRow, AppError> {
    sqlx::query(
        r#"
        SELECT *
        FROM scheduled_jobs
        WHERE id = $1
          AND tenant_id = $2
          AND created_by_user_id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("cron job not found".to_string()))
}

async fn ensure_cron_target_conversation(
    state: &AppState,
    ctx: &PlatformRequestContext,
    job: &sqlx::postgres::PgRow,
) -> Result<Uuid, AppError> {
    let target_mode: String = job.try_get("target_mode")?;
    let target_conversation_id: Option<Uuid> = job.try_get("target_conversation_id")?;
    let source_conversation_id: Option<Uuid> = job.try_get("source_conversation_id")?;
    if target_mode == "existing"
        && let Some(conversation_id) = target_conversation_id.or(source_conversation_id)
    {
        ensure_conversation_exists(state, ctx.tenant_id, conversation_id).await?;
        return Ok(conversation_id);
    }

    let job_id: Uuid = job.try_get("id")?;
    let job_name: String = job.try_get("name")?;
    let agent_id: Option<Uuid> = job.try_get("assistant_profile_id")?;
    let agent_snapshot: Value = job.try_get("agent_snapshot")?;
    let row = sqlx::query(
        r#"
        INSERT INTO conversations (
            tenant_id, created_by_user_id, agent_id, title, metadata
        )
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(agent_id)
    .bind(format!("[Scheduled] {job_name}"))
    .bind(json!({
        "biwork": {
            "type": "acp",
            "assistant": agent_snapshot,
        },
        "extra": {
            "cron_job_id": job_id.to_string(),
            "cron_job_name": job_name,
        },
    }))
    .fetch_one(&state.connect_pool)
    .await?;
    let conversation_id: Uuid = row.try_get("id")?;
    sqlx::query(
        r#"
        UPDATE scheduled_jobs
        SET target_conversation_id = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(job_id)
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .execute(&state.connect_pool)
    .await?;
    Ok(conversation_id)
}

pub(super) fn cron_trigger_artifact_payload(
    cron_job_id: Uuid,
    cron_job_name: &str,
    triggered_at: OffsetDateTime,
) -> Value {
    json!({
        "cron_job_id": cron_job_id.to_string(),
        "cron_job_name": cron_job_name,
        "triggered_at": epoch_ms(triggered_at),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_resume_disables_one_shot_at_job() {
        let due_at = offset_datetime_from_epoch_ms(1_000).expect("valid timestamp");
        let checked_at = offset_datetime_from_epoch_ms(2_000).expect("valid timestamp");

        let state = next_cron_state_after_resume("at", "1000", due_at, checked_at)
            .expect("valid at schedule")
            .expect("at schedules are resumable");

        assert!(state.disable_job);
        assert_eq!(state.next_run_at, None);
    }

    #[test]
    fn cron_resume_advances_every_job_past_checked_time() {
        let due_at = offset_datetime_from_epoch_ms(10_000).expect("valid timestamp");
        let checked_at = offset_datetime_from_epoch_ms(15_000).expect("valid timestamp");

        let state = next_cron_state_after_resume("every", "3000", due_at, checked_at)
            .expect("valid every schedule")
            .expect("every schedules are resumable");

        assert!(!state.disable_job);
        assert_eq!(state.next_run_at.map(epoch_ms), Some(16_000));
    }

    #[test]
    fn cron_resume_skips_unparsed_cron_expression() {
        let due_at = offset_datetime_from_epoch_ms(10_000).expect("valid timestamp");
        let checked_at = offset_datetime_from_epoch_ms(15_000).expect("valid timestamp");

        let state = next_cron_state_after_resume("cron", "0 9 * * *", due_at, checked_at)
            .expect("cron expression is intentionally delegated");

        assert!(state.is_none());
    }

    #[test]
    fn cron_audit_summary_uses_stable_non_secret_fields() {
        let conversation_id = Uuid::parse_str("00000000-0000-0000-0000-000000000101").unwrap();
        let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000202").unwrap();
        let next_run_at = offset_datetime_from_epoch_ms(1234).expect("valid timestamp");

        let summary = cron_audit_summary(
            Some("cron.system_resume"),
            Some(conversation_id),
            Some(run_id),
            Some(next_run_at),
        );

        assert_eq!(
            summary,
            "trigger=cron.system_resume; conversation_id=00000000-0000-0000-0000-000000000101; run_id=00000000-0000-0000-0000-000000000202; next_run_at_ms=1234"
        );
    }

    #[test]
    fn cron_failure_summaries_include_manual_trigger_and_error() {
        let run_summary = cron_failure_run_summary("cron.manual", "runtime unavailable");
        assert_eq!(run_summary["trigger"], "cron.manual");
        assert_eq!(run_summary["error"], "runtime unavailable");

        let audit_summary = cron_failure_audit_summary("cron.manual", "runtime unavailable");
        assert_eq!(
            audit_summary,
            "trigger=cron.manual; error=runtime unavailable"
        );
    }

    #[test]
    fn cron_schedule_accepts_empty_expression_as_manual_only() {
        let (kind, expr, timezone, description) = schedule_parts(Some(&json!({
            "kind": "cron",
            "expr": "",
            "tz": "Asia/Shanghai",
            "description": "Manual"
        })))
        .unwrap();

        assert_eq!(kind, "cron");
        assert_eq!(expr, "");
        assert_eq!(timezone.as_deref(), Some("Asia/Shanghai"));
        assert_eq!(description, "Manual");
    }

    #[test]
    fn cron_agent_snapshot_version_id_accepts_aliases_and_rejects_invalid_values() {
        let agent_version_id = Uuid::new_v4();
        assert_eq!(
            cron_agent_snapshot_version_id(&json!({
                "agent": {
                    "agentVersionId": agent_version_id.to_string(),
                },
            }))
            .unwrap(),
            Some(agent_version_id)
        );
        assert_eq!(
            cron_agent_snapshot_version_id(&json!({ "agent_version_id": null })).unwrap(),
            None
        );
        assert!(matches!(
            cron_agent_snapshot_version_id(&json!({ "agent_version_id": "not-a-uuid" })),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn cron_prompt_template_accepts_biwork_create_payload_shapes() {
        assert_eq!(
            cron_prompt_template(&json!({ "prompt": " Say hello " })).unwrap(),
            "Say hello"
        );
        assert_eq!(
            cron_prompt_template(&json!({ "message": "Daily summary" })).unwrap(),
            "Daily summary"
        );
        assert_eq!(
            cron_prompt_template(&json!({
                "target": {
                    "payload": {
                        "kind": "message",
                        "text": "Check inbox"
                    }
                }
            }))
            .unwrap(),
            "Check inbox"
        );
        assert!(matches!(
            cron_prompt_template(&json!({ "prompt": "   " })),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn cron_skill_save_requires_non_empty_content_and_existing_job() {
        assert_eq!(
            required_string(&json!({ "content": "  skill body  " }), "content").unwrap(),
            "skill body"
        );
        assert!(matches!(
            required_string(&json!({ "content": "   " }), "content"),
            Err(AppError::InvalidInput(_))
        ));
        assert!(ensure_cron_skill_job_updated(1).is_ok());
        assert!(matches!(
            ensure_cron_skill_job_updated(0),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn cron_job_ws_payload_is_direct_job_contract() {
        let payload = cron_job_ws_payload(&json!({
            "id": "cron_001",
            "name": "Daily summary",
            "metadata": {
                "conversation_id": "conv_001",
            },
        }));

        assert_eq!(payload["id"], "cron_001");
        assert_eq!(payload["job_id"], "cron_001");
        assert_eq!(payload["name"], "Daily summary");
        assert!(payload.get("job").is_none());
    }
}
