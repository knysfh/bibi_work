use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde_json::{Value, json};
use sqlx::Row;
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{agent_platform::ferriskey_oidc::PlatformRequestContext, core::errors::AppError},
    startup::AppState,
};

use super::{
    biwork_compat_service::{epoch_ms, ok, required_string},
    biwork_conversation_message_service::message_content_text,
    biwork_conversation_support::{ensure_conversation_exists, parse_uuid_id},
};

pub async fn biwork_list_conversation_artifacts(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    sync_biwork_conversation_artifacts_from_events(&state, &ctx, conversation_id).await?;

    let rows = sqlx::query(
        r#"
        SELECT a.id,
               a.conversation_id,
               a.scheduled_job_id,
               a.artifact_kind,
               a.status,
               a.payload,
               a.created_at,
               a.updated_at
        FROM scheduled_job_artifacts a
        JOIN scheduled_jobs j
          ON j.id = a.scheduled_job_id
         AND j.tenant_id = a.tenant_id
        WHERE a.tenant_id = $1
          AND a.conversation_id = $2
          AND j.created_by_user_id = $3
          AND j.deleted_at IS NULL
        ORDER BY a.created_at ASC, a.id ASC
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(ctx.platform_user_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let artifacts = rows
        .iter()
        .map(biwork_conversation_artifact_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ok(Value::Array(artifacts)))
}

pub async fn biwork_update_conversation_artifact(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((conversation_id, artifact_id)): Path<(Uuid, String)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    sync_biwork_conversation_artifacts_from_events(&state, &ctx, conversation_id).await?;
    let artifact_id = parse_uuid_id(&artifact_id, "conversation artifact")?;
    let status = biwork_conversation_artifact_status(&required_string(&payload, "status")?)?;
    let row = sqlx::query(
        r#"
        UPDATE scheduled_job_artifacts a
        SET status = $5,
            updated_at = CURRENT_TIMESTAMP
        FROM scheduled_jobs j
        WHERE a.scheduled_job_id = j.id
          AND a.tenant_id = j.tenant_id
          AND a.id = $1
          AND a.tenant_id = $2
          AND a.conversation_id = $3
          AND j.created_by_user_id = $4
          AND j.deleted_at IS NULL
        RETURNING a.id,
                  a.conversation_id,
                  a.scheduled_job_id,
                  a.artifact_kind,
                  a.status,
                  a.payload,
                  a.created_at,
                  a.updated_at
        "#,
    )
    .bind(artifact_id)
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(ctx.platform_user_id)
    .bind(status)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation artifact not found".to_string()))?;

    Ok(ok(biwork_conversation_artifact_from_row(&row)?))
}

pub(super) struct ParsedSkillSuggestArtifact {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) skill_content: String,
}

struct ConversationArtifactCandidate {
    source_event_id: Uuid,
    run_id: Option<Uuid>,
    created_at: OffsetDateTime,
    parsed: ParsedSkillSuggestArtifact,
}

struct PendingArtifactDelta {
    run_id: Uuid,
    first_event_id: Uuid,
    created_at: OffsetDateTime,
    content: String,
}

async fn sync_biwork_conversation_artifacts_from_events(
    state: &AppState,
    ctx: &PlatformRequestContext,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let cron_job = sqlx::query(
        r#"
        SELECT id, name
        FROM scheduled_jobs
        WHERE tenant_id = $1
          AND created_by_user_id = $2
          AND deleted_at IS NULL
          AND (source_conversation_id = $3 OR target_conversation_id = $3)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(conversation_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    let Some(cron_job) = cron_job else {
        return Ok(());
    };
    let cron_job_id: Uuid = cron_job.try_get("id")?;

    let rows = sqlx::query(
        r#"
        SELECT id, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN ('message.completed', 'message.delta')
        ORDER BY seq ASC
        LIMIT 1000
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut candidates = Vec::new();
    let mut pending_delta: Option<PendingArtifactDelta> = None;
    for row in rows {
        let event_type: String = row.try_get("type")?;
        if event_type == "message.delta" {
            append_skill_suggest_delta_candidate(&row, &mut pending_delta, &mut candidates)?;
            continue;
        }

        flush_skill_suggest_delta_candidate(&mut pending_delta, &mut candidates);
        let payload: Value = row.try_get("payload")?;
        let role = payload
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("assistant");
        if role != "assistant" {
            continue;
        }
        let Some(content) = payload.get("content").and_then(message_content_text) else {
            continue;
        };
        if let Some(parsed) = parse_skill_suggest_artifact(&content) {
            candidates.push(ConversationArtifactCandidate {
                source_event_id: row.try_get("id")?,
                run_id: row.try_get("run_id")?,
                created_at: row.try_get("created_at")?,
                parsed,
            });
        }
    }
    flush_skill_suggest_delta_candidate(&mut pending_delta, &mut candidates);

    let mut candidates_by_key: HashMap<String, ConversationArtifactCandidate> = HashMap::new();
    for candidate in candidates {
        let artifact_key =
            biwork_skill_suggest_artifact_key(candidate.run_id, candidate.source_event_id);
        candidates_by_key.insert(artifact_key, candidate);
    }

    for (artifact_key, candidate) in candidates_by_key {
        let payload = skill_suggest_artifact_payload(cron_job_id, &candidate.parsed);
        sqlx::query(
            r#"
            INSERT INTO scheduled_job_artifacts (
                id, tenant_id, scheduled_job_id, conversation_id, source_event_id,
                artifact_key, artifact_kind, status, payload, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'skill_suggest', 'pending', $7, $8, CURRENT_TIMESTAMP)
            ON CONFLICT (tenant_id, scheduled_job_id, artifact_kind, artifact_key)
            DO UPDATE
            SET conversation_id = EXCLUDED.conversation_id,
                source_event_id = COALESCE(scheduled_job_artifacts.source_event_id, EXCLUDED.source_event_id),
                payload = CASE
                    WHEN scheduled_job_artifacts.status = 'pending' THEN EXCLUDED.payload
                    ELSE scheduled_job_artifacts.payload
                END,
                updated_at = CASE
                    WHEN scheduled_job_artifacts.status = 'pending' THEN CURRENT_TIMESTAMP
                    ELSE scheduled_job_artifacts.updated_at
                END
            "#,
        )
        .bind(candidate.source_event_id)
        .bind(ctx.tenant_id)
        .bind(cron_job_id)
        .bind(conversation_id)
        .bind(candidate.source_event_id)
        .bind(artifact_key)
        .bind(payload)
        .bind(candidate.created_at)
        .execute(&state.connect_pool)
        .await?;
    }

    Ok(())
}

fn append_skill_suggest_delta_candidate(
    row: &sqlx::postgres::PgRow,
    pending: &mut Option<PendingArtifactDelta>,
    candidates: &mut Vec<ConversationArtifactCandidate>,
) -> Result<(), AppError> {
    let payload: Value = row.try_get("payload")?;
    let Some(content) = payload.get("content").and_then(message_content_text) else {
        return Ok(());
    };
    if content.is_empty() {
        return Ok(());
    }
    let run_id = row
        .try_get::<Option<Uuid>, _>("run_id")?
        .unwrap_or_else(Uuid::new_v4);
    let first_event_id = row.try_get::<Uuid, _>("id")?;
    let created_at = row.try_get::<OffsetDateTime, _>("created_at")?;

    match pending {
        Some(message) if message.run_id == run_id => {
            message.content.push_str(&content);
        }
        Some(_) => {
            flush_skill_suggest_delta_candidate(pending, candidates);
            *pending = Some(PendingArtifactDelta {
                run_id,
                first_event_id,
                created_at,
                content,
            });
        }
        None => {
            *pending = Some(PendingArtifactDelta {
                run_id,
                first_event_id,
                created_at,
                content,
            });
        }
    }
    Ok(())
}

fn flush_skill_suggest_delta_candidate(
    pending: &mut Option<PendingArtifactDelta>,
    candidates: &mut Vec<ConversationArtifactCandidate>,
) {
    let Some(message) = pending.take() else {
        return;
    };
    if let Some(parsed) = parse_skill_suggest_artifact(&message.content) {
        candidates.push(ConversationArtifactCandidate {
            source_event_id: message.first_event_id,
            run_id: Some(message.run_id),
            created_at: message.created_at,
            parsed,
        });
    }
}

pub(super) fn parse_skill_suggest_artifact(content: &str) -> Option<ParsedSkillSuggestArtifact> {
    let block = extract_case_insensitive_block(content, "[SKILL_SUGGEST]", "[/SKILL_SUGGEST]")?;
    let name = block_line_field(block, "name")?;
    let description = block_line_field(block, "description").unwrap_or_else(|| name.clone());
    let skill_content = block_multiline_field(block, "content")?;
    if !is_valid_skill_suggest_content(&skill_content) {
        return None;
    }
    Some(ParsedSkillSuggestArtifact {
        name,
        description,
        skill_content,
    })
}

fn extract_case_insensitive_block<'a>(
    content: &'a str,
    start_marker: &str,
    end_marker: &str,
) -> Option<&'a str> {
    let lower = content.to_ascii_lowercase();
    let start = lower.find(&start_marker.to_ascii_lowercase())?;
    let block_start = start + start_marker.len();
    let end = lower[block_start..].find(&end_marker.to_ascii_lowercase())? + block_start;
    Some(&content[block_start..end])
}

fn block_line_field(block: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}:");
    block.lines().find_map(|line| {
        let trimmed = line.trim_start();
        if !starts_with_ascii_case_insensitive(trimmed, &prefix) {
            return None;
        }
        let value = trimmed[prefix.len()..].trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn block_multiline_field(block: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}:");
    let mut offset = 0;
    for segment in block.split_inclusive('\n') {
        let trimmed = segment.trim_start();
        if starts_with_ascii_case_insensitive(trimmed, &prefix) {
            let leading = segment.len().saturating_sub(trimmed.len());
            let value_start = offset + leading + prefix.len();
            let line_end = offset + segment.len();
            let inline_value = block[value_start..line_end].trim();
            let value = if inline_value.is_empty() {
                block[line_end..].trim()
            } else {
                block[value_start..].trim()
            };
            return (!value.is_empty()).then(|| value.to_string());
        }
        offset += segment.len();
    }
    None
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn is_valid_skill_suggest_content(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.starts_with("---")
        && trimmed.contains("\n---")
        && trimmed.lines().any(|line| {
            let line = line.trim_start();
            starts_with_ascii_case_insensitive(line, "name:")
        })
        && trimmed.lines().any(|line| {
            let line = line.trim_start();
            starts_with_ascii_case_insensitive(line, "description:")
        })
}

pub(super) fn biwork_skill_suggest_artifact_key(
    run_id: Option<Uuid>,
    source_event_id: Uuid,
) -> String {
    format!("skill_suggest.{}", run_id.unwrap_or(source_event_id))
}

pub(super) fn skill_suggest_artifact_payload(
    cron_job_id: Uuid,
    parsed: &ParsedSkillSuggestArtifact,
) -> Value {
    json!({
        "cron_job_id": cron_job_id.to_string(),
        "name": parsed.name.clone(),
        "description": parsed.description.clone(),
        "skill_content": parsed.skill_content.clone(),
        "skillContent": parsed.skill_content.clone(),
    })
}

pub(super) fn biwork_conversation_artifact_status(value: &str) -> Result<&'static str, AppError> {
    match value.trim() {
        "active" => Ok("active"),
        "pending" => Ok("pending"),
        "dismissed" => Ok("dismissed"),
        "saved" => Ok("saved"),
        _ => Err(AppError::InvalidInput(
            "unsupported conversation artifact status".to_string(),
        )),
    }
}

fn biwork_conversation_artifact_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let id: Uuid = row.try_get("id")?;
    let conversation_id: Option<Uuid> = row.try_get("conversation_id")?;
    let scheduled_job_id: Uuid = row.try_get("scheduled_job_id")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(json!({
        "id": id.to_string(),
        "conversation_id": conversation_id.map(|id| id.to_string()).unwrap_or_default(),
        "cron_job_id": scheduled_job_id.to_string(),
        "kind": row.try_get::<String, _>("artifact_kind")?,
        "status": row.try_get::<String, _>("status")?,
        "payload": row.try_get::<Value, _>("payload")?,
        "created_at": epoch_ms(created_at),
        "updated_at": epoch_ms(updated_at),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    use super::super::biwork_cron_service::cron_trigger_artifact_payload;

    #[test]
    fn skill_suggest_artifact_parser_accepts_valid_biwork_block() {
        let content = r#"
Here is the skill:

[SKILL_SUGGEST]
name: Daily Summary
description: Summarize daily activity
content:
---
name: daily-summary
description: Summarize daily activity
---

Use this skill for daily reports.
[/SKILL_SUGGEST]
"#;

        let parsed = parse_skill_suggest_artifact(content).expect("valid skill suggestion");
        assert_eq!(parsed.name, "Daily Summary");
        assert_eq!(parsed.description, "Summarize daily activity");
        assert!(parsed.skill_content.contains("name: daily-summary"));

        let cron_job_id = Uuid::new_v4();
        let payload = skill_suggest_artifact_payload(cron_job_id, &parsed);
        assert_eq!(payload["cron_job_id"], cron_job_id.to_string());
        assert_eq!(payload["name"], "Daily Summary");
        assert_eq!(payload["skill_content"], payload["skillContent"]);
    }

    #[test]
    fn skill_suggest_artifact_parser_rejects_invalid_skill_content() {
        let content = r#"
[SKILL_SUGGEST]
name: Bad Skill
description: Missing frontmatter
content:
plain text only
[/SKILL_SUGGEST]
"#;

        assert!(parse_skill_suggest_artifact(content).is_none());
    }

    #[test]
    fn conversation_artifact_helpers_preserve_biwork_contract() {
        let event_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let cron_job_id = Uuid::new_v4();
        let triggered_at = OffsetDateTime::UNIX_EPOCH + Duration::seconds(12);

        assert_eq!(
            biwork_skill_suggest_artifact_key(Some(run_id), event_id),
            format!("skill_suggest.{run_id}")
        );
        let payload = cron_trigger_artifact_payload(cron_job_id, "Daily", triggered_at);
        assert_eq!(payload["cron_job_id"], cron_job_id.to_string());
        assert_eq!(payload["cron_job_name"], "Daily");
        assert_eq!(payload["triggered_at"], 12_000);
        assert_eq!(
            biwork_conversation_artifact_status("pending").unwrap(),
            "pending"
        );
        assert_eq!(
            biwork_conversation_artifact_status("saved").unwrap(),
            "saved"
        );
        assert!(biwork_conversation_artifact_status("unknown").is_err());
    }
}
