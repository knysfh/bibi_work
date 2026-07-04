use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

const LOCAL_EXEC_PROTOCOL: &str = "local_executor.v1";
const LOCAL_EXEC_KIND_FILE_IO: &str = "file_io";
const LOCAL_EXEC_POLL_INTERVAL_MS: u64 = 200;
const LOCAL_EXEC_DEFAULT_TIMEOUT_MS: i32 = 30_000;
const LOCAL_EXEC_MAX_TIMEOUT_MS: i32 = 300_000;
const LOCAL_EXEC_DEFAULT_MAX_OUTPUT_BYTES: i32 = 1_048_576;
const LOCAL_EXEC_MAX_OUTPUT_BYTES: i32 = 8 * 1_048_576;

pub async fn request_local_exec(
    State(state): State<AppState>,
    Json(payload): Json<LocalExecRequest>,
) -> Result<Json<LocalExecResponse>, AppError> {
    let timeout_ms = bounded_timeout_ms(payload.timeout_ms);
    let max_output_bytes = bounded_max_output_bytes(payload.max_output_bytes);

    if payload.operation.is_none() && payload.command.is_some() {
        return queue_legacy_local_exec(state, payload, timeout_ms, max_output_bytes)
            .await
            .map(Json);
    }

    let operation = payload
        .operation
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("local exec operation is required".to_string()))?;
    validate_local_operation(operation)?;
    let local_mount_id = payload
        .local_mount_id
        .ok_or_else(|| AppError::InvalidInput("local_mount_id is required".to_string()))?;
    let device_id = payload
        .device_id
        .or(payload.actor_device_id)
        .ok_or_else(|| AppError::InvalidInput("device_id is required".to_string()))?;
    let virtual_path = payload
        .virtual_path
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("virtual_path is required".to_string()))?;
    validate_virtual_local_path(virtual_path)?;

    let mount =
        load_authorized_local_mount(&state.connect_pool, &payload, local_mount_id, device_id)
            .await?;
    if !virtual_path.starts_with(&mount.virtual_path) {
        return Err(AppError::InvalidInput(
            "virtual_path is outside the local mount".to_string(),
        ));
    }
    ensure_local_mount_capability(&mount.capabilities, capability_for_operation(operation))?;

    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        capability_for_operation(operation),
        "local_mount",
        format!("{local_mount_id}:{virtual_path}"),
        Some(AuthzContext {
            project_id: payload.project_id,
            run_id: payload.run_id,
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;

    let command = json!({
        "protocol": LOCAL_EXEC_PROTOCOL,
        "kind": LOCAL_EXEC_KIND_FILE_IO,
        "local_mount_id": local_mount_id,
        "operation": operation,
        "virtual_path": virtual_path,
        "content": payload.content,
        "query": payload.query,
        "expected_revision": payload.expected_revision,
        "max_output_bytes": max_output_bytes
    });
    let row = insert_local_exec_request(
        &state.connect_pool,
        payload.tenant_id,
        Some(device_id),
        payload.project_id,
        payload.run_id,
        command,
        timeout_ms,
        max_output_bytes,
    )
    .await?;

    let result =
        wait_for_local_exec_result(&state.connect_pool, row.id, payload.tenant_id, timeout_ms)
            .await?;
    Ok(Json(LocalExecResponse {
        id: row.id,
        tenant_id: row.tenant_id,
        device_id: row.device_id,
        local_mount_id: Some(local_mount_id),
        project_id: row.project_id,
        run_id: row.run_id,
        status: result.status,
        command: row.command,
        result: result.result,
        error: result.error,
        timeout_ms: row.timeout_ms,
        max_output_bytes: row.max_output_bytes,
        created_at: row.created_at,
    }))
}

pub async fn next_local_exec_request(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<LocalExecNextQuery>,
) -> Result<Json<Option<LocalExecWorkItemResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    mark_expired_local_exec_requests(&state.connect_pool, query.tenant_id, ctx.device_id).await?;
    let row = sqlx::query(
        r#"
        WITH next_request AS (
            SELECT id
            FROM local_exec_requests
            WHERE tenant_id = $1
              AND device_id = $2
              AND status = 'queued'
            ORDER BY created_at ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE local_exec_requests
        SET status = 'dispatching', updated_at = CURRENT_TIMESTAMP
        WHERE id IN (SELECT id FROM next_request)
        RETURNING id, tenant_id, device_id, command, timeout_ms, max_output_bytes, created_at
        "#,
    )
    .bind(query.tenant_id)
    .bind(ctx.device_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    let Some(row) = row else {
        return Ok(Json(None));
    };
    Ok(Json(Some(LocalExecWorkItemResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        command: row.try_get("command")?,
        timeout_ms: row.try_get("timeout_ms")?,
        max_output_bytes: row.try_get("max_output_bytes")?,
        created_at: row.try_get("created_at")?,
    })))
}

async fn mark_expired_local_exec_requests(
    pool: &PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE local_exec_requests
        SET status = 'timed_out', updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND device_id = $2
          AND status IN ('queued', 'dispatching')
          AND created_at + (timeout_ms * INTERVAL '1 millisecond') <= CURRENT_TIMESTAMP
        "#,
    )
    .bind(tenant_id)
    .bind(device_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn complete_local_exec_request(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(request_id): Path<Uuid>,
    Json(payload): Json<LocalExecCompleteRequest>,
) -> Result<Json<LocalExecResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let status = match payload.status.as_str() {
        "completed" => "completed",
        "failed" => "failed",
        _ => {
            return Err(AppError::InvalidInput(
                "local exec completion status must be completed or failed".to_string(),
            ));
        }
    };
    let row = sqlx::query(
        r#"
        UPDATE local_exec_requests
        SET status = $3, updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND device_id = $4
          AND status IN ('queued', 'dispatching')
        RETURNING id, tenant_id, device_id, project_id, run_id, command,
                  timeout_ms, max_output_bytes, created_at
        "#,
    )
    .bind(request_id)
    .bind(payload.tenant_id)
    .bind(status)
    .bind(ctx.device_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("local exec request not found".to_string()))?;

    let event_type = if status == "completed" {
        "local_exec.completed"
    } else {
        "local_exec.failed"
    };
    let event_payload = json!({
        "status": status,
        "result": payload.result,
        "error": payload.error
    });
    sqlx::query(
        r#"
        INSERT INTO local_exec_events (tenant_id, local_exec_request_id, type, payload)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(payload.tenant_id)
    .bind(request_id)
    .bind(event_type)
    .bind(&event_payload)
    .execute(&state.connect_pool)
    .await?;

    Ok(Json(LocalExecResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        local_mount_id: command_local_mount_id(row.try_get("command")?),
        project_id: row.try_get("project_id")?,
        run_id: row.try_get("run_id")?,
        status: status.to_string(),
        command: row.try_get("command")?,
        result: payload.result,
        error: payload.error,
        timeout_ms: row.try_get("timeout_ms")?,
        max_output_bytes: row.try_get("max_output_bytes")?,
        created_at: row.try_get("created_at")?,
    }))
}

async fn queue_legacy_local_exec(
    state: AppState,
    payload: LocalExecRequest,
    timeout_ms: i32,
    max_output_bytes: i32,
) -> Result<LocalExecResponse, AppError> {
    let actor = ActorRef {
        user_id: payload.actor_user_id,
        device_id: payload.actor_device_id,
        session_id: payload.actor_session_id,
        roles: Vec::new(),
    };
    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        actor,
        "execute",
        "local_exec",
        payload
            .device_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unbound-device".to_string()),
        Some(AuthzContext {
            project_id: payload.project_id,
            run_id: payload.run_id,
            risk_level: Some("critical".to_string()),
            ..Default::default()
        }),
    )
    .await?;
    let row = insert_local_exec_request(
        &state.connect_pool,
        payload.tenant_id,
        payload.device_id,
        payload.project_id,
        payload.run_id,
        payload.command.unwrap_or_else(|| json!({})),
        timeout_ms,
        max_output_bytes,
    )
    .await?;
    Ok(LocalExecResponse {
        id: row.id,
        tenant_id: row.tenant_id,
        device_id: row.device_id,
        local_mount_id: None,
        project_id: row.project_id,
        run_id: row.run_id,
        status: "queued".to_string(),
        command: row.command,
        result: None,
        error: None,
        timeout_ms: row.timeout_ms,
        max_output_bytes: row.max_output_bytes,
        created_at: row.created_at,
    })
}

