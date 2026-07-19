use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde_json::{Map, Value, json};
use sqlx::Row;
use std::collections::HashMap;
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
    biwork_compat_service::{biwork_failure, epoch_ms, ok, required_string, trimmed_string},
    biwork_event_support::{emit_conversation_list_changed_event, latest_user_conversation_id},
    biwork_extension_service::resolve_channel_plugin_package,
    run_service::create_and_dispatch_conversation_run,
    support::require_ferriskey_allow,
};

fn parse_uuid_id(value: &str, label: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::NotFound(format!("{label} not found")))
}

pub async fn biwork_list_channel_plugins(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let connector_rows = sqlx::query(
        r#"
        SELECT connector_key, runtime_kind, status, enabled, connected, config_ref,
               last_connected_at, last_error
        FROM channel_connectors
        WHERE tenant_id = $1
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let active_user_rows = sqlx::query(
        r#"
        SELECT platform, COUNT(*)::bigint AS active_users
        FROM channel_authorized_users
        WHERE tenant_id = $1 AND status = 'active'
        GROUP BY platform
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut connectors = HashMap::new();
    for row in connector_rows {
        connectors.insert(row.try_get::<String, _>("connector_key")?, row);
    }
    let mut active_users = HashMap::new();
    for row in active_user_rows {
        active_users.insert(
            row.try_get::<String, _>("platform")?,
            row.try_get::<i64, _>("active_users")?,
        );
    }

    let mut plugins = Vec::new();
    for &(key, name) in builtin_channel_plugins() {
        let active_count = active_users.get(key).copied().unwrap_or(0);
        if let Some(row) = connectors.remove(key) {
            plugins.push(channel_plugin_from_row(&row, key, name, active_count)?);
        } else {
            plugins.push(json!({
                "plugin_id": key,
                "id": key,
                "type": key,
                "name": name,
                "enabled": false,
                "connected": false,
                "status": "disabled",
                "active_users": active_count,
                "has_token": false,
                "is_extension": false,
            }));
        }
    }
    for (key, row) in connectors {
        let active_count = active_users.get(&key).copied().unwrap_or(0);
        plugins.push(channel_plugin_from_row(&row, &key, &key, active_count)?);
    }

    Ok(ok(Value::Array(plugins)))
}

pub async fn biwork_enable_channel_plugin(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let plugin_id = required_string(&payload, "plugin_id")?;
    let config = payload.get("config").cloned().unwrap_or_else(|| json!({}));
    let source =
        resolve_channel_connector_source(&state, ctx.tenant_id, ctx.device_id, &plugin_id).await?;
    sqlx::query(
        r#"
        INSERT INTO channel_connectors (
            tenant_id, connector_key, source_extension_package_id, runtime_kind, status, enabled, connected, config_ref
        )
        VALUES ($1, $2, $3, $4, 'configured', TRUE, FALSE, $5)
        ON CONFLICT (tenant_id, connector_key)
        DO UPDATE SET enabled = TRUE,
                      source_extension_package_id = EXCLUDED.source_extension_package_id,
                      runtime_kind = EXCLUDED.runtime_kind,
                      status = 'configured',
                      config_ref = EXCLUDED.config_ref,
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&plugin_id)
    .bind(source.extension_package_id)
    .bind(source.runtime_kind)
    .bind(config)
    .execute(&state.connect_pool)
    .await?;
    write_channel_audit(
        &state,
        &ctx,
        format!("connector:{plugin_id}"),
        "enable_connector",
        "allow",
        Some("channel.plugin.enable"),
        Some(channel_audit_summary(&[
            ("plugin_id", plugin_id.as_str()),
            ("runtime_kind", source.runtime_kind),
        ])),
    )
    .await?;
    let status = load_channel_plugin_status(&state, ctx.tenant_id, &plugin_id).await?;
    emit_channel_plugin_status_event(&state, &ctx, &plugin_id, status.clone()).await?;
    Ok(ok(status))
}

pub async fn biwork_disable_channel_plugin(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let plugin_id = required_string(&payload, "plugin_id")?;
    let source = resolve_channel_connector_source_for_disable(
        &state,
        ctx.tenant_id,
        ctx.device_id,
        &plugin_id,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO channel_connectors (
            tenant_id, connector_key, source_extension_package_id, runtime_kind, status, enabled, connected
        )
        VALUES ($1, $2, $3, $4, 'disabled', FALSE, FALSE)
        ON CONFLICT (tenant_id, connector_key)
        DO UPDATE SET enabled = FALSE,
                      source_extension_package_id = EXCLUDED.source_extension_package_id,
                      runtime_kind = EXCLUDED.runtime_kind,
                      connected = FALSE,
                      status = 'disabled',
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&plugin_id)
    .bind(source.extension_package_id)
    .bind(source.runtime_kind)
    .execute(&state.connect_pool)
    .await?;
    write_channel_audit(
        &state,
        &ctx,
        format!("connector:{plugin_id}"),
        "disable_connector",
        "allow",
        Some("channel.plugin.disable"),
        Some(channel_audit_summary(&[
            ("plugin_id", plugin_id.as_str()),
            ("runtime_kind", source.runtime_kind),
        ])),
    )
    .await?;
    let status = load_channel_plugin_status(&state, ctx.tenant_id, &plugin_id).await?;
    emit_channel_plugin_status_event(&state, &ctx, &plugin_id, status.clone()).await?;
    Ok(ok(status))
}

pub async fn biwork_test_channel_plugin(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let plugin_id = required_string(&payload, "plugin_id")?;
    Ok(ok(biwork_failure(
        "CHANNEL_PLUGIN_LOCAL_RUNTIME_REQUIRED",
        format!("local channel connector runtime is not attached for {plugin_id}"),
        json!({ "plugin_id": plugin_id }),
    )))
}

pub async fn biwork_channel_ingress_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let platform = required_platform_string(&payload)?;
    let platform_user_id = required_string(&payload, "platform_user_id")?;
    let chat_id = trimmed_string(&payload, "chat_id").unwrap_or_else(|| platform_user_id.clone());
    let content = channel_ingress_message_content(&payload)?;
    let external_message_id = trimmed_string(&payload, "message_id")
        .or_else(|| trimmed_string(&payload, "external_message_id"));

    let binding = load_channel_ingress_binding(&state, ctx.tenant_id, &platform, &platform_user_id)
        .await?
        .ok_or_else(|| {
            AppError::Unauthorized(
                "channel user is not authorized or connector is disabled".to_string(),
            )
        })?;

    let (session_id, conversation_id, created_conversation) = ensure_channel_ingress_session(
        &state,
        &ctx,
        &platform,
        &platform_user_id,
        &chat_id,
        &binding,
        &payload,
    )
    .await?;

    let run = create_and_dispatch_conversation_run(
        &state,
        &ctx,
        conversation_id,
        CreateRunRequest {
            tenant_id: ctx.tenant_id,
            agent_id: binding.assistant_profile_id,
            agent_version_id: None,
            project_id: None,
            input: Some(channel_ingress_run_input(
                &platform,
                &platform_user_id,
                &chat_id,
                content.clone(),
                &payload,
            )),
            run_config_snapshot: Some(channel_ingress_run_snapshot(
                &platform,
                binding.channel_user_id,
                &chat_id,
                binding.default_model_profile_id,
            )),
            idempotency_key: external_message_id
                .as_ref()
                .map(|id| format!("channel:{platform}:{chat_id}:{id}")),
            thread_id: Some(conversation_id.to_string()),
        },
    )
    .await?;

    sqlx::query(
        r#"
        UPDATE channel_authorized_users
        SET last_active_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(binding.channel_user_id)
    .bind(ctx.tenant_id)
    .execute(&state.connect_pool)
    .await?;

    write_channel_audit(
        &state,
        &ctx,
        format!("session:{session_id}"),
        "ingress_message",
        "allow",
        Some("channel.ingress.message"),
        Some(channel_audit_summary(&[
            ("platform", platform.as_str()),
            ("platform_user_id", platform_user_id.as_str()),
            ("chat_id", chat_id.as_str()),
            ("run_id", &run.id.to_string()),
        ])),
    )
    .await?;

    if created_conversation {
        emit_conversation_list_changed_event(
            &state,
            &ctx,
            conversation_id,
            "created",
            "channel.ingress",
        )
        .await?;
    }

    Ok(ok(channel_ingress_response_json(
        session_id,
        conversation_id,
        run.id,
        created_conversation,
    )))
}

pub async fn biwork_list_channel_pairings(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    require_biwork_channel_route_authz(&state, &ctx, "read", "channel", "pairings").await?;
    let rows = sqlx::query(
        r#"
        SELECT code, platform, platform_user_id, display_name, created_at, expires_at
        FROM channel_pairing_requests
        WHERE tenant_id = $1
          AND status = 'pending'
          AND expires_at > CURRENT_TIMESTAMP
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let pairings = rows
        .iter()
        .map(|row| {
            Ok(json!({
                "code": row.try_get::<String, _>("code")?,
                "platform_type": row.try_get::<String, _>("platform")?,
                "platform_user_id": row.try_get::<String, _>("platform_user_id")?,
                "display_name": row.try_get::<Option<String>, _>("display_name")?,
                "requested_at": epoch_ms(row.try_get("created_at")?),
                "expires_at": epoch_ms(row.try_get("expires_at")?),
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(Value::Array(pairings)))
}

pub async fn biwork_request_channel_pairing(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let platform = required_platform_string(&payload)?;
    let platform_user_id = required_string(&payload, "platform_user_id")?;
    require_biwork_channel_route_authz(&state, &ctx, "request", "channel_pairing", &platform)
        .await?;
    let display_name = trimmed_string(&payload, "display_name");
    let code = trimmed_string(&payload, "code").unwrap_or_else(generated_channel_pairing_code);
    let expires_at =
        OffsetDateTime::now_utc() + Duration::seconds(channel_pairing_ttl_seconds(&payload)?);
    let connector_enabled = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT enabled
        FROM channel_connectors
        WHERE tenant_id = $1 AND connector_key = $2
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .fetch_optional(&state.connect_pool)
    .await?
    .unwrap_or(false);
    if !connector_enabled {
        return Err(AppError::Conflict(format!(
            "channel connector is disabled for {platform}"
        )));
    }

    let pairing_row = sqlx::query(
        r#"
        INSERT INTO channel_pairing_requests (
            tenant_id, platform, code, platform_user_id, display_name, payload, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (tenant_id, code)
        DO UPDATE SET display_name = EXCLUDED.display_name,
                      payload = EXCLUDED.payload,
                      expires_at = EXCLUDED.expires_at,
                      created_at = CURRENT_TIMESTAMP
        WHERE channel_pairing_requests.status = 'pending'
          AND channel_pairing_requests.platform = EXCLUDED.platform
          AND channel_pairing_requests.platform_user_id = EXCLUDED.platform_user_id
        RETURNING code, platform, platform_user_id, display_name, created_at, expires_at
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .bind(&code)
    .bind(&platform_user_id)
    .bind(display_name.as_deref())
    .bind(payload.get("payload").cloned().unwrap_or_else(|| json!({})))
    .bind(expires_at)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| {
        AppError::Conflict("channel pairing code is already used by another request".to_string())
    })?;

    let pairing = channel_pairing_from_row(&pairing_row)?;
    write_channel_audit(
        &state,
        &ctx,
        format!("pairing:{code}"),
        "request_pairing",
        "allow",
        Some("channel.pairing.request"),
        Some(channel_audit_summary(&[
            ("platform", platform.as_str()),
            ("platform_user_id", platform_user_id.as_str()),
        ])),
    )
    .await?;
    emit_channel_pairing_requested_event(&state, &ctx, pairing.clone()).await?;
    Ok(ok(pairing))
}

pub async fn biwork_approve_channel_pairing(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    decide_channel_pairing(&state, &ctx, &payload, "approved").await
}

pub async fn biwork_reject_channel_pairing(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    decide_channel_pairing(&state, &ctx, &payload, "rejected").await
}

pub async fn biwork_list_channel_users(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    require_biwork_channel_route_authz(&state, &ctx, "read", "channel", "users").await?;
    let rows = sqlx::query(
        r#"
        SELECT u.id, u.platform, u.platform_user_id, u.display_name, u.authorized_at,
               u.last_active_at,
               (
                 SELECT s.id
                 FROM channel_sessions s
                 WHERE s.tenant_id = u.tenant_id
                   AND s.channel_user_id = u.id
                   AND s.ended_at IS NULL
                 ORDER BY s.last_activity_at DESC
                 LIMIT 1
               ) AS session_id
        FROM channel_authorized_users u
        WHERE u.tenant_id = $1 AND u.status = 'active'
        ORDER BY u.authorized_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let users = rows
        .iter()
        .map(|row| {
            let last_active: Option<OffsetDateTime> = row.try_get("last_active_at")?;
            Ok(json!({
                "id": row.try_get::<Uuid, _>("id")?.to_string(),
                "platform_type": row.try_get::<String, _>("platform")?,
                "platform_user_id": row.try_get::<String, _>("platform_user_id")?,
                "display_name": row.try_get::<Option<String>, _>("display_name")?,
                "authorized_at": epoch_ms(row.try_get("authorized_at")?),
                "last_active": last_active.map(epoch_ms),
                "session_id": row.try_get::<Option<Uuid>, _>("session_id")?.map(|id| id.to_string()),
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(Value::Array(users)))
}

pub async fn biwork_revoke_channel_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let user_id = parse_uuid_id(&required_string(&payload, "user_id")?, "channel user")?;
    require_biwork_channel_route_authz(
        &state,
        &ctx,
        "revoke",
        "channel_user",
        &user_id.to_string(),
    )
    .await?;
    let result = sqlx::query(
        r#"
        UPDATE channel_authorized_users
        SET status = 'revoked',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(user_id)
    .bind(ctx.tenant_id)
    .execute(&state.connect_pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("channel user not found".to_string()));
    }
    sqlx::query(
        r#"
        UPDATE channel_sessions
        SET ended_at = COALESCE(ended_at, CURRENT_TIMESTAMP),
            last_activity_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND channel_user_id = $2
          AND ended_at IS NULL
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(user_id)
    .execute(&state.connect_pool)
    .await?;
    let user_id_text = user_id.to_string();
    write_channel_audit(
        &state,
        &ctx,
        format!("user:{user_id}"),
        "revoke_user",
        "allow",
        Some("channel.user.revoke"),
        Some(channel_audit_summary(&[("channel_user_id", &user_id_text)])),
    )
    .await?;
    Ok(ok(channel_user_revocation_contract_json(user_id)))
}

pub async fn biwork_list_channel_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    require_biwork_channel_route_authz(&state, &ctx, "read", "channel", "sessions").await?;
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.agent_type, s.conversation_id, s.workspace, s.chat_id,
               s.created_at, s.last_activity_at, u.platform_user_id
        FROM channel_sessions s
        LEFT JOIN channel_authorized_users u ON u.id = s.channel_user_id
        WHERE s.tenant_id = $1 AND s.ended_at IS NULL
        ORDER BY s.last_activity_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let sessions = rows
        .iter()
        .map(|row| {
            Ok(json!({
                "id": row.try_get::<Uuid, _>("id")?.to_string(),
                "user_id": row.try_get::<Option<String>, _>("platform_user_id")?.unwrap_or_default(),
                "agent_type": row.try_get::<String, _>("agent_type")?,
                "conversation_id": row.try_get::<Option<Uuid>, _>("conversation_id")?.map(|id| id.to_string()),
                "workspace": row.try_get::<Option<String>, _>("workspace")?,
                "chat_id": row.try_get::<Option<String>, _>("chat_id")?,
                "created_at": epoch_ms(row.try_get("created_at")?),
                "last_activity": epoch_ms(row.try_get("last_activity_at")?),
            }))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(Value::Array(sessions)))
}

pub async fn biwork_get_channel_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(platform): Path<String>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT s.platform, s.assistant_profile_id, s.default_model_profile_id,
               a.name AS assistant_name,
               mp.model_name, mp.provider_id
        FROM channel_platform_settings s
        LEFT JOIN agents a ON a.id = s.assistant_profile_id AND a.tenant_id = s.tenant_id
        LEFT JOIN llm_model_profiles mp ON mp.id = s.default_model_profile_id AND mp.tenant_id = s.tenant_id
        WHERE s.tenant_id = $1 AND s.platform = $2
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .fetch_optional(&state.connect_pool)
    .await?;
    Ok(ok(channel_settings_json(&platform, row.as_ref())?))
}

pub async fn biwork_set_channel_assistant(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(platform): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let assistant_id = parse_uuid_id(&required_string(&payload, "assistant_id")?, "assistant")?;
    let assistant_exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM agents
            WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        )
        "#,
    )
    .bind(assistant_id)
    .bind(ctx.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if !assistant_exists {
        return Err(AppError::NotFound("assistant not found".to_string()));
    }
    sqlx::query(
        r#"
        INSERT INTO channel_platform_settings (
            tenant_id, platform, assistant_profile_id, updated_by_user_id
        )
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (tenant_id, platform)
        DO UPDATE SET assistant_profile_id = EXCLUDED.assistant_profile_id,
                      updated_by_user_id = EXCLUDED.updated_by_user_id,
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .bind(assistant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;
    let assistant_id_text = assistant_id.to_string();
    write_channel_audit(
        &state,
        &ctx,
        format!("settings:{platform}"),
        "set_assistant",
        "allow",
        Some("channel.settings.assistant"),
        Some(channel_audit_summary(&[
            ("platform", platform.as_str()),
            ("assistant_id", &assistant_id_text),
        ])),
    )
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_set_channel_default_model(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(platform): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let model_profile_id =
        resolve_channel_default_model_profile_id(&state, ctx.tenant_id, &payload).await?;
    sqlx::query(
        r#"
        INSERT INTO channel_platform_settings (
            tenant_id, platform, default_model_profile_id, updated_by_user_id
        )
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (tenant_id, platform)
        DO UPDATE SET default_model_profile_id = EXCLUDED.default_model_profile_id,
                      updated_by_user_id = EXCLUDED.updated_by_user_id,
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .bind(model_profile_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;
    let model_profile_id_text = model_profile_id.to_string();
    write_channel_audit(
        &state,
        &ctx,
        format!("settings:{platform}"),
        "set_default_model",
        "allow",
        Some("channel.settings.default_model"),
        Some(channel_audit_summary(&[
            ("platform", platform.as_str()),
            ("model_profile_id", &model_profile_id_text),
        ])),
    )
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_sync_channel_settings(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let platform = required_platform_string(&payload)?;
    let source =
        resolve_channel_connector_source(&state, ctx.tenant_id, ctx.device_id, &platform).await?;
    let synced_at = OffsetDateTime::now_utc();
    let sync_metadata = channel_settings_sync_metadata(synced_at);
    let connector_policy = channel_connector_sync_policy(synced_at);

    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO channel_platform_settings (
            tenant_id, platform, settings, updated_by_user_id
        )
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (tenant_id, platform)
        DO UPDATE SET settings = channel_platform_settings.settings || EXCLUDED.settings,
                      updated_by_user_id = EXCLUDED.updated_by_user_id,
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .bind(&sync_metadata)
    .bind(ctx.platform_user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO channel_connectors (
            tenant_id, connector_key, source_extension_package_id, runtime_kind,
            status, enabled, connected, policy
        )
        VALUES ($1, $2, $3, $4, 'disabled', FALSE, FALSE, $5)
        ON CONFLICT (tenant_id, connector_key)
        DO UPDATE SET source_extension_package_id = EXCLUDED.source_extension_package_id,
                      runtime_kind = EXCLUDED.runtime_kind,
                      policy = channel_connectors.policy || EXCLUDED.policy,
                      updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&platform)
    .bind(source.extension_package_id)
    .bind(source.runtime_kind)
    .bind(&connector_policy)
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    write_channel_audit(
        &state,
        &ctx,
        format!("settings:{platform}"),
        "sync_settings",
        "allow",
        Some("channel.settings.sync"),
        Some(channel_audit_summary(&[
            ("platform", platform.as_str()),
            ("runtime_kind", source.runtime_kind),
        ])),
    )
    .await?;

    let status = load_channel_plugin_status(&state, ctx.tenant_id, &platform).await?;
    emit_channel_plugin_status_event(&state, &ctx, &platform, status.clone()).await?;

    Ok(ok(channel_settings_sync_response(
        &platform, status, synced_at,
    )))
}

async fn emit_channel_plugin_status_event(
    state: &AppState,
    ctx: &PlatformRequestContext,
    plugin_id: &str,
    status: Value,
) -> Result<(), AppError> {
    let Some(conversation_id) = latest_user_conversation_id(state, ctx).await? else {
        return Ok(());
    };
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        ctx.tenant_id,
        conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!(
                "channel.plugin-status-changed.{plugin_id}.{}",
                Uuid::new_v4()
            )),
            event_type: "channel.plugin-status-changed".to_string(),
            payload: Some(channel_plugin_status_changed_payload(plugin_id, status)),
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

async fn emit_channel_pairing_requested_event(
    state: &AppState,
    ctx: &PlatformRequestContext,
    pairing: Value,
) -> Result<(), AppError> {
    let Some(conversation_id) = latest_user_conversation_id(state, ctx).await? else {
        return Ok(());
    };
    let code = pairing
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        ctx.tenant_id,
        conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!(
                "channel.pairing-requested.{code}.{}",
                Uuid::new_v4()
            )),
            event_type: "channel.pairing-requested".to_string(),
            payload: Some(pairing),
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

async fn emit_channel_user_authorized_event(
    state: &AppState,
    ctx: &PlatformRequestContext,
    user: Value,
) -> Result<(), AppError> {
    let Some(conversation_id) = latest_user_conversation_id(state, ctx).await? else {
        return Ok(());
    };
    let user_id = user.get("id").and_then(Value::as_str).unwrap_or("unknown");
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        ctx.tenant_id,
        conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!(
                "channel.user-authorized.{user_id}.{}",
                Uuid::new_v4()
            )),
            event_type: "channel.user-authorized".to_string(),
            payload: Some(user),
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

fn builtin_channel_plugins() -> &'static [(&'static str, &'static str)] {
    &[
        ("telegram", "Telegram"),
        ("lark", "Lark"),
        ("dingtalk", "DingTalk"),
        ("slack", "Slack"),
        ("discord", "Discord"),
        ("weixin", "WeChat"),
        ("wecom", "WeCom"),
    ]
}

struct ChannelConnectorSource {
    runtime_kind: &'static str,
    extension_package_id: Option<Uuid>,
}

fn is_builtin_channel_plugin(plugin_id: &str) -> bool {
    builtin_channel_plugins()
        .iter()
        .any(|(key, _)| *key == plugin_id)
}

fn classify_channel_connector_source(
    plugin_id: &str,
    extension_package_id: Option<Uuid>,
) -> ChannelConnectorSource {
    if is_builtin_channel_plugin(plugin_id) {
        return ChannelConnectorSource {
            runtime_kind: "builtin",
            extension_package_id: None,
        };
    }
    match extension_package_id {
        Some(id) => ChannelConnectorSource {
            runtime_kind: "extension",
            extension_package_id: Some(id),
        },
        None => ChannelConnectorSource {
            runtime_kind: "builtin",
            extension_package_id: None,
        },
    }
}

async fn resolve_channel_connector_source(
    state: &AppState,
    tenant_id: Uuid,
    device_id: Uuid,
    plugin_id: &str,
) -> Result<ChannelConnectorSource, AppError> {
    if is_builtin_channel_plugin(plugin_id) {
        return Ok(classify_channel_connector_source(plugin_id, None));
    }
    let extension_package_id =
        resolve_channel_plugin_package(state, tenant_id, device_id, plugin_id).await?;
    let Some(extension_package_id) = extension_package_id else {
        return Err(AppError::NotFound(
            "channel extension contribution not allowed".to_string(),
        ));
    };
    Ok(classify_channel_connector_source(
        plugin_id,
        Some(extension_package_id),
    ))
}

async fn resolve_channel_connector_source_for_disable(
    state: &AppState,
    tenant_id: Uuid,
    device_id: Uuid,
    plugin_id: &str,
) -> Result<ChannelConnectorSource, AppError> {
    match resolve_channel_connector_source(state, tenant_id, device_id, plugin_id).await {
        Ok(source) => Ok(source),
        Err(AppError::NotFound(_)) => {
            let row = sqlx::query(
                r#"
                SELECT runtime_kind, source_extension_package_id
                FROM channel_connectors
                WHERE tenant_id = $1 AND connector_key = $2
                "#,
            )
            .bind(tenant_id)
            .bind(plugin_id)
            .fetch_optional(&state.connect_pool)
            .await?;
            let Some(row) = row else {
                return Err(AppError::NotFound(
                    "channel connector not found".to_string(),
                ));
            };
            let runtime_kind: String = row.try_get("runtime_kind")?;
            let extension_package_id: Option<Uuid> = row.try_get("source_extension_package_id")?;
            Ok(ChannelConnectorSource {
                runtime_kind: if runtime_kind == "extension" {
                    "extension"
                } else {
                    "builtin"
                },
                extension_package_id,
            })
        }
        Err(err) => Err(err),
    }
}

fn channel_plugin_from_row(
    row: &sqlx::postgres::PgRow,
    key: &str,
    name: &str,
    active_users: i64,
) -> Result<Value, AppError> {
    let last_connected_at: Option<OffsetDateTime> = row.try_get("last_connected_at")?;
    let config_ref: Value = row.try_get("config_ref")?;
    let runtime_kind: String = row.try_get("runtime_kind")?;
    Ok(channel_plugin_contract_json(ChannelPluginContract {
        key,
        name,
        enabled: row.try_get::<bool, _>("enabled")?,
        connected: row.try_get::<bool, _>("connected")?,
        status: &row.try_get::<String, _>("status")?,
        last_connected_at,
        error: row.try_get::<Option<String>, _>("last_error")?,
        active_users,
        has_token: !config_ref.as_object().is_none_or(Map::is_empty),
        is_extension: runtime_kind == "extension",
    }))
}

struct ChannelPluginContract<'a> {
    key: &'a str,
    name: &'a str,
    enabled: bool,
    connected: bool,
    status: &'a str,
    last_connected_at: Option<OffsetDateTime>,
    error: Option<String>,
    active_users: i64,
    has_token: bool,
    is_extension: bool,
}

fn channel_plugin_contract_json(plugin: ChannelPluginContract<'_>) -> Value {
    let ChannelPluginContract {
        key,
        name,
        enabled,
        connected,
        status,
        last_connected_at,
        error,
        active_users,
        has_token,
        is_extension,
    } = plugin;
    json!({
        "plugin_id": key,
        "id": key,
        "type": key,
        "name": name,
        "enabled": enabled,
        "connected": connected,
        "status": status,
        "last_connected": last_connected_at.map(epoch_ms),
        "error": error,
        "active_users": active_users,
        "has_token": has_token,
        "is_extension": is_extension,
    })
}

fn channel_plugin_display_name(plugin_id: &str) -> String {
    builtin_channel_plugins()
        .iter()
        .find_map(|(key, name)| (*key == plugin_id).then(|| (*name).to_string()))
        .unwrap_or_else(|| plugin_id.to_string())
}

fn channel_plugin_status_changed_payload(plugin_id: &str, status: Value) -> Value {
    json!({
        "plugin_id": plugin_id,
        "status": status,
    })
}

fn channel_pairing_contract_json(
    code: &str,
    platform: &str,
    platform_user_id: &str,
    display_name: Option<String>,
    created_at: OffsetDateTime,
    expires_at: OffsetDateTime,
) -> Value {
    json!({
        "code": code,
        "platform_type": platform,
        "platform_user_id": platform_user_id,
        "display_name": display_name,
        "requested_at": epoch_ms(created_at),
        "expires_at": epoch_ms(expires_at),
    })
}

fn channel_pairing_decision_contract_json(
    code: &str,
    status: &str,
    platform: &str,
    platform_user_id: &str,
    user: Option<Value>,
) -> Value {
    json!({
        "code": code,
        "status": status,
        "platform_type": platform,
        "platform_user_id": platform_user_id,
        "user": user,
    })
}

fn channel_pairing_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    Ok(channel_pairing_contract_json(
        &row.try_get::<String, _>("code")?,
        &row.try_get::<String, _>("platform")?,
        &row.try_get::<String, _>("platform_user_id")?,
        row.try_get("display_name")?,
        row.try_get("created_at")?,
        row.try_get("expires_at")?,
    ))
}

fn channel_user_contract_json(
    id: Uuid,
    platform: &str,
    platform_user_id: &str,
    display_name: Option<String>,
    authorized_at: OffsetDateTime,
    last_active_at: Option<OffsetDateTime>,
    session_id: Option<Uuid>,
) -> Value {
    json!({
        "id": id.to_string(),
        "platform_type": platform,
        "platform_user_id": platform_user_id,
        "display_name": display_name,
        "authorized_at": epoch_ms(authorized_at),
        "last_active": last_active_at.map(epoch_ms),
        "session_id": session_id.map(|id| id.to_string()),
    })
}

fn channel_user_revocation_contract_json(user_id: Uuid) -> Value {
    json!({
        "user_id": user_id.to_string(),
        "status": "revoked",
    })
}

fn channel_user_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let last_active_at: Option<OffsetDateTime> = row.try_get("last_active_at")?;
    Ok(channel_user_contract_json(
        row.try_get("id")?,
        &row.try_get::<String, _>("platform")?,
        &row.try_get::<String, _>("platform_user_id")?,
        row.try_get("display_name")?,
        row.try_get("authorized_at")?,
        last_active_at,
        row.try_get("session_id").unwrap_or(None),
    ))
}

fn required_platform_string(payload: &Value) -> Result<String, AppError> {
    trimmed_string(payload, "platform")
        .or_else(|| trimmed_string(payload, "platform_type"))
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| AppError::InvalidInput("platform is required".to_string()))
}

fn generated_channel_pairing_code() -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>()
        .to_ascii_uppercase()
}

fn channel_pairing_ttl_seconds(payload: &Value) -> Result<i64, AppError> {
    let Some(ttl) = payload
        .get("ttl_seconds")
        .or_else(|| payload.get("ttlSeconds"))
        .and_then(Value::as_i64)
    else {
        return Ok(600);
    };
    if !(30..=86_400).contains(&ttl) {
        return Err(AppError::InvalidInput(
            "ttl_seconds must be between 30 and 86400".to_string(),
        ));
    }
    Ok(ttl)
}

struct ChannelIngressBinding {
    channel_user_id: Uuid,
    assistant_profile_id: Option<Uuid>,
    default_model_profile_id: Option<Uuid>,
    display_name: Option<String>,
}

async fn load_channel_ingress_binding(
    state: &AppState,
    tenant_id: Uuid,
    platform: &str,
    platform_user_id: &str,
) -> Result<Option<ChannelIngressBinding>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT u.id AS channel_user_id,
               u.display_name,
               s.assistant_profile_id,
               s.default_model_profile_id
        FROM channel_authorized_users u
        JOIN channel_connectors c
          ON c.tenant_id = u.tenant_id
         AND c.connector_key = u.platform
         AND c.enabled IS TRUE
        LEFT JOIN channel_platform_settings s
          ON s.tenant_id = u.tenant_id
         AND s.platform = u.platform
        WHERE u.tenant_id = $1
          AND u.platform = $2
          AND u.platform_user_id = $3
          AND u.status = 'active'
        "#,
    )
    .bind(tenant_id)
    .bind(platform)
    .bind(platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    row.map(|row| {
        Ok(ChannelIngressBinding {
            channel_user_id: row.try_get("channel_user_id")?,
            assistant_profile_id: row.try_get("assistant_profile_id")?,
            default_model_profile_id: row.try_get("default_model_profile_id")?,
            display_name: row.try_get("display_name")?,
        })
    })
    .transpose()
}

async fn ensure_channel_ingress_session(
    state: &AppState,
    ctx: &PlatformRequestContext,
    platform: &str,
    platform_user_id: &str,
    chat_id: &str,
    binding: &ChannelIngressBinding,
    payload: &Value,
) -> Result<(Uuid, Uuid, bool), AppError> {
    let existing = sqlx::query(
        r#"
        SELECT id, conversation_id
        FROM channel_sessions
        WHERE tenant_id = $1
          AND platform = $2
          AND channel_user_id = $3
          AND chat_id = $4
          AND ended_at IS NULL
        ORDER BY last_activity_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(platform)
    .bind(binding.channel_user_id)
    .bind(chat_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    if let Some(row) = existing.as_ref() {
        let session_id: Uuid = row.try_get("id")?;
        if let Some(conversation_id) = row.try_get::<Option<Uuid>, _>("conversation_id")? {
            sqlx::query(
                r#"
                UPDATE channel_sessions
                SET last_activity_at = CURRENT_TIMESTAMP
                WHERE id = $1 AND tenant_id = $2
                "#,
            )
            .bind(session_id)
            .bind(ctx.tenant_id)
            .execute(&state.connect_pool)
            .await?;
            return Ok((session_id, conversation_id, false));
        }
    }

    let metadata = channel_ingress_conversation_metadata(
        platform,
        platform_user_id,
        chat_id,
        binding.channel_user_id,
        payload,
    );
    let title =
        channel_ingress_conversation_title(platform, binding.display_name.as_deref(), chat_id);
    let mut tx = state.connect_pool.begin().await?;
    let conversation_id: Uuid = sqlx::query_scalar(
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
    .bind(binding.assistant_profile_id)
    .bind(title)
    .bind(metadata)
    .fetch_one(&mut *tx)
    .await?;

    let session_id = if let Some(row) = existing.as_ref() {
        let session_id: Uuid = row.try_get("id")?;
        sqlx::query(
            r#"
            UPDATE channel_sessions
            SET conversation_id = $3,
                last_activity_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(session_id)
        .bind(ctx.tenant_id)
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
        session_id
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO channel_sessions (
                tenant_id, platform, channel_user_id, agent_type, conversation_id, chat_id
            )
            VALUES ($1, $2, $3, 'acp', $4, $5)
            RETURNING id
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(platform)
        .bind(binding.channel_user_id)
        .bind(conversation_id)
        .bind(chat_id)
        .fetch_one(&mut *tx)
        .await?
    };
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok((session_id, conversation_id, true))
}

fn channel_ingress_message_content(payload: &Value) -> Result<Value, AppError> {
    let content = payload
        .get("content")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("text"))
        .cloned()
        .filter(|value| !value.is_null())
        .ok_or_else(|| AppError::InvalidInput("content is required".to_string()))?;
    if content.as_str().is_some_and(|text| text.trim().is_empty()) {
        return Err(AppError::InvalidInput("content is required".to_string()));
    }
    Ok(content)
}

fn channel_ingress_run_input(
    platform: &str,
    platform_user_id: &str,
    chat_id: &str,
    content: Value,
    payload: &Value,
) -> Value {
    json!({
        "messages": [
            {
                "role": "user",
                "content": content,
            }
        ],
        "biwork": {
            "client": "biwork",
            "trigger": "channel.ingress",
            "channel": {
                "platform_type": platform,
                "platform_user_id": platform_user_id,
                "chat_id": chat_id,
                "message_id": trimmed_string(payload, "message_id")
                    .or_else(|| trimmed_string(payload, "external_message_id")),
            },
            "files": payload.get("files").cloned().unwrap_or_else(|| json!([])),
        },
    })
}

fn channel_ingress_run_snapshot(
    platform: &str,
    channel_user_id: Uuid,
    chat_id: &str,
    model_profile_id: Option<Uuid>,
) -> Value {
    json!({
        "runtime": { "kind": "deepagents" },
        "model_profile_id": model_profile_id,
        "ui": {
            "client": "biwork",
            "conversation_type": "channel",
        },
        "channel": {
            "platform_type": platform,
            "channel_user_id": channel_user_id.to_string(),
            "chat_id": chat_id,
        },
    })
}

fn channel_ingress_response_json(
    session_id: Uuid,
    conversation_id: Uuid,
    run_id: Uuid,
    created_conversation: bool,
) -> Value {
    json!({
        "session_id": session_id.to_string(),
        "conversation_id": conversation_id.to_string(),
        "run_id": run_id.to_string(),
        "created_conversation": created_conversation,
    })
}

fn channel_ingress_conversation_metadata(
    platform: &str,
    platform_user_id: &str,
    chat_id: &str,
    channel_user_id: Uuid,
    payload: &Value,
) -> Value {
    json!({
        "biwork": {
            "type": "channel",
            "channel": {
                "platform_type": platform,
                "platform_user_id": platform_user_id,
                "chat_id": chat_id,
                "channel_user_id": channel_user_id.to_string(),
            },
        },
        "extra": {
            "channel_chat_id": chat_id,
            "channel_platform": platform,
            "channel_metadata": payload.get("metadata").cloned().unwrap_or_else(|| json!({})),
        },
    })
}

fn channel_ingress_conversation_title(
    platform: &str,
    display_name: Option<&str>,
    chat_id: &str,
) -> String {
    let label = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(chat_id);
    format!("{platform} / {label}")
}

async fn load_channel_plugin_status(
    state: &AppState,
    tenant_id: Uuid,
    plugin_id: &str,
) -> Result<Value, AppError> {
    let row = sqlx::query(
        r#"
        SELECT connector_key, runtime_kind, status, enabled, connected, config_ref,
               last_connected_at, last_error
        FROM channel_connectors
        WHERE tenant_id = $1 AND connector_key = $2
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("channel connector not found".to_string()))?;

    let active_users: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::bigint
        FROM channel_authorized_users
        WHERE tenant_id = $1 AND platform = $2 AND status = 'active'
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .fetch_one(&state.connect_pool)
    .await?;

    channel_plugin_from_row(
        &row,
        plugin_id,
        &channel_plugin_display_name(plugin_id),
        active_users,
    )
}

async fn write_channel_audit(
    state: &AppState,
    ctx: &PlatformRequestContext,
    resource_id: String,
    action: &str,
    decision: &str,
    reason_code: Option<&str>,
    output_summary: Option<String>,
) -> Result<(), AppError> {
    let mut tx = state.connect_pool.begin().await?;
    audit::insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: ctx.tenant_id,
            actor_user_id: Some(ctx.platform_user_id),
            actor_device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            resource_type: "channel",
            resource_id: &resource_id,
            action,
            decision,
            policy_version: "biwork-channel-v1",
            reason_code,
            run_id: None,
            conversation_id: None,
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

async fn require_biwork_channel_route_authz(
    state: &AppState,
    ctx: &PlatformRequestContext,
    action: &str,
    resource_type: &str,
    resource_id: &str,
) -> Result<(), AppError> {
    require_ferriskey_allow(
        state,
        ctx,
        ctx.tenant_id,
        action,
        resource_type,
        resource_id.to_string(),
        None,
    )
    .await
    .map(|_| ())
}

fn channel_audit_summary(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn channel_pairing_can_be_approved(
    expires_at: OffsetDateTime,
    connector_enabled: bool,
    now: OffsetDateTime,
) -> bool {
    connector_enabled && expires_at > now
}

async fn channel_pairing_approval_failure_reason(
    state: &AppState,
    ctx: &PlatformRequestContext,
    code: &str,
) -> Result<Option<String>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT p.expires_at,
               EXISTS (
                   SELECT 1
                   FROM channel_connectors c
                   WHERE c.tenant_id = p.tenant_id
                     AND c.connector_key = p.platform
                     AND c.enabled IS TRUE
               ) AS connector_enabled
        FROM channel_pairing_requests p
        WHERE p.tenant_id = $1 AND p.code = $2 AND p.status = 'pending'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(code)
    .fetch_optional(&state.connect_pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let expires_at: OffsetDateTime = row.try_get("expires_at")?;
    let connector_enabled: bool = row.try_get("connector_enabled")?;
    if channel_pairing_can_be_approved(expires_at, connector_enabled, OffsetDateTime::now_utc()) {
        Ok(Some(
            "channel pairing approval lost a concurrent update; retry the request".to_string(),
        ))
    } else if !connector_enabled {
        Ok(Some(
            "channel connector is disabled; pairing approval is not allowed".to_string(),
        ))
    } else {
        Ok(Some(
            "channel pairing request is expired; request a new pairing code".to_string(),
        ))
    }
}

async fn decide_channel_pairing(
    state: &AppState,
    ctx: &PlatformRequestContext,
    payload: &Value,
    status: &str,
) -> Result<Json<Value>, AppError> {
    let code = required_string(payload, "code")?;
    let action = if status == "approved" {
        "approve"
    } else {
        "reject"
    };
    require_biwork_channel_route_authz(state, ctx, action, "channel_pairing", &code).await?;
    let pairing = if status == "approved" {
        sqlx::query(
            r#"
            UPDATE channel_pairing_requests p
            SET status = $4,
                decided_by_user_id = $3,
                decided_at = CURRENT_TIMESTAMP
            WHERE p.tenant_id = $1
              AND p.code = $2
              AND p.status = 'pending'
              AND p.expires_at > CURRENT_TIMESTAMP
              AND EXISTS (
                  SELECT 1
                  FROM channel_connectors c
                  WHERE c.tenant_id = p.tenant_id
                    AND c.connector_key = p.platform
                    AND c.enabled IS TRUE
              )
            RETURNING p.platform, p.platform_user_id, p.display_name
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(&code)
        .bind(ctx.platform_user_id)
        .bind(status)
        .fetch_optional(&state.connect_pool)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE channel_pairing_requests
            SET status = $4,
                decided_by_user_id = $3,
                decided_at = CURRENT_TIMESTAMP
            WHERE tenant_id = $1 AND code = $2 AND status = 'pending'
            RETURNING platform, platform_user_id, display_name
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(&code)
        .bind(ctx.platform_user_id)
        .bind(status)
        .fetch_optional(&state.connect_pool)
        .await?
    };

    let Some(row) = pairing else {
        if status == "approved"
            && let Some(reason) = channel_pairing_approval_failure_reason(state, ctx, &code).await?
        {
            return Err(AppError::Conflict(reason));
        }
        return Err(AppError::NotFound(
            "channel pairing request not found".to_string(),
        ));
    };
    let platform: String = row.try_get("platform")?;
    let platform_user_id: String = row.try_get("platform_user_id")?;
    let display_name: Option<String> = row.try_get("display_name")?;

    let authorized_user = if status == "approved" {
        Some(
            sqlx::query(
            r#"
                INSERT INTO channel_authorized_users (
                    tenant_id, platform, platform_user_id, display_name, status
                )
                VALUES ($1, $2, $3, $4, 'active')
                ON CONFLICT (tenant_id, platform, platform_user_id)
                DO UPDATE SET status = 'active',
                              display_name = EXCLUDED.display_name,
                              updated_at = CURRENT_TIMESTAMP
                RETURNING id, platform, platform_user_id, display_name, authorized_at, last_active_at
                "#,
            )
            .bind(ctx.tenant_id)
            .bind(&platform)
            .bind(&platform_user_id)
            .bind(&display_name)
            .fetch_one(&state.connect_pool)
            .await?,
        )
    } else {
        None
    };
    write_channel_audit(
        state,
        ctx,
        format!("pairing:{code}"),
        if status == "approved" {
            "approve_pairing"
        } else {
            "reject_pairing"
        },
        "allow",
        Some(if status == "approved" {
            "channel.pairing.approve"
        } else {
            "channel.pairing.reject"
        }),
        Some(channel_audit_summary(&[
            ("code", code.as_str()),
            ("platform", platform.as_str()),
            ("platform_user_id", platform_user_id.as_str()),
        ])),
    )
    .await?;
    let user = if let Some(user_row) = authorized_user.as_ref() {
        let user = channel_user_from_row(user_row)?;
        emit_channel_user_authorized_event(state, ctx, user.clone()).await?;
        Some(user)
    } else {
        None
    };
    Ok(ok(channel_pairing_decision_contract_json(
        &code,
        status,
        &platform,
        &platform_user_id,
        user,
    )))
}

fn channel_settings_json(
    platform: &str,
    row: Option<&sqlx::postgres::PgRow>,
) -> Result<Value, AppError> {
    let Some(row) = row else {
        return Ok(channel_settings_contract_json(
            platform, None, None, None, None, None,
        ));
    };
    let assistant_profile_id: Option<Uuid> = row.try_get("assistant_profile_id")?;
    let default_model_profile_id: Option<Uuid> = row.try_get("default_model_profile_id")?;
    let provider_id: Option<Uuid> = row.try_get("provider_id")?;
    Ok(channel_settings_contract_json(
        platform,
        assistant_profile_id,
        row.try_get::<Option<String>, _>("assistant_name")
            .ok()
            .flatten(),
        default_model_profile_id,
        provider_id,
        row.try_get::<Option<String>, _>("model_name")
            .ok()
            .flatten(),
    ))
}

fn channel_settings_contract_json(
    platform: &str,
    assistant_profile_id: Option<Uuid>,
    assistant_name: Option<String>,
    default_model_profile_id: Option<Uuid>,
    provider_id: Option<Uuid>,
    model_name: Option<String>,
) -> Value {
    json!({
        "platform": platform,
        "assistant": assistant_profile_id.map(|id| {
            json!({
                "assistant_id": id.to_string(),
                "name": assistant_name,
            })
        }),
        "default_model": default_model_profile_id.map(|id| {
            json!({
                "id": provider_id.map(|value| value.to_string()).unwrap_or_else(|| id.to_string()),
                "model_profile_id": id.to_string(),
                "use_model": model_name.unwrap_or_default(),
            })
        }),
    })
}

fn channel_settings_sync_metadata(synced_at: OffsetDateTime) -> Value {
    json!({
        "last_synced_at": epoch_ms(synced_at),
        "last_sync_source": "biwork-desktop",
    })
}

fn channel_connector_sync_policy(synced_at: OffsetDateTime) -> Value {
    json!({
        "last_settings_sync_at": epoch_ms(synced_at),
        "settings_source": "rust-compat",
    })
}

fn channel_settings_sync_response(
    platform: &str,
    connector_status: Value,
    synced_at: OffsetDateTime,
) -> Value {
    json!({
        "platform": platform,
        "synced": true,
        "synced_at": epoch_ms(synced_at),
        "connector": connector_status,
    })
}

async fn resolve_channel_default_model_profile_id(
    state: &AppState,
    tenant_id: Uuid,
    payload: &Value,
) -> Result<Uuid, AppError> {
    if let Some(profile_id) = trimmed_string(payload, "model_profile_id")
        .or_else(|| trimmed_string(payload, "profile_id"))
    {
        let model_profile_id = parse_uuid_id(&profile_id, "model profile")?;
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS(
                SELECT 1
                FROM llm_model_profiles
                WHERE id = $1 AND tenant_id = $2 AND status = 'active'
            )
            "#,
        )
        .bind(model_profile_id)
        .bind(tenant_id)
        .fetch_one(&state.connect_pool)
        .await?;
        if !exists {
            return Err(AppError::NotFound("model profile not found".to_string()));
        }
        return Ok(model_profile_id);
    }

    let provider_id = parse_uuid_id(&required_string(payload, "id")?, "provider")?;
    let model_name = required_string(payload, "use_model")?;
    sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM llm_model_profiles
        WHERE tenant_id = $1
          AND provider_id = $2
          AND model_name = $3
          AND status = 'active'
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(provider_id)
    .bind(&model_name)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("model profile not found".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offset_datetime_from_epoch_ms(epoch_ms_value: i64) -> Result<OffsetDateTime, AppError> {
        let seconds = epoch_ms_value.div_euclid(1_000);
        let milliseconds = epoch_ms_value.rem_euclid(1_000);
        OffsetDateTime::from_unix_timestamp(seconds)
            .map(|value| value + Duration::milliseconds(milliseconds))
            .map_err(|_| AppError::InvalidInput("timestamp is out of range".to_string()))
    }

    #[test]
    fn channel_connector_source_classifies_builtin_extension_and_unknown_plugins() {
        let extension_id = Uuid::new_v4();

        let builtin = classify_channel_connector_source("telegram", Some(extension_id));
        assert_eq!(builtin.runtime_kind, "builtin");
        assert_eq!(builtin.extension_package_id, None);

        let extension = classify_channel_connector_source("ext-wecom-bot", Some(extension_id));
        assert_eq!(extension.runtime_kind, "extension");
        assert_eq!(extension.extension_package_id, Some(extension_id));

        let unknown = classify_channel_connector_source("unknown-plugin", None);
        assert_eq!(unknown.runtime_kind, "builtin");
        assert_eq!(unknown.extension_package_id, None);
    }

    #[test]
    fn channel_plugin_contract_contains_biwork_required_fields() {
        let last_connected_at = offset_datetime_from_epoch_ms(12_345).expect("valid timestamp");

        let plugin = channel_plugin_contract_json(ChannelPluginContract {
            key: "telegram",
            name: "Telegram",
            enabled: true,
            connected: false,
            status: "disconnected",
            last_connected_at: Some(last_connected_at),
            error: Some("token expired".to_string()),
            active_users: 7,
            has_token: true,
            is_extension: false,
        });

        assert_eq!(plugin["plugin_id"], "telegram");
        assert_eq!(plugin["id"], "telegram");
        assert_eq!(plugin["type"], "telegram");
        assert_eq!(plugin["name"], "Telegram");
        assert_eq!(plugin["enabled"], true);
        assert_eq!(plugin["connected"], false);
        assert_eq!(plugin["status"], "disconnected");
        assert_eq!(plugin["last_connected"], 12_345);
        assert_eq!(plugin["error"], "token expired");
        assert_eq!(plugin["active_users"], 7);
        assert_eq!(plugin["has_token"], true);
        assert_eq!(plugin["is_extension"], false);
    }

    #[test]
    fn channel_plugin_status_changed_payload_matches_biwork_contract() {
        let status = channel_plugin_contract_json(ChannelPluginContract {
            key: "telegram",
            name: "Telegram",
            enabled: true,
            connected: false,
            status: "configured",
            last_connected_at: None,
            error: None,
            active_users: 0,
            has_token: true,
            is_extension: false,
        });

        let payload = channel_plugin_status_changed_payload("telegram", status);

        assert_eq!(payload["plugin_id"], "telegram");
        assert_eq!(payload["status"]["id"], "telegram");
        assert_eq!(payload["status"]["type"], "telegram");
        assert_eq!(payload["status"]["name"], "Telegram");
        assert_eq!(payload["status"]["enabled"], true);
        assert_eq!(payload["status"]["status"], "configured");
        assert!(payload.get("payload").is_none());
    }

    #[test]
    fn channel_pairing_contract_matches_biwork_mapper_fields() {
        let created_at = offset_datetime_from_epoch_ms(42_000).expect("valid timestamp");
        let expires_at = offset_datetime_from_epoch_ms(642_000).expect("valid timestamp");

        let pairing = channel_pairing_contract_json(
            "PAIR1234",
            "telegram",
            "platform-user-1",
            Some("Alice".to_string()),
            created_at,
            expires_at,
        );

        assert_eq!(pairing["code"], "PAIR1234");
        assert_eq!(pairing["platform_type"], "telegram");
        assert_eq!(pairing["platform_user_id"], "platform-user-1");
        assert_eq!(pairing["display_name"], "Alice");
        assert_eq!(pairing["requested_at"], 42_000);
        assert_eq!(pairing["expires_at"], 642_000);
    }

    #[test]
    fn channel_pairing_decision_contract_returns_status_and_optional_user() {
        let user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap();
        let authorized_at = offset_datetime_from_epoch_ms(42_000).expect("valid timestamp");
        let user = channel_user_contract_json(
            user_id,
            "telegram",
            "platform-user-1",
            Some("Alice".to_string()),
            authorized_at,
            None,
            None,
        );
        let approved = channel_pairing_decision_contract_json(
            "PAIR1234",
            "approved",
            "telegram",
            "platform-user-1",
            Some(user),
        );
        let rejected = channel_pairing_decision_contract_json(
            "PAIR1234",
            "rejected",
            "telegram",
            "platform-user-1",
            None,
        );

        assert_eq!(approved["code"], "PAIR1234");
        assert_eq!(approved["status"], "approved");
        assert_eq!(approved["platform_type"], "telegram");
        assert_eq!(approved["platform_user_id"], "platform-user-1");
        assert_eq!(approved["user"]["id"], user_id.to_string());
        assert!(rejected["user"].is_null());
    }

    #[test]
    fn channel_pairing_request_helpers_validate_platform_and_ttl() {
        let payload = json!({
            "platform_type": "Telegram",
            "ttlSeconds": 300,
        });

        assert_eq!(required_platform_string(&payload).unwrap(), "telegram");
        assert_eq!(channel_pairing_ttl_seconds(&payload).unwrap(), 300);
        assert!(matches!(
            channel_pairing_ttl_seconds(&json!({ "ttl_seconds": 5 })),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn channel_pairing_approval_requires_enabled_connector_and_unexpired_code() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::seconds(60);
        let future = now + Duration::seconds(30);
        let past = now - Duration::seconds(1);

        assert!(channel_pairing_can_be_approved(future, true, now));
        assert!(!channel_pairing_can_be_approved(future, false, now));
        assert!(!channel_pairing_can_be_approved(past, true, now));
    }

    #[test]
    fn channel_ingress_contract_builds_run_input_snapshot_and_metadata() {
        let channel_user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000901").unwrap();
        let model_profile_id = Uuid::parse_str("00000000-0000-0000-0000-000000000902").unwrap();
        let payload = json!({
            "content": "hello from telegram",
            "message_id": "msg-1",
            "files": [{"name": "photo.png"}],
            "metadata": {"chat_type": "private"},
        });

        let content = channel_ingress_message_content(&payload).expect("content");
        let input =
            channel_ingress_run_input("telegram", "platform-user-1", "chat-1", content, &payload);
        let snapshot = channel_ingress_run_snapshot(
            "telegram",
            channel_user_id,
            "chat-1",
            Some(model_profile_id),
        );
        let metadata = channel_ingress_conversation_metadata(
            "telegram",
            "platform-user-1",
            "chat-1",
            channel_user_id,
            &payload,
        );

        assert_eq!(input["messages"][0]["role"], "user");
        assert_eq!(input["messages"][0]["content"], "hello from telegram");
        assert_eq!(input["biwork"]["trigger"], "channel.ingress");
        assert_eq!(input["biwork"]["channel"]["platform_type"], "telegram");
        assert_eq!(
            input["biwork"]["channel"]["platform_user_id"],
            "platform-user-1"
        );
        assert_eq!(input["biwork"]["channel"]["chat_id"], "chat-1");
        assert_eq!(input["biwork"]["channel"]["message_id"], "msg-1");
        assert_eq!(input["biwork"]["files"][0]["name"], "photo.png");

        assert_eq!(snapshot["runtime"]["kind"], "deepagents");
        assert_eq!(snapshot["model_profile_id"], model_profile_id.to_string());
        assert_eq!(snapshot["ui"]["conversation_type"], "channel");
        assert_eq!(
            snapshot["channel"]["channel_user_id"],
            channel_user_id.to_string()
        );

        assert_eq!(metadata["biwork"]["type"], "channel");
        assert_eq!(metadata["biwork"]["channel"]["platform_type"], "telegram");
        assert_eq!(
            metadata["biwork"]["channel"]["platform_user_id"],
            "platform-user-1"
        );
        assert_eq!(metadata["extra"]["channel_chat_id"], "chat-1");
        assert_eq!(
            metadata["extra"]["channel_metadata"]["chat_type"],
            "private"
        );
    }

    #[test]
    fn channel_ingress_response_contract_matches_biwork_mapper_fields() {
        let session_id = Uuid::parse_str("00000000-0000-0000-0000-000000000301").unwrap();
        let conversation_id = Uuid::parse_str("00000000-0000-0000-0000-000000000302").unwrap();
        let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000303").unwrap();

        let response = channel_ingress_response_json(session_id, conversation_id, run_id, true);

        assert_eq!(response["session_id"], session_id.to_string());
        assert_eq!(response["conversation_id"], conversation_id.to_string());
        assert_eq!(response["run_id"], run_id.to_string());
        assert_eq!(response["created_conversation"], true);
    }

    #[test]
    fn channel_ingress_rejects_empty_content() {
        assert!(matches!(
            channel_ingress_message_content(&json!({ "content": "   " })),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            channel_ingress_message_content(&json!({})),
            Err(AppError::InvalidInput(_))
        ));
    }

    #[test]
    fn channel_ingress_fail_closed_before_side_effects() {
        let source = include_str!("biwork_channel_service.rs");
        let handler_start = source
            .find("pub async fn biwork_channel_ingress_message")
            .expect("channel ingress handler exists");
        let handler_end = source
            .find("pub async fn biwork_list_channel_pairings")
            .expect("next channel handler exists");
        let handler_source = &source[handler_start..handler_end];

        let binding_check = handler_source
            .find("load_channel_ingress_binding")
            .expect("ingress checks binding");
        let session_side_effect = handler_source
            .find("ensure_channel_ingress_session")
            .expect("ingress may create session/conversation");
        let run_side_effect = handler_source
            .find("create_and_dispatch_conversation_run")
            .expect("ingress may create run");

        assert!(binding_check < session_side_effect);
        assert!(binding_check < run_side_effect);

        let binding_start = source
            .find("async fn load_channel_ingress_binding")
            .expect("binding loader exists");
        let binding_end = source
            .find("async fn ensure_channel_ingress_session")
            .expect("session helper exists");
        let binding_source = &source[binding_start..binding_end];

        assert!(binding_source.contains("JOIN channel_connectors c"));
        assert!(binding_source.contains("c.enabled IS TRUE"));
        assert!(binding_source.contains("u.status = 'active'"));
    }

    #[test]
    fn channel_user_contract_matches_biwork_mapper_fields() {
        let user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap();
        let session_id = Uuid::parse_str("00000000-0000-0000-0000-000000000888").unwrap();
        let authorized_at = offset_datetime_from_epoch_ms(42_000).expect("valid timestamp");
        let last_active_at = offset_datetime_from_epoch_ms(43_000).expect("valid timestamp");

        let user = channel_user_contract_json(
            user_id,
            "telegram",
            "platform-user-1",
            Some("Alice".to_string()),
            authorized_at,
            Some(last_active_at),
            Some(session_id),
        );

        assert_eq!(user["id"], user_id.to_string());
        assert_eq!(user["platform_type"], "telegram");
        assert_eq!(user["platform_user_id"], "platform-user-1");
        assert_eq!(user["display_name"], "Alice");
        assert_eq!(user["authorized_at"], 42_000);
        assert_eq!(user["last_active"], 43_000);
        assert_eq!(user["session_id"], session_id.to_string());
    }

    #[test]
    fn channel_user_revocation_contract_matches_biwork_mapper_fields() {
        let user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000777").unwrap();
        let revoked = channel_user_revocation_contract_json(user_id);

        assert_eq!(revoked["user_id"], user_id.to_string());
        assert_eq!(revoked["status"], "revoked");
    }

    #[test]
    fn channel_settings_contract_defaults_to_null_bindings() {
        let settings =
            channel_settings_json("telegram", None).expect("default settings should serialize");

        assert_eq!(settings["platform"], "telegram");
        assert!(settings["assistant"].is_null());
        assert!(settings["default_model"].is_null());
    }

    #[test]
    fn channel_settings_contract_uses_provider_id_for_default_model_ref() {
        let assistant_id = Uuid::parse_str("00000000-0000-0000-0000-000000000501").unwrap();
        let model_profile_id = Uuid::parse_str("00000000-0000-0000-0000-000000000502").unwrap();
        let provider_id = Uuid::parse_str("00000000-0000-0000-0000-000000000503").unwrap();

        let settings = channel_settings_contract_json(
            "telegram",
            Some(assistant_id),
            Some("Ops assistant".to_string()),
            Some(model_profile_id),
            Some(provider_id),
            Some("gpt-5".to_string()),
        );

        assert_eq!(
            settings["assistant"]["assistant_id"],
            assistant_id.to_string()
        );
        assert_eq!(settings["assistant"]["name"], "Ops assistant");
        assert_eq!(settings["default_model"]["id"], provider_id.to_string());
        assert_eq!(
            settings["default_model"]["model_profile_id"],
            model_profile_id.to_string()
        );
        assert_eq!(settings["default_model"]["use_model"], "gpt-5");
    }

    #[test]
    fn channel_settings_sync_contract_records_rust_sync_state() {
        let synced_at = offset_datetime_from_epoch_ms(42_000).expect("valid timestamp");
        let status = channel_plugin_contract_json(ChannelPluginContract {
            key: "telegram",
            name: "Telegram",
            enabled: false,
            connected: false,
            status: "disabled",
            last_connected_at: None,
            error: None,
            active_users: 0,
            has_token: false,
            is_extension: false,
        });

        let metadata = channel_settings_sync_metadata(synced_at);
        let policy = channel_connector_sync_policy(synced_at);
        let response = channel_settings_sync_response("telegram", status, synced_at);

        assert_eq!(metadata["last_synced_at"], 42_000);
        assert_eq!(metadata["last_sync_source"], "biwork-desktop");
        assert_eq!(policy["last_settings_sync_at"], 42_000);
        assert_eq!(policy["settings_source"], "rust-compat");
        assert_eq!(response["platform"], "telegram");
        assert_eq!(response["synced"], true);
        assert_eq!(response["synced_at"], 42_000);
        assert_eq!(response["connector"]["plugin_id"], "telegram");
        assert_eq!(response["connector"]["enabled"], false);
    }

    #[test]
    fn channel_audit_summary_uses_stable_non_empty_fields() {
        let summary = channel_audit_summary(&[
            ("plugin_id", "telegram"),
            ("runtime_kind", "builtin"),
            ("secret", ""),
        ]);

        assert_eq!(summary, "plugin_id=telegram; runtime_kind=builtin");
    }
}
