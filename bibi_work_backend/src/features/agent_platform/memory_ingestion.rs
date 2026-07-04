use std::time::Duration;

use sqlx::{PgPool, Row, postgres::PgRow};
use tokio::time::MissedTickBehavior;
use tracing::warn;
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::memory_vector::{MemoryVectorIndexRequest, MemoryVectorIndexResult};

#[derive(Debug, Clone)]
struct MemoryIngestionJob {
    id: Uuid,
    tenant_id: Uuid,
    memory_id: Uuid,
    attempts: i32,
}

#[derive(Debug, Clone)]
struct MemoryIndexItem {
    memory_id: Uuid,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
    agent_id: Option<Uuid>,
    project_id: Option<Uuid>,
    layer: String,
    content: String,
    content_hash: String,
    confidence: f64,
    status: String,
    visibility: String,
    sensitivity: String,
}

#[derive(Debug, PartialEq, Eq)]
enum MemoryIndexAction {
    Upsert,
    Delete,
}

pub fn spawn_memory_ingestion_worker(state: AppState) {
    if !state.memory_vector_client.is_enabled() {
        return;
    }

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(
            state.memory_vector_client.worker_interval_milliseconds(),
        ));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            if let Err(err) = process_pending_memory_ingestion(&state).await {
                warn!("background memory ingestion failed: {}", err);
            }
        }
    });
}

pub async fn process_pending_memory_ingestion(state: &AppState) -> Result<usize, AppError> {
    process_pending_memory_ingestion_for_scope(state, None).await
}

pub async fn process_pending_memory_ingestion_for_tenant(
    state: &AppState,
    tenant_id: Uuid,
) -> Result<usize, AppError> {
    process_pending_memory_ingestion_for_scope(state, Some(tenant_id)).await
}

async fn process_pending_memory_ingestion_for_scope(
    state: &AppState,
    tenant_id: Option<Uuid>,
) -> Result<usize, AppError> {
    let jobs = claim_pending_jobs(
        &state.connect_pool,
        state.memory_vector_client.worker_batch_size(),
        tenant_id,
    )
    .await?;
    let mut processed = 0_usize;

    for job in jobs {
        processed += 1;
        if let Err(err) = process_job(state, &job).await {
            complete_job_with_error(
                &state.connect_pool,
                &job,
                state.memory_vector_client.worker_max_attempts(),
                &err,
            )
            .await?;
        }
    }

    Ok(processed)
}

async fn process_job(state: &AppState, job: &MemoryIngestionJob) -> Result<(), String> {
    let Some(memory) = load_memory_for_index(&state.connect_pool, job)
        .await
        .map_err(|err| err.to_string())?
    else {
        state
            .memory_vector_client
            .delete_memory_point(job.memory_id)
            .await?;
        complete_deleted_memory_job(&state.connect_pool, job)
            .await
            .map_err(|err| err.to_string())?;
        return Ok(());
    };

    match memory_index_action(&memory) {
        MemoryIndexAction::Upsert => {
            let result = state
                .memory_vector_client
                .index_memory(memory.index_request())
                .await?;
            complete_indexed_memory_job(&state.connect_pool, job, &result)
                .await
                .map_err(|err| err.to_string())?;
        }
        MemoryIndexAction::Delete => {
            state
                .memory_vector_client
                .delete_memory_point(memory.memory_id)
                .await?;
            complete_skipped_memory_job(
                &state.connect_pool,
                job,
                "memory is not approved for vector indexing",
            )
            .await
            .map_err(|err| err.to_string())?;
        }
    }

    Ok(())
}

async fn claim_pending_jobs(
    pool: &PgPool,
    batch_size: i64,
    tenant_id: Option<Uuid>,
) -> Result<Vec<MemoryIngestionJob>, AppError> {
    let rows = sqlx::query(
        r#"
        UPDATE memory_ingestion_jobs
        SET status = 'running',
            attempts = attempts + 1,
            started_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id IN (
            SELECT id
            FROM memory_ingestion_jobs
            WHERE status = 'pending'
              AND scheduled_at <= CURRENT_TIMESTAMP
              AND ($2::uuid IS NULL OR tenant_id = $2)
            ORDER BY scheduled_at, created_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING id, tenant_id, memory_id, attempts
        "#,
    )
    .bind(batch_size.max(1))
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(job_from_row).collect()
}

async fn load_memory_for_index(
    pool: &PgPool,
    job: &MemoryIngestionJob,
) -> Result<Option<MemoryIndexItem>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, layer, content, content_hash,
               confidence, status, visibility, sensitivity
        FROM memory_items
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(job.memory_id)
    .bind(job.tenant_id)
    .fetch_optional(pool)
    .await?;

    row.map(memory_index_item_from_row).transpose()
}