async fn insert_local_exec_request(
    pool: &PgPool,
    tenant_id: Uuid,
    device_id: Option<Uuid>,
    project_id: Option<Uuid>,
    run_id: Option<Uuid>,
    command: Value,
    timeout_ms: i32,
    max_output_bytes: i32,
) -> Result<LocalExecRow, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO local_exec_requests (
            tenant_id, device_id, project_id, run_id, command, timeout_ms, max_output_bytes
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, tenant_id, device_id, project_id, run_id, command,
                  timeout_ms, max_output_bytes, created_at
        "#,
    )
    .bind(tenant_id)
    .bind(device_id)
    .bind(project_id)
    .bind(run_id)
    .bind(command)
    .bind(timeout_ms)
    .bind(max_output_bytes)
    .fetch_one(pool)
    .await?;
    Ok(LocalExecRow {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        project_id: row.try_get("project_id")?,
        run_id: row.try_get("run_id")?,
        command: row.try_get("command")?,
        timeout_ms: row.try_get("timeout_ms")?,
        max_output_bytes: row.try_get("max_output_bytes")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn wait_for_local_exec_result(
    pool: &PgPool,
    request_id: Uuid,
    tenant_id: Uuid,
    timeout_ms: i32,
) -> Result<LocalExecResult, AppError> {
    let mut elapsed_ms = 0_u64;
    let timeout_ms = u64::try_from(timeout_ms).unwrap_or(LOCAL_EXEC_DEFAULT_TIMEOUT_MS as u64);
    while elapsed_ms <= timeout_ms {
        if let Some(result) = load_local_exec_result(pool, request_id, tenant_id).await? {
            return Ok(result);
        }
        sleep(Duration::from_millis(LOCAL_EXEC_POLL_INTERVAL_MS)).await;
        elapsed_ms += LOCAL_EXEC_POLL_INTERVAL_MS;
    }
    sqlx::query(
        r#"
        UPDATE local_exec_requests
        SET status = 'timed_out', updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND status IN ('queued', 'dispatching')
        "#,
    )
    .bind(request_id)
    .bind(tenant_id)
    .execute(pool)
    .await?;
    Ok(LocalExecResult {
        status: "timed_out".to_string(),
        result: None,
        error: Some("local executor request timed out".to_string()),
    })
}

async fn load_local_exec_result(
    pool: &PgPool,
    request_id: Uuid,
    tenant_id: Uuid,
) -> Result<Option<LocalExecResult>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT type, payload
        FROM local_exec_events
        WHERE tenant_id = $1 AND local_exec_request_id = $2
          AND type IN ('local_exec.completed', 'local_exec.failed')
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(request_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let event_type: String = row.try_get("type")?;
    let payload: Value = row.try_get("payload")?;
    Ok(Some(LocalExecResult {
        status: if event_type == "local_exec.completed" {
            "completed".to_string()
        } else {
            "failed".to_string()
        },
        result: payload
            .get("result")
            .cloned()
            .filter(|value| !value.is_null()),
        error: payload
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

async fn load_authorized_local_mount(
    pool: &PgPool,
    payload: &LocalExecRequest,
    local_mount_id: Uuid,
    device_id: Uuid,
) -> Result<LocalMountAccess, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, virtual_path, capabilities
        FROM local_mounts
        WHERE id = $1
          AND tenant_id = $2
          AND user_id = $3
          AND device_id = $4
          AND status = 'active'
        "#,
    )
    .bind(local_mount_id)
    .bind(payload.tenant_id)
    .bind(payload.actor_user_id)
    .bind(device_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("active local mount not found".to_string()))?;
    Ok(LocalMountAccess {
        virtual_path: row.try_get("virtual_path")?,
        capabilities: row.try_get("capabilities")?,
    })
}

fn validate_local_operation(operation: &str) -> Result<(), AppError> {
    if matches!(operation, "read_text" | "write_text" | "list" | "search") {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "unsupported local exec operation: {operation}"
        )))
    }
}

fn capability_for_operation(operation: &str) -> &'static str {
    if operation == "write_text" {
        "write"
    } else {
        "read"
    }
}

