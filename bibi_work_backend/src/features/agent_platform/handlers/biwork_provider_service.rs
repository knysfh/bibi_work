use axum::{
    Extension, Json,
    extract::{Path, State},
};
use reqwest::Url;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashSet;
use std::time::{Duration as StdDuration, Instant};
use uuid::Uuid;

use crate::{
    features::{agent_platform::ferriskey_oidc::PlatformRequestContext, core::errors::AppError},
    startup::AppState,
};

use super::{
    biwork_compat_service::{ok, required_string, trimmed_string, value_string},
    llm_catalog_service,
    support::require_ferriskey_allow,
};

fn parse_provider_id(value: &str, label: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::NotFound(format!("{label} not found")))
}

pub async fn biwork_check_provider_health(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let provider_id = parse_provider_id(&required_string(&payload, "provider_id")?, "provider")?;
    let model = required_string(&payload, "model")?;
    Ok(ok(biwork_provider_health_check(
        &state,
        &ctx,
        provider_id,
        model,
    )
    .await?))
}

pub async fn biwork_test_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let model = required_string(&payload, "model")?;
    Ok(ok(biwork_provider_health_check(
        &state,
        &ctx,
        provider_id,
        model,
    )
    .await?))
}

async fn biwork_provider_health_check(
    state: &AppState,
    ctx: &PlatformRequestContext,
    provider_id: Uuid,
    model: String,
) -> Result<Value, AppError> {
    let started = std::time::Instant::now();
    let row = sqlx::query(
        r#"
        SELECT p.provider_key, p.status, mp.id AS model_profile_id
        FROM llm_providers p
        LEFT JOIN llm_model_profiles mp
          ON mp.provider_id = p.id
         AND mp.tenant_id = p.tenant_id
         AND mp.status = 'active'
         AND mp.model_name = $3
        WHERE p.id = $1 AND p.tenant_id = $2
        "#,
    )
    .bind(provider_id)
    .bind(ctx.tenant_id)
    .bind(&model)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("provider not found".to_string()))?;
    let provider_status: String = row.try_get("status")?;
    let provider_key: String = row.try_get("provider_key")?;
    if provider_status != "active" {
        return Ok(json!({
            "provider_id": provider_id.to_string(),
            "platform": provider_key,
            "model": model,
            "status": "unhealthy",
            "elapsed_ms": i64::try_from(started.elapsed().as_millis()).unwrap_or(0),
            "message": "provider is disabled",
            "error_kind": "invalid_request",
        }));
    }

    let model_profile_id: Option<Uuid> = row.try_get("model_profile_id")?;
    let Some(model_profile_id) = model_profile_id else {
        return Ok(json!({
            "provider_id": provider_id.to_string(),
            "platform": provider_key,
            "model": model,
            "status": "unhealthy",
            "elapsed_ms": i64::try_from(started.elapsed().as_millis()).unwrap_or(0),
            "message": "active model profile not found for provider/model",
            "error_kind": "not_found",
        }));
    };

    let result = llm_catalog_service::test_llm_model_profile_for_tenant(
        state,
        ctx.tenant_id,
        model_profile_id,
    )
    .await?;
    Ok(json!({
        "provider_id": provider_id.to_string(),
        "platform": result.provider_key,
        "model": result.model_name,
        "status": if result.success { "healthy" } else { "unhealthy" },
        "elapsed_ms": i64::try_from(result.latency_ms).unwrap_or(i64::MAX),
        "message": result.message,
        "error_kind": if result.success { Value::Null } else { json!("api_error") },
        "http_status": result.http_status,
        "model_available": result.model_available,
        "checked_model_count": result.checked_model_count,
    }))
}

