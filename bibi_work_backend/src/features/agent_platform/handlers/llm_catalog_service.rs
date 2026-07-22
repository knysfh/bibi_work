use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::time::Instant;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*, secret_resolver},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

#[derive(Debug, Deserialize)]
pub struct LlmCredentialListQuery {
    pub tenant_id: Option<Uuid>,
    pub provider_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LlmProfileTestResponse {
    pub success: bool,
    pub provider_key: String,
    pub model_name: String,
    pub http_status: Option<u16>,
    pub latency_ms: u128,
    pub message: String,
    pub model_available: Option<bool>,
    pub checked_model_count: Option<usize>,
}

struct LlmProfileTestTarget {
    provider_key: String,
    model_name: String,
    base_url: String,
    auth_scheme: String,
    default_headers_template: Value,
    secret_ref: Option<String>,
}

pub async fn list_llm_providers(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<ResourceResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, display_name AS name, provider_key AS description, status,
               jsonb_build_object(
                   'provider_key', provider_key,
                   'base_url', base_url,
                   'auth_scheme', auth_scheme,
                   'default_headers_template', default_headers_template
               ) AS metadata,
               created_at, updated_at
        FROM llm_providers
        WHERE tenant_id = $1
          AND ($2::text IS NULL OR status = $2)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;

    let providers = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(providers))
}