fn validate_virtual_local_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(AppError::InvalidInput(
            "virtual_path is required".to_string(),
        ));
    }
    if path.contains('\0') {
        return Err(AppError::InvalidInput(
            "virtual_path contains null byte".to_string(),
        ));
    }
    if path.starts_with("//") || !path.starts_with("/local/") {
        return Err(AppError::InvalidInput(
            "virtual_path must start with /local/".to_string(),
        ));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(AppError::InvalidInput(
            "virtual_path may not contain ..".to_string(),
        ));
    }
    Ok(())
}

fn ensure_local_mount_capability(capabilities: &Value, required: &str) -> Result<(), AppError> {
    let Some(items) = capabilities.as_array() else {
        return Err(AppError::PermissionDenied(
            "local mount capabilities are invalid".to_string(),
        ));
    };
    if items.iter().any(|item| item.as_str() == Some(required)) {
        Ok(())
    } else {
        Err(AppError::PermissionDenied(format!(
            "local mount lacks {required} capability"
        )))
    }
}

fn bounded_timeout_ms(timeout_ms: Option<i32>) -> i32 {
    timeout_ms
        .unwrap_or(LOCAL_EXEC_DEFAULT_TIMEOUT_MS)
        .clamp(1_000, LOCAL_EXEC_MAX_TIMEOUT_MS)
}

