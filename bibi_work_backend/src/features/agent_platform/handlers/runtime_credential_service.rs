use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{agent_platform::secret_resolver, core::errors::AppError},
    startup::AppState,
};

#[derive(Debug, Deserialize)]
pub struct RuntimeCredentialQuery {
    tenant_id: Uuid,
    run_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct RuntimeCredentialResponse {
    tenant_id: Uuid,
    run_id: Uuid,
    credential_id: Uuid,
    provider_key: String,
    auth_scheme: String,
    secret: String,
    expires_at: OffsetDateTime,
}

pub async fn get_runtime_credential(
    State(state): State<AppState>,
    Path(runtime_credential_id): Path<String>,
    Query(query): Query<RuntimeCredentialQuery>,
) -> Result<Json<RuntimeCredentialResponse>, AppError> {
    let credential = secret_resolver::load_runtime_credential(
        &state,
        query.tenant_id,
        query.run_id,
        &runtime_credential_id,
    )
    .await?;

    Ok(Json(RuntimeCredentialResponse {
        tenant_id: credential.tenant_id,
        run_id: credential.run_id,
        credential_id: credential.credential_id,
        provider_key: credential.provider_key,
        auth_scheme: credential.auth_scheme,
        secret: credential.secret,
        expires_at: credential.expires_at,
    }))
}