pub async fn get_llm_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, display_name AS name, provider_key AS description, status,
               jsonb_build_object(
                   'provider_key', provider_key,
                   'base_url', base_url,
                   'auth_scheme', auth_scheme,
                   'default_headers_template', default_headers_template
               ) AS metadata,
               created_at, updated_at
        FROM llm_providers
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(provider_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm provider not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_llm_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
    Json(payload): Json<UpdateLlmProviderRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "llm_provider",
        provider_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE llm_providers
        SET display_name = COALESCE($3, display_name),
            base_url = COALESCE($4, base_url),
            auth_scheme = COALESCE($5, auth_scheme),
            default_headers_template = COALESCE($6, default_headers_template),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, display_name AS name, provider_key AS description, status,
                  jsonb_build_object(
                      'provider_key', provider_key,
                      'base_url', base_url,
                      'auth_scheme', auth_scheme,
                      'default_headers_template', default_headers_template
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(provider_id)
    .bind(payload.tenant_id)
    .bind(payload.display_name)
    .bind(payload.base_url)
    .bind(payload.auth_scheme)
    .bind(payload.default_headers_template)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm provider not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_llm_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(provider_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "llm_provider",
        provider_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE llm_providers
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, display_name AS name, provider_key AS description, status,
                  jsonb_build_object(
                      'provider_key', provider_key,
                      'base_url', base_url,
                      'auth_scheme', auth_scheme,
                      'default_headers_template', default_headers_template
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(provider_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm provider not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_llm_provider(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateLlmProviderRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "llm_provider").await?;
    let row = sqlx::query(
        r#"
        INSERT INTO llm_providers (
            tenant_id, provider_key, display_name, base_url, auth_scheme, default_headers_template
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, display_name AS name, NULL::text AS description, status,
                  jsonb_build_object(
                    'provider_key', provider_key,
                    'base_url', base_url,
                    'auth_scheme', auth_scheme,
                    'default_headers_template', default_headers_template
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.provider_key)
    .bind(payload.display_name)
    .bind(payload.base_url)
    .bind(payload.auth_scheme.unwrap_or_else(|| "bearer".to_string()))
    .bind(
        payload
            .default_headers_template
            .unwrap_or_else(|| json!({})),
    )
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_llm_credential(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateLlmCredentialRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "llm_credential").await?;
    ensure_llm_provider_available(&state, payload.tenant_id, payload.provider_id).await?;
    let secret_ref = normalize_secret_ref(payload.secret_ref)?;
    let row = sqlx::query(
        r#"
        WITH inserted AS (
        INSERT INTO llm_credentials (
            tenant_id, provider_id, owner_scope, owner_resource_id,
            secret_ref, secret_hash, expires_at, created_by_user_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING *
        )
        SELECT c.id, c.tenant_id,
               concat('credential ', left(c.id::text, 8)) AS name,
               p.display_name AS description,
               c.rotation_status AS status,
               jsonb_build_object(
                 'provider_id', c.provider_id,
                 'provider_key', p.provider_key,
                 'provider_name', p.display_name,
                 'resolver_scheme', CASE
                   WHEN c.secret_ref LIKE 'local://%' THEN 'local'
                   WHEN c.secret_ref LIKE 'vault://%' THEN 'vault'
                   WHEN c.secret_ref LIKE 'kms://%' THEN 'kms'
                   ELSE 'env'
                 END,
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', c.secret_ref IS NOT NULL,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'last_rotated_at', c.last_rotated_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id,
                 'rotated_by_user_id', c.rotated_by_user_id,
                 'auto_rotation_enabled', c.auto_rotation_enabled,
                 'rotation_interval_seconds', c.rotation_interval_seconds,
                 'rotate_before_seconds', c.rotate_before_seconds,
                 'next_rotation_at', c.next_rotation_at,
                 'rotation_attempts', c.rotation_attempts,
                 'rotation_error', c.rotation_error
               ) AS metadata,
               c.created_at, c.updated_at
        FROM inserted c
        JOIN llm_providers p ON p.id = c.provider_id
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.provider_id)
    .bind(payload.owner_scope.unwrap_or_else(|| "tenant".to_string()))
    .bind(payload.owner_resource_id)
    .bind(secret_ref)
    .bind(payload.secret_hash)
    .bind(payload.expires_at)
    .bind(ctx.platform_user_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_llm_credentials(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<LlmCredentialListQuery>,
) -> Result<Json<Vec<ResourceResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT c.id, c.tenant_id,
               concat('credential ', left(c.id::text, 8)) AS name,
               p.display_name AS description,
               CASE WHEN c.revoked_at IS NOT NULL THEN 'revoked' ELSE c.rotation_status END AS status,
               jsonb_build_object(
                 'provider_id', c.provider_id,
                 'provider_key', p.provider_key,
                 'provider_name', p.display_name,
                 'resolver_scheme', CASE
                   WHEN c.secret_ref LIKE 'local://%' THEN 'local'
                   WHEN c.secret_ref LIKE 'vault://%' THEN 'vault'
                   WHEN c.secret_ref LIKE 'kms://%' THEN 'kms'
                   ELSE 'env'
                 END,
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', c.secret_ref IS NOT NULL,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'last_rotated_at', c.last_rotated_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id,
                 'rotated_by_user_id', c.rotated_by_user_id,
                 'auto_rotation_enabled', c.auto_rotation_enabled,
                 'rotation_interval_seconds', c.rotation_interval_seconds,
                 'rotate_before_seconds', c.rotate_before_seconds,
                 'next_rotation_at', c.next_rotation_at,
                 'rotation_attempts', c.rotation_attempts,
                 'rotation_error', c.rotation_error
               ) AS metadata,
               c.created_at, c.updated_at
        FROM llm_credentials c
        JOIN llm_providers p ON p.id = c.provider_id
        WHERE c.tenant_id = $1
          AND p.status <> 'deleted'
          AND ($2::uuid IS NULL OR c.provider_id = $2)
          AND (
            $3::text IS NULL
            OR CASE WHEN c.revoked_at IS NOT NULL THEN 'revoked' ELSE c.rotation_status END = $3
          )
        ORDER BY c.created_at DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(query.provider_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;

    let credentials = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(credentials))
}

pub async fn rotate_llm_credential(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(credential_id): Path<Uuid>,
    Json(payload): Json<RotateLlmCredentialRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "rotate",
        "llm_credential",
        credential_id.to_string(),
        None,
    )
    .await?;
    secret_resolver::revoke_runtime_credentials_for_credential(&state, credential_id).await?;
    let secret_ref = normalize_secret_ref(payload.secret_ref)?;
    let row = sqlx::query(
        r#"
        WITH updated AS (
          UPDATE llm_credentials
          SET secret_ref = $3,
              secret_hash = $4,
              expires_at = $5,
              rotation_status = 'active',
              last_rotated_at = CURRENT_TIMESTAMP,
              rotated_by_user_id = $6,
              rotation_started_at = NULL,
              rotation_claim_id = NULL,
              rotation_attempts = 0,
              rotation_error = NULL,
              next_rotation_at = CASE
                  WHEN auto_rotation_enabled THEN CURRENT_TIMESTAMP
                      + rotation_interval_seconds * INTERVAL '1 second'
                  ELSE NULL
              END,
              updated_at = CURRENT_TIMESTAMP
          WHERE id = $1
            AND tenant_id = $2
            AND revoked_at IS NULL
          RETURNING *
        )
        SELECT c.id, c.tenant_id,
               concat('credential ', left(c.id::text, 8)) AS name,
               p.display_name AS description,
               c.rotation_status AS status,
               jsonb_build_object(
                 'provider_id', c.provider_id,
                 'provider_key', p.provider_key,
                 'provider_name', p.display_name,
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', c.secret_ref IS NOT NULL,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'last_rotated_at', c.last_rotated_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id,
                 'rotated_by_user_id', c.rotated_by_user_id,
                 'auto_rotation_enabled', c.auto_rotation_enabled,
                 'rotation_interval_seconds', c.rotation_interval_seconds,
                 'rotate_before_seconds', c.rotate_before_seconds,
                 'next_rotation_at', c.next_rotation_at,
                 'rotation_attempts', c.rotation_attempts,
                 'rotation_error', c.rotation_error
               ) AS metadata,
               c.created_at, c.updated_at
        FROM updated c
        JOIN llm_providers p ON p.id = c.provider_id
        "#,
    )
    .bind(credential_id)
    .bind(payload.tenant_id)
    .bind(secret_ref)
    .bind(payload.secret_hash)
    .bind(payload.expires_at)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| {
        AppError::NotFound("active llm credential not found for rotation".to_string())
    })?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn revoke_llm_credential(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(credential_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "revoke",
        "llm_credential",
        credential_id.to_string(),
        None,
    )
    .await?;
    secret_resolver::revoke_runtime_credentials_for_credential(&state, credential_id).await?;
    let row = sqlx::query(
        r#"
        WITH updated AS (
          UPDATE llm_credentials
          SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
              rotation_status = 'revoked',
              auto_rotation_enabled = FALSE,
              rotation_interval_seconds = NULL,
              next_rotation_at = NULL,
              rotation_started_at = NULL,
              rotation_claim_id = NULL,
              updated_at = CURRENT_TIMESTAMP
          WHERE id = $1 AND tenant_id = $2
          RETURNING *
        )
        SELECT c.id, c.tenant_id,
               concat('credential ', left(c.id::text, 8)) AS name,
               p.display_name AS description,
               'revoked' AS status,
               jsonb_build_object(
                 'provider_id', c.provider_id,
                 'provider_key', p.provider_key,
                 'provider_name', p.display_name,
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', c.secret_ref IS NOT NULL,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'last_rotated_at', c.last_rotated_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id,
                 'rotated_by_user_id', c.rotated_by_user_id,
                 'auto_rotation_enabled', c.auto_rotation_enabled,
                 'rotation_interval_seconds', c.rotation_interval_seconds,
                 'rotate_before_seconds', c.rotate_before_seconds,
                 'next_rotation_at', c.next_rotation_at,
                 'rotation_attempts', c.rotation_attempts,
                 'rotation_error', c.rotation_error
               ) AS metadata,
               c.created_at, c.updated_at
        FROM updated c
        JOIN llm_providers p ON p.id = c.provider_id
        "#,
    )
    .bind(credential_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm credential not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_llm_credential_rotation_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(credential_id): Path<Uuid>,
    Json(payload): Json<UpdateLlmCredentialRotationPolicyRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "configure_rotation",
        "llm_credential",
        credential_id.to_string(),
        None,
    )
    .await?;
    if payload.enabled {
        let secret_ref = sqlx::query_scalar::<_, String>(
            "SELECT secret_ref FROM llm_credentials WHERE id = $1 AND tenant_id = $2 AND revoked_at IS NULL",
        )
        .bind(credential_id)
        .bind(payload.tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
        .ok_or_else(|| AppError::NotFound("active llm credential not found".to_string()))?;
        if secret_ref.starts_with("local://") {
            return Err(AppError::InvalidInput(
                "locally stored credentials are replaced manually and cannot use automatic rotation"
                    .to_string(),
            ));
        }
    }
    let interval_seconds = if payload.enabled {
        if !state.credential_rotation_worker_enabled
            || !state.secret_resolver.rotation_gateway_configured()
        {
            return Err(AppError::Conflict(
                "automatic credential rotation is not configured on this server".to_string(),
            ));
        }
        Some(
            payload
                .interval_seconds
                .filter(|value| (300..=31_536_000).contains(value))
                .ok_or_else(|| {
                    AppError::InvalidInput(
                        "enabled rotation policy requires interval_seconds between 300 and 31536000"
                            .to_string(),
                    )
                })?,
        )
    } else {
        None
    };
    let rotate_before_seconds = payload.rotate_before_seconds.unwrap_or(86_400);
    if !(0..=2_592_000).contains(&rotate_before_seconds) {
        return Err(AppError::InvalidInput(
            "rotate_before_seconds must be between 0 and 2592000".to_string(),
        ));
    }
    let row = sqlx::query(
        r#"
        WITH updated AS (
          UPDATE llm_credentials
          SET auto_rotation_enabled = $3,
              rotation_interval_seconds = $4,
              rotate_before_seconds = $5,
              next_rotation_at = CASE
                  WHEN $3 THEN CURRENT_TIMESTAMP + $4 * INTERVAL '1 second'
                  ELSE NULL
              END,
              rotation_started_at = NULL,
              rotation_claim_id = NULL,
              rotation_attempts = 0,
              rotation_error = NULL,
              updated_at = CURRENT_TIMESTAMP
          WHERE id = $1 AND tenant_id = $2 AND revoked_at IS NULL
          RETURNING *
        )
        SELECT c.id, c.tenant_id,
               concat('credential ', left(c.id::text, 8)) AS name,
               p.display_name AS description,
               c.rotation_status AS status,
               jsonb_build_object(
                 'provider_id', c.provider_id,
                 'provider_key', p.provider_key,
                 'provider_name', p.display_name,
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', TRUE,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'last_rotated_at', c.last_rotated_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id,
                 'rotated_by_user_id', c.rotated_by_user_id,
                 'auto_rotation_enabled', c.auto_rotation_enabled,
                 'rotation_interval_seconds', c.rotation_interval_seconds,
                 'rotate_before_seconds', c.rotate_before_seconds,
                 'next_rotation_at', c.next_rotation_at,
                 'rotation_attempts', c.rotation_attempts,
                 'rotation_error', c.rotation_error
               ) AS metadata,
               c.created_at, c.updated_at
        FROM updated c JOIN llm_providers p ON p.id = c.provider_id
        "#,
    )
    .bind(credential_id)
    .bind(payload.tenant_id)
    .bind(payload.enabled)
    .bind(interval_seconds)
    .bind(rotate_before_seconds)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("active llm credential not found".to_string()))?;
    Ok(Json(resource_from_row(row)?))
}

pub async fn get_llm_credential_rotation_health(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<LlmCredentialRotationHealthQuery>,
) -> Result<Json<Value>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, query.tenant_id, "read", "llm_credential").await?;
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) FILTER (WHERE auto_rotation_enabled AND revoked_at IS NULL) AS enabled,
               COUNT(*) FILTER (
                   WHERE auto_rotation_enabled AND revoked_at IS NULL
                     AND next_rotation_at <= CURRENT_TIMESTAMP
               ) AS due,
               COUNT(*) FILTER (WHERE rotation_started_at IS NOT NULL) AS running,
               COUNT(*) FILTER (WHERE rotation_error IS NOT NULL) AS credentials_with_errors,
               (SELECT COUNT(*) FROM llm_credential_rotation_attempts attempt
                WHERE attempt.tenant_id = $1 AND attempt.status = 'failed'
                  AND attempt.started_at > CURRENT_TIMESTAMP - INTERVAL '24 hours') AS failed_24h
        FROM llm_credentials credential
        JOIN llm_providers provider ON provider.id = credential.provider_id
        WHERE credential.tenant_id = $1
          AND provider.status <> 'deleted'
        "#,
    )
    .bind(query.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    Ok(Json(json!({
        "tenant_id": query.tenant_id,
        "worker_enabled": state.credential_rotation_worker_enabled,
        "gateway_configured": state.secret_resolver.rotation_gateway_configured(),
        "enabled_credentials": row.try_get::<i64, _>("enabled")?,
        "due_credentials": row.try_get::<i64, _>("due")?,
        "running_rotations": row.try_get::<i64, _>("running")?,
        "credentials_with_errors": row.try_get::<i64, _>("credentials_with_errors")?,
        "failed_attempts_24h": row.try_get::<i64, _>("failed_24h")?,
        "healthy": row.try_get::<i64, _>("credentials_with_errors")? == 0
            && (row.try_get::<i64, _>("enabled")? == 0
                || (state.credential_rotation_worker_enabled
                    && state.secret_resolver.rotation_gateway_configured()))
    })))
}

pub async fn list_llm_credential_rotation_attempts(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<LlmCredentialRotationAttemptQuery>,
) -> Result<Json<Vec<Value>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, query.tenant_id, "read", "llm_credential").await?;
    let status = query.status.as_deref().map(str::trim);
    if status.is_some_and(|value| !matches!(value, "running" | "succeeded" | "failed")) {
        return Err(AppError::InvalidInput(
            "rotation attempt status must be running, succeeded, or failed".to_string(),
        ));
    }
    let rows = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT jsonb_build_object(
            'id', id, 'tenant_id', tenant_id, 'credential_id', credential_id,
            'status', status, 'resolver_scheme', resolver_scheme,
            'previous_ref_hash', previous_ref_hash, 'new_ref_hash', new_ref_hash,
            'error_summary', error_summary, 'started_at', started_at,
            'completed_at', completed_at
        )
        FROM llm_credential_rotation_attempts
        WHERE tenant_id = $1 AND ($2::TEXT IS NULL OR status = $2)
        ORDER BY started_at DESC, id DESC LIMIT $3
        "#,
    )
    .bind(query.tenant_id)
    .bind(status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;
    Ok(Json(rows))
}

pub async fn list_llm_model_profiles(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<ResourceResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, profile_name AS name, model_name AS description, status,
               jsonb_build_object(
                   'provider_id', provider_id,
                   'credential_id', credential_id,
                   'context_window', context_window,
                   'max_input_tokens', max_input_tokens,
                   'max_output_tokens', max_output_tokens,
                   'temperature', temperature,
                   'top_p', top_p,
                   'reasoning_effort', reasoning_effort,
                   'response_format', response_format,
                   'tool_choice_policy', tool_choice_policy,
                   'rate_limit_policy', rate_limit_policy,
                   'cost_policy', cost_policy
               ) AS metadata,
               created_at, updated_at
        FROM llm_model_profiles
        WHERE tenant_id = $1
          AND ($2::text IS NULL OR status = $2)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;

    let profiles = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(profiles))
}

pub async fn get_llm_model_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(profile_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, profile_name AS name, model_name AS description, status,
               jsonb_build_object(
                   'provider_id', provider_id,
                   'credential_id', credential_id,
                   'context_window', context_window,
                   'max_input_tokens', max_input_tokens,
                   'max_output_tokens', max_output_tokens,
                   'temperature', temperature,
                   'top_p', top_p,
                   'reasoning_effort', reasoning_effort,
                   'response_format', response_format,
                   'tool_choice_policy', tool_choice_policy,
                   'rate_limit_policy', rate_limit_policy,
                   'cost_policy', cost_policy
               ) AS metadata,
               created_at, updated_at
        FROM llm_model_profiles
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(profile_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm model profile not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn test_llm_model_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(profile_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<LlmProfileTestResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "read",
        "llm_model_profile",
        profile_id.to_string(),
        None,
    )
    .await?;
    Ok(Json(
        test_llm_model_profile_for_tenant(&state, payload.tenant_id, profile_id).await?,
    ))
}

pub(super) async fn test_llm_model_profile_for_tenant(
    state: &AppState,
    tenant_id: Uuid,
    profile_id: Uuid,
) -> Result<LlmProfileTestResponse, AppError> {
    let target = load_llm_profile_test_target(state, tenant_id, profile_id).await?;
    let url = llm_models_url(&target.base_url)?;
    let timeout_ms =
        json_u64(&target.default_headers_template, "test_timeout_ms").unwrap_or(10_000);
    let http = Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .build()
        .map_err(|err| AppError::InvalidInput(format!("failed to build LLM test client: {err}")))?;
    let mut request = http.get(url);
    request = apply_llm_test_auth(state, tenant_id, request, &target).await?;

    let started = Instant::now();
    let response = request.send().await;
    let latency_ms = started.elapsed().as_millis();
    let result = match response {
        Ok(response) => {
            let status = response.status();
            let http_status = Some(status.as_u16());
            if !status.is_success() {
                LlmProfileTestResponse {
                    success: false,
                    provider_key: target.provider_key,
                    model_name: target.model_name,
                    http_status,
                    latency_ms,
                    message: format!("LLM provider returned HTTP {}", status.as_u16()),
                    model_available: None,
                    checked_model_count: None,
                }
            } else {
                match response.json::<Value>().await {
                    Ok(body) => {
                        let model_ids = extract_llm_model_ids(&body);
                        let model_available = model_ids
                            .iter()
                            .any(|candidate| llm_model_id_matches(candidate, &target.model_name));
                        LlmProfileTestResponse {
                            success: model_available,
                            provider_key: target.provider_key,
                            model_name: target.model_name.clone(),
                            http_status,
                            latency_ms,
                            message: if model_available {
                                "LLM provider connection succeeded and target model is available"
                                    .to_string()
                            } else {
                                format!(
                                    "LLM provider responded, but model '{}' was not found",
                                    target.model_name
                                )
                            },
                            model_available: Some(model_available),
                            checked_model_count: Some(model_ids.len()),
                        }
                    }
                    Err(err) => LlmProfileTestResponse {
                        success: false,
                        provider_key: target.provider_key,
                        model_name: target.model_name,
                        http_status,
                        latency_ms,
                        message: format!("LLM provider response did not contain valid JSON: {err}"),
                        model_available: None,
                        checked_model_count: None,
                    },
                }
            }
        }
        Err(err) => LlmProfileTestResponse {
            success: false,
            provider_key: target.provider_key,
            model_name: target.model_name,
            http_status: None,
            latency_ms,
            message: format!("LLM provider request failed: {err}"),
            model_available: None,
            checked_model_count: None,
        },
    };
    Ok(result)
}

pub async fn update_llm_model_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(profile_id): Path<Uuid>,
    Json(payload): Json<UpdateLlmModelProfileRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "llm_model_profile",
        profile_id.to_string(),
        None,
    )
    .await?;
    if payload.credential_id.is_some() {
        let provider_id: Uuid = sqlx::query_scalar(
            r#"
            SELECT provider_id
            FROM llm_model_profiles
            WHERE id = $1 AND tenant_id = $2
            "#,
        )
        .bind(profile_id)
        .bind(payload.tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
        .ok_or_else(|| AppError::NotFound("llm model profile not found".to_string()))?;
        ensure_llm_credential_matches_provider(
            &state,
            payload.tenant_id,
            provider_id,
            payload.credential_id,
        )
        .await?;
    }
    let row = sqlx::query(
        r#"
        UPDATE llm_model_profiles
        SET credential_id = COALESCE($3, credential_id),
            profile_name = COALESCE($4, profile_name),
            model_name = COALESCE($5, model_name),
            context_window = COALESCE($6, context_window),
            max_input_tokens = COALESCE($7, max_input_tokens),
            max_output_tokens = COALESCE($8, max_output_tokens),
            temperature = COALESCE($9, temperature),
            top_p = COALESCE($10, top_p),
            reasoning_effort = COALESCE($11, reasoning_effort),
            response_format = COALESCE($12, response_format),
            tool_choice_policy = COALESCE($13, tool_choice_policy),
            rate_limit_policy = COALESCE($14, rate_limit_policy),
            cost_policy = COALESCE($15, cost_policy),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, profile_name AS name, model_name AS description, status,
                  jsonb_build_object(
                    'provider_id', provider_id,
                    'credential_id', credential_id,
                    'context_window', context_window,
                    'max_input_tokens', max_input_tokens,
                    'max_output_tokens', max_output_tokens,
                    'temperature', temperature,
                    'top_p', top_p,
                    'reasoning_effort', reasoning_effort,
                    'response_format', response_format,
                    'tool_choice_policy', tool_choice_policy,
                    'rate_limit_policy', rate_limit_policy,
                    'cost_policy', cost_policy
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(profile_id)
    .bind(payload.tenant_id)
    .bind(payload.credential_id)
    .bind(payload.profile_name)
    .bind(payload.model_name)
    .bind(payload.context_window)
    .bind(payload.max_input_tokens)
    .bind(payload.max_output_tokens)
    .bind(payload.temperature)
    .bind(payload.top_p)
    .bind(payload.reasoning_effort)
    .bind(payload.response_format)
    .bind(payload.tool_choice_policy)
    .bind(payload.rate_limit_policy)
    .bind(payload.cost_policy)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm model profile not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_llm_model_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(profile_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "llm_model_profile",
        profile_id.to_string(),
        None,
    )
    .await?;
    let mut tx = state.connect_pool.begin().await?;
    super::biwork_provider_service::ensure_model_profiles_not_referenced_by_fixed_assistants_tx(
        &mut tx,
        payload.tenant_id,
        &[profile_id],
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE llm_model_profiles
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, profile_name AS name, model_name AS description, status,
                  jsonb_build_object(
                    'provider_id', provider_id,
                    'credential_id', credential_id,
                    'context_window', context_window,
                    'max_input_tokens', max_input_tokens,
                    'max_output_tokens', max_output_tokens,
                    'temperature', temperature,
                    'top_p', top_p,
                    'reasoning_effort', reasoning_effort,
                    'response_format', response_format,
                    'tool_choice_policy', tool_choice_policy,
                    'rate_limit_policy', rate_limit_policy,
                    'cost_policy', cost_policy
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(profile_id)
    .bind(payload.tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("llm model profile not found".to_string()))?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_llm_model_profile(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateLlmModelProfileRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(
        &state,
        &ctx,
        payload.tenant_id,
        "create",
        "llm_model_profile",
    )
    .await?;
    ensure_llm_provider_available(&state, payload.tenant_id, payload.provider_id).await?;
    ensure_llm_credential_matches_provider(
        &state,
        payload.tenant_id,
        payload.provider_id,
        payload.credential_id,
    )
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO llm_model_profiles (
            tenant_id, provider_id, credential_id, profile_name, model_name,
            context_window, max_input_tokens, max_output_tokens, temperature, top_p,
            reasoning_effort, response_format, tool_choice_policy, rate_limit_policy, cost_policy
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
        RETURNING id, tenant_id, profile_name AS name, model_name AS description, status,
                  jsonb_build_object(
                    'provider_id', provider_id,
                    'credential_id', credential_id,
                    'context_window', context_window,
                    'max_input_tokens', max_input_tokens,
                    'max_output_tokens', max_output_tokens,
                    'temperature', temperature,
                    'top_p', top_p,
                    'reasoning_effort', reasoning_effort,
                    'response_format', response_format,
                    'tool_choice_policy', tool_choice_policy,
                    'rate_limit_policy', rate_limit_policy,
                    'cost_policy', cost_policy
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.provider_id)
    .bind(payload.credential_id)
    .bind(payload.profile_name)
    .bind(payload.model_name)
    .bind(payload.context_window)
    .bind(payload.max_input_tokens)
    .bind(payload.max_output_tokens)
    .bind(payload.temperature)
    .bind(payload.top_p)
    .bind(payload.reasoning_effort)
    .bind(payload.response_format.unwrap_or_else(|| json!({})))
    .bind(payload.tool_choice_policy.unwrap_or_else(|| json!({})))
    .bind(payload.rate_limit_policy.unwrap_or_else(|| json!({})))
    .bind(payload.cost_policy.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

async fn load_llm_profile_test_target(
    state: &AppState,
    tenant_id: Uuid,
    profile_id: Uuid,
) -> Result<LlmProfileTestTarget, AppError> {
    let row = sqlx::query(
        r#"
        SELECT mp.model_name, mp.credential_id,
               p.provider_key, p.base_url, p.auth_scheme, p.default_headers_template,
               c.secret_ref
        FROM llm_model_profiles mp
        JOIN llm_providers p ON p.id = mp.provider_id
        LEFT JOIN llm_credentials c
          ON c.id = mp.credential_id
         AND c.tenant_id = mp.tenant_id
         AND c.revoked_at IS NULL
         AND c.rotation_status = 'active'
         AND (c.expires_at IS NULL OR c.expires_at > CURRENT_TIMESTAMP)
        WHERE mp.id = $1
          AND mp.tenant_id = $2
          AND mp.status = 'active'
          AND p.status = 'active'
        "#,
    )
    .bind(profile_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm model profile not found".to_string()))?;

    let credential_id: Option<Uuid> = row.try_get("credential_id")?;
    let secret_ref: Option<String> = row.try_get("secret_ref")?;
    if credential_id.is_some() && secret_ref.is_none() {
        return Err(AppError::InvalidInput(
            "llm credential is not active".to_string(),
        ));
    }
    let base_url: Option<String> = row.try_get("base_url")?;
    let base_url = base_url
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("llm provider base_url is required".to_string()))?;

    Ok(LlmProfileTestTarget {
        provider_key: row.try_get("provider_key")?,
        model_name: row.try_get("model_name")?,
        base_url,
        auth_scheme: row.try_get("auth_scheme")?,
        default_headers_template: row.try_get("default_headers_template")?,
        secret_ref,
    })
}

fn llm_models_url(base_url: &str) -> Result<String, AppError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "llm provider base_url is required".to_string(),
        ));
    }
    if trimmed.ends_with("/models") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{trimmed}/models"))
    }
}

fn normalize_secret_ref(secret_ref: String) -> Result<String, AppError> {
    let trimmed = secret_ref.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "llm credential secret_ref is required".to_string(),
        ));
    }
    secret_resolver::validate_secret_ref(trimmed)?;
    Ok(trimmed.to_string())
}

fn extract_llm_model_ids(body: &Value) -> Vec<String> {
    let Some(models) = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
    else {
        return Vec::new();
    };

    let mut model_ids = Vec::new();
    for model in models {
        if let Some(id) = model.as_str().map(str::trim).filter(|id| !id.is_empty()) {
            model_ids.push(id.to_string());
            continue;
        }
        let Some(object) = model.as_object() else {
            continue;
        };
        for field in ["id", "name", "model"] {
            if let Some(id) = object
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                model_ids.push(id.to_string());
                break;
            }
        }
    }
    model_ids
}

