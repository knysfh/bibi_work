use axum::{Extension, Json, extract::State};

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn api_authz_check(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuthzCheckRequest>,
) -> Result<Json<AuthzDecision>, AppError> {
    let mut check = payload;
    let decision = if check.actor.user_id != ctx.platform_user_id {
        AuthzDecision::deny("request-validation", "actor_mismatch")
    } else {
        normalize_request_actor(&mut check, &ctx);
        state.authz_service.check(&check).await
    };

    write_authz_audit(&state.connect_pool, &check, &decision).await?;
    Ok(Json(decision))
}

pub async fn api_authz_batch_check(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<AuthzBatchCheckRequest>,
) -> Result<Json<AuthzBatchCheckResponse>, AppError> {
    let mut decisions = Vec::with_capacity(payload.checks.len());
    for mut check in payload.checks {
        let decision = if check.actor.user_id != ctx.platform_user_id {
            AuthzDecision::deny("request-validation", "actor_mismatch")
        } else {
            normalize_request_actor(&mut check, &ctx);
            state.authz_service.check(&check).await
        };
        write_authz_audit(&state.connect_pool, &check, &decision).await?;
        decisions.push(decision);
    }

    Ok(Json(AuthzBatchCheckResponse { decisions }))
}

pub async fn internal_authz_check(
    State(state): State<AppState>,
    Json(payload): Json<AuthzCheckRequest>,
) -> Result<Json<AuthzDecision>, AppError> {
    let decision = state.authz_service.check(&payload).await;
    write_authz_audit(&state.connect_pool, &payload, &decision).await?;
    Ok(Json(decision))
}

pub async fn internal_authz_batch_check(
    State(state): State<AppState>,
    Json(payload): Json<AuthzBatchCheckRequest>,
) -> Result<Json<AuthzBatchCheckResponse>, AppError> {
    let response = state.authz_service.batch_check(&payload).await;
    for (check, decision) in payload.checks.iter().zip(response.decisions.iter()) {
        write_authz_audit(&state.connect_pool, check, decision).await?;
    }
    Ok(Json(response))
}