async fn complete_indexed_memory_job(
    pool: &PgPool,
    job: &MemoryIngestionJob,
    result: &MemoryVectorIndexResult,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE memory_embeddings
        SET provider = 'external-http',
            vector_dimension = $3,
            vector_hash = $4,
            qdrant_collection = $5,
            qdrant_point_id = $6,
            index_status = 'indexed',
            last_error = NULL,
            indexed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE memory_id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(job.memory_id)
    .bind(job.tenant_id)
    .bind(result.vector_dimension)
    .bind(&result.vector_hash)
    .bind(&result.collection_name)
    .bind(&result.point_id)
    .execute(pool)
    .await?;

    complete_job(pool, job.id).await
}

async fn complete_skipped_memory_job(
    pool: &PgPool,
    job: &MemoryIngestionJob,
    reason: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE memory_embeddings
        SET index_status = 'skipped',
            last_error = $3,
            indexed_at = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE memory_id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(job.memory_id)
    .bind(job.tenant_id)
    .bind(reason)
    .execute(pool)
    .await?;

    complete_job(pool, job.id).await
}

async fn complete_deleted_memory_job(
    pool: &PgPool,
    job: &MemoryIngestionJob,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE memory_embeddings
        SET index_status = 'deleted',
            last_error = NULL,
            indexed_at = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE memory_id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(job.memory_id)
    .bind(job.tenant_id)
    .execute(pool)
    .await?;

    complete_job(pool, job.id).await
}

async fn complete_job(pool: &PgPool, job_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE memory_ingestion_jobs
        SET status = 'completed',
            completed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(job_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn complete_job_with_error(
    pool: &PgPool,
    job: &MemoryIngestionJob,
    max_attempts: i32,
    error: &str,
) -> Result<(), AppError> {
    let terminal = job.attempts >= max_attempts;
    let next_status = if terminal { "failed" } else { "pending" };
    let embedding_status = if terminal { "failed" } else { "pending" };

    sqlx::query(
        r#"
        UPDATE memory_embeddings
        SET index_status = $3,
            last_error = $4,
            updated_at = CURRENT_TIMESTAMP
        WHERE memory_id = $1
          AND tenant_id = $2
        "#,
    )
    .bind(job.memory_id)
    .bind(job.tenant_id)
    .bind(embedding_status)
    .bind(error)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE memory_ingestion_jobs
        SET status = $2,
            last_error = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(job.id)
    .bind(next_status)
    .bind(error)
    .execute(pool)
    .await?;

    Ok(())
}

fn job_from_row(row: PgRow) -> Result<MemoryIngestionJob, AppError> {
    Ok(MemoryIngestionJob {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        memory_id: row.try_get("memory_id")?,
        attempts: row.try_get("attempts")?,
    })
}

fn memory_index_item_from_row(row: PgRow) -> Result<MemoryIndexItem, AppError> {
    Ok(MemoryIndexItem {
        memory_id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        user_id: row.try_get("user_id")?,
        agent_id: row.try_get("agent_id")?,
        project_id: row.try_get("project_id")?,
        layer: row.try_get("layer")?,
        content: row.try_get("content")?,
        content_hash: row.try_get("content_hash")?,
        confidence: row.try_get("confidence")?,
        status: row.try_get("status")?,
        visibility: row.try_get("visibility")?,
        sensitivity: row.try_get("sensitivity")?,
    })
}

fn memory_index_action(memory: &MemoryIndexItem) -> MemoryIndexAction {
    if memory.status == "approved" && memory.sensitivity != "secret" {
        MemoryIndexAction::Upsert
    } else {
        MemoryIndexAction::Delete
    }
}

impl MemoryIndexItem {
    fn index_request(&self) -> MemoryVectorIndexRequest {
        MemoryVectorIndexRequest {
            memory_id: self.memory_id,
            tenant_id: self.tenant_id,
            user_id: self.user_id,
            agent_id: self.agent_id,
            project_id: self.project_id,
            layer: self.layer.clone(),
            content: self.content.clone(),
            content_hash: self.content_hash.clone(),
            confidence: self.confidence,
            status: self.status.clone(),
            visibility: self.visibility.clone(),
            sensitivity: self.sensitivity.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_item(status: &str, sensitivity: &str) -> MemoryIndexItem {
        MemoryIndexItem {
            memory_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            user_id: Some(Uuid::new_v4()),
            agent_id: None,
            project_id: None,
            layer: "semantic".to_string(),
            content: "sales memory".to_string(),
            content_hash: "hash".to_string(),
            confidence: 0.8,
            status: status.to_string(),
            visibility: "private".to_string(),
            sensitivity: sensitivity.to_string(),
        }
    }

    #[test]
    fn memory_index_action_upserts_only_approved_non_secret_memories() {
        assert_eq!(
            memory_index_action(&index_item("approved", "normal")),
            MemoryIndexAction::Upsert
        );
        assert_eq!(
            memory_index_action(&index_item("candidate", "normal")),
            MemoryIndexAction::Delete
        );
        assert_eq!(
            memory_index_action(&index_item("approved", "secret")),
            MemoryIndexAction::Delete
        );
    }
}
