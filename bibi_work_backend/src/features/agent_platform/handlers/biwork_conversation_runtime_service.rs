use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::Row;
use std::collections::HashSet;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    features::{agent_platform::ferriskey_oidc::PlatformRequestContext, core::errors::AppError},
    startup::AppState,
};

use super::{
    biwork_agent_support::BIWORK_ACTIVE_LEASE_SECONDS,
    biwork_compat_service::{active_lease_payload, epoch_ms, ok},
    biwork_conversation_support::ensure_conversation_exists,
};

#[derive(Debug, Deserialize)]
pub struct ConfigOptionPayload {
    value: String,
}

pub async fn biwork_runtime_ensure(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let runtime =
        load_biwork_conversation_runtime_summary(&state, ctx.tenant_id, conversation_id).await?;
    Ok(ok(json!({
        "recovered": false,
        "config_options": [],
        "runtime": runtime,
    })))
}

pub async fn biwork_set_config_option(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((conversation_id, option_id)): Path<(Uuid, String)>,
    Json(payload): Json<ConfigOptionPayload>,
) -> Result<Json<Value>, AppError> {
    let value = payload.value.trim();
    if value.is_empty() {
        return Err(AppError::InvalidInput("value is required".to_string()));
    }
    let row = sqlx::query(
        r#"
        SELECT metadata
        FROM conversations
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;
    let mut metadata: Value = row.try_get("metadata")?;
    if !metadata.is_object() {
        metadata = json!({});
    }
    let metadata_object = metadata.as_object_mut().expect("metadata is object");
    let extra = metadata_object
        .entry("extra".to_string())
        .or_insert_with(|| json!({}));
    if !extra.is_object() {
        *extra = json!({});
    }
    let extra_object = extra.as_object_mut().expect("extra is object");
    let pending = extra_object
        .entry("pending_config_options".to_string())
        .or_insert_with(|| json!({}));
    if !pending.is_object() {
        *pending = json!({});
    }
    pending
        .as_object_mut()
        .expect("pending options is object")
        .insert(option_id.clone(), json!(value));

    sqlx::query(
        r#"
        UPDATE conversations
        SET metadata = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .bind(metadata)
    .execute(&state.connect_pool)
    .await?;

    Ok(ok(json!({
        "confirmation": "observed",
        "config_options": [
            {
                "id": option_id,
                "name": option_id,
                "label": option_id,
                "type": "select",
                "option_type": "select",
                "current_value": value,
                "options": [
                    {
                        "value": value,
                        "name": value,
                        "label": value,
                    }
                ],
            }
        ],
    })))
}

pub async fn biwork_openclaw_runtime(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT metadata
        FROM conversations
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;
    let metadata: Value = row.try_get("metadata")?;
    let extra = metadata
        .get("extra")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let runtime_validation = extra
        .get("runtime_validation")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));

    Ok(ok(json!({
        "conversation_id": conversation_id.to_string(),
        "runtime": {
            "workspace": extra.get("workspace").cloned().unwrap_or(Value::Null),
            "backend": extra.get("backend").cloned().unwrap_or_else(|| json!("deepagents")),
            "agent_name": extra.get("agent_name").cloned().unwrap_or(Value::Null),
            "cli_path": extra.get("cli_path").cloned().unwrap_or(Value::Null),
            "model": metadata.pointer("/biwork/model/use_model")
                .cloned()
                .or_else(|| metadata.pointer("/biwork/model/model").cloned())
                .unwrap_or(Value::Null),
            "session_key": Value::Null,
            "is_connected": false,
            "has_active_session": false,
            "identity_hash": Value::Null,
        },
        "expected": runtime_validation,
    })))
}

