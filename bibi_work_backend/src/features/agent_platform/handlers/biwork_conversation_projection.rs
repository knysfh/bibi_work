use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};
use sqlx::{Row, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::{
    biwork_agent_support::runtime_kind,
    biwork_compat_service::{epoch_ms, value_string},
    biwork_conversation_runtime_service::{
        conversation_runtime_summary_for_run, conversation_status_for_runtime,
        load_biwork_conversation_runtime_summary,
    },
    biwork_conversation_support::merge_conversation_extra,
};

pub(super) async fn conversation_from_row(
    state: &AppState,
    tenant_id: Uuid,
    row: &PgRow,
) -> Result<Value, AppError> {
    let agent_id: Option<Uuid> = row.try_get("agent_id")?;
    let assistant = if let Some(agent_id) = agent_id {
        load_conversation_assistant(state, tenant_id, agent_id).await?
    } else {
        Value::Null
    };
    let conversation_id: Uuid = row.try_get("id")?;
    let runtime =
        load_biwork_conversation_runtime_summary(state, tenant_id, conversation_id).await?;
    project_conversation(row, assistant, runtime)
}

pub(super) async fn conversations_from_rows(
    state: &AppState,
    tenant_id: Uuid,
    rows: Vec<PgRow>,
) -> Result<Vec<Value>, AppError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let conversation_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("id"))
        .collect::<Result<Vec<_>, _>>()?;
    let agent_ids = rows
        .iter()
        .map(|row| row.try_get::<Option<Uuid>, _>("agent_id"))
        .collect::<Result<HashSet<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    let (assistants, runtimes) = tokio::try_join!(
        load_conversation_assistants(state, tenant_id, &agent_ids),
        load_conversation_runtimes(state, tenant_id, &conversation_ids),
    )?;

    rows.iter()
        .map(|row| {
            let conversation_id: Uuid = row.try_get("id")?;
            let agent_id: Option<Uuid> = row.try_get("agent_id")?;
            let assistant = agent_id
                .and_then(|id| assistants.get(&id).cloned())
                .unwrap_or(Value::Null);
            let runtime = runtimes
                .get(&conversation_id)
                .cloned()
                .unwrap_or_else(|| conversation_runtime_summary_for_run(None, None, 0));
            project_conversation(row, assistant, runtime)
        })
        .collect()
}

fn project_conversation(row: &PgRow, assistant: Value, runtime: Value) -> Result<Value, AppError> {
    let id: Uuid = row.try_get("id")?;
    let metadata: Value = row.try_get("metadata")?;
    let extra = metadata
        .get("extra")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let conversation_type = metadata
        .pointer("/biwork/type")
        .and_then(Value::as_str)
        .unwrap_or("acp");
    let model = metadata
        .pointer("/biwork/model")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or(Value::Null);
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;

    Ok(json!({
        "id": id.to_string(),
        "name": row.try_get::<String, _>("title")?,
        "created_at": epoch_ms(created_at),
        "modified_at": epoch_ms(updated_at),
        "type": conversation_type,
        "status": conversation_status_for_runtime(&runtime),
        "runtime": runtime,
        "assistant": assistant,
        "model": model,
        "extra": merge_conversation_extra(extra),
        "source": "biwork",
    }))
}

async fn load_conversation_assistant(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Uuid,
) -> Result<Value, AppError> {
    Ok(load_conversation_assistants(state, tenant_id, &[agent_id])
        .await?
        .remove(&agent_id)
        .unwrap_or(Value::Null))
}

async fn load_conversation_assistants(
    state: &AppState,
    tenant_id: Uuid,
    agent_ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, AppError> {
    if agent_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT id, name, metadata, draft_config
        FROM agents
        WHERE tenant_id = $1
          AND id = ANY($2)
          AND deleted_at IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(agent_ids)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let id: Uuid = row.try_get("id")?;
            let metadata: Value = row.try_get("metadata")?;
            let draft_config: Value = row.try_get("draft_config")?;
            Ok((
                id,
                json!({
                    "id": id.to_string(),
                    "source": value_string(&metadata, "source").unwrap_or_else(|| "builtin".to_string()),
                    "name": row.try_get::<String, _>("name")?,
                    "avatar": value_string(&metadata, "avatar").unwrap_or_default(),
                    "backend": runtime_kind(&draft_config, &metadata),
                }),
            ))
        })
        .collect()
}

async fn load_conversation_runtimes(
    state: &AppState,
    tenant_id: Uuid,
    conversation_ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, AppError> {
    let rows = sqlx::query(
        r#"
        WITH selected AS (
            SELECT UNNEST($2::uuid[]) AS conversation_id
        ),
        pending AS (
            SELECT conversation_id, COUNT(*)::BIGINT AS pending_confirmations
            FROM approvals
            WHERE tenant_id = $1
              AND conversation_id = ANY($2)
              AND status = 'pending'
            GROUP BY conversation_id
        ),
        active_run AS (
            SELECT DISTINCT ON (conversation_id)
                   conversation_id, id, status
            FROM runs
            WHERE tenant_id = $1
              AND conversation_id = ANY($2)
              AND status NOT IN ('completed', 'failed', 'cancelled')
            ORDER BY conversation_id, updated_at DESC, started_at DESC NULLS LAST, queued_at DESC
        )
        SELECT selected.conversation_id,
               COALESCE(pending.pending_confirmations, 0)::BIGINT AS pending_confirmations,
               active_run.id AS run_id,
               active_run.status AS run_status
        FROM selected
        LEFT JOIN pending USING (conversation_id)
        LEFT JOIN active_run USING (conversation_id)
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_ids)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let conversation_id: Uuid = row.try_get("conversation_id")?;
            let pending_confirmations: i64 = row.try_get("pending_confirmations")?;
            let run_id: Option<Uuid> = row.try_get("run_id")?;
            let run_status: Option<String> = row.try_get("run_status")?;
            Ok((
                conversation_id,
                conversation_runtime_summary_for_run(
                    run_status.as_deref(),
                    run_id,
                    pending_confirmations,
                ),
            ))
        })
        .collect()
}