fn llm_model_id_matches(candidate: &str, target: &str) -> bool {
    let normalize = |value: &str| value.trim().trim_start_matches("models/").to_string();
    let candidate = normalize(candidate);
    let target = normalize(target);
    candidate == target
}

async fn apply_llm_test_auth(
    state: &AppState,
    tenant_id: Uuid,
    request: reqwest::RequestBuilder,
    target: &LlmProfileTestTarget,
) -> Result<reqwest::RequestBuilder, AppError> {
    let Some(secret_ref) = target.secret_ref.as_deref() else {
        return Ok(request);
    };
    let secret = secret_resolver::resolve_secret_for_tenant(state, tenant_id, secret_ref).await?;
    match target.auth_scheme.as_str() {
        "bearer" => Ok(request.bearer_auth(secret)),
        "api_key_header" => {
            let header = json_string(&target.default_headers_template, "api_key_header")
                .or_else(|| json_string(&target.default_headers_template, "auth_header"))
                .unwrap_or_else(|| "x-api-key".to_string());
            Ok(request.header(header, secret))
        }
        "none" => Ok(request),
        other => Err(AppError::InvalidInput(format!(
            "unsupported llm auth_scheme for profile test: {other}"
        ))),
    }
}

async fn ensure_llm_provider_available(
    state: &AppState,
    tenant_id: Uuid,
    provider_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
          SELECT 1
          FROM llm_providers
          WHERE id = $1 AND tenant_id = $2 AND status = 'active'
        )
        "#,
    )
    .bind(provider_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "llm provider must be active and belong to the tenant".to_string(),
        ))
    }
}