pub async fn biwork_active_lease(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let leased_until = OffsetDateTime::now_utc() + Duration::seconds(BIWORK_ACTIVE_LEASE_SECONDS);
    let lease = active_lease_payload(
        ctx.platform_user_id,
        ctx.session_id,
        ctx.device_id,
        leased_until,
    );
    let updated: Option<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE conversations
        SET metadata = jsonb_set(
                jsonb_set(
                    metadata,
                    '{biwork}',
                    COALESCE(metadata->'biwork', '{}'::jsonb),
                    true
                ),
                '{biwork,active_lease}',
                $3::jsonb,
                true
            ),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        RETURNING id
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .bind(&lease)
    .fetch_optional(&state.connect_pool)
    .await?;
    if updated.is_none() {
        return Err(AppError::NotFound("conversation not found".to_string()));
    }

    Ok(ok(json!({
        "leased_until_ms": epoch_ms(leased_until),
        "lease": lease,
    })))
}

pub async fn biwork_slash_commands(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT c.metadata AS conversation_metadata,
               a.metadata AS assistant_metadata,
               COALESCE((
                   SELECT av.config_snapshot
                   FROM agent_versions av
                   WHERE av.agent_id = a.id AND av.tenant_id = a.tenant_id
                   ORDER BY (av.status = 'published') DESC, av.created_at DESC
                   LIMIT 1
               ), a.draft_config) AS assistant_config
        FROM conversations c
        LEFT JOIN agents a
          ON a.id = c.agent_id
         AND a.tenant_id = c.tenant_id
         AND a.deleted_at IS NULL
        WHERE c.id = $1
          AND c.tenant_id = $2
          AND c.deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;

    let conversation_metadata: Value = row.try_get("conversation_metadata")?;
    let assistant_metadata: Option<Value> = row.try_get("assistant_metadata")?;
    let assistant_config: Option<Value> = row.try_get("assistant_config")?;
    let commands = biwork_slash_commands_from_sources(
        &conversation_metadata,
        assistant_metadata.as_ref(),
        assistant_config.as_ref(),
    );
    Ok(ok(Value::Array(commands)))
}

pub(super) fn conversation_runtime_summary() -> Value {
    json!({
        "state": "idle",
        "can_send_message": true,
        "has_task": false,
        "is_processing": false,
        "pending_confirmations": 0,
        "turn_id": Value::Null,
    })
}

pub(super) fn conversation_runtime_summary_for_run(
    status: Option<&str>,
    run_id: Option<Uuid>,
    pending_confirmations: i64,
) -> Value {
    let pending_confirmations = pending_confirmations.max(0);
    let has_pending_confirmation = pending_confirmations > 0 || status == Some("waiting_approval");
    let (state, task_status) = match status {
        Some("queued" | "pending") => ("starting", Some("pending")),
        Some("waiting_approval") if has_pending_confirmation => {
            ("waiting_confirmation", Some("running"))
        }
        Some("running") if has_pending_confirmation => ("waiting_confirmation", Some("running")),
        Some("running") => ("running", Some("running")),
        Some(_) => ("running", Some("running")),
        None if has_pending_confirmation => ("waiting_confirmation", Some("running")),
        None => return conversation_runtime_summary(),
    };

    json!({
        "state": state,
        "can_send_message": false,
        "has_task": true,
        "task_status": task_status,
        "is_processing": true,
        "pending_confirmations": pending_confirmations,
        "turn_id": run_id.map(|id| id.to_string()),
    })
}

pub(super) fn biwork_slash_commands_from_sources(
    conversation_metadata: &Value,
    assistant_metadata: Option<&Value>,
    assistant_config: Option<&Value>,
) -> Vec<Value> {
    let mut commands = Vec::new();
    let mut seen = HashSet::new();
    for source in [
        Some(conversation_metadata),
        conversation_metadata.pointer("/biwork/assistant"),
        conversation_metadata.pointer("/extra/assistant"),
        conversation_metadata.pointer("/extra/runtime"),
        assistant_metadata,
        assistant_config,
    ]
    .into_iter()
    .flatten()
    {
        collect_biwork_slash_commands(source, &mut seen, &mut commands);
    }
    commands
}

