use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use futures_util::future::join_all;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext,
            models::{ActorRef, AuthzCheckRequest, AuthzContext, ResourceRef},
        },
        core::errors::AppError,
    },
    startup::AppState,
};

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

use super::{
    biwork_agent_support::{
        biwork_agent_type, biwork_assistant_runtime_disabled_reason, normalize_biwork_agent_source,
        runtime_kind,
    },
    biwork_compat_service::{epoch_ms, ok, required_string, trimmed_string, value_string},
    support::require_ferriskey_allow,
};

#[derive(Debug, Deserialize, Default)]
pub struct AssistantDetailQuery {
    locale: Option<String>,
}

pub async fn biwork_list_assistants(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let model_tags = list_model_tags(&state, ctx.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.runtime_id, a.name, a.description, a.status, a.metadata,
               a.created_at, a.updated_at, runtime.runtime_kind,
               runtime.source AS runtime_source, runtime.metadata AS runtime_metadata,
               runtime.status AS runtime_status,
               COALESCE((
                   SELECT av.config_snapshot
                   FROM assistant_versions av
                   WHERE av.assistant_id = a.id AND av.tenant_id = a.tenant_id
                   ORDER BY (av.status = 'published') DESC, av.created_at DESC
                   LIMIT 1
               ), a.draft_config) AS config
        FROM assistants a
        JOIN agent_runtimes runtime
          ON runtime.id = a.runtime_id AND runtime.tenant_id = a.tenant_id
        WHERE a.tenant_id = $1 AND a.deleted_at IS NULL
        ORDER BY a.updated_at DESC, a.created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let decisions = join_all(rows.iter().map(|row| async {
        let agent_id = row.try_get::<Uuid, _>("id")?;
        Ok::<bool, sqlx::Error>(
            state
                .authz_service
                .check(&biwork_agent_run_authz_request(&ctx, agent_id))
                .await
                .is_allow(),
        )
    }))
    .await;
    let assistants = rows
        .into_iter()
        .zip(decisions)
        .filter_map(|(row, decision)| match decision {
            Ok(true) => Some(assistant_from_row(&row, &model_tags)),
            Ok(false) => None,
            Err(error) => Some(Err(error.into())),
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(Value::Array(assistants)))
}

pub(super) fn biwork_agent_run_authz_request(
    ctx: &PlatformRequestContext,
    agent_id: Uuid,
) -> AuthzCheckRequest {
    AuthzCheckRequest {
        tenant_id: ctx.tenant_id,
        actor: ActorRef {
            user_id: ctx.platform_user_id,
            device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            roles: ctx.roles.clone(),
        },
        action: "run".to_string(),
        resource: ResourceRef {
            resource_type: "agent".to_string(),
            id: agent_id.to_string(),
            path: None,
        },
        context: Some(AuthzContext {
            agent_id: Some(agent_id),
            trace_id: Some(ctx.trace_id.clone()),
            ..Default::default()
        }),
    }
}

pub async fn biwork_create_assistant(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = required_string(&payload, "name")?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "create",
        "agent",
        name.clone(),
        None,
    )
    .await?;

    let requested_id = trimmed_string(&payload, "id");
    if let Some(existing_ref) = requested_id.as_deref()
        && let Some(existing_id) =
            try_resolve_biwork_assistant_id(&state, ctx.tenant_id, existing_ref).await?
    {
        let assistant =
            update_biwork_assistant_record(&state, ctx.tenant_id, existing_id, &payload).await?;
        return Ok(ok(assistant));
    }

    let assistant_id = requested_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .unwrap_or_else(Uuid::new_v4);
    let status = if payload
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        "active"
    } else {
        "disabled"
    };

    let mut tx = state.connect_pool.begin().await?;
    let runtime_id =
        resolve_assistant_runtime_id(&mut tx, ctx.tenant_id, requested_runtime_id(&payload)?)
            .await?;
    let (mut draft_config, metadata) =
        biwork_assistant_documents(runtime_id, None, None, &payload, true)?;
    normalize_biwork_assistant_model_profile(&mut tx, ctx.tenant_id, &mut draft_config).await?;
    sqlx::query(
        r#"
        INSERT INTO assistants (
            id, tenant_id, owner_user_id, runtime_id, name, description,
            draft_config, metadata, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(assistant_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(runtime_id)
    .bind(name)
    .bind(
        payload
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
    .bind(&draft_config)
    .bind(&metadata)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    publish_biwork_assistant_version(&mut tx, ctx.tenant_id, assistant_id, &draft_config).await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    let model_tags = list_model_tags(&state, ctx.tenant_id).await?;
    let row = load_assistant_response_row(&state.connect_pool, ctx.tenant_id, assistant_id).await?;
    Ok(ok(assistant_from_row(&row, &model_tags)?))
}

pub async fn biwork_import_assistants(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let assistants = payload
        .get("assistants")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("assistants is required".to_string()))?;
    let mut imported = 0_i64;
    let mut skipped = 0_i64;
    let mut errors = Vec::new();

    for assistant in assistants {
        let Some(name) = trimmed_string(assistant, "name") else {
            skipped += 1;
            errors.push(json!({
                "id": trimmed_string(assistant, "id").unwrap_or_default(),
                "error": "name is required",
            }));
            continue;
        };
        require_ferriskey_allow(&state, &ctx, ctx.tenant_id, "create", "agent", name, None).await?;
        let requested_id = trimmed_string(assistant, "id");
        if let Some(existing_ref) = requested_id.as_deref()
            && let Some(existing_id) =
                try_resolve_biwork_assistant_id(&state, ctx.tenant_id, existing_ref).await?
        {
            update_biwork_assistant_record(&state, ctx.tenant_id, existing_id, assistant).await?;
            imported += 1;
            continue;
        }

        let assistant_id = requested_id
            .as_deref()
            .and_then(|id| Uuid::parse_str(id).ok())
            .unwrap_or_else(Uuid::new_v4);
        let mut tx = state.connect_pool.begin().await?;
        let runtime_id =
            resolve_assistant_runtime_id(&mut tx, ctx.tenant_id, requested_runtime_id(assistant)?)
                .await?;
        let (mut draft_config, metadata) =
            biwork_assistant_documents(runtime_id, None, None, assistant, true)?;
        normalize_biwork_assistant_model_profile(&mut tx, ctx.tenant_id, &mut draft_config).await?;
        sqlx::query(
            r#"
            INSERT INTO assistants (
                id, tenant_id, owner_user_id, runtime_id, name, description,
                draft_config, metadata, status
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active')
            "#,
        )
        .bind(assistant_id)
        .bind(ctx.tenant_id)
        .bind(ctx.platform_user_id)
        .bind(runtime_id)
        .bind(required_string(assistant, "name")?)
        .bind(
            assistant
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .bind(&draft_config)
        .bind(&metadata)
        .execute(&mut *tx)
        .await?;
        publish_biwork_assistant_version(&mut tx, ctx.tenant_id, assistant_id, &draft_config)
            .await?;
        tx.commit()
            .await
            .map_err(|_| AppError::DatabaseTransaction)?;
        imported += 1;
    }

    Ok(ok(json!({
        "imported": imported,
        "skipped": skipped,
        "failed": errors.len(),
        "errors": errors,
    })))
}

pub async fn biwork_get_assistant(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<AssistantDetailQuery>,
    Path(assistant_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_id).await?;
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
    let model_tags = list_model_tags(&state, ctx.tenant_id).await?;
    let row = sqlx::query(
        r#"
        SELECT a.id, a.runtime_id, a.name, a.description, a.status, a.metadata,
               a.created_at, a.updated_at, runtime.runtime_kind,
               runtime.source AS runtime_source, runtime.metadata AS runtime_metadata,
               runtime.status AS runtime_status,
               COALESCE((
                   SELECT av.config_snapshot
                   FROM assistant_versions av
                   WHERE av.assistant_id = a.id AND av.tenant_id = a.tenant_id
                   ORDER BY (av.status = 'published') DESC, av.created_at DESC
                   LIMIT 1
               ), a.draft_config) AS config
        FROM assistants a
        JOIN agent_runtimes runtime
          ON runtime.id = a.runtime_id AND runtime.tenant_id = a.tenant_id
        WHERE a.id = $1 AND a.tenant_id = $2 AND a.deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))?;

    let config: Value = row.try_get("config")?;
    let assistant = assistant_from_row(&row, &model_tags)?;
    let rule_content = biwork_assistant_rule_content(&config, query.locale.as_deref());
    let id = string_field(&assistant, "id");
    let name = string_field(&assistant, "name");
    let description = assistant
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let avatar = assistant
        .get("avatar")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let agent = assistant.get("agent").cloned().unwrap_or_else(|| json!({}));
    let enabled = assistant
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let sort_order = assistant
        .get("sort_order")
        .and_then(Value::as_i64)
        .unwrap_or(100);
    let last_used_at = assistant
        .get("last_used_at")
        .cloned()
        .unwrap_or(Value::Null);
    let skill_ids = assistant
        .get("enabled_skills")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let custom_skill_names = assistant
        .get("custom_skill_names")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let disabled_builtin_skills = assistant
        .get("disabled_builtin_skills")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let defaults = assistant
        .get("defaults")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let preferences = assistant
        .get("preferences")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let runtime_id = assistant
        .get("runtime_id")
        .or_else(|| assistant.get("agent_id"))
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();

    Ok(ok(json!({
        "id": id,
        "source": assistant.get("source").cloned().unwrap_or_else(|| json!("builtin")),
        "agent_status": assistant.get("agent_status").cloned().unwrap_or_else(|| json!("online")),
        "team_selectable": true,
        "deletable": assistant.get("deletable").cloned().unwrap_or(json!(false)),
        "profile": {
            "name": name,
            "name_i18n": assistant.get("name_i18n").cloned().unwrap_or_else(|| json!({})),
            "description": description,
            "description_i18n": assistant.get("description_i18n").cloned().unwrap_or_else(|| json!({})),
            "avatar": avatar,
        },
        "state": {
            "enabled": enabled,
            "sort_order": sort_order,
            "last_used_at": last_used_at,
        },
        "engine": {
            "runtime_id": runtime_id,
            "agent_id": runtime_id,
            "agent": agent,
        },
        "rules": {
            "content": rule_content,
            "storage_mode": "inline",
        },
        "prompts": {
            "recommended": assistant.get("prompts").cloned().unwrap_or_else(|| json!([])),
            "recommended_i18n": assistant.get("prompts_i18n").cloned().unwrap_or_else(|| json!({})),
        },
        "defaults": defaults,
        "capabilities": {
            "default_skill_ids": skill_ids,
            "custom_skill_names": custom_skill_names,
            "default_disabled_builtin_skill_ids": disabled_builtin_skills,
        },
        "preferences": preferences,
    })))
}

pub async fn biwork_update_assistant(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(assistant_id): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;
    let assistant =
        update_biwork_assistant_record(&state, ctx.tenant_id, agent_id, &payload).await?;
    Ok(ok(assistant))
}

pub async fn biwork_set_assistant_state(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(assistant_id): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;
    let mut metadata: Value = sqlx::query_scalar(
        r#"
        SELECT metadata
        FROM assistants
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))?;
    let metadata_object = metadata.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("assistant metadata must be an object".to_string())
    })?;
    if let Some(sort_order) = payload.get("sort_order").and_then(Value::as_i64) {
        metadata_object.insert("sort_order".to_string(), json!(sort_order));
    }
    if let Some(last_used_at) = payload.get("last_used_at").and_then(Value::as_i64) {
        metadata_object.insert("last_used_at".to_string(), json!(last_used_at));
    }
    let status = payload
        .get("enabled")
        .and_then(Value::as_bool)
        .map(|enabled| {
            if enabled {
                "active".to_string()
            } else {
                "disabled".to_string()
            }
        });
    let result = sqlx::query(
        r#"
        UPDATE assistants
        SET status = COALESCE($3, status),
            metadata = $4,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(status)
    .bind(metadata)
    .execute(&state.connect_pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("assistant not found".to_string()));
    }
    let model_tags = list_model_tags(&state, ctx.tenant_id).await?;
    let row = load_assistant_response_row(&state.connect_pool, ctx.tenant_id, agent_id).await?;
    Ok(ok(assistant_from_row(&row, &model_tags)?))
}

pub async fn biwork_delete_assistant(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(assistant_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "delete",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;
    let deleted: Option<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE assistants
        SET status = 'deleted',
            deleted_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
          AND COALESCE(metadata->>'assistant_source', metadata->>'source', 'user') <> 'builtin'
        RETURNING id
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?;
    if deleted.is_some() {
        Ok(ok(Value::Null))
    } else {
        Err(AppError::NotFound("assistant not found".to_string()))
    }
}

pub async fn biwork_read_assistant_rule(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let assistant_ref = required_string(&payload, "assistant_id")?;
    let locale = trimmed_string(&payload, "locale");
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_ref).await?;
    let draft_config: Value = sqlx::query_scalar(
        r#"
        SELECT draft_config
        FROM assistants
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))?;

    Ok(ok(json!(biwork_assistant_rule_content(
        &draft_config,
        locale.as_deref()
    ))))
}