fn json_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn json_u64(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

async fn ensure_llm_credential_matches_provider(
    state: &AppState,
    tenant_id: Uuid,
    provider_id: Uuid,
    credential_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(credential_id) = credential_id else {
        return Ok(());
    };
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
          SELECT 1
          FROM llm_credentials
          WHERE id = $1
            AND tenant_id = $2
            AND provider_id = $3
            AND revoked_at IS NULL
            AND rotation_status = 'active'
            AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
        )
        "#,
    )
    .bind(credential_id)
    .bind(tenant_id)
    .bind(provider_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "llm credential must be active and belong to the selected provider".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        extract_llm_model_ids, llm_model_id_matches, llm_models_url, normalize_secret_ref,
    };

    #[test]
    fn llm_models_url_appends_models_endpoint_once() {
        assert_eq!(
            llm_models_url("https://llm.example.test/v1").unwrap(),
            "https://llm.example.test/v1/models"
        );
        assert_eq!(
            llm_models_url("https://llm.example.test/v1/models").unwrap(),
            "https://llm.example.test/v1/models"
        );
    }

    #[test]
    fn normalizes_llm_credential_secret_ref() {
        assert_eq!(
            normalize_secret_ref(" env://OPENAI_API_KEY ".to_string()).unwrap(),
            "env://OPENAI_API_KEY"
        );
        assert!(normalize_secret_ref("   ".to_string()).is_err());
    }

    #[test]
    fn extracts_openai_and_gemini_model_ids() {
        let openai = json!({
            "data": [
                {"id": "gpt-5"},
                {"id": "gpt-5-mini"}
            ]
        });
        assert_eq!(extract_llm_model_ids(&openai), vec!["gpt-5", "gpt-5-mini"]);

        let gemini = json!({
            "models": [
                {"name": "models/gemini-2.5-pro"},
                {"name": "models/gemini-2.5-flash"}
            ]
        });
        assert_eq!(
            extract_llm_model_ids(&gemini),
            vec!["models/gemini-2.5-pro", "models/gemini-2.5-flash"]
        );
    }

    #[test]
    fn model_id_match_accepts_gemini_models_prefix_only() {
        assert!(llm_model_id_matches(
            "models/gemini-2.5-pro",
            "gemini-2.5-pro"
        ));
        assert!(llm_model_id_matches("gpt-5", "gpt-5"));
        assert!(!llm_model_id_matches("gpt-5-mini", "gpt-5"));
    }
}
