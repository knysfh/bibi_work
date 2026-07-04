use serde_json::json;
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::{
    audit::{self, NewAuditLog},
    file_store,
    models::{FileLockRequest, FileLockResponse, FileUnlockRequest, FileWriteRequest},
};

const DEFAULT_FILE_LOCK_TTL_SECONDS: i64 = 15 * 60;
const MAX_FILE_LOCK_TTL_SECONDS: i64 = 24 * 60 * 60;

pub async fn acquire_lock(
    state: &AppState,
    payload: FileLockRequest,
) -> Result<FileLockResponse, AppError> {
    let path_hash = file_store::path_hash(&payload.path)?;
    let ttl_seconds = payload
        .ttl_seconds
        .unwrap_or(DEFAULT_FILE_LOCK_TTL_SECONDS)
        .clamp(1, MAX_FILE_LOCK_TTL_SECONDS);
    let expires_at = OffsetDateTime::now_utc() + Duration::seconds(ttl_seconds);

    let mut tx = state.connect_pool.begin().await?;
    lock_file_path_tx(&mut tx, payload.project_id, &path_hash).await?;
    delete_expired_lock_tx(&mut tx, payload.tenant_id, payload.project_id, &path_hash).await?;

    if let Some(row) =
        load_active_lock_tx(&mut tx, payload.tenant_id, payload.project_id, &path_hash).await?
    {
        if !lock_held_by_actor(&row, payload.actor_user_id, payload.run_id)? {
            return Err(AppError::Conflict(
                "file is locked by another actor".to_string(),
            ));
        }

        let row = sqlx::query(
            r#"
            UPDATE file_locks
            SET path = $2,
                reason = $3,
                expires_at = $4,
                metadata = metadata || $5,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            RETURNING id, tenant_id, project_id, path, path_hash, holder_run_id,
                      holder_user_id, lock_token, reason, expires_at, created_at
            "#,
        )
        .bind(row.try_get::<Uuid, _>("id")?)
        .bind(&payload.path)
        .bind(payload.reason.clone())
        .bind(expires_at)
        .bind(json!({
            "last_action": "extend",
            "ttl_seconds": ttl_seconds,
            "run_id": payload.run_id
        }))
        .fetch_one(&mut *tx)
        .await?;
        let response = lock_response_from_row(row, &payload.path)?;
        insert_file_lock_audit_tx(&mut tx, &payload, &path_hash, &response, "lock.extend").await?;
        tx.commit()
            .await
            .map_err(|_| AppError::DatabaseTransaction)?;
        return Ok(response);
    }

    let row = sqlx::query(
        r#"
        INSERT INTO file_locks (
            tenant_id, project_id, path, path_hash, holder_run_id, holder_user_id,
            expires_at, reason, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, tenant_id, project_id, path, path_hash, holder_run_id,
                  holder_user_id, lock_token, reason, expires_at, created_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.project_id)
    .bind(&payload.path)
    .bind(&path_hash)
    .bind(payload.run_id)
    .bind(payload.actor_user_id)
    .bind(expires_at)
    .bind(payload.reason.clone())
    .bind(json!({
        "last_action": "acquire",
        "ttl_seconds": ttl_seconds,
        "run_id": payload.run_id
    }))
    .fetch_one(&mut *tx)
    .await?;

    let response = lock_response_from_row(row, &payload.path)?;
    insert_file_lock_audit_tx(&mut tx, &payload, &path_hash, &response, "lock.acquire").await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(response)
}

pub async fn release_lock(
    state: &AppState,
    payload: FileUnlockRequest,
) -> Result<FileLockResponse, AppError> {
    let path_hash = file_store::path_hash(&payload.path)?;
    let mut tx = state.connect_pool.begin().await?;
    lock_file_path_tx(&mut tx, payload.project_id, &path_hash).await?;
    delete_expired_lock_tx(&mut tx, payload.tenant_id, payload.project_id, &path_hash).await?;

    let row =
        load_active_lock_tx(&mut tx, payload.tenant_id, payload.project_id, &path_hash).await?;
    let Some(row) = row else {
        return Err(AppError::NotFound("file lock not found".to_string()));
    };
    if !lock_held_by_unlocker(&row, &payload)? {
        return Err(AppError::Conflict(
            "file lock is held by another actor".to_string(),
        ));
    }
    let response = lock_response_from_row(row, &payload.path)?;
    sqlx::query("DELETE FROM file_locks WHERE id = $1")
        .bind(response.id)
        .execute(&mut *tx)
        .await?;
    insert_file_unlock_audit_tx(&mut tx, &payload, &path_hash, &response).await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(response)
}

pub async fn ensure_write_permitted_tx(
    tx: &mut Transaction<'_, Postgres>,
    payload: &FileWriteRequest,
    path_hash: &str,
) -> Result<(), AppError> {
    delete_expired_lock_tx(tx, payload.tenant_id, payload.project_id, path_hash).await?;
    let Some(row) =
        load_active_lock_tx(tx, payload.tenant_id, payload.project_id, path_hash).await?
    else {
        return Ok(());
    };

    let lock_token: String = row.try_get("lock_token")?;
    if payload.lock_token.as_deref() == Some(lock_token.as_str())
        || lock_held_by_actor(&row, payload.actor_user_id, payload.run_id)?
    {
        return Ok(());
    }

    Err(AppError::Conflict(
        "file is locked by another actor".to_string(),
    ))
}

async fn delete_expired_lock_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    project_id: Uuid,
    path_hash: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        DELETE FROM file_locks
        WHERE tenant_id = $1
          AND project_id = $2
          AND path_hash = $3
          AND expires_at <= CURRENT_TIMESTAMP
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn load_active_lock_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    project_id: Uuid,
    path_hash: &str,
) -> Result<Option<PgRow>, AppError> {
    sqlx::query(
        r#"
        SELECT id, tenant_id, project_id, path, path_hash, holder_run_id,
               holder_user_id, lock_token, reason, expires_at, created_at
        FROM file_locks
        WHERE tenant_id = $1
          AND project_id = $2
          AND path_hash = $3
          AND expires_at > CURRENT_TIMESTAMP
        FOR UPDATE
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)
}

