use std::num::TryFromIntError;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use sqlx::Error as SqlxError;
use thiserror::Error;
use tracing::error;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database connection error")]
    DatabaseConnection,
    #[error("Database query error")]
    DatabaseQuery,
    #[error("SQL query timed out")]
    DatabaseQueryTimeout,
    #[error("Failed to commit transaction")]
    DatabaseTransaction,
    #[error("Resource not found: {0}")]
    NotFound(String),
    #[error("Resource conflict: {0}")]
    Conflict(String),
    #[error("Workspace revision conflict: {0}")]
    WorkspaceRevisionConflict(String),
    #[error("Permission Denied: {0}")]
    PermissionDenied(String),
    #[error("Failed to connect to Redis")]
    RedisConnection,
    #[error("Failed to execute Redis command")]
    RedisCommand,
    #[error("Object store error: {0}")]
    ObjectStore(String),
    #[error("Upstream service unavailable: {0}")]
    UpstreamUnavailable(String),
    #[error("Invalid input, input: {0}")]
    InvalidInput(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Missing Authorization header")]
    MissingAuthHeader,
    #[error("Invalid Authorization header format")]
    InvalidAuthHeaderFormat,
}

impl From<SqlxError> for AppError {
    fn from(err: SqlxError) -> Self {
        match err {
            SqlxError::Database(database_error) => {
                error!(
                    database_code = database_error.code().as_deref(),
                    database_constraint = database_error.constraint(),
                    database_table = database_error.table(),
                    database_message = database_error.message(),
                    "database query failed"
                );
                AppError::DatabaseQuery
            }
            SqlxError::Io(error) => {
                error!(error = %error, "database I/O failed");
                AppError::DatabaseConnection
            }
            SqlxError::Tls(error) => {
                error!(error = %error, "database TLS failed");
                AppError::DatabaseConnection
            }
            SqlxError::PoolTimedOut => {
                error!("database pool acquisition timed out");
                AppError::DatabaseConnection
            }
            SqlxError::PoolClosed => {
                error!("database pool is closed");
                AppError::DatabaseConnection
            }
            SqlxError::RowNotFound => {
                error!("database query unexpectedly returned no rows");
                AppError::DatabaseQuery
            }
            error => {
                error!(error = %error, "database query failed");
                AppError::DatabaseQuery
            }
        }
    }
}

impl From<TryFromIntError> for AppError {
    fn from(err: TryFromIntError) -> Self {
        error!("Integer overflow for pagination: {}", err);
        AppError::InvalidInput("Pagination parameters are out of range.".to_string())
    }
}

impl From<redis::RedisError> for AppError {
    fn from(err: redis::RedisError) -> Self {
        if err.is_io_error() {
            AppError::RedisConnection
        } else {
            AppError::RedisCommand
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, legacy_code, message) = match self {
            AppError::DatabaseConnection => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_CONNECTION_FAILED",
                None,
                "Failed to get a transaction".to_string(),
            ),
            AppError::DatabaseQuery => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_QUERY_FAILED",
                None,
                "Database query error".to_string(),
            ),
            AppError::DatabaseQueryTimeout => (
                StatusCode::GATEWAY_TIMEOUT,
                "DATABASE_QUERY_TIMEOUT",
                None,
                "The database query took too long to execute.".to_string(),
            ),
            AppError::DatabaseTransaction => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "DATABASE_TRANSACTION_FAILED",
                None,
                "Failed to commit transaction".to_string(),
            ),
            AppError::NotFound(content) => (StatusCode::NOT_FOUND, "NOT_FOUND", None, content),
            AppError::Conflict(content) => (StatusCode::CONFLICT, "CONFLICT", None, content),
            AppError::WorkspaceRevisionConflict(content) => (
                StatusCode::CONFLICT,
                "WORKSPACE_REVISION_CONFLICT",
                None,
                content,
            ),
            AppError::PermissionDenied(content) => (
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                Some("PERMISSION_DENIED"),
                format!("Not have permission to perform, {content}"),
            ),
            AppError::RedisConnection => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "REDIS_CONNECTION_FAILED",
                None,
                "Internal server error (redis connection)".to_string(),
            ),
            AppError::RedisCommand => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "REDIS_COMMAND_FAILED",
                None,
                "Internal server error (redis command)".to_string(),
            ),
            AppError::ObjectStore(content) => (
                StatusCode::BAD_GATEWAY,
                "OBJECT_STORE_FAILED",
                None,
                content,
            ),
            AppError::UpstreamUnavailable(content) => (
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_UNAVAILABLE",
                None,
                content,
            ),
            AppError::InvalidInput(content) => (
                StatusCode::BAD_REQUEST,
                "VALIDATION_ERROR",
                Some("INVALID_INPUT"),
                format!("Invalid input, input: {}", content),
            ),
            AppError::Unauthorized(content) => {
                (StatusCode::UNAUTHORIZED, "UNAUTHORIZED", None, content)
            }
            AppError::MissingAuthHeader => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                Some("MISSING_AUTH_HEADER"),
                "Authorization header is required".to_string(),
            ),
            AppError::InvalidAuthHeaderFormat => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED",
                Some("INVALID_AUTH_HEADER_FORMAT"),
                "Authorization header must start with 'Bearer '".to_string(),
            ),
        };

        let mut error_response = json!({
            "success": false,
            "trace_id": Uuid::new_v4().to_string(),
            "code": code,
            "error": message,
            "message": message,
            "details": {},
        });
        if let Some(legacy_code) = legacy_code
            && let Some(object) = error_response.as_object_mut()
        {
            object.insert("legacy_code".to_string(), json!(legacy_code));
            object.insert("details".to_string(), json!({ "legacy_code": legacy_code }));
        }

        (status, Json(error_response)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn app_error_body(error: AppError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&body).unwrap();
        (status, value)
    }

    #[tokio::test]
    async fn app_error_response_uses_biwork_error_envelope() {
        let (status, body) =
            app_error_body(AppError::InvalidInput("name is required".to_string())).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"], false);
        assert_eq!(body["code"], "VALIDATION_ERROR");
        assert_eq!(body["legacy_code"], "INVALID_INPUT");
        assert_eq!(body["details"]["legacy_code"], "INVALID_INPUT");
        assert_eq!(body["error"], body["message"]);
        assert!(Uuid::parse_str(body["trace_id"].as_str().unwrap()).is_ok());
    }

    #[tokio::test]
    async fn workspace_revision_conflict_uses_biwork_contract_code() {
        let (status, body) = app_error_body(AppError::WorkspaceRevisionConflict(
            "stale write".to_string(),
        ))
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["success"], false);
        assert_eq!(body["code"], "WORKSPACE_REVISION_CONFLICT");
        assert_eq!(body["error"], "stale write");
    }

    #[tokio::test]
    async fn auth_and_permission_errors_use_stable_biwork_codes() {
        let (missing_status, missing_body) = app_error_body(AppError::MissingAuthHeader).await;
        assert_eq!(missing_status, StatusCode::UNAUTHORIZED);
        assert_eq!(missing_body["code"], "UNAUTHORIZED");
        assert_eq!(missing_body["legacy_code"], "MISSING_AUTH_HEADER");

        let (forbidden_status, forbidden_body) =
            app_error_body(AppError::PermissionDenied("x".to_string())).await;
        assert_eq!(forbidden_status, StatusCode::FORBIDDEN);
        assert_eq!(forbidden_body["code"], "FORBIDDEN");
        assert_eq!(forbidden_body["legacy_code"], "PERMISSION_DENIED");
    }
}