pub async fn biwork_write_assistant_rule(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let assistant_ref = required_string(&payload, "assistant_id")?;
    let content = payload
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("content is required".to_string()))?;
    let locale = trimmed_string(&payload, "locale");
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_ref).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;

    update_biwork_assistant_rule(
        &state,
        ctx.tenant_id,
        agent_id,
        locale.as_deref(),
        Some(content),
    )
    .await?;
    Ok(ok(json!(true)))
}

pub async fn biwork_delete_assistant_rule(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(assistant_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let agent_id = resolve_biwork_assistant_id(&state, ctx.tenant_id, &assistant_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;

    update_biwork_assistant_rule(&state, ctx.tenant_id, agent_id, None, None).await?;
    Ok(ok(json!(true)))
}

async fn list_model_tags(state: &AppState, tenant_id: Uuid) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT p.id AS provider_id, p.provider_key, mp.model_name
        FROM llm_model_profiles mp
        JOIN llm_providers p ON p.id = mp.provider_id
        WHERE mp.tenant_id = $1 AND mp.status = 'active' AND p.status = 'active'
        ORDER BY p.display_name, mp.profile_name
        LIMIT 200
        "#,
    )
    .bind(tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut tags = Vec::with_capacity(rows.len());
    for row in rows {
        let provider_id: Uuid = row.try_get("provider_id")?;
        let model_name: String = row.try_get("model_name")?;
        tags.push(format!("{provider_id}:{model_name}"));
    }
    Ok(tags)
}

fn requested_runtime_id(payload: &Value) -> Result<Option<Uuid>, AppError> {
    let Some(reference) =
        trimmed_string(payload, "runtime_id").or_else(|| trimmed_string(payload, "agent_id"))
    else {
        return Ok(None);
    };
    Uuid::parse_str(&reference)
        .map(Some)
        .map_err(|_| AppError::InvalidInput("runtime_id must be a uuid".to_string()))
}

async fn resolve_assistant_runtime_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    requested_runtime_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if let Some(runtime_id) = requested_runtime_id {
        return sqlx::query_scalar(
            r#"
            SELECT id
            FROM agent_runtimes
            WHERE id = $1
              AND tenant_id = $2
              AND status <> 'disabled'
              AND deleted_at IS NULL
            "#,
        )
        .bind(runtime_id)
        .bind(tenant_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            AppError::InvalidInput(
                "runtime_id does not reference an active execution runtime".to_string(),
            )
        });
    }

    sqlx::query_scalar(
        r#"
        INSERT INTO agent_runtimes (
            tenant_id, name, description, runtime_kind, source,
            draft_config, capabilities, metadata, status
        )
        VALUES (
            $1, 'BiWork Runtime', 'Built-in model-driven execution runtime',
            'deepagents', 'internal',
            '{"acp_backend":"deepagents","runtime":{"kind":"deepagents"}}'::jsonb,
            '{}'::jsonb, '{"builtin_runtime":true}'::jsonb, 'active'
        )
        ON CONFLICT (tenant_id, runtime_kind)
            WHERE metadata->>'builtin_runtime' = 'true' AND deleted_at IS NULL
        DO UPDATE SET updated_at = agent_runtimes.updated_at
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)
}