async fn lock_file_path_tx(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    path_hash: &str,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{project_id}:{path_hash}:file-lock"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn lock_held_by_actor(
    row: &PgRow,
    actor_user_id: Uuid,
    run_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let holder_user_id: Option<Uuid> = row.try_get("holder_user_id")?;
    let holder_run_id: Option<Uuid> = row.try_get("holder_run_id")?;

    if let Some(holder_run_id) = holder_run_id {
        return Ok(Some(holder_run_id) == run_id);
    }

    Ok(holder_user_id == Some(actor_user_id))
}

fn lock_held_by_unlocker(row: &PgRow, payload: &FileUnlockRequest) -> Result<bool, AppError> {
    let lock_token: String = row.try_get("lock_token")?;
    if payload.lock_token.as_deref() == Some(lock_token.as_str()) {
        return Ok(true);
    }

    if payload.lock_token.is_some() {
        return Ok(false);
    }

    lock_held_by_actor(row, payload.actor_user_id, payload.run_id)
}

fn lock_response_from_row(row: PgRow, fallback_path: &str) -> Result<FileLockResponse, AppError> {
    Ok(FileLockResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        project_id: row.try_get("project_id")?,
        path: row
            .try_get::<Option<String>, _>("path")?
            .unwrap_or_else(|| fallback_path.to_string()),
        path_hash: row.try_get("path_hash")?,
        holder_run_id: row.try_get("holder_run_id")?,
        holder_user_id: row.try_get("holder_user_id")?,
        lock_token: row.try_get("lock_token")?,
        reason: row.try_get("reason")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn insert_file_lock_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    payload: &FileLockRequest,
    path_hash: &str,
    response: &FileLockResponse,
    action: &str,
) -> Result<(), AppError> {
    let resource_id = format!("{}:{path_hash}", payload.project_id);
    let output_summary = format!(
        "lock_id={}; expires_at={}",
        response.id, response.expires_at
    );
    audit::insert_audit_log_tx(
        tx,
        NewAuditLog {
            tenant_id: payload.tenant_id,
            actor_user_id: Some(payload.actor_user_id),
            actor_device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            resource_type: "file",
            resource_id: &resource_id,
            action,
            decision: "allow",
            policy_version: "local-policy-v1",
            reason_code: payload.reason.as_deref(),
            run_id: payload.run_id,
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some(&payload.path),
            output_summary: Some(&output_summary),
            risk_level: Some("medium"),
            ip: None,
            user_agent: None,
            trace_id: None,
        },
    )
    .await?;
    Ok(())
}

async fn insert_file_unlock_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    payload: &FileUnlockRequest,
    path_hash: &str,
    response: &FileLockResponse,
) -> Result<(), AppError> {
    let resource_id = format!("{}:{path_hash}", payload.project_id);
    let output_summary = format!("lock_id={}", response.id);
    audit::insert_audit_log_tx(
        tx,
        NewAuditLog {
            tenant_id: payload.tenant_id,
            actor_user_id: Some(payload.actor_user_id),
            actor_device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            resource_type: "file",
            resource_id: &resource_id,
            action: "lock.release",
            decision: "allow",
            policy_version: "local-policy-v1",
            reason_code: payload.reason.as_deref(),
            run_id: payload.run_id,
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: None,
            input_summary: Some(&payload.path),
            output_summary: Some(&output_summary),
            risk_level: Some("medium"),
            ip: None,
            user_agent: None,
            trace_id: None,
        },
    )
    .await?;
    Ok(())
}
