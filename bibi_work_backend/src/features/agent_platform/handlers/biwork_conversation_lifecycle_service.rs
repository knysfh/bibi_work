use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext, models::AuthzContext, run_snapshot,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    agent_catalog_service::latest_published_agent_version_id,
    biwork_compat_service::ok,
    biwork_conversation_projection::{conversation_from_row, conversations_from_rows},
    biwork_conversation_service::{
        ConversationAudit, biwork_cancel_conversation,
        biwork_cron_job_id_from_conversation_metadata, conversation_audit_summary,
        write_conversation_audit,
    },
    biwork_conversation_support::ensure_conversation_exists,
    biwork_cron_service::associated_cron_conversations,
    biwork_event_support::emit_conversation_list_changed_event,
    support::require_ferriskey_allow,
};

#[derive(Debug, Deserialize)]
pub struct CompatListQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

pub async fn biwork_create_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("New conversation");
    let extra = payload.get("extra").cloned().unwrap_or_else(|| json!({}));
    let agent_id = payload
        .pointer("/assistant/id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    if let Some(agent_id) = agent_id {
        require_ferriskey_allow(
            &state,
            &ctx,
            ctx.tenant_id,
            "run",
            "agent",
            agent_id.to_string(),
            Some(AuthzContext {
                agent_id: Some(agent_id),
                ..Default::default()
            }),
        )
        .await?;
    }
    let agent_version_id =
        latest_published_agent_version_id(&state.connect_pool, ctx.tenant_id, agent_id).await?;
    let selected_model_reference = payload
        .pointer("/assistant/conversation_overrides/model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let selected_model_profile_id = if let Some(reference) = selected_model_reference {
        Some(
            run_snapshot::resolve_active_model_profile_reference(
                &state.connect_pool,
                ctx.tenant_id,
                reference,
            )
            .await?,
        )
    } else {
        None
    };
    validate_conversation_assistant_model(
        &state,
        ctx.tenant_id,
        agent_version_id,
        selected_model_profile_id,
    )
    .await?;
    let metadata = json!({
        "biwork": {
            "type": payload.get("type").and_then(Value::as_str).unwrap_or("acp"),
            "assistant": payload.get("assistant").cloned().unwrap_or(Value::Null),
            "agent_version_id": agent_version_id,
            "model_profile_id": selected_model_profile_id,
            "model": payload.get("model").cloned().unwrap_or(Value::Null),
        },
        "extra": extra,
    });

    let row = sqlx::query(
        r#"
        INSERT INTO conversations (
            tenant_id, created_by_user_id, agent_id, title, metadata
        )
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, title, status, metadata, workspace_id, project_id, agent_id,
                  created_at, updated_at
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(agent_id)
    .bind(name)
    .bind(metadata)
    .fetch_one(&state.connect_pool)
    .await?;

    let conversation_id: Uuid = row.try_get("id")?;
    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "create",
            decision: "allow",
            reason_code: Some("conversation.create"),
            run_id: None,
            output_summary: Some(conversation_audit_summary(&[("title", name)])),
        },
    )
    .await?;
    emit_conversation_list_changed_event(
        &state,
        &ctx,
        conversation_id,
        "created",
        "conversation.create",
    )
    .await?;

    Ok(ok(conversation_from_row(&state, ctx.tenant_id, &row).await?))
}

async fn validate_conversation_assistant_model(
    state: &AppState,
    tenant_id: Uuid,
    agent_version_id: Option<Uuid>,
    selected_model_profile_id: Option<Uuid>,
) -> Result<(), AppError> {
    if selected_model_profile_id.is_some() {
        return Ok(());
    }
    let Some(agent_version_id) = agent_version_id else {
        return Ok(());
    };
    let snapshot: Value = sqlx::query_scalar(
        r#"
        SELECT config_snapshot
        FROM agent_versions
        WHERE id = $1 AND tenant_id = $2 AND status = 'published'
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant version not found".to_string()))?;
    if snapshot
        .pointer("/defaults/model/mode")
        .and_then(Value::as_str)
        == Some("auto")
    {
        return Err(AppError::InvalidInput(
            "assistant uses automatic model selection; select a model in New Chat".to_string(),
        ));
    }
    let profile_id = snapshot
        .get("model_profile_id")
        .or_else(|| snapshot.pointer("/agent/model_profile_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            AppError::InvalidInput(
                "assistant has no fixed model; select a model in New Chat or edit the assistant"
                    .to_string(),
            )
        })?;
    run_snapshot::resolve_active_model_profile_reference(&state.connect_pool, tenant_id, profile_id)
        .await
        .map(|_| ())
        .map_err(|_| {
            AppError::InvalidInput(
            "assistant fixed model is no longer active; select another model or edit the assistant"
                .to_string(),
        )
        })
}