async fn load_assistant_response_row(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    assistant_id: Uuid,
) -> Result<sqlx::postgres::PgRow, AppError> {
    sqlx::query(
        r#"
        SELECT assistant.id, assistant.runtime_id, assistant.name,
               assistant.description, assistant.status, assistant.metadata,
               assistant.draft_config AS config, assistant.created_at,
               assistant.updated_at, runtime.runtime_kind,
               runtime.source AS runtime_source, runtime.metadata AS runtime_metadata,
               runtime.status AS runtime_status
        FROM assistants assistant
        JOIN agent_runtimes runtime
          ON runtime.id = assistant.runtime_id
         AND runtime.tenant_id = assistant.tenant_id
        WHERE assistant.id = $1
          AND assistant.tenant_id = $2
          AND assistant.deleted_at IS NULL
        "#,
    )
    .bind(assistant_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))
}

fn assistant_from_row(
    row: &sqlx::postgres::PgRow,
    model_tags: &[String],
) -> Result<Value, AppError> {
    let id: Uuid = row.try_get("id")?;
    let name: String = row.try_get("name")?;
    let description: Option<String> = row.try_get("description")?;
    let status: String = row.try_get("status")?;
    let metadata: Value = row.try_get("metadata")?;
    let config: Value = row.try_get("config")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    let runtime_id = row
        .try_get::<Uuid, _>("runtime_id")
        .ok()
        .or_else(|| {
            value_string(&config, "engine_agent_id").and_then(|value| Uuid::parse_str(&value).ok())
        })
        .unwrap_or(id);
    let runtime = row
        .try_get::<String, _>("runtime_kind")
        .ok()
        .unwrap_or_else(|| runtime_kind(&config, &metadata));
    let raw_source = value_string(&metadata, "assistant_source")
        .or_else(|| value_string(&metadata, "source"))
        .unwrap_or_else(|| "builtin".to_string());
    let source = normalize_biwork_assistant_source(&raw_source);
    let runtime_metadata = row
        .try_get::<Value, _>("runtime_metadata")
        .unwrap_or_else(|_| metadata.clone());
    let raw_agent_source = row
        .try_get::<String, _>("runtime_source")
        .ok()
        .or_else(|| value_string(&runtime_metadata, "source"));
    let agent_source = normalize_biwork_agent_source(raw_agent_source.as_deref());
    let agent_type = biwork_agent_type(&runtime, &runtime_metadata);
    let runtime_disabled_reason = biwork_assistant_runtime_disabled_reason(&runtime, &agent_type);
    let runtime_status_message = runtime_disabled_reason.clone();
    let runtime_runnable = runtime_disabled_reason.is_none();
    let runtime_active = row
        .try_get::<String, _>("runtime_status")
        .map(|status| status != "disabled")
        .unwrap_or(true);
    let selectable = status != "disabled" && runtime_active && runtime_runnable;
    let avatar = value_string(&metadata, "avatar").unwrap_or_default();
    let name_i18n = metadata
        .get("name_i18n")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let description_i18n = metadata
        .get("description_i18n")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let prompts = config
        .get("prompts")
        .or_else(|| config.get("recommended_prompts"))
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let prompts_i18n = config
        .get("prompts_i18n")
        .or_else(|| config.get("recommended_prompts_i18n"))
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let enabled_skills = config
        .get("skills")
        .or_else(|| config.get("enabled_skills"))
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let custom_skill_names = config
        .get("custom_skill_names")
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let disabled_builtin_skills = config
        .get("disabled_builtin_skills")
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let models = config
        .get("models")
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!(model_tags));
    let defaults = biwork_assistant_defaults(&config, enabled_skills.clone());
    let preferences = biwork_assistant_preferences(&config);
    let last_used_at = metadata
        .get("last_used_at")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| epoch_ms(updated_at));
    let deletable = source == "user";

    Ok(json!({
        "id": id.to_string(),
        "source": source,
        "name": name,
        "name_i18n": name_i18n,
        "description": description.unwrap_or_default(),
        "description_i18n": description_i18n,
        "avatar": avatar,
        "enabled": selectable,
        "sort_order": metadata.get("sort_order").and_then(Value::as_i64).unwrap_or(100),
        "runtime_id": runtime_id,
        "agent_id": runtime_id,
        "agent": {
            "type": agent_type,
            "source": agent_source,
            "acp_backend": runtime,
        },
        "enabled_skills": enabled_skills,
        "custom_skill_names": custom_skill_names,
        "disabled_builtin_skills": disabled_builtin_skills,
        "context": biwork_assistant_rule_content(&config, None),
        "context_i18n": config.get("context_i18n").cloned().filter(Value::is_object).unwrap_or_else(|| json!({})),
        "prompts": prompts,
        "prompts_i18n": prompts_i18n,
        "models": models,
        "defaults": defaults,
        "preferences": preferences,
        "last_used_at": last_used_at,
        "agent_status": if selectable { "online" } else { "offline" },
        "agent_status_message": runtime_status_message,
        "team_selectable": selectable,
        "team_block_reason": runtime_disabled_reason,
        "deletable": deletable,
        "created_at": epoch_ms(created_at),
    }))
}