fn collect_biwork_slash_commands(
    source: &Value,
    seen: &mut HashSet<String>,
    commands: &mut Vec<Value>,
) {
    match source {
        Value::String(raw) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(raw) {
                collect_biwork_slash_commands(&parsed, seen, commands);
            }
        }
        Value::Array(items) => {
            for item in items {
                if let Some(command) = normalize_biwork_slash_command(item)
                    && let Some(command_name) = command.get("command").and_then(Value::as_str)
                    && seen.insert(command_name.to_ascii_lowercase())
                {
                    commands.push(command);
                }
            }
        }
        Value::Object(object) => {
            for key in ["slash_commands", "available_commands", "commands"] {
                if let Some(value) = object.get(key) {
                    collect_biwork_slash_commands(value, seen, commands);
                }
            }
            if let Some(command) = normalize_biwork_slash_command(source)
                && let Some(command_name) = command.get("command").and_then(Value::as_str)
                && seen.insert(command_name.to_ascii_lowercase())
            {
                commands.push(command);
            }
        }
        _ => {}
    }
}

fn normalize_biwork_slash_command(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let command = object
        .get("command")
        .or_else(|| object.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .map(|name| name.trim_start_matches('/').trim())
        .filter(|name| !name.is_empty())?;
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            object
                .get("hint")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(command);

    let mut normalized = Map::new();
    normalized.insert("command".to_string(), json!(command));
    normalized.insert("description".to_string(), json!(description));
    if let Some(hint) = object
        .get("hint")
        .or_else(|| value.pointer("/input/hint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
    {
        normalized.insert("hint".to_string(), json!(hint));
    }
    if let Some(completion_behavior) = object
        .get("completion_behavior")
        .or_else(|| object.get("completionBehavior"))
        .or_else(|| value.pointer("/_meta/completion_behavior"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| matches!(*value, "normal" | "neutral_tip_on_empty"))
    {
        normalized.insert(
            "completion_behavior".to_string(),
            json!(completion_behavior),
        );
    }
    if let Some(empty_turn_tip_code) = object
        .get("empty_turn_tip_code")
        .or_else(|| object.get("emptyTurnTipCode"))
        .or_else(|| value.pointer("/_meta/empty_turn_tip_code"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|code| !code.is_empty())
    {
        normalized.insert(
            "empty_turn_tip_code".to_string(),
            json!(empty_turn_tip_code),
        );
    }
    if let Some(empty_turn_tip_params) = object
        .get("empty_turn_tip_params")
        .or_else(|| object.get("emptyTurnTipParams"))
        .or_else(|| value.pointer("/_meta/empty_turn_tip_params"))
        .cloned()
        .filter(Value::is_object)
    {
        normalized.insert("empty_turn_tip_params".to_string(), empty_turn_tip_params);
    }
    Some(Value::Object(normalized))
}

pub(super) fn conversation_status_for_runtime(runtime: &Value) -> &'static str {
    match runtime.get("state").and_then(Value::as_str) {
        Some("starting") => "pending",
        Some("running" | "cancelling" | "waiting_confirmation") => "running",
        _ => "finished",
    }
}

pub(super) async fn load_biwork_conversation_runtime_summary(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
) -> Result<Value, AppError> {
    let pending_confirmations: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM approvals
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status = 'pending'
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .fetch_one(&state.connect_pool)
    .await?;

    let run = sqlx::query(
        r#"
        SELECT id, status
        FROM runs
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled')
        ORDER BY updated_at DESC, started_at DESC NULLS LAST, queued_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    let Some(run) = run else {
        return Ok(conversation_runtime_summary_for_run(
            None,
            None,
            pending_confirmations,
        ));
    };
    let run_id: Uuid = run.try_get("id")?;
    let status: String = run.try_get("status")?;
    Ok(conversation_runtime_summary_for_run(
        Some(status.as_str()),
        Some(run_id),
        pending_confirmations,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biwork_slash_commands_project_assistant_contract_fields() {
        let conversation_metadata = json!({
            "biwork": {
                "assistant": {
                    "available_commands": [
                        {
                            "name": "/review",
                            "description": "Review the current diff",
                            "input": {"hint": "diff scope"},
                            "_meta": {
                                "completion_behavior": "neutral_tip_on_empty",
                                "empty_turn_tip_code": "acp.empty_turn.choose_command",
                                "empty_turn_tip_params": {"command_count": 1}
                            },
                            "ignored_secret": "do-not-leak"
                        }
                    ]
                }
            }
        });

        let commands = biwork_slash_commands_from_sources(&conversation_metadata, None, None);

        assert_eq!(
            commands,
            vec![json!({
                "command": "review",
                "description": "Review the current diff",
                "hint": "diff scope",
                "completion_behavior": "neutral_tip_on_empty",
                "empty_turn_tip_code": "acp.empty_turn.choose_command",
                "empty_turn_tip_params": {"command_count": 1}
            })]
        );
    }

    #[test]
    fn biwork_slash_commands_fallback_to_assistant_config_and_dedupe() {
        let conversation_metadata = json!({
            "extra": {
                "runtime": {
                    "available_commands": "{\"commands\":[{\"command\":\"review\",\"description\":\"Conversation scoped\"}]}"
                }
            }
        });
        let assistant_config = json!({
            "slash_commands": [
                {"command": "review", "description": "Assistant duplicate"},
                {"command": "summarize", "description": "Summarize the conversation"}
            ]
        });

        let commands = biwork_slash_commands_from_sources(
            &conversation_metadata,
            None,
            Some(&assistant_config),
        );

        assert_eq!(
            commands,
            vec![
                json!({
                    "command": "review",
                    "description": "Conversation scoped"
                }),
                json!({
                    "command": "summarize",
                    "description": "Summarize the conversation"
                })
            ]
        );
    }

    #[test]
    fn conversation_runtime_summary_reflects_active_run_status() {
        let run_id = Uuid::new_v4();

        let idle = conversation_runtime_summary_for_run(None, None, 0);
        assert_eq!(idle["state"], "idle");
        assert_eq!(idle["can_send_message"], true);
        assert_eq!(idle["has_task"], false);
        assert!(idle["turn_id"].is_null());

        let queued = conversation_runtime_summary_for_run(Some("queued"), Some(run_id), 0);
        assert_eq!(queued["state"], "starting");
        assert_eq!(queued["task_status"], "pending");
        assert_eq!(queued["can_send_message"], false);
        assert_eq!(queued["turn_id"], run_id.to_string());

        let running = conversation_runtime_summary_for_run(Some("running"), Some(run_id), 0);
        assert_eq!(running["state"], "running");
        assert_eq!(running["task_status"], "running");
        assert_eq!(running["is_processing"], true);

        let waiting =
            conversation_runtime_summary_for_run(Some("waiting_approval"), Some(run_id), 2);
        assert_eq!(waiting["state"], "waiting_confirmation");
        assert_eq!(waiting["pending_confirmations"], 2);
        assert_eq!(waiting["has_task"], true);
    }

    #[test]
    fn conversation_status_follows_runtime_state_for_biwork_lists() {
        assert_eq!(
            conversation_status_for_runtime(&json!({ "state": "idle" })),
            "finished"
        );
        assert_eq!(
            conversation_status_for_runtime(&json!({ "state": "starting" })),
            "pending"
        );
        assert_eq!(
            conversation_status_for_runtime(&json!({ "state": "running" })),
            "running"
        );
        assert_eq!(
            conversation_status_for_runtime(&json!({ "state": "waiting_confirmation" })),
            "running"
        );
    }
}