pub async fn biwork_clone_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let conversation = payload.get("conversation").unwrap_or(&payload);
    let name = conversation
        .get("name")
        .or_else(|| conversation.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Cloned conversation");
    let agent_id = conversation
        .pointer("/assistant/id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok());
    if let Some(agent_id) = agent_id {
        require_ferriskey_allow(
            &state,
            &ctx,
            ctx.tenant_id,
            "run",
            "agent",
            agent_id.to_string(),
            Some(AuthzContext {
                agent_id: Some(agent_id),
                ..Default::default()
            }),
        )
        .await?;
    }
    let agent_version_id =
        latest_published_agent_version_id(&state.connect_pool, ctx.tenant_id, agent_id).await?;
    let extra = conversation
        .get("extra")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let metadata = json!({
        "biwork": {
            "type": conversation
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("acp"),
            "assistant": conversation.get("assistant").cloned().unwrap_or(Value::Null),
            "agent_version_id": agent_version_id,
            "model": conversation.get("model").cloned().unwrap_or(Value::Null),
            "cloned_from": conversation.get("id").cloned().unwrap_or(Value::Null),
        },
        "extra": extra,
    });

    let row = sqlx::query(
        r#"
        INSERT INTO conversations (
            tenant_id, created_by_user_id, agent_id, title, metadata
        )
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, title, status, metadata, workspace_id, project_id, agent_id,
                  created_at, updated_at
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(agent_id)
    .bind(name)
    .bind(metadata)
    .fetch_one(&state.connect_pool)
    .await?;

    let conversation_id: Uuid = row.try_get("id")?;
    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "clone",
            decision: "allow",
            reason_code: Some("conversation.clone"),
            run_id: None,
            output_summary: Some(conversation_audit_summary(&[("title", name)])),
        },
    )
    .await?;
    emit_conversation_list_changed_event(
        &state,
        &ctx,
        conversation_id,
        "created",
        "conversation.clone",
    )
    .await?;

    Ok(ok(conversation_from_row(&state, ctx.tenant_id, &row).await?))
}

pub async fn biwork_list_conversations(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<CompatListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query
        .cursor
        .as_deref()
        .and_then(|cursor| cursor.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let rows = sqlx::query(
        r#"
        SELECT id, title, status, metadata, workspace_id, project_id, agent_id, created_at, updated_at
        FROM conversations
        WHERE tenant_id = $1 AND created_by_user_id = $2 AND deleted_at IS NULL
        ORDER BY updated_at DESC, created_at DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(limit + 1)
    .bind(offset)
    .fetch_all(&state.connect_pool)
    .await?;

    let has_more = rows.len() as i64 > limit;
    let items = conversations_from_rows(
        &state,
        ctx.tenant_id,
        rows.into_iter().take(limit as usize).collect(),
    )
    .await?;

    Ok(ok(json!({
        "items": items,
        "total": offset + i64::try_from(items.len()).unwrap_or(0),
        "has_more": has_more,
        "next_cursor": if has_more { json!((offset + limit).to_string()) } else { Value::Null },
    })))
}

pub async fn biwork_get_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, title, status, metadata, workspace_id, project_id, agent_id, created_at, updated_at
        FROM conversations
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;

    Ok(ok(conversation_from_row(&state, ctx.tenant_id, &row).await?))
}

pub async fn biwork_update_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let updates = if let Some(updates) = payload.get("updates").and_then(Value::as_object) {
        updates
    } else {
        payload.as_object().ok_or_else(|| {
            AppError::InvalidInput("conversation update must be an object".to_string())
        })?
    };
    let title = updates
        .get("name")
        .or_else(|| updates.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let extra = updates.get("extra").cloned().filter(Value::is_object);
    let merge_extra = payload
        .get("merge_extra")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let result = sqlx::query(
        r#"
        UPDATE conversations
        SET title = COALESCE($3, title),
            metadata = CASE
                WHEN $5::jsonb IS NULL THEN metadata
                WHEN $6 THEN jsonb_set(
                    metadata,
                    '{extra}',
                    COALESCE(metadata->'extra', '{}'::jsonb) || $5::jsonb,
                    true
                )
                ELSE jsonb_set(metadata, '{extra}', $5::jsonb, true)
            END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND created_by_user_id = $4 AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .bind(title)
    .bind(ctx.platform_user_id)
    .bind(extra)
    .bind(merge_extra)
    .execute(&state.connect_pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("conversation not found".to_string()));
    }

    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "update",
            decision: "allow",
            reason_code: Some("conversation.update"),
            run_id: None,
            output_summary: Some(conversation_audit_summary(&[(
                "title",
                title.unwrap_or(""),
            )])),
        },
    )
    .await?;
    emit_conversation_list_changed_event(
        &state,
        &ctx,
        conversation_id,
        "updated",
        "conversation.update",
    )
    .await?;

    Ok(ok(json!(true)))
}