pub(super) fn normalize_biwork_assistant_source(source: &str) -> String {
    match source.trim().to_ascii_lowercase().as_str() {
        "builtin" => "builtin".to_string(),
        "generated" | "cli" => "generated".to_string(),
        _ => "user".to_string(),
    }
}

async fn resolve_biwork_assistant_id(
    state: &AppState,
    tenant_id: Uuid,
    assistant_ref: &str,
) -> Result<Uuid, AppError> {
    if let Ok(agent_id) = Uuid::parse_str(assistant_ref) {
        return Ok(agent_id);
    }
    try_resolve_biwork_assistant_id(state, tenant_id, assistant_ref)
        .await?
        .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))
}

async fn try_resolve_biwork_assistant_id(
    state: &AppState,
    tenant_id: Uuid,
    assistant_ref: &str,
) -> Result<Option<Uuid>, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM assistants
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND (metadata->>'biwork_id' = $2 OR name = $2)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(assistant_ref)
    .fetch_optional(&state.connect_pool)
    .await
    .map_err(AppError::from)
}

fn biwork_assistant_defaults(config: &Value, skills: Value) -> Value {
    let defaults = config
        .get("defaults")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    json!({
        "model": defaults.get("model").cloned().unwrap_or_else(|| json!({ "mode": "inherit" })),
        "permission": defaults.get("permission").cloned().unwrap_or_else(|| json!({ "mode": "inherit" })),
        "thought_level": defaults.get("thought_level").cloned().unwrap_or_else(|| json!({ "mode": "inherit" })),
        "skills": defaults.get("skills").cloned().unwrap_or_else(|| json!({ "mode": "replace", "value": skills })),
        "mcps": defaults.get("mcps").cloned().unwrap_or_else(|| json!({ "mode": "replace", "value": [] })),
    })
}

