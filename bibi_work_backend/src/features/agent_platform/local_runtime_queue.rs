use serde_json::Value;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::features::core::errors::AppError;

const POLL_INTERVAL_MS: u64 = 200;

pub struct LocalRuntimeRequestRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub device_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub command: Value,
    pub timeout_ms: i32,
    pub max_output_bytes: i32,
    pub created_at: OffsetDateTime,
}

pub struct LocalRuntimeResult {
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub struct EnqueueLocalRuntimeRequest {
    pub tenant_id: Uuid,
    pub device_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub command: Value,
    pub timeout_ms: i32,
    pub max_output_bytes: i32,
}

pub async fn enqueue(
    pool: &PgPool,
    request: EnqueueLocalRuntimeRequest,
) -> Result<LocalRuntimeRequestRow, AppError> {
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
    .bind(request.tenant_id)
    .bind(request.device_id)
    .bind(request.project_id)
    .bind(request.run_id)
    .bind(request.command)
    .bind(request.timeout_ms)
    .bind(request.max_output_bytes)
    .fetch_one(pool)
    .await?;
    Ok(LocalRuntimeRequestRow {
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

pub async fn wait_for_result(
    pool: &PgPool,
    request_id: Uuid,
    tenant_id: Uuid,
    timeout_ms: i32,
) -> Result<LocalRuntimeResult, AppError> {
    let timeout_ms = u64::try_from(timeout_ms).unwrap_or(30_000);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Some(result) = load_result(pool, request_id, tenant_id).await? {
            return Ok(result);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
    let update = sqlx::query(
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
    if update.rows_affected() == 0
        && let Some(result) = load_result(pool, request_id, tenant_id).await?
    {
        return Ok(result);
    }
    Ok(LocalRuntimeResult {
        status: "timed_out".to_string(),
        result: None,
        error: Some("local runtime request timed out".to_string()),
    })
}

pub async fn mark_expired(pool: &PgPool, tenant_id: Uuid, device_id: Uuid) -> Result<(), AppError> {
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

async fn load_result(
    pool: &PgPool,
    request_id: Uuid,
    tenant_id: Uuid,
) -> Result<Option<LocalRuntimeResult>, AppError> {
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
    Ok(Some(LocalRuntimeResult {
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
