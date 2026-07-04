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
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', c.secret_ref IS NOT NULL,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id
               ) AS metadata,
               c.created_at, c.revoked_at AS updated_at
        FROM inserted c
        JOIN llm_providers p ON p.id = c.provider_id
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.provider_id)
    .bind(payload.owner_scope.unwrap_or_else(|| "tenant".to_string()))
    .bind(payload.owner_resource_id)
    .bind(payload.secret_ref)
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
                 'owner_scope', c.owner_scope,
                 'owner_resource_id', c.owner_resource_id,
                 'has_secret_ref', c.secret_ref IS NOT NULL,
                 'has_secret_hash', c.secret_hash IS NOT NULL,
                 'expires_at', c.expires_at,
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id
               ) AS metadata,
               c.created_at, c.revoked_at AS updated_at
        FROM llm_credentials c
        JOIN llm_providers p ON p.id = c.provider_id
        WHERE c.tenant_id = $1
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
    let row = sqlx::query(
        r#"
        WITH updated AS (
          UPDATE llm_credentials
          SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
              rotation_status = 'revoked'
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
                 'revoked_at', c.revoked_at,
                 'created_by_user_id', c.created_by_user_id
               ) AS metadata,
               c.created_at, c.revoked_at AS updated_at
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
    let target = load_llm_profile_test_target(&state, payload.tenant_id, profile_id).await?;
    let url = llm_models_url(&target.base_url)?;
    let timeout_ms =
        json_u64(&target.default_headers_template, "test_timeout_ms").unwrap_or(10_000);
    let http = Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .build()
        .map_err(|err| AppError::InvalidInput(format!("failed to build LLM test client: {err}")))?;
    let mut request = http.get(url);
    request = apply_llm_test_auth(request, &target)?;

    let started = Instant::now();
    let response = request.send().await;
    let latency_ms = started.elapsed().as_millis();
    let result = match response {
        Ok(response) => {
            let status = response.status();
            LlmProfileTestResponse {
                success: status.is_success(),
                provider_key: target.provider_key,
                model_name: target.model_name,
                http_status: Some(status.as_u16()),
                latency_ms,
                message: if status.is_success() {
                    "LLM provider connection succeeded".to_string()
                } else {
                    format!("LLM provider returned HTTP {}", status.as_u16())
                },
            }
        }
        Err(err) => LlmProfileTestResponse {
            success: false,
            provider_key: target.provider_key,
            model_name: target.model_name,
            http_status: None,
            latency_ms,
            message: format!("LLM provider request failed: {err}"),
        },
    };
    Ok(Json(result))
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
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("llm model profile not found".to_string()))?;

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

fn apply_llm_test_auth(
    request: reqwest::RequestBuilder,
    target: &LlmProfileTestTarget,
) -> Result<reqwest::RequestBuilder, AppError> {
    let Some(secret_ref) = target.secret_ref.as_deref() else {
        return Ok(request);
    };
    let secret = secret_resolver::resolve_secret_ref(secret_ref)?;
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