fn biwork_assistant_preferences(config: &Value) -> Value {
    let preferences = config
        .get("preferences")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    json!({
        "last_model_id": preferences.get("last_model_id").cloned().unwrap_or(Value::Null),
        "last_permission_value": preferences.get("last_permission_value").cloned().unwrap_or(Value::Null),
        "last_thought_level_value": preferences.get("last_thought_level_value").cloned().unwrap_or(Value::Null),
        "last_skill_ids": preferences.get("last_skill_ids").cloned().filter(Value::is_array).unwrap_or_else(|| json!([])),
        "last_disabled_builtin_skill_ids": preferences.get("last_disabled_builtin_skill_ids").cloned().filter(Value::is_array).unwrap_or_else(|| json!([])),
        "last_mcp_ids": preferences.get("last_mcp_ids").cloned().filter(Value::is_array).unwrap_or_else(|| json!([])),
    })
}

fn normalized_biwork_locale(locale: Option<&str>) -> Option<String> {
    locale
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn is_default_biwork_rule_locale(locale: &str) -> bool {
    let normalized = locale.replace('_', "-").to_ascii_lowercase();
    matches!(normalized.as_str(), "en" | "en-us")
}

pub(super) fn biwork_assistant_rule_content(config: &Value, locale: Option<&str>) -> String {
    if let Some(locale) = normalized_biwork_locale(locale)
        && let Some(content) = config
            .get("context_i18n")
            .and_then(Value::as_object)
            .and_then(|context_i18n| context_i18n.get(&locale))
            .and_then(Value::as_str)
    {
        return content.to_string();
    }

    config
        .get("system_prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn set_biwork_assistant_rule_content(
    config: &mut Value,
    locale: Option<&str>,
    content: Option<&str>,
) -> Result<(), AppError> {
    let object = config
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidInput("assistant config must be an object".to_string()))?;
    let locale = normalized_biwork_locale(locale);
    let previous_system_prompt = object
        .get("system_prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let previous_locale_content = locale.as_deref().and_then(|locale| {
        object
            .get("context_i18n")
            .and_then(Value::as_object)
            .and_then(|context_i18n| context_i18n.get(locale))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });

    let Some(content) = content else {
        object.remove("system_prompt");
        object.remove("context_i18n");
        return Ok(());
    };

    if content.trim().is_empty() {
        if let Some(locale) = locale {
            if let Some(context_i18n) = object
                .get_mut("context_i18n")
                .and_then(Value::as_object_mut)
            {
                context_i18n.remove(&locale);
                if context_i18n.is_empty() {
                    object.remove("context_i18n");
                }
            }
            if previous_locale_content.as_deref() == Some(previous_system_prompt.as_str()) {
                object.remove("system_prompt");
            }
        } else {
            object.remove("system_prompt");
        }
        return Ok(());
    }

    if let Some(locale) = locale {
        let should_update_default = previous_system_prompt.trim().is_empty()
            || previous_locale_content.as_deref() == Some(previous_system_prompt.as_str())
            || is_default_biwork_rule_locale(&locale);
        let context_i18n = object
            .entry("context_i18n".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let context_i18n_object = context_i18n.as_object_mut().ok_or_else(|| {
            AppError::InvalidInput("assistant context_i18n must be an object".to_string())
        })?;
        context_i18n_object.insert(locale, Value::String(content.to_string()));
        if should_update_default {
            object.insert(
                "system_prompt".to_string(),
                Value::String(content.to_string()),
            );
        }
    } else {
        object.insert(
            "system_prompt".to_string(),
            Value::String(content.to_string()),
        );
    }
    Ok(())
}

pub(super) fn biwork_assistant_documents(
    runtime_id: Uuid,
    existing_config: Option<Value>,
    existing_metadata: Option<Value>,
    payload: &Value,
    is_create: bool,
) -> Result<(Value, Value), AppError> {
    let mut config = existing_config.unwrap_or_else(|| json!({}));
    let mut metadata = existing_metadata.unwrap_or_else(|| json!({}));
    let config_object = config
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidInput("assistant config must be an object".to_string()))?;
    let metadata_object = metadata.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("assistant metadata must be an object".to_string())
    })?;

    if is_create {
        metadata_object.insert(
            "assistant_source".to_string(),
            Value::String(
                value_string(payload, "source")
                    .filter(|source| matches!(source.as_str(), "builtin" | "generated" | "user"))
                    .unwrap_or_else(|| "user".to_string()),
            ),
        );
        metadata_object.insert("sort_order".to_string(), json!(100));
    }
    if let Some(biwork_id) = trimmed_string(payload, "id") {
        metadata_object.insert("biwork_id".to_string(), Value::String(biwork_id));
    }
    if let Some(avatar) = trimmed_string(payload, "avatar") {
        metadata_object.insert("avatar".to_string(), Value::String(avatar));
    }
    for (payload_key, metadata_key) in [
        ("name_i18n", "name_i18n"),
        ("description_i18n", "description_i18n"),
    ] {
        if let Some(value) = payload.get(payload_key).cloned().filter(Value::is_object) {
            metadata_object.insert(metadata_key.to_string(), value);
        }
    }
    if let Some(sort_order) = payload.get("sort_order").and_then(Value::as_i64) {
        metadata_object.insert("sort_order".to_string(), json!(sort_order));
    }
    if let Some(last_used_at) = payload.get("last_used_at").and_then(Value::as_i64) {
        metadata_object.insert("last_used_at".to_string(), json!(last_used_at));
    }

    config_object.insert(
        "engine_agent_id".to_string(),
        Value::String(runtime_id.to_string()),
    );
    for (payload_key, config_key) in [
        ("enabled_skills", "skills"),
        ("custom_skill_names", "custom_skill_names"),
        ("disabled_builtin_skills", "disabled_builtin_skills"),
        ("models", "models"),
        ("prompts", "prompts"),
        ("recommended_prompts", "prompts"),
    ] {
        if let Some(value) = payload.get(payload_key).cloned().filter(Value::is_array) {
            config_object.insert(config_key.to_string(), value);
        }
    }
    for (payload_key, config_key) in [
        ("prompts_i18n", "prompts_i18n"),
        ("recommended_prompts_i18n", "prompts_i18n"),
        ("context_i18n", "context_i18n"),
    ] {
        if let Some(value) = payload.get(payload_key).cloned().filter(Value::is_object) {
            config_object.insert(config_key.to_string(), value);
        }
    }
    if let Some(defaults) = payload.get("defaults").cloned().filter(Value::is_object) {
        config_object.insert("defaults".to_string(), defaults);
    } else if is_create {
        let skills_for_defaults = config_object
            .get("skills")
            .cloned()
            .filter(Value::is_array)
            .unwrap_or_else(|| json!([]));
        config_object.insert(
            "defaults".to_string(),
            json!({
                "model": { "mode": "auto" },
                "permission": { "mode": "inherit" },
                "thought_level": { "mode": "inherit" },
                "skills": { "mode": "replace", "value": skills_for_defaults },
                "mcps": { "mode": "replace", "value": [] },
            }),
        );
    }
    if let Some(context) = trimmed_string(payload, "context") {
        config_object.insert("system_prompt".to_string(), Value::String(context));
    }
    config_object.remove("runtime");
    config_object.remove("acp_backend");

    Ok((config, metadata))
}