pub async fn biwork_delete_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let owned_conversation_exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM conversations
            WHERE id = $1
              AND tenant_id = $2
              AND created_by_user_id = $3
              AND deleted_at IS NULL
        )
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if !owned_conversation_exists {
        return Err(AppError::NotFound("conversation not found".to_string()));
    }

    let _ = biwork_cancel_conversation(
        State(state.clone()),
        Extension(ctx.clone()),
        Path(conversation_id),
        Json(json!({})),
    )
    .await?;

    let result = sqlx::query(
        r#"
        UPDATE conversations
        SET deleted_at = CURRENT_TIMESTAMP,
            status = 'archived',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND created_by_user_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("conversation not found".to_string()));
    }

    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "delete",
            decision: "allow",
            reason_code: Some("conversation.delete"),
            run_id: None,
            output_summary: None,
        },
    )
    .await?;
    emit_conversation_list_changed_event(
        &state,
        &ctx,
        conversation_id,
        "deleted",
        "conversation.delete",
    )
    .await?;

    Ok(ok(json!(true)))
}

pub async fn biwork_reset_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'cancelled',
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status IN ('queued', 'pending', 'running', 'waiting_approval')
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        DELETE FROM run_events
        WHERE tenant_id = $1 AND conversation_id = $2
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE conversations
        SET updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    write_conversation_audit(
        &state,
        &ctx,
        ConversationAudit {
            conversation_id,
            action: "reset",
            decision: "allow",
            reason_code: Some("conversation.reset"),
            run_id: None,
            output_summary: None,
        },
    )
    .await?;
    emit_conversation_list_changed_event(
        &state,
        &ctx,
        conversation_id,
        "updated",
        "conversation.reset",
    )
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_conversation_associated(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, metadata
        FROM conversations
        WHERE id = $1
          AND tenant_id = $2
          AND created_by_user_id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;
    let metadata: Value = row.try_get("metadata")?;

    let job_id = if let Some(job_id) = biwork_cron_job_id_from_conversation_metadata(&metadata) {
        Some(job_id)
    } else {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT id
            FROM scheduled_jobs
            WHERE tenant_id = $1
              AND created_by_user_id = $2
              AND deleted_at IS NULL
              AND (
                source_conversation_id = $3
                OR target_conversation_id = $3
              )
            ORDER BY updated_at DESC
            LIMIT 1
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(ctx.platform_user_id)
        .bind(conversation_id)
        .fetch_optional(&state.connect_pool)
        .await?
    };

    let Some(job_id) = job_id else {
        return Ok(ok(json!([])));
    };
    let rows = associated_cron_conversations(&state, &ctx, job_id).await?;

    let conversations = conversations_from_rows(&state, ctx.tenant_id, rows).await?;
    Ok(ok(Value::Array(conversations)))
}

pub async fn biwork_active_conversation_count(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM conversations
        WHERE tenant_id = $1
          AND created_by_user_id = $2
          AND deleted_at IS NULL
          AND status = 'active'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(ok(json!({ "count": count })))
}
