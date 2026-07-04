use std::num::TryFromIntError;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sqlx::Error as SqlxError;
use thiserror::Error;
use tracing::error;

use crate::features::core::models::GenericResponse;

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
    #[error("Permission Denied: {0}")]
    PermissionDenied(String),
    #[error("Failed to connect to Redis")]
    RedisConnection,
    #[error("Failed to execute Redis command")]
    RedisCommand,
    #[error("Object store error: {0}")]
    ObjectStore(String),
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
            SqlxError::Database(_) => AppError::DatabaseQuery,
            SqlxError::Io(_) | SqlxError::Tls(_) => AppError::DatabaseConnection,
            SqlxError::PoolTimedOut | SqlxError::PoolClosed => AppError::DatabaseConnection,
            SqlxError::RowNotFound => AppError::DatabaseQuery,
            _ => AppError::DatabaseQuery,
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
        let (status, error_response) = match self {
            AppError::DatabaseConnection => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GenericResponse {
                    code: "DATABASE_CONNECTION_FAILED".to_string(),
                    message: "Failed to get a transaction".to_string(),
                },
            ),
            AppError::DatabaseQuery => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GenericResponse {
                    code: "DATABASE_QUERY_FAILED".to_string(),
                    message: "Database query error".to_string(),
                },
            ),
            AppError::DatabaseQueryTimeout => (
                StatusCode::GATEWAY_TIMEOUT,
                GenericResponse {
                    code: "DATABASE_QUERY_TIMEOUT".to_string(),
                    message: "The database query took too long to execute.".to_string(),
                },
            ),
            AppError::DatabaseTransaction => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GenericResponse {
                    code: "DATABASE_TRANSACTION_FAILED".to_string(),
                    message: "Failed to commit transaction".to_string(),
                },
            ),
            AppError::NotFound(content) => (
                StatusCode::NOT_FOUND,
                GenericResponse {
                    code: "NOT_FOUND".to_string(),
                    message: content,
                },
            ),
            AppError::Conflict(content) => (
                StatusCode::CONFLICT,
                GenericResponse {
                    code: "CONFLICT".to_string(),
                    message: content,
                },
            ),
            AppError::PermissionDenied(content) => (
                StatusCode::FORBIDDEN,
                GenericResponse {
                    code: "PERMISSION_DENIED".to_string(),
                    message: format!("Not have permission to perform, {content}"),
                },
            ),
            AppError::RedisConnection => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GenericResponse {
                    code: "REDIS_CONNECTION_FAILED".to_string(),
                    message: "Internal server error (redis connection)".to_string(),
                },
            ),
            AppError::RedisCommand => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GenericResponse {
                    code: "REDIS_COMMAND_FAILED".to_string(),
                    message: "Internal server error (redis command)".to_string(),
                },
            ),
            AppError::ObjectStore(content) => (
                StatusCode::BAD_GATEWAY,
                GenericResponse {
                    code: "OBJECT_STORE_FAILED".to_string(),
                    message: content,
                },
            ),
            AppError::InvalidInput(content) => (
                StatusCode::BAD_REQUEST,
                GenericResponse {
                    code: "INVALID_INPUT".to_string(),
                    message: format!("Invalid input, input: {}", content),
                },
            ),
            AppError::Unauthorized(content) => (
                StatusCode::UNAUTHORIZED,
                GenericResponse {
                    code: "UNAUTHORIZED".to_string(),
                    message: content,
                },
            ),
            AppError::MissingAuthHeader => (
                StatusCode::UNAUTHORIZED,
                GenericResponse {
                    code: "MISSING_AUTH_HEADER".to_string(),
                    message: "Authorization header is required".to_string(),
                },
            ),
            AppError::InvalidAuthHeaderFormat => (
                StatusCode::UNAUTHORIZED,
                GenericResponse {
                    code: "INVALID_AUTH_HEADER_FORMAT".to_string(),
                    message: "Authorization header must start with 'Bearer '".to_string(),
                },
            ),
        };

        (status, Json(error_response)).into_response()
    }
}