async fn update_biwork_assistant_record(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Uuid,
    payload: &Value,
) -> Result<Value, AppError> {
    let row = sqlx::query(
        r#"
        SELECT name, description, draft_config, metadata, status, runtime_id
        FROM assistants
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))?;
    let existing_config: Value = row.try_get("draft_config")?;
    let existing_metadata: Value = row.try_get("metadata")?;
    let existing_runtime_id: Uuid = row.try_get("runtime_id")?;
    let name = trimmed_string(payload, "name");
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let status = payload
        .get("enabled")
        .and_then(Value::as_bool)
        .map(|enabled| {
            if enabled {
                "active".to_string()
            } else {
                "disabled".to_string()
            }
        });

    let mut tx = state.connect_pool.begin().await?;
    let runtime_id = resolve_assistant_runtime_id(
        &mut tx,
        tenant_id,
        requested_runtime_id(payload)?.or(Some(existing_runtime_id)),
    )
    .await?;
    let (mut draft_config, metadata) = biwork_assistant_documents(
        runtime_id,
        Some(existing_config),
        Some(existing_metadata),
        payload,
        false,
    )?;
    normalize_biwork_assistant_model_profile(&mut tx, tenant_id, &mut draft_config).await?;
    sqlx::query(
        r#"
        UPDATE assistants
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            runtime_id = $5,
            draft_config = $6,
            metadata = $7,
            status = COALESCE($8, status),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .bind(name)
    .bind(description)
    .bind(runtime_id)
    .bind(&draft_config)
    .bind(&metadata)
    .bind(status)
    .execute(&mut *tx)
    .await?;
    publish_biwork_assistant_version(&mut tx, tenant_id, agent_id, &draft_config).await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    let model_tags = list_model_tags(state, tenant_id).await?;
    let row = load_assistant_response_row(&state.connect_pool, tenant_id, agent_id).await?;
    assistant_from_row(&row, &model_tags)
}

async fn update_biwork_assistant_rule(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Uuid,
    locale: Option<&str>,
    content: Option<&str>,
) -> Result<(), AppError> {
    let mut tx = state.connect_pool.begin().await?;
    let mut draft_config: Value = sqlx::query_scalar(
        r#"
        SELECT draft_config
        FROM assistants
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))?;

    set_biwork_assistant_rule_content(&mut draft_config, locale, content)?;
    normalize_biwork_assistant_model_profile(&mut tx, tenant_id, &mut draft_config).await?;
    sqlx::query(
        r#"
        UPDATE assistants
        SET draft_config = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .bind(&draft_config)
    .execute(&mut *tx)
    .await?;
    publish_biwork_assistant_version(&mut tx, tenant_id, agent_id, &draft_config).await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(())
}

fn snapshot_model_profile_id(snapshot: &Value) -> Result<Option<Uuid>, AppError> {
    let Some(value) = snapshot.get("model_profile_id").or_else(|| {
        snapshot
            .get("agent")
            .and_then(|agent| agent.get("model_profile_id"))
    }) else {
        return Ok(None);
    };
    let value = value.as_str().ok_or_else(|| {
        AppError::InvalidInput("model_profile_id must be a uuid string".to_string())
    })?;
    Uuid::parse_str(value)
        .map(Some)
        .map_err(|_| AppError::InvalidInput("model_profile_id must be a uuid".to_string()))
}

fn fixed_model_reference(config: &Value) -> Option<&str> {
    let model = config.pointer("/defaults/model")?;
    if model.get("mode").and_then(Value::as_str) != Some("fixed") {
        return None;
    }
    model
        .get("value")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn assistant_model_mode(config: &Value) -> Option<&str> {
    config
        .pointer("/defaults/model/mode")
        .and_then(Value::as_str)
}

fn insert_model_profile_id(config: &mut Value, model_profile_id: Uuid) -> Result<(), AppError> {
    let object = config
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidInput("assistant config must be an object".to_string()))?;
    object.insert("model_profile_id".to_string(), json!(model_profile_id));
    Ok(())
}