pub async fn biwork_list_providers(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT p.id, p.provider_key, p.display_name, p.base_url, p.status,
               credential.secret_mask, credential.secret_count,
               COALESCE(jsonb_agg(DISTINCT mp.model_name) FILTER (WHERE mp.id IS NOT NULL), '[]'::jsonb) AS models,
               COALESCE(
                   jsonb_object_agg(mp.model_name, COALESCE(mp.display_name, mp.model_name))
                       FILTER (WHERE mp.id IS NOT NULL),
                   '{}'::jsonb
               ) AS model_labels,
               COALESCE(
                   jsonb_object_agg(mp.model_name, mp.id::text) FILTER (WHERE mp.id IS NOT NULL),
                   '{}'::jsonb
               ) AS model_profile_ids
        FROM llm_providers p
        LEFT JOIN llm_model_profiles mp
          ON mp.provider_id = p.id AND mp.tenant_id = p.tenant_id AND mp.status = 'active'
        LEFT JOIN LATERAL (
          SELECT c.secret_mask, c.secret_count
          FROM llm_credentials c
          WHERE c.provider_id = p.id
            AND c.tenant_id = p.tenant_id
            AND c.revoked_at IS NULL
            AND c.rotation_status = 'active'
          ORDER BY c.created_at DESC, c.id DESC
          LIMIT 1
        ) credential ON TRUE
        WHERE p.tenant_id = $1 AND p.status <> 'deleted'
        GROUP BY p.id, p.provider_key, p.display_name, p.base_url, p.status,
                 credential.secret_mask, credential.secret_count
        ORDER BY p.updated_at DESC, p.created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut providers = Vec::with_capacity(rows.len());
    for row in rows {
        let models = row.try_get::<Value, _>("models")?;
        let enabled_models = model_enabled_map(&models);
        providers.push(json!({
            "id": row.try_get::<Uuid, _>("id")?.to_string(),
            "platform": row.try_get::<String, _>("provider_key")?,
            "name": row.try_get::<String, _>("display_name")?,
            "base_url": row.try_get::<Option<String>, _>("base_url")?.unwrap_or_default(),
            "api_key": "",
            "api_key_mask": row.try_get::<Option<String>, _>("secret_mask")?,
            "api_key_count": row.try_get::<Option<i32>, _>("secret_count")?.unwrap_or(0),
            "models": models,
            "model_labels": row.try_get::<Value, _>("model_labels")?,
            "model_profile_ids": row.try_get::<Value, _>("model_profile_ids")?,
            "enabled": row.try_get::<String, _>("status")? == "active",
            "model_enabled": enabled_models,
        }));
    }

    Ok(ok(Value::Array(providers)))
}

pub async fn biwork_create_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "create",
        "llm_provider",
        "new".to_string(),
        None,
    )
    .await?;
    let platform = required_string(&payload, "platform")?;
    let name = required_string(&payload, "name")?;
    let base_url = payload
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let status = if payload
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        "active"
    } else {
        "disabled"
    };
    let api_key = provider_api_key_replacement(&payload)?;
    let mut tx = state.connect_pool.begin().await?;
    let provider_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO llm_providers (
            tenant_id, provider_key, display_name, base_url, status
        )
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (tenant_id, provider_key, display_name)
        DO UPDATE SET base_url = EXCLUDED.base_url,
                      status = EXCLUDED.status,
                      updated_at = CURRENT_TIMESTAMP
        WHERE llm_providers.status = 'deleted'
        RETURNING id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(platform)
    .bind(name)
    .bind(base_url)
    .bind(status)
    .fetch_one(&mut *tx)
    .await?;
    let credential_id = if let Some(api_key) = api_key {
        Some(
            replace_biwork_provider_credential_tx(
                &mut tx,
                &state,
                ctx.tenant_id,
                provider_id,
                ctx.platform_user_id,
                &api_key,
            )
            .await?,
        )
    } else {
        active_provider_credential_id_tx(&mut tx, ctx.tenant_id, provider_id).await?
    };
    replace_biwork_provider_models_tx(
        &mut tx,
        ctx.tenant_id,
        provider_id,
        payload.get("models"),
        payload.get("model_labels"),
        credential_id,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(
        biwork_load_provider(&state, ctx.tenant_id, provider_id).await?
    ))
}

