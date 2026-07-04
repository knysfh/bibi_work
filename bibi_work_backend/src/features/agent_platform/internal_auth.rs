use axum::{
    extract::{Request, State},
    http::header::AUTHORIZATION,
    middleware::Next,
    response::Response,
};
use constant_time_eq::constant_time_eq;

use crate::{features::core::errors::AppError, startup::AppState};

pub async fn internal_token_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let Some(token) = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Err(AppError::Unauthorized(
            "missing internal bearer token".to_string(),
        ));
    };

    if !constant_time_eq(token.as_bytes(), state.internal_shared_token.as_bytes()) {
        return Err(AppError::Unauthorized(
            "invalid internal bearer token".to_string(),
        ));
    }

    Ok(next.run(request).await)
}
