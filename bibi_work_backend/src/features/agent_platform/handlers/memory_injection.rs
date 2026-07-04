use serde_json::{Value, json};
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{agent_platform::models::*, core::errors::AppError},
    startup::AppState,
};

use super::memory_service;

pub(super) struct MemoryInjectionRequest {
    pub actor: ActorRef,
    pub tenant_id: Uuid,
    pub run_id: Uuid,
    pub agent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
}

pub(super) async fn inject_memory_context_for_run(
    state: &AppState,
    request: MemoryInjectionRequest,
    input: &Value,
    snapshot: &mut Value,
) -> Result<(), AppError> {
    let enabled = memory_retrieval_enabled(snapshot);
    let query = enabled
        .then(|| memory_query_for_run(input, snapshot))
        .flatten();
    let Some(query) = query else {
        write_memory_context_meta(
            snapshot,
            json!({
                "source": "not_retrieved",
                "enabled": enabled,
                "reason": if enabled { "no_query" } else { "disabled" }
            }),
        )?;
        persist_run_snapshot(&state.connect_pool, request.run_id, snapshot).await?;
        return Ok(());
    };

    let retrieve_request = MemoryRetrieveForRunRequest {
        tenant_id: request.tenant_id,
        actor: request.actor.clone(),
        run_id: Some(request.run_id),
        user_id: Some(request.actor.user_id),
        agent_id: request.agent_id,
        project_id: request.project_id,
        layer: memory_retrieval_layer(snapshot),
        query,
        limit: Some(memory_retrieval_limit(snapshot)),
        min_score: memory_retrieval_min_score(snapshot),
    };

    match memory_service::retrieve_memory_context_for_run(state, retrieve_request).await {
        Ok(response) => {
            let memories = serde_json::to_value(&response.memories).map_err(|_| {
                AppError::InvalidInput("failed to encode memory context".to_string())
            })?;
            write_memory_context(snapshot, memories)?;
            write_memory_context_meta(
                snapshot,
                json!({
                    "source": response.source,
                    "enabled": enabled,
                    "count": response.memories.len(),
                    "vector_attempted": response.vector_attempted,
                    "vector_error": response.vector_error
                }),
            )?;
        }
        Err(err) => {
            warn!(
                "memory context injection failed for run {}: {}",
                request.run_id, err
            );
            write_memory_context(snapshot, json!([]))?;
            write_memory_context_meta(
                snapshot,
                json!({
                    "source": "memory_retrieval_failed",
                    "enabled": enabled,
                    "count": 0,
                    "error": err.to_string().chars().take(500).collect::<String>()
                }),
            )?;
        }
    }

    persist_run_snapshot(&state.connect_pool, request.run_id, snapshot).await
}

async fn persist_run_snapshot(
    pool: &sqlx::PgPool,
    run_id: Uuid,
    snapshot: &Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE runs
        SET run_config_snapshot = $2,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .bind(snapshot)
    .execute(pool)
    .await?;
    Ok(())
}

fn write_memory_context(snapshot: &mut Value, memories: Value) -> Result<(), AppError> {
    snapshot_object_mut(snapshot)?.insert("memory_context".to_string(), memories);
    Ok(())
}

fn write_memory_context_meta(snapshot: &mut Value, meta: Value) -> Result<(), AppError> {
    snapshot_object_mut(snapshot)?.insert("memory_context_meta".to_string(), meta);
    Ok(())
}

fn memory_query_for_run(input: &Value, snapshot: &Value) -> Option<String> {
    retrieval_query(snapshot)
        .or_else(|| memory_query(snapshot))
        .or_else(|| snapshot.get("node").and_then(retrieval_query))
        .or_else(|| snapshot.get("node").and_then(memory_query))
        .or_else(|| retrieval_query(input))
        .or_else(|| memory_query(input))
        .or_else(|| prompt_query(input))
        .or_else(|| latest_message_content(input))
        .or_else(|| input.get("workflow_input").and_then(retrieval_query))
        .or_else(|| input.get("workflow_input").and_then(memory_query))
        .or_else(|| input.get("workflow_input").and_then(prompt_query))
        .or_else(|| input.get("workflow_input").and_then(latest_message_content))
}

fn retrieval_query(value: &Value) -> Option<String> {
    value
        .get("memory_retrieval")
        .and_then(|config| config.get("query"))
        .and_then(Value::as_str)
        .and_then(non_empty_string)
}

fn memory_query(value: &Value) -> Option<String> {
    value
        .get("memory_query")
        .and_then(Value::as_str)
        .and_then(non_empty_string)
}