pub async fn biwork_update_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "llm_provider",
        provider_id.to_string(),
        None,
    )
    .await?;
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
    let api_key = provider_api_key_replacement(&payload)?;
    let mut tx = state.connect_pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE llm_providers
        SET provider_key = COALESCE($3, provider_key),
            display_name = COALESCE($4, display_name),
            base_url = COALESCE($5, base_url),
            status = COALESCE($6, status),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(provider_id)
    .bind(ctx.tenant_id)
    .bind(value_string(&payload, "platform"))
    .bind(value_string(&payload, "name"))
    .bind(value_string(&payload, "base_url"))
    .bind(status)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::NotFound("provider not found".to_string()));
    }
    let replaces_api_key = api_key.is_some();
    let credential_id = if let Some(api_key) = api_key.as_deref() {
        let credential_id = replace_biwork_provider_credential_tx(
            &mut tx,
            &state,
            ctx.tenant_id,
            provider_id,
            ctx.platform_user_id,
            api_key,
        )
        .await?;
        Some(credential_id)
    } else {
        active_provider_credential_id_tx(&mut tx, ctx.tenant_id, provider_id).await?
    };
    if payload.get("models").is_some() {
        replace_biwork_provider_models_tx(
            &mut tx,
            ctx.tenant_id,
            provider_id,
            payload.get("models"),
            payload.get("model_labels"),
            credential_id,
        )
        .await?;
    } else if replaces_api_key {
        sqlx::query(
            r#"
            UPDATE llm_model_profiles
            SET credential_id = $3,
                updated_at = CURRENT_TIMESTAMP
            WHERE tenant_id = $1 AND provider_id = $2 AND status = 'active'
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(provider_id)
        .bind(credential_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(
        biwork_load_provider(&state, ctx.tenant_id, provider_id).await?
    ))
}

pub async fn biwork_delete_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "disable",
        "llm_provider",
        provider_id.to_string(),
        None,
    )
    .await?;
    let mut tx = state.connect_pool.begin().await?;
    let model_profile_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM llm_model_profiles
        WHERE tenant_id = $1 AND provider_id = $2
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(provider_id)
    .fetch_all(&mut *tx)
    .await?;
    ensure_model_profiles_not_referenced_by_fixed_assistants_tx(
        &mut tx,
        ctx.tenant_id,
        &model_profile_ids,
    )
    .await?;
    let deleted = sqlx::query_scalar::<_, Uuid>(
        r#"
        UPDATE llm_providers
        SET status = 'deleted',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id
        "#,
    )
    .bind(provider_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    if deleted.is_none() {
        return Err(AppError::NotFound("provider not found".to_string()));
    }
    sqlx::query(
        r#"
        DELETE FROM llm_local_secrets secret
        USING llm_credentials credential
        WHERE credential.tenant_id = $1
          AND credential.provider_id = $2
          AND credential.secret_ref = 'local://' || secret.id::text
          AND secret.tenant_id = $1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(provider_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE llm_credentials
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            rotation_status = 'revoked',
            auto_rotation_enabled = false,
            rotation_interval_seconds = NULL,
            next_rotation_at = NULL,
            rotation_started_at = NULL,
            rotation_claim_id = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND provider_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(provider_id)
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(Value::Null))
}

pub async fn biwork_fetch_provider_models(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let provider = biwork_load_provider(&state, ctx.tenant_id, provider_id).await?;
    Ok(ok(json!({
        "models": provider.get("models").cloned().unwrap_or_else(|| json!([])),
    })))
}

pub async fn biwork_fetch_model_list(Json(payload): Json<Value>) -> Result<Json<Value>, AppError> {
    let protocol = provider_protocol_for_fetch(&payload)?;
    let api_key = required_provider_api_key(&payload, protocol)?;
    let base_url = provider_base_url_for_protocol(&payload, protocol)?;
    let try_fix = payload
        .get("try_fix")
        .or_else(|| payload.get("tryFix"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let result = fetch_provider_models_http(protocol, &base_url, &api_key, try_fix)
        .await
        .map_err(AppError::InvalidInput)?;
    let mut response = json!({
        "models": result.models,
    });
    if let Some(fixed_base_url) = result.fixed_base_url
        && let Some(object) = response.as_object_mut()
    {
        object.insert("fixed_base_url".to_string(), json!(fixed_base_url));
    }
    Ok(ok(response))
}

pub async fn biwork_detect_provider_protocol(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let api_key = trimmed_string(&payload, "api_key")
        .or_else(|| trimmed_string(&payload, "apiKey"))
        .unwrap_or_default();
    let preferred = payload
        .get("preferredProtocol")
        .or_else(|| payload.get("preferred_protocol"))
        .and_then(Value::as_str);
    let protocols = provider_protocol_detection_candidates(&payload, preferred);
    let try_fix = true;
    let mut last_error = None;

    if !api_key.trim().is_empty() {
        for protocol in protocols {
            let base_url = match provider_base_url_for_protocol_name(&payload, protocol) {
                Ok(base_url) => base_url,
                Err(err) => {
                    last_error = Some(err.to_string());
                    continue;
                }
            };
            match fetch_provider_models_http(protocol, &base_url, &api_key, try_fix).await {
                Ok(result) => {
                    return Ok(ok(provider_detection_success_response(result)));
                }
                Err(err) => last_error = Some(err),
            }
        }
    }

    let guessed = provider_protocol_hint(
        trimmed_string(&payload, "base_url")
            .or_else(|| trimmed_string(&payload, "baseUrl"))
            .as_deref(),
        Some(api_key.as_str()),
    )
    .unwrap_or("unknown");
    Ok(ok(json!({
        "success": false,
        "code": "PROVIDER_PROTOCOL_DETECTION_FAILED",
        "protocol": guessed,
        "confidence": if guessed == "unknown" { 0 } else { 40 },
        "error": last_error.clone().unwrap_or_else(|| "api_key is required for provider protocol detection".to_string()),
        "message": last_error.unwrap_or_else(|| "api_key is required for provider protocol detection".to_string()),
        "details": {
            "protocol": guessed,
        },
        "suggestion": {
            "type": "check_key",
            "message": "Check the provider URL and API key, then retry protocol detection.",
            "i18nKey": "settings.protocolDetection.checkKey",
        },
    })))
}

async fn biwork_load_provider(
    state: &AppState,
    tenant_id: Uuid,
    provider_id: Uuid,
) -> Result<Value, AppError> {
    let row = sqlx::query(
        r#"
        SELECT p.id, p.provider_key, p.display_name, p.base_url, p.status,
               credential.secret_mask, credential.secret_count,
               COALESCE(jsonb_agg(DISTINCT mp.model_name) FILTER (WHERE mp.id IS NOT NULL), '[]'::jsonb) AS models,
               COALESCE(
                   jsonb_object_agg(mp.model_name, COALESCE(mp.display_name, mp.model_name))
                       FILTER (WHERE mp.id IS NOT NULL),
                   '{}'::jsonb
               ) AS model_labels,
               COALESCE(
                   jsonb_object_agg(mp.model_name, mp.id::text) FILTER (WHERE mp.id IS NOT NULL),
                   '{}'::jsonb
               ) AS model_profile_ids
        FROM llm_providers p
        LEFT JOIN llm_model_profiles mp
          ON mp.provider_id = p.id
         AND mp.tenant_id = p.tenant_id
         AND mp.status = 'active'
        LEFT JOIN LATERAL (
          SELECT c.secret_mask, c.secret_count
          FROM llm_credentials c
          WHERE c.provider_id = p.id
            AND c.tenant_id = p.tenant_id
            AND c.revoked_at IS NULL
            AND c.rotation_status = 'active'
          ORDER BY c.created_at DESC, c.id DESC
          LIMIT 1
        ) credential ON TRUE
        WHERE p.id = $1 AND p.tenant_id = $2
        GROUP BY p.id, p.provider_key, p.display_name, p.base_url, p.status,
                 credential.secret_mask, credential.secret_count
        "#,
    )
    .bind(provider_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("provider not found".to_string()))?;
    let models = row.try_get::<Value, _>("models")?;
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?.to_string(),
        "platform": row.try_get::<String, _>("provider_key")?,
        "name": row.try_get::<String, _>("display_name")?,
        "base_url": row.try_get::<Option<String>, _>("base_url")?.unwrap_or_default(),
        "api_key": "",
        "api_key_mask": row.try_get::<Option<String>, _>("secret_mask")?,
        "api_key_count": row.try_get::<Option<i32>, _>("secret_count")?.unwrap_or(0),
        "models": models,
        "model_labels": row.try_get::<Value, _>("model_labels")?,
        "model_profile_ids": row.try_get::<Value, _>("model_profile_ids")?,
        "enabled": row.try_get::<String, _>("status")? == "active",
        "model_enabled": model_enabled_map(&models),
    }))
}

