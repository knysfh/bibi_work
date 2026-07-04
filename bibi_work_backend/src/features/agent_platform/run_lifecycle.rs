use serde_json::json;
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::{event_store, models::RunEventInput};

pub async fn mark_dispatch_failed(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    run_id: Uuid,
    trace_id: Option<String>,
    error: &str,
) -> Result<Option<Uuid>, AppError> {
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'failed',
            completed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(run_id)
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;

    let workflow_run_id = sqlx::query_scalar(
        r#"
        UPDATE workflow_node_runs
        SET status = 'failed',
            completed_at = CURRENT_TIMESTAMP,
            last_error = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE agent_run_id = $1
          AND tenant_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled', 'blocked', 'skipped')
        RETURNING workflow_run_id
        "#,
    )
    .bind(run_id)
    .bind(tenant_id)
    .bind(error.chars().take(512).collect::<String>())
    .fetch_optional(&mut *tx)
    .await?;

    let event = event_store::insert_event_tx(
        &mut tx,
        tenant_id,
        conversation_id,
        Some(run_id),
        RunEventInput {
            event_id: Some(format!("run.failed.dispatch.{run_id}")),
            event_type: "run.failed".to_string(),
            payload: Some(json!({
                "run_id": run_id,
                "error": error.chars().take(1000).collect::<String>(),
                "error_type": "runtime_dispatch_failed"
            })),
            trace_id,
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(state, &event).await;

    Ok(workflow_run_id)
}