fn clear_model_profile_id(config: &mut Value) -> Result<(), AppError> {
    let object = config
        .as_object_mut()
        .ok_or_else(|| AppError::InvalidInput("assistant config must be an object".to_string()))?;
    object.remove("model_profile_id");
    if let Some(agent) = object.get_mut("agent").and_then(Value::as_object_mut) {
        agent.remove("model_profile_id");
        agent.remove("model");
    }
    object.remove("model");
    Ok(())
}

async fn active_model_profile_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    model_profile_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT mp.id
        FROM llm_model_profiles mp
        JOIN llm_providers provider
          ON provider.id = mp.provider_id
         AND provider.tenant_id = mp.tenant_id
         AND provider.status = 'active'
        WHERE mp.id = $1
          AND mp.tenant_id = $2
          AND mp.status = 'active'
        "#,
    )
    .bind(model_profile_id)
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)
}

async fn model_profile_id_by_reference(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    reference: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Ok(model_profile_id) = Uuid::parse_str(reference) {
        return active_model_profile_id(tx, tenant_id, model_profile_id).await;
    }

    let exact: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT mp.id
        FROM llm_model_profiles mp
        JOIN llm_providers provider
          ON provider.id = mp.provider_id
         AND provider.tenant_id = mp.tenant_id
         AND provider.status = 'active'
        WHERE mp.tenant_id = $1
          AND mp.status = 'active'
          AND mp.profile_name = $2
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(reference)
    .fetch_optional(&mut **tx)
    .await?;
    if exact.is_some() {
        return Ok(exact);
    }

    let matches = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT mp.id
        FROM llm_model_profiles mp
        JOIN llm_providers provider
          ON provider.id = mp.provider_id
         AND provider.tenant_id = mp.tenant_id
         AND provider.status = 'active'
        WHERE mp.tenant_id = $1
          AND mp.status = 'active'
          AND mp.model_name = $2
        ORDER BY mp.created_at ASC, mp.id ASC
        LIMIT 2
        "#,
    )
    .bind(tenant_id)
    .bind(reference)
    .fetch_all(&mut **tx)
    .await?;
    match matches.as_slice() {
        [] => Ok(None),
        [model_profile_id] => Ok(Some(*model_profile_id)),
        _ => Err(AppError::InvalidInput(format!(
            "assistant model reference '{reference}' is ambiguous; use a model profile id or profile name"
        ))),
    }
}

pub(super) async fn normalize_biwork_assistant_model_profile(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    config: &mut Value,
) -> Result<Option<Uuid>, AppError> {
    if assistant_model_mode(config) == Some("auto") {
        clear_model_profile_id(config)?;
        return Ok(None);
    }

    if let Some(reference) = fixed_model_reference(config) {
        let model_profile_id = model_profile_id_by_reference(tx, tenant_id, reference)
            .await?
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "assistant model reference '{reference}' does not reference an active model profile"
                ))
            })?;
        insert_model_profile_id(config, model_profile_id)?;
        return Ok(Some(model_profile_id));
    }

    if let Some(model_profile_id) = snapshot_model_profile_id(config)? {
        if active_model_profile_id(tx, tenant_id, model_profile_id)
            .await?
            .is_some()
        {
            insert_model_profile_id(config, model_profile_id)?;
            return Ok(Some(model_profile_id));
        }
        return Err(AppError::InvalidInput(
            "model_profile_id does not reference an active model profile and provider".to_string(),
        ));
    }

    if let Some(reference) = std::env::var("DEFAULT_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        let model_profile_id = model_profile_id_by_reference(tx, tenant_id, &reference)
            .await?
            .ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "DEFAULT_MODEL '{reference}' does not reference an active model profile"
                ))
            })?;
        insert_model_profile_id(config, model_profile_id)?;
        return Ok(Some(model_profile_id));
    }

    Err(AppError::InvalidInput(
        "assistant snapshot requires model_profile_id; configure a fixed model or DEFAULT_MODEL"
            .to_string(),
    ))
}

async fn validate_biwork_assistant_model_profile(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    config: &Value,
) -> Result<(), AppError> {
    if assistant_model_mode(config) == Some("auto") {
        if snapshot_model_profile_id(config)?.is_some() {
            return Err(AppError::InvalidInput(
                "automatic assistant model selection must not persist model_profile_id".to_string(),
            ));
        }
        return Ok(());
    }
    let model_profile_id = snapshot_model_profile_id(config)?.ok_or_else(|| {
        AppError::InvalidInput("agent version snapshot must include model_profile_id".to_string())
    })?;
    if active_model_profile_id(tx, tenant_id, model_profile_id)
        .await?
        .is_some()
    {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "model_profile_id does not reference an active model profile and provider".to_string(),
        ))
    }
}