fn prompt_query(value: &Value) -> Option<String> {
    value
        .get("user_prompt")
        .and_then(Value::as_str)
        .and_then(non_empty_string)
        .or_else(|| {
            value
                .get("prompt")
                .and_then(Value::as_str)
                .and_then(non_empty_string)
        })
}

fn latest_message_content(input: &Value) -> Option<String> {
    input
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find_map(|message| message.get("content").and_then(message_content_as_text))
        })
        .and_then(|content| non_empty_string(&content))
}

fn message_content_as_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.get("content").and_then(Value::as_str))
        })
        .collect::<Vec<_>>()
        .join("\n");
    non_empty_string(&text)
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(2000).collect())
    }
}

fn memory_retrieval_config(snapshot: &Value) -> Option<&Value> {
    snapshot.get("memory_retrieval").or_else(|| {
        snapshot
            .get("node")
            .and_then(|node| node.get("memory_retrieval"))
    })
}

fn memory_retrieval_enabled(snapshot: &Value) -> bool {
    memory_retrieval_config(snapshot)
        .and_then(|value| value.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn memory_retrieval_limit(snapshot: &Value) -> i64 {
    memory_retrieval_config(snapshot)
        .and_then(|value| value.get("limit"))
        .and_then(Value::as_i64)
        .unwrap_or(8)
        .clamp(1, 50)
}

fn memory_retrieval_layer(snapshot: &Value) -> Option<String> {
    memory_retrieval_config(snapshot)
        .and_then(|value| value.get("layer"))
        .and_then(Value::as_str)
        .and_then(non_empty_string)
}

fn memory_retrieval_min_score(snapshot: &Value) -> Option<f64> {
    memory_retrieval_config(snapshot)
        .and_then(|value| value.get("min_score"))
        .and_then(Value::as_f64)
        .filter(|score| score.is_finite())
}

fn snapshot_object_mut(
    snapshot: &mut Value,
) -> Result<&mut serde_json::Map<String, Value>, AppError> {
    snapshot.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("run_config_snapshot must be a JSON object".to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_query_prefers_explicit_retrieval_query() {
        let input = json!({"user_prompt": "from input"});
        let snapshot = json!({
            "memory_query": "legacy query",
            "memory_retrieval": {"query": " explicit query "}
        });

        assert_eq!(
            memory_query_for_run(&input, &snapshot).as_deref(),
            Some("explicit query")
        );
    }

    #[test]
    fn memory_query_uses_workflow_node_query_before_workflow_input() {
        let input = json!({
            "workflow_input": {"user_prompt": "workflow input query"}
        });
        let snapshot = json!({
            "node": {
                "memory_retrieval": {"query": " node query "}
            }
        });

        assert_eq!(
            memory_query_for_run(&input, &snapshot).as_deref(),
            Some("node query")
        );
    }

    #[test]
    fn memory_query_falls_back_to_latest_message_content() {
        let input = json!({
            "messages": [
                {"role": "user", "content": "old"},
                {"role": "user", "content": [{"text": "new"}, {"text": "details"}]}
            ]
        });

        assert_eq!(
            memory_query_for_run(&input, &json!({})).as_deref(),
            Some("new\ndetails")
        );
    }

    #[test]
    fn memory_query_falls_back_to_workflow_input_prompt() {
        let input = json!({
            "workflow_input": {
                "prompt": " workflow prompt "
            }
        });

        assert_eq!(
            memory_query_for_run(&input, &json!({})).as_deref(),
            Some("workflow prompt")
        );
    }

    #[test]
    fn memory_retrieval_config_uses_safe_defaults_and_clamps_limit() {
        assert!(memory_retrieval_enabled(&json!({})));
        assert_eq!(memory_retrieval_limit(&json!({})), 8);
        assert_eq!(
            memory_retrieval_limit(&json!({"memory_retrieval": {"limit": 500}})),
            50
        );
        assert_eq!(
            memory_retrieval_layer(&json!({"memory_retrieval": {"layer": " semantic "}}))
                .as_deref(),
            Some("semantic")
        );
        assert!(!memory_retrieval_enabled(
            &json!({"memory_retrieval": {"enabled": false}})
        ));
    }

    #[test]
    fn memory_retrieval_config_can_come_from_workflow_node() {
        let snapshot = json!({
            "node": {
                "memory_retrieval": {
                    "enabled": false,
                    "limit": 2,
                    "layer": "project"
                }
            }
        });

        assert!(!memory_retrieval_enabled(&snapshot));
        assert_eq!(memory_retrieval_limit(&snapshot), 2);
        assert_eq!(
            memory_retrieval_layer(&snapshot).as_deref(),
            Some("project")
        );
    }
}
