use axum::{
    Extension, Json,
    extract::{Query, State},
};

use crate::{
    features::{
        agent_platform::{
            audit::{
                AuditHashChainSealResponse, AuditHashChainVerifyResponse, seal_audit_hash_chain,
                verify_audit_hash_chain,
            },
            ferriskey_oidc::PlatformRequestContext,
            models::*,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn verify_audit_hash_chain_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<AuditHashChainVerifyQuery>,
) -> Result<Json<AuditHashChainVerifyResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        query.tenant_id,
        "verify",
        "audit_log",
        format!("tenant:{}", query.tenant_id),
        None,
    )
    .await?;

    let limit = query.limit.unwrap_or(1000).clamp(1, 10_000);
    let response = verify_audit_hash_chain(&state.connect_pool, query.tenant_id, limit).await?;
    Ok(Json(response))
}

pub async fn seal_audit_hash_chain_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuditHashChainSealRequest>,
) -> Result<Json<AuditHashChainSealResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "seal",
        "audit_log",
        format!("tenant:{}", payload.tenant_id),
        None,
    )
    .await?;

    let max_rows = payload.max_rows.unwrap_or(1000).clamp(1, 10_000);
    let response = seal_audit_hash_chain(
        &state.connect_pool,
        &state.rustfs_client,
        payload.tenant_id,
        Some(ctx.platform_user_id),
        max_rows,
    )
    .await?;
    Ok(Json(response))
}