async fn replace_biwork_provider_models_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    provider_id: Uuid,
    models: Option<&Value>,
    model_labels: Option<&Value>,
    credential_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(models) = models.and_then(Value::as_array) else {
        return Ok(());
    };
    let selected_model_names = models
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();
    let existing_profiles = sqlx::query(
        r#"
        SELECT id, model_name
        FROM llm_model_profiles
        WHERE tenant_id = $1 AND provider_id = $2 AND status = 'active'
        "#,
    )
    .bind(tenant_id)
    .bind(provider_id)
    .fetch_all(&mut **tx)
    .await?;
    let removed_profile_ids = existing_profiles
        .iter()
        .filter_map(|row| {
            let model_name = row.try_get::<String, _>("model_name").ok()?;
            (!selected_model_names.contains(model_name.as_str()))
                .then(|| row.try_get::<Uuid, _>("id").ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    ensure_model_profiles_not_referenced_by_fixed_assistants_tx(
        tx,
        tenant_id,
        &removed_profile_ids,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE llm_model_profiles
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND provider_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(provider_id)
    .execute(&mut **tx)
    .await?;
    for model in models {
        let Some(model_name) = model
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let display_name = model_labels
            .and_then(Value::as_object)
            .and_then(|labels| labels.get(model_name))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(model_name);
        let profile_name = format!("{provider_id}:{model_name}");
        sqlx::query(
            r#"
            INSERT INTO llm_model_profiles (
                tenant_id, provider_id, credential_id, profile_name, model_name, display_name, status
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'active')
            ON CONFLICT (tenant_id, profile_name)
            DO UPDATE SET model_name = EXCLUDED.model_name,
                          display_name = EXCLUDED.display_name,
                          provider_id = EXCLUDED.provider_id,
                          credential_id = EXCLUDED.credential_id,
                          status = 'active',
                          updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(tenant_id)
        .bind(provider_id)
        .bind(credential_id)
        .bind(profile_name)
        .bind(model_name)
        .bind(display_name)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(super) async fn ensure_model_profiles_not_referenced_by_fixed_assistants_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    model_profile_ids: &[Uuid],
) -> Result<(), AppError> {
    if model_profile_ids.is_empty() {
        return Ok(());
    }
    let assistant_names = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT agent.name
        FROM agent_versions version
        JOIN agents agent
          ON agent.id = version.agent_id
         AND agent.tenant_id = version.tenant_id
         AND agent.deleted_at IS NULL
         AND agent.status <> 'disabled'
        JOIN llm_model_profiles profile
          ON profile.tenant_id = version.tenant_id
         AND profile.id::text = COALESCE(
               version.config_snapshot->>'model_profile_id',
               version.config_snapshot#>>'{agent,model_profile_id}'
             )
        WHERE version.tenant_id = $1
          AND version.status = 'published'
          AND profile.id = ANY($2)
          AND COALESCE(version.config_snapshot#>>'{defaults,model,mode}', 'fixed') <> 'auto'
        ORDER BY agent.name
        LIMIT 20
        "#,
    )
    .bind(tenant_id)
    .bind(model_profile_ids)
    .fetch_all(&mut **tx)
    .await?;
    if assistant_names.is_empty() {
        return Ok(());
    }
    Err(AppError::Conflict(format!(
        "model is used by fixed assistant(s): {}; reassign those assistants before deleting the model or provider",
        assistant_names.join(", ")
    )))
}

fn provider_api_key_replacement(payload: &Value) -> Result<Option<String>, AppError> {
    let Some(raw) = payload
        .get("api_key")
        .or_else(|| payload.get("apiKey"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let keys = raw
        .split([',', '\n', '\r'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if keys.len() != 1 {
        return Err(AppError::InvalidInput(
            "only one API key is supported; replace it as one complete value".to_string(),
        ));
    }
    Ok(Some(keys[0].to_string()))
}

fn mask_api_key(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() < 4 {
        return format!("******{value}");
    }
    let prefix = chars.iter().take(4).collect::<String>();
    let suffix = chars.iter().rev().take(4).rev().collect::<String>();
    format!("{prefix}******{suffix}")
}

async fn active_provider_credential_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    provider_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar(
        r#"
        SELECT id
        FROM llm_credentials
        WHERE tenant_id = $1
          AND provider_id = $2
          AND revoked_at IS NULL
          AND rotation_status = 'active'
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(provider_id)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn replace_biwork_provider_credential_tx(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    tenant_id: Uuid,
    provider_id: Uuid,
    actor_user_id: Uuid,
    api_key: &str,
) -> Result<Uuid, AppError> {
    sqlx::query(
        r#"
        DELETE FROM llm_local_secrets secret
        USING llm_credentials credential
        WHERE credential.tenant_id = $1
          AND credential.provider_id = $2
          AND credential.secret_ref = 'local://' || secret.id::text
          AND secret.tenant_id = $1
        "#,
    )
    .bind(tenant_id)
    .bind(provider_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE llm_credentials
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            rotation_status = 'revoked',
            auto_rotation_enabled = false,
            rotation_interval_seconds = NULL,
            next_rotation_at = NULL,
            rotation_started_at = NULL,
            rotation_claim_id = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND provider_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(provider_id)
    .execute(&mut **tx)
    .await?;

    let secret_id = Uuid::new_v4();
    let encryption_key =
        super::super::secret_resolver::local_secret_encryption_key(&state.internal_shared_token);
    sqlx::query(
        r#"
        INSERT INTO llm_local_secrets (
            id, tenant_id, ciphertext, created_by_user_id
        )
        VALUES (
            $1, $2, pgp_sym_encrypt($3, $4, 'cipher-algo=aes256, compress-algo=0'), $5
        )
        "#,
    )
    .bind(secret_id)
    .bind(tenant_id)
    .bind(api_key)
    .bind(encryption_key)
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;

    let secret_hash = format!("sha256:{}", hex::encode(Sha256::digest(api_key.as_bytes())));
    let credential_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO llm_credentials (
            id, tenant_id, provider_id, secret_ref, secret_hash,
            secret_mask, secret_count, created_by_user_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, 1, $7)
        "#,
    )
    .bind(credential_id)
    .bind(tenant_id)
    .bind(provider_id)
    .bind(format!("local://{secret_id}"))
    .bind(secret_hash)
    .bind(mask_api_key(api_key))
    .bind(actor_user_id)
    .execute(&mut **tx)
    .await?;
    Ok(credential_id)
}

fn model_enabled_map(models: &Value) -> Value {
    let mut map = Map::new();
    if let Some(items) = models.as_array() {
        for item in items {
            if let Some(model) = item.as_str() {
                map.insert(model.to_string(), Value::Bool(true));
            }
        }
    }
    Value::Object(map)
}

struct ProviderModelFetch {
    protocol: &'static str,
    models: Vec<Value>,
    fixed_base_url: Option<String>,
    latency_ms: u128,
}

fn provider_protocol_for_fetch(payload: &Value) -> Result<&'static str, AppError> {
    let platform = trimmed_string(payload, "platform").unwrap_or_else(|| "openai".to_string());
    normalize_provider_protocol_name(&platform).ok_or_else(|| {
        AppError::InvalidInput(format!(
            "provider platform does not support Rust model discovery: {platform}"
        ))
    })
}

fn normalize_provider_protocol_name(value: &str) -> Option<&'static str> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value == "unknown" {
        return None;
    }
    if value.contains("gemini") || value.contains("google") {
        return Some("gemini");
    }
    if value.contains("anthropic") || value.contains("claude") {
        return Some("anthropic");
    }
    if value.contains("bedrock") {
        return None;
    }
    Some("openai")
}

fn first_provider_api_key(value: &str) -> Option<String> {
    value
        .split([',', '\n', '\r'])
        .map(str::trim)
        .find(|item| !item.is_empty())
        .map(ToOwned::to_owned)
}

fn required_provider_api_key(payload: &Value, protocol: &str) -> Result<String, AppError> {
    let raw = trimmed_string(payload, "api_key")
        .or_else(|| trimmed_string(payload, "apiKey"))
        .ok_or_else(|| AppError::InvalidInput(format!("api_key is required for {protocol}")))?;
    first_provider_api_key(&raw)
        .ok_or_else(|| AppError::InvalidInput(format!("api_key is required for {protocol}")))
}

fn provider_base_url_for_protocol(payload: &Value, protocol: &str) -> Result<String, AppError> {
    provider_base_url_for_protocol_name(payload, protocol).map_err(AppError::InvalidInput)
}

fn provider_base_url_for_protocol_name(payload: &Value, protocol: &str) -> Result<String, String> {
    if let Some(base_url) = trimmed_string(payload, "base_url")
        .or_else(|| trimmed_string(payload, "baseUrl"))
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(base_url);
    }
    match protocol {
        "openai" => Ok("https://api.openai.com/v1".to_string()),
        "gemini" => Ok("https://generativelanguage.googleapis.com/v1beta".to_string()),
        "anthropic" => Ok("https://api.anthropic.com/v1".to_string()),
        other => Err(format!("unsupported provider protocol: {other}")),
    }
}

fn provider_protocol_hint(base_url: Option<&str>, api_key: Option<&str>) -> Option<&'static str> {
    if let Some(key) = api_key.and_then(first_provider_api_key) {
        if key.starts_with("AIza") {
            return Some("gemini");
        }
        if key.starts_with("sk-ant-") {
            return Some("anthropic");
        }
    }
    let base_url = base_url.unwrap_or_default().to_ascii_lowercase();
    if base_url.contains("generativelanguage.googleapis.com")
        || base_url.contains("gemini.google.com")
        || base_url.contains("googleapis.com")
    {
        return Some("gemini");
    }
    if base_url.contains("anthropic.com") || base_url.contains("claude") {
        return Some("anthropic");
    }
    if base_url.starts_with("http://") || base_url.starts_with("https://") {
        return Some("openai");
    }
    None
}

fn provider_protocol_detection_candidates(
    payload: &Value,
    preferred: Option<&str>,
) -> Vec<&'static str> {
    let mut candidates = Vec::new();
    if let Some(protocol) = preferred.and_then(normalize_provider_protocol_name) {
        candidates.push(protocol);
    }
    let base_url =
        trimmed_string(payload, "base_url").or_else(|| trimmed_string(payload, "baseUrl"));
    let api_key = trimmed_string(payload, "api_key").or_else(|| trimmed_string(payload, "apiKey"));
    if let Some(protocol) = provider_protocol_hint(base_url.as_deref(), api_key.as_deref())
        && !candidates.contains(&protocol)
    {
        candidates.push(protocol);
    }
    for protocol in ["openai", "gemini", "anthropic"] {
        if !candidates.contains(&protocol) {
            candidates.push(protocol);
        }
    }
    candidates
}

fn provider_model_base_candidates(protocol: &str, base_url: &str, try_fix: bool) -> Vec<String> {
    let mut candidates = vec![base_url.trim().trim_end_matches('/').to_string()];
    if protocol == "openai"
        && try_fix
        && let Some(fixed) = provider_base_with_v1(base_url)
        && !candidates.contains(&fixed)
    {
        candidates.push(fixed);
    }
    candidates
}

fn provider_base_with_v1(base_url: &str) -> Option<String> {
    let mut url = Url::parse(base_url.trim()).ok()?;
    let current = url.path().trim_end_matches('/');
    if current.ends_with("/v1") || current == "/v1" {
        return None;
    }
    let next = if current.is_empty() {
        "/v1".to_string()
    } else {
        format!("{current}/v1")
    };
    url.set_path(&next);
    Some(url.to_string().trim_end_matches('/').to_string())
}

fn provider_models_url(protocol: &str, base_url: &str, api_key: &str) -> Result<Url, String> {
    let mut url = Url::parse(base_url.trim())
        .map_err(|err| format!("provider base_url is invalid: {err}"))?;
    let current = url.path().trim_end_matches('/');
    if !current.ends_with("/models") {
        let next = if current.is_empty() {
            "/models".to_string()
        } else {
            format!("{current}/models")
        };
        url.set_path(&next);
    }
    if protocol == "gemini" {
        url.query_pairs_mut().append_pair("key", api_key);
    }
    Ok(url)
}

async fn fetch_provider_models_http(
    protocol: &'static str,
    base_url: &str,
    api_key: &str,
    try_fix: bool,
) -> Result<ProviderModelFetch, String> {
    let client = reqwest::Client::builder()
        .timeout(StdDuration::from_secs(15))
        .build()
        .map_err(|err| format!("failed to build provider HTTP client: {err}"))?;
    let started_at = Instant::now();
    let mut last_error = None;
    for candidate_base in provider_model_base_candidates(protocol, base_url, try_fix) {
        let url = provider_models_url(protocol, &candidate_base, api_key)?;
        let request = match protocol {
            "gemini" => client.get(url),
            "anthropic" => client
                .get(url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01"),
            _ => client.get(url).bearer_auth(api_key),
        };
        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                last_error = Some(format!("{protocol} model list request failed: {err}"));
                continue;
            }
        };
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| format!("{protocol} model list response read failed: {err}"))?;
        if !status.is_success() {
            last_error = Some(format!(
                "{protocol} model list returned HTTP {}: {}",
                status.as_u16(),
                body.chars().take(240).collect::<String>()
            ));
            continue;
        }
        let json_body = serde_json::from_str::<Value>(&body)
            .map_err(|err| format!("{protocol} model list JSON parse failed: {err}"))?;
        let models = extract_provider_models(protocol, &json_body);
        if models.is_empty() {
            last_error = Some(format!(
                "{protocol} model list response did not contain models"
            ));
            continue;
        }
        let fixed_base_url = if candidate_base != base_url.trim().trim_end_matches('/') {
            Some(candidate_base)
        } else {
            None
        };
        return Ok(ProviderModelFetch {
            protocol,
            models,
            fixed_base_url,
            latency_ms: started_at.elapsed().as_millis(),
        });
    }
    Err(last_error.unwrap_or_else(|| "provider model discovery failed".to_string()))
}

fn extract_provider_models(protocol: &str, body: &Value) -> Vec<Value> {
    let Some(items) = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut models = Vec::new();
    let mut seen = Vec::<String>::new();
    for item in items {
        let model_id = if let Some(value) = item.as_str() {
            Some(value)
        } else {
            item.get("id")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
        };
        let Some(model_id) = model_id else {
            continue;
        };
        let normalized = normalize_provider_model_id(protocol, model_id);
        if normalized.is_empty() || seen.contains(&normalized) {
            continue;
        }
        seen.push(normalized.clone());
        models.push(json!(normalized));
    }
    models
}

fn normalize_provider_model_id(protocol: &str, model_id: &str) -> String {
    let model_id = model_id.trim();
    if protocol == "gemini" {
        return model_id
            .strip_prefix("models/")
            .unwrap_or(model_id)
            .to_string();
    }
    model_id.to_string()
}

fn provider_model_value_id(value: &Value) -> Option<String> {
    value.as_str().map(ToOwned::to_owned).or_else(|| {
        value
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn provider_detection_success_response(result: ProviderModelFetch) -> Value {
    let models = result
        .models
        .iter()
        .filter_map(provider_model_value_id)
        .collect::<Vec<_>>();
    let latency = i64::try_from(result.latency_ms).unwrap_or(i64::MAX);
    let mut response = json!({
        "success": true,
        "protocol": result.protocol,
        "confidence": 95,
        "latency": latency,
        "models": models,
        "suggestion": {
            "type": "none",
            "message": "Provider protocol detected successfully.",
            "i18nKey": "settings.protocolDetection.success",
        },
    });
    if let Some(fixed_base_url) = result.fixed_base_url
        && let Some(object) = response.as_object_mut()
    {
        object.insert("fixedBaseUrl".to_string(), json!(fixed_base_url));
    }
    response
}

#[cfg(test)]
mod tests {
    use sqlx::postgres::PgPoolOptions;

    use super::*;

    #[test]
    fn provider_model_parser_accepts_openai_and_gemini_shapes() {
        let openai = extract_provider_models(
            "openai",
            &json!({
                "data": [
                    { "id": "gpt-4.1" },
                    { "id": "gpt-4.1" },
                    { "id": "gpt-4o-mini" }
                ]
            }),
        );
        assert_eq!(openai, vec![json!("gpt-4.1"), json!("gpt-4o-mini")]);

        let gemini = extract_provider_models(
            "gemini",
            &json!({
                "models": [
                    { "name": "models/gemini-2.0-flash" },
                    { "name": "models/gemini-1.5-pro" }
                ]
            }),
        );
        assert_eq!(
            gemini,
            vec![json!("gemini-2.0-flash"), json!("gemini-1.5-pro")]
        );
    }

    #[test]
    fn provider_model_base_candidates_try_fix_openai_v1() {
        assert_eq!(
            provider_model_base_candidates("openai", "https://api.example.test", true),
            vec![
                "https://api.example.test".to_string(),
                "https://api.example.test/v1".to_string()
            ]
        );
        assert_eq!(
            provider_model_base_candidates("openai", "https://api.example.test/v1", true),
            vec!["https://api.example.test/v1".to_string()]
        );
        assert_eq!(
            provider_model_base_candidates(
                "gemini",
                "https://generativelanguage.googleapis.com/v1beta",
                true
            ),
            vec!["https://generativelanguage.googleapis.com/v1beta".to_string()]
        );
    }

    #[test]
    fn provider_protocol_detection_candidates_use_preferred_then_hints() {
        let payload = json!({
            "base_url": "https://generativelanguage.googleapis.com/v1beta",
            "api_key": "AIThisIsTestKeywxyz123456789"
        });

        assert_eq!(
            provider_protocol_detection_candidates(&payload, Some("anthropic"))[..3],
            ["anthropic", "gemini", "openai"]
        );
        assert_eq!(
            provider_protocol_detection_candidates(&payload, None)[..3],
            ["gemini", "openai", "anthropic"]
        );
    }

    #[test]
    fn api_key_mask_uses_fixed_six_asterisks() {
        assert_eq!(mask_api_key("sk-example-secret-1234"), "sk-e******1234");
        assert_eq!(mask_api_key("abcd"), "abcd******abcd");
        assert_eq!(mask_api_key("abc"), "******abc");
        assert_eq!(mask_api_key("密钥"), "******密钥");
    }

    #[test]
    fn provider_api_key_replacement_is_single_and_complete() {
        assert_eq!(
            provider_api_key_replacement(&json!({ "api_key": "  sk-test  " })).unwrap(),
            Some("sk-test".to_string())
        );
        assert_eq!(
            provider_api_key_replacement(&json!({ "api_key": "" })).unwrap(),
            None
        );
        assert!(provider_api_key_replacement(&json!({ "api_key": "first\nsecond" })).is_err());
        assert!(provider_api_key_replacement(&json!({ "api_key": "first,second" })).is_err());
    }

    #[tokio::test]
    async fn fixed_assistant_blocks_model_deletion_but_auto_assistant_does_not()
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
        let profile_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Model reference guard test")
            .bind(format!("model-reference-guard-{tenant_id}"))
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_providers (id, tenant_id, provider_key, display_name, status)
            VALUES ($1, $2, 'custom', $3, 'active')
            "#,
        )
        .bind(provider_id)
        .bind(tenant_id)
        .bind(format!("Provider {provider_id}"))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_model_profiles (
                id, tenant_id, provider_id, profile_name, model_name, status
            )
            VALUES ($1, $2, $3, $4, 'guard-model', 'active')
            "#,
        )
        .bind(profile_id)
        .bind(tenant_id)
        .bind(provider_id)
        .bind(format!("{provider_id}:guard-model"))
        .execute(&mut *tx)
        .await?;

        let runtime_id = Uuid::new_v4();
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
        let fixed_agent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO assistants (id, tenant_id, runtime_id, name, draft_config, status) VALUES ($1, $2, $3, 'Fixed assistant', jsonb_build_object('engine_agent_id', $3::text), 'active')",
        )
        .bind(fixed_agent_id)
        .bind(tenant_id)
        .bind(runtime_id)
        .execute(&mut *tx)
        .await?;
        let fixed_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agent_versions (
                tenant_id, agent_id, version_label, config_snapshot, status
            ) VALUES ($1, $2, 'fixed-v1', $3, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(fixed_agent_id)
        .bind(json!({
            "model_profile_id": profile_id,
            "defaults": {"model": {"mode": "fixed", "value": profile_id}}
        }))
        .fetch_one(&mut *tx)
        .await?;
        assert!(
            ensure_model_profiles_not_referenced_by_fixed_assistants_tx(
                &mut tx,
                tenant_id,
                &[profile_id],
            )
            .await
            .is_err()
        );

        sqlx::query("UPDATE agent_versions SET status = 'disabled' WHERE id = $1")
            .bind(fixed_version_id)
            .execute(&mut *tx)
            .await?;
        let auto_agent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO assistants (id, tenant_id, runtime_id, name, draft_config, status) VALUES ($1, $2, $3, 'Auto assistant', jsonb_build_object('engine_agent_id', $3::text), 'active')",
        )
        .bind(auto_agent_id)
        .bind(tenant_id)
        .bind(runtime_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_versions (
                tenant_id, agent_id, version_label, config_snapshot, status
            ) VALUES ($1, $2, 'auto-v1', $3, 'published')
            "#,
        )
        .bind(tenant_id)
        .bind(auto_agent_id)
        .bind(json!({
            "model_profile_id": profile_id,
            "defaults": {"model": {"mode": "auto"}}
        }))
        .execute(&mut *tx)
        .await?;
        ensure_model_profiles_not_referenced_by_fixed_assistants_tx(
            &mut tx,
            tenant_id,
            &[profile_id],
        )
        .await?;

        tx.rollback().await?;
        Ok(())
    }
}
