use axum::{
    Extension, Json,
    extract::{Query, State},
};

use crate::{
    features::{
        agent_platform::{
            audit::{
                AuditHashBackfillReport, AuditHashChainSealResponse, AuditHashChainVerifyResponse,
                audit_hash_backfill_status, backfill_historical_audit_hashes,
                seal_audit_hash_chain, verify_audit_hash_chain,
            },
            audit_governance::{
                AuditLegalHoldResponse, AuditPartitionCleanupResponse,
                AuditRetentionEligibilityResponse, audit_partition_cleanup,
                audit_retention_eligibility, create_audit_legal_hold, list_audit_legal_holds,
                release_audit_legal_hold,
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

pub async fn list_audit_legal_holds_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<AuditLegalHoldQuery>,
) -> Result<Json<Vec<AuditLegalHoldResponse>>, AppError> {
    require_audit_governance_access(&state, &ctx, query.tenant_id, "read").await?;
    let holds = list_audit_legal_holds(
        &state.connect_pool,
        query.tenant_id,
        query.status.as_deref(),
        query.limit.unwrap_or(100),
    )
    .await?;
    Ok(Json(holds))
}

pub async fn create_audit_legal_hold_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateAuditLegalHoldRequest>,
) -> Result<Json<AuditLegalHoldResponse>, AppError> {
    require_audit_governance_access(&state, &ctx, payload.tenant_id, "hold").await?;
    let hold = create_audit_legal_hold(&state.connect_pool, ctx.platform_user_id, payload).await?;
    Ok(Json(hold))
}

pub async fn release_audit_legal_hold_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    axum::extract::Path(hold_id): axum::extract::Path<uuid::Uuid>,
    Json(payload): Json<ReleaseAuditLegalHoldRequest>,
) -> Result<Json<AuditLegalHoldResponse>, AppError> {
    require_audit_governance_access(&state, &ctx, payload.tenant_id, "release").await?;
    let hold = release_audit_legal_hold(
        &state.connect_pool,
        payload.tenant_id,
        hold_id,
        ctx.platform_user_id,
    )
    .await?;
    Ok(Json(hold))
}

pub async fn audit_retention_eligibility_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<AuditRetentionEligibilityQuery>,
) -> Result<Json<AuditRetentionEligibilityResponse>, AppError> {
    require_audit_governance_access(&state, &ctx, query.tenant_id, "read").await?;
    let response = audit_retention_eligibility(
        &state.connect_pool,
        query.tenant_id,
        query.limit.unwrap_or(100),
    )
    .await?;
    Ok(Json(response))
}

pub async fn audit_hash_backfill_status_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<AuditHashBackfillQuery>,
) -> Result<Json<AuditHashBackfillReport>, AppError> {
    require_audit_governance_access(&state, &ctx, query.tenant_id, "verify").await?;
    Ok(Json(
        audit_hash_backfill_status(&state.connect_pool, query.tenant_id).await?,
    ))
}

pub async fn backfill_audit_hash_chain_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuditHashBackfillRequest>,
) -> Result<Json<AuditHashBackfillReport>, AppError> {
    require_audit_governance_access(&state, &ctx, payload.tenant_id, "backfill").await?;
    Ok(Json(
        backfill_historical_audit_hashes(&state.connect_pool, payload.tenant_id, payload.dry_run)
            .await?,
    ))
}

pub async fn cleanup_audit_partition_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuditPartitionCleanupRequest>,
) -> Result<Json<AuditPartitionCleanupResponse>, AppError> {
    if !ctx.roles.iter().any(|role| role == "platform_admin") {
        return Err(AppError::PermissionDenied(
            "audit partition cleanup requires the platform_admin role".to_string(),
        ));
    }
    require_audit_governance_access(&state, &ctx, payload.tenant_id, "cleanup").await?;
    Ok(Json(
        audit_partition_cleanup(
            &state.connect_pool,
            &payload.partition_name,
            payload.dry_run,
            state.audit_partition_cleanup_enabled,
        )
        .await?,
    ))
}

async fn require_audit_governance_access(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: uuid::Uuid,
    action: &str,
) -> Result<(), AppError> {
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        state,
        ctx,
        tenant_id,
        action,
        "audit_log",
        format!("tenant:{tenant_id}"),
        None,
    )
    .await?;
    Ok(())
}