pub(super) async fn disable_model_incomplete_published_versions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    agent_id: Uuid,
    safe_version_id: Uuid,
) -> Result<u64, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE assistant_versions
        SET status = 'disabled'
        WHERE tenant_id = $1
          AND assistant_id = $2
          AND id <> $3
          AND status = 'published'
          AND NULLIF(BTRIM(COALESCE(
                config_snapshot->>'model_profile_id',
                config_snapshot#>>'{agent,model_profile_id}',
                ''
              )), '') IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(agent_id)
    .bind(safe_version_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

async fn publish_biwork_assistant_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    agent_id: Uuid,
    config: &Value,
) -> Result<(), AppError> {
    validate_biwork_assistant_model_profile(tx, tenant_id, config).await?;
    let bytes = serde_json::to_vec(config)
        .map_err(|_| AppError::InvalidInput("failed to encode assistant config".to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let schema_hash = format!("sha256:{}", hex::encode(hasher.finalize()));
    let version_label = format!("biwork-{}", Uuid::new_v4().simple());
    let version_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO assistant_versions
            (tenant_id, assistant_id, version_label, config_snapshot, schema_hash, status)
        VALUES ($1, $2, $3, $4, $5, 'published')
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .bind(agent_id)
    .bind(version_label)
    .bind(config)
    .bind(schema_hash)
    .fetch_one(&mut **tx)
    .await?;
    disable_model_incomplete_published_versions(tx, tenant_id, agent_id, version_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[test]
    fn assistant_model_snapshot_helpers_preserve_the_runtime_contract() {
        let model_profile_id = Uuid::new_v4();
        assert_eq!(
            snapshot_model_profile_id(&json!({"model_profile_id": model_profile_id})).unwrap(),
            Some(model_profile_id)
        );
        assert_eq!(
            snapshot_model_profile_id(&json!({"agent": {"model_profile_id": model_profile_id}}))
                .unwrap(),
            Some(model_profile_id)
        );
        assert!(snapshot_model_profile_id(&json!({})).unwrap().is_none());
        assert!(snapshot_model_profile_id(&json!({"model_profile_id": "invalid"})).is_err());

        let mut config = json!({
            "runtime": {"kind": "deepagents"},
            "defaults": {"model": {"mode": "fixed", "value": "profile-a"}}
        });
        assert_eq!(fixed_model_reference(&config), Some("profile-a"));
        insert_model_profile_id(&mut config, model_profile_id).unwrap();
        assert_eq!(config["model_profile_id"], json!(model_profile_id));
        assert_eq!(config["runtime"]["kind"], json!("deepagents"));
    }

    #[tokio::test]
    async fn fixed_assistant_resolves_model_without_runtime_owned_model_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        let mut tx = pool.begin().await?;
        let tenant_id = Uuid::new_v4();
        let provider_id = Uuid::new_v4();
        let model_profile_id = Uuid::new_v4();
        let source_agent_id = Uuid::new_v4();
        let runtime_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Assistant model inheritance test")
            .bind(format!("assistant-model-inheritance-{tenant_id}"))
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_providers (id, tenant_id, provider_key, display_name, status)
            VALUES ($1, $2, $3, $4, 'active')
            "#,
        )
        .bind(provider_id)
        .bind(tenant_id)
        .bind(format!("provider-{provider_id}"))
        .bind("Assistant model inheritance provider")
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_model_profiles (
                id, tenant_id, provider_id, profile_name, model_name, status
            )
            VALUES ($1, $2, $3, $4, $5, 'active')
            "#,
        )
        .bind(model_profile_id)
        .bind(tenant_id)
        .bind(provider_id)
        .bind(format!("profile-{model_profile_id}"))
        .bind("assistant-model")
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_runtimes (
                id, tenant_id, name, runtime_kind, source, metadata, status
            ) VALUES ($1, $2, 'Test Runtime', 'deepagents', 'internal',
                      '{"builtin_runtime":true}'::jsonb, 'active')
            "#,
        )
        .bind(runtime_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO assistants (
                id, tenant_id, runtime_id, name, draft_config, status
            ) VALUES ($1, $2, $3, 'Source assistant',
                      jsonb_build_object('engine_agent_id', $3::text), 'active')
            "#,
        )
        .bind(source_agent_id)
        .bind(tenant_id)
        .bind(runtime_id)
        .execute(&mut *tx)
        .await?;
        let safe_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agent_versions (
                tenant_id, agent_id, version_label, config_snapshot, status
            )
            VALUES ($1, $2, $3, $4, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(source_agent_id)
        .bind("source-v1")
        .bind(json!({
            "model_profile_id": model_profile_id,
            "runtime": {"kind": "deepagents"}
        }))
        .fetch_one(&mut *tx)
        .await?;

        let mut copied_config = json!({
            "engine_agent_id": runtime_id,
            "defaults": {"model": {"mode": "fixed", "value": format!("profile-{model_profile_id}")}}
        });
        let resolved =
            normalize_biwork_assistant_model_profile(&mut tx, tenant_id, &mut copied_config)
                .await?;

        assert_eq!(resolved, Some(model_profile_id));
        assert_eq!(copied_config["model_profile_id"], json!(model_profile_id));
        validate_biwork_assistant_model_profile(&mut tx, tenant_id, &copied_config).await?;

        let incomplete_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agent_versions (
                tenant_id, agent_id, version_label, config_snapshot, status
            )
            VALUES ($1, $2, $3, $4, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(source_agent_id)
        .bind("incomplete-v2")
        .bind(json!({"runtime": {"kind": "deepagents"}}))
        .fetch_one(&mut *tx)
        .await?;
        assert_eq!(
            disable_model_incomplete_published_versions(
                &mut tx,
                tenant_id,
                source_agent_id,
                safe_version_id,
            )
            .await?,
            1
        );
        let incomplete_status: String =
            sqlx::query_scalar("SELECT status FROM agent_versions WHERE id = $1")
                .bind(incomplete_version_id)
                .fetch_one(&mut *tx)
                .await?;
        assert_eq!(incomplete_status, "disabled");
        tx.rollback().await?;
        Ok(())
    }

    #[test]
    fn automatic_assistant_model_selection_does_not_persist_a_profile() {
        let model_profile_id = Uuid::new_v4();
        let mut config = json!({
            "model_profile_id": model_profile_id,
            "model": {"model_name": "stale"},
            "agent": {
                "model_profile_id": model_profile_id,
                "model": {"model_name": "stale"}
            },
            "defaults": {"model": {"mode": "auto"}}
        });

        clear_model_profile_id(&mut config).unwrap();

        assert!(snapshot_model_profile_id(&config).unwrap().is_none());
        assert!(config.get("model").is_none());
        assert!(config["agent"].get("model").is_none());
    }
}