fn bounded_max_output_bytes(max_output_bytes: Option<i32>) -> i32 {
    max_output_bytes
        .unwrap_or(LOCAL_EXEC_DEFAULT_MAX_OUTPUT_BYTES)
        .clamp(1_024, LOCAL_EXEC_MAX_OUTPUT_BYTES)
}

fn command_local_mount_id(command: Value) -> Option<Uuid> {
    command
        .get("local_mount_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

struct LocalExecRow {
    id: Uuid,
    tenant_id: Uuid,
    device_id: Option<Uuid>,
    project_id: Option<Uuid>,
    run_id: Option<Uuid>,
    command: Value,
    timeout_ms: i32,
    max_output_bytes: i32,
    created_at: OffsetDateTime,
}

struct LocalExecResult {
    status: String,
    result: Option<Value>,
    error: Option<String>,
}

struct LocalMountAccess {
    virtual_path: String,
    capabilities: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_local_virtual_paths() {
        assert!(validate_virtual_local_path("/local/main/src/lib.rs").is_ok());
        assert!(validate_virtual_local_path("/local/main/../secret").is_err());
        assert!(validate_virtual_local_path("/workspace/file").is_err());
        assert!(validate_virtual_local_path("//local/main/file").is_err());
    }

    #[test]
    fn maps_file_operations_to_capabilities() {
        assert_eq!(capability_for_operation("read_text"), "read");
        assert_eq!(capability_for_operation("list"), "read");
        assert_eq!(capability_for_operation("search"), "read");
        assert_eq!(capability_for_operation("write_text"), "write");
    }
}
