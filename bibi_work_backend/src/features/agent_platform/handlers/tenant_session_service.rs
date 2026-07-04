use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::{errors::AppError, models::GenericResponse},
    },
    startup::AppState,
};

use super::support::*;

pub async fn create_tenant(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateTenantRequest>,
) -> Result<Json<TenantResponse>, AppError> {
    let id = Uuid::new_v4();
    let metadata = payload.metadata.unwrap_or_else(|| json!({}));
    let mut tx = state.connect_pool.begin().await?;

    let row = sqlx::query(
        r#"
        INSERT INTO tenants (id, name, slug, metadata)
        VALUES ($1, $2, $3, $4)
        RETURNING id, name, slug, metadata, created_at
        "#,
    )
    .bind(id)
    .bind(payload.name)
    .bind(payload.slug)
    .bind(metadata)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
        VALUES ($1, $2, 'admin')
        ON CONFLICT (tenant_id, user_id) DO UPDATE SET role = EXCLUDED.role
        "#,
    )
    .bind(id)
    .bind(ctx.platform_user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Json(TenantResponse {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        slug: row.try_get("slug")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
    }))
}

pub async fn list_tenants(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Vec<TenantResponse>>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT t.id, t.name, t.slug, t.metadata, t.created_at
        FROM tenants t
        JOIN user_tenant_memberships m ON m.tenant_id = t.id
        WHERE m.user_id = $1 AND t.deleted_at IS NULL
        ORDER BY t.created_at DESC
        "#,
    )
    .bind(ctx.platform_user_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let tenants = rows
        .into_iter()
        .map(|row| {
            Ok(TenantResponse {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                slug: row.try_get("slug")?,
                metadata: row.try_get("metadata")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;

    Ok(Json(tenants))
}

pub async fn create_device(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateDeviceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let device_fingerprint = sha256_hex(
        format!(
            "{}:{}:{}",
            payload.platform,
            payload.device_name,
            payload.public_key.clone().unwrap_or_default()
        )
        .as_bytes(),
    );

    let row = sqlx::query(
        r#"
        INSERT INTO devices (
            tenant_id, user_id, device_fingerprint, device_name, platform, public_key,
            trust_level, last_seen_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP)
        ON CONFLICT (tenant_id, user_id, device_fingerprint)
        DO UPDATE SET
            device_name = EXCLUDED.device_name,
            platform = EXCLUDED.platform,
            public_key = EXCLUDED.public_key,
            trust_level = EXCLUDED.trust_level,
            revoked_at = NULL,
            last_seen_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        RETURNING id, tenant_id, device_name AS name, platform, trust_level AS status,
                  jsonb_build_object('public_key', public_key) AS metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(device_fingerprint)
    .bind(payload.device_name)
    .bind(payload.platform)
    .bind(payload.public_key)
    .bind(
        payload
            .trust_level
            .unwrap_or_else(|| "standard".to_string()),
    )
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_devices(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<DeviceResponse>>, AppError> {
    let tenant_id = query.tenant_id.unwrap_or(ctx.tenant_id);
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, device_name, platform, trust_level,
               last_seen_at, revoked_at, created_at, updated_at
        FROM devices
        WHERE tenant_id = $1 AND user_id = $2
        ORDER BY last_seen_at DESC NULLS LAST, created_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(ctx.platform_user_id)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let devices = rows
        .into_iter()
        .map(device_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(devices))
}

pub async fn revoke_device(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(device_id): Path<Uuid>,
    Json(payload): Json<RevokeDeviceRequest>,
) -> Result<Json<DeviceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let device = load_device(&state.connect_pool, payload.tenant_id, device_id).await?;
    if device.user_id != ctx.platform_user_id {
        require_ferriskey_allow(
            &state,
            &ctx,
            payload.tenant_id,
            "manage",
            "device",
            device_id.to_string(),
            None,
        )
        .await?;
    }

    let mut tx = state.connect_pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE devices
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, user_id, device_name, platform, trust_level,
                  last_seen_at, revoked_at, created_at, updated_at
        "#,
    )
    .bind(device_id)
    .bind(payload.tenant_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE platform_sessions
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND device_id = $2 AND revoked_at IS NULL
        "#,
    )
    .bind(payload.tenant_id)
    .bind(device_id)
    .execute(&mut *tx)
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Json(device_from_row(row)?))
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<SessionResponse>>, AppError> {
    let tenant_id = query.tenant_id.unwrap_or(ctx.tenant_id);
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, device_id, ferriskey_subject,
               ferriskey_session_state, token_jti, token_exp, roles_snapshot,
               last_seen_at, source_ip, user_agent, revoked_at, created_at, updated_at
        FROM platform_sessions
        WHERE tenant_id = $1 AND user_id = $2
        ORDER BY last_seen_at DESC NULLS LAST, created_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(ctx.platform_user_id)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let sessions = rows
        .into_iter()
        .map(session_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(sessions))
}

pub async fn revoke_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(session_id): Path<Uuid>,
    Json(payload): Json<RevokeSessionRequest>,
) -> Result<Json<SessionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let session = load_session(&state.connect_pool, payload.tenant_id, session_id).await?;
    if session.user_id != ctx.platform_user_id {
        require_ferriskey_allow(
            &state,
            &ctx,
            payload.tenant_id,
            "manage",
            "session",
            session_id.to_string(),
            None,
        )
        .await?;
    }

    let row = sqlx::query(
        r#"
        UPDATE platform_sessions
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, user_id, device_id, ferriskey_subject,
                  ferriskey_session_state, token_jti, token_exp, roles_snapshot,
                  last_seen_at, source_ip, user_agent, revoked_at, created_at, updated_at
        "#,
    )
    .bind(session_id)
    .bind(payload.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(session_from_row(row)?))
}

pub async fn logout_current_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<GenericResponse>, AppError> {
    sqlx::query(
        r#"
        UPDATE platform_sessions
        SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND user_id = $3
        "#,
    )
    .bind(ctx.session_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;

    Ok(Json(GenericResponse {
        code: "LOGGED_OUT".to_string(),
        message: "Current platform session revoked".to_string(),
    }))
}
