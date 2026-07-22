use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{agent_platform::ferriskey_oidc::PlatformRequestContext, core::errors::AppError},
    startup::AppState,
};

use super::{
    biwork_compat_service::{epoch_ms, ok, value_string},
    biwork_conversation_support::{ensure_conversation_exists, merge_conversation_extra},
};

#[derive(Debug, Deserialize, Default)]
pub struct ConversationMessagesQuery {
    limit: Option<i64>,
    before: Option<String>,
    after: Option<String>,
    anchor_message_id: Option<String>,
    content_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MessageSearchQuery {
    keyword: String,
    page: Option<i64>,
    page_size: Option<i64>,
}

pub async fn biwork_conversation_messages(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Query(query): Query<ConversationMessagesQuery>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let limit = conversation_messages_limit(query.limit);
    let before = parse_message_cursor(query.before.as_deref(), "before")?;
    let after = parse_message_cursor(query.after.as_deref(), "after")?;
    if before.is_some() && after.is_some() {
        return Err(AppError::InvalidInput(
            "before and after cannot be used together".to_string(),
        ));
    }
    let _content_mode = query.content_mode.as_deref().unwrap_or("compact");

    let (rows, has_more_before, has_more_after) = if let Some(anchor_message_id) =
        query.anchor_message_id.as_deref()
    {
        load_anchor_message_page_rows(
            &state,
            ctx.tenant_id,
            conversation_id,
            anchor_message_id,
            limit,
        )
        .await?
    } else if let Some(cursor) = before {
        load_before_message_page_rows(&state, ctx.tenant_id, conversation_id, cursor, limit).await?
    } else if let Some(cursor) = after {
        load_after_message_page_rows(&state, ctx.tenant_id, conversation_id, cursor, limit).await?
    } else {
        load_latest_message_page_rows(&state, ctx.tenant_id, conversation_id, limit).await?
    };

    Ok(ok(conversation_message_page_payload(
        conversation_id,
        rows,
        has_more_before,
        has_more_after,
    )?))
}

pub async fn biwork_get_conversation_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((conversation_id, message_id)): Path<(Uuid, String)>,
) -> Result<Json<Value>, AppError> {
    ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, seq, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
          AND (
              id::text = $3
              OR event_id = $3
              OR payload->>'message_id' = $3
              OR payload->>'tool_call_id' = $3
              OR run_id::text = $3
              OR CONCAT('assistant.', run_id::text) = $3
              OR CONCAT('user.', run_id::text) = $3
              OR CONCAT('error.', run_id::text) = $3
              OR CONCAT('tool.', payload->>'tool_call_id') = $3
          )
        ORDER BY seq ASC
        LIMIT 200
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(conversation_id)
    .bind(&message_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut items = Vec::with_capacity(rows.len());
    let mut pending_delta: Option<PendingDeltaMessage> = None;
    for row in rows {
        let event_type: String = row.try_get("type")?;
        match event_type.as_str() {
            "message.completed" => {
                reconcile_pending_delta_before_completed(
                    conversation_id,
                    row.try_get::<Option<Uuid>, _>("run_id")?,
                    &mut pending_delta,
                    &mut items,
                );
                if let Some(message) = message_from_event_row(conversation_id, &row)? {
                    items.push(message);
                }
            }
            "message.delta" => {
                append_delta_message(conversation_id, &row, &mut pending_delta, &mut items)?;
            }
            "tool.call.started"
            | "tool.call.delta"
            | "tool.call.completed"
            | "tool.call.failed" => {
                flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
                if let Some(message) = tool_message_from_event_row(conversation_id, &row)? {
                    push_or_replace_tool_message(&mut items, message);
                }
            }
            "run.completed" => {
                flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
            }
            "run.failed" => {
                flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
                items.push(run_failed_message_from_event_row(conversation_id, &row)?);
            }
            _ => {}
        }
    }
    flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
    let message = items
        .into_iter()
        .find(|item| {
            item.get("id").and_then(Value::as_str) == Some(message_id.as_str())
                || item.get("msg_id").and_then(Value::as_str) == Some(message_id.as_str())
        })
        .ok_or_else(|| AppError::NotFound("message not found".to_string()))?;
    Ok(ok(message))
}

pub async fn biwork_search_messages(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<MessageSearchQuery>,
) -> Result<Json<Value>, AppError> {
    let keyword = query.keyword.trim();
    if keyword.is_empty() {
        return Ok(ok(json!({
            "items": [],
            "total": 0,
            "has_more": false,
        })));
    }
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1).saturating_mul(page_size);
    let rows = sqlx::query(
        r#"
        SELECT e.id AS event_row_id,
               e.type AS event_type,
               e.run_id,
               e.payload,
               e.created_at AS message_created_at,
               c.id AS conversation_id,
               c.title,
               c.metadata,
               c.created_at AS conversation_created_at,
               c.updated_at AS conversation_updated_at
        FROM run_events e
        JOIN conversations c
          ON c.id = e.conversation_id
         AND c.tenant_id = e.tenant_id
        WHERE e.tenant_id = $1
          AND c.created_by_user_id = $2
          AND c.deleted_at IS NULL
          AND e.type IN ('message.completed', 'message.delta')
          AND e.payload::text ILIKE ('%' || $3 || '%')
        ORDER BY e.created_at DESC
        LIMIT $4 OFFSET $5
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(keyword)
    .bind(page_size + 1)
    .bind(offset)
    .fetch_all(&state.connect_pool)
    .await?;

    let has_more = rows.len() as i64 > page_size;
    let mut items = Vec::with_capacity(page_size as usize);
    for row in rows.into_iter().take(page_size as usize) {
        let payload: Value = row.try_get("payload")?;
        let Some(preview_text) = payload.get("content").and_then(message_content_text) else {
            continue;
        };
        if preview_text.trim().is_empty() {
            continue;
        }
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
        let conversation_created_at: OffsetDateTime = row.try_get("conversation_created_at")?;
        let conversation_updated_at: OffsetDateTime = row.try_get("conversation_updated_at")?;
        let event_row_id: Uuid = row.try_get("event_row_id")?;
        let event_type: String = row.try_get("event_type")?;
        let run_id: Option<Uuid> = row.try_get("run_id")?;
        items.push(json!({
            "message_id": biwork_search_result_message_id(
                event_row_id,
                event_type.as_str(),
                run_id,
                &payload,
            ),
            "message_type": "text",
            "message_created_at": epoch_ms(row.try_get("message_created_at")?),
            "preview_text": preview_text,
            "conversation": {
                "id": row.try_get::<Uuid, _>("conversation_id")?.to_string(),
                "name": row.try_get::<String, _>("title")?,
                "type": conversation_type,
                "model": model,
                "status": "finished",
                "source": "biwork",
                "pinned": false,
                "pinned_at": Value::Null,
                "channel_chat_id": Value::Null,
                "created_at": epoch_ms(conversation_created_at),
                "modified_at": epoch_ms(conversation_updated_at),
                "extra": merge_conversation_extra(extra),
            },
        }));
    }

    Ok(ok(json!({
        "items": items,
        "total": offset + i64::try_from(items.len()).unwrap_or(0),
        "has_more": has_more,
    })))
}

pub(super) fn conversation_messages_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 200)
}

pub(super) fn parse_message_cursor(
    value: Option<&str>,
    label: &str,
) -> Result<Option<i64>, AppError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let seq = value
        .parse::<i64>()
        .map_err(|_| AppError::InvalidInput(format!("{label} cursor must be an event seq")))?;
    if seq < 0 {
        return Err(AppError::InvalidInput(format!(
            "{label} cursor must be non-negative"
        )));
    }
    Ok(Some(seq))
}

async fn load_latest_message_page_rows(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    limit: i64,
) -> Result<(Vec<sqlx::postgres::PgRow>, bool, bool), AppError> {
    let mut rows = sqlx::query(
        r#"
        SELECT id, seq, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
        ORDER BY seq DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(limit + 1)
    .fetch_all(&state.connect_pool)
    .await?;
    let has_more_before = trim_desc_page_extra(&mut rows, limit);
    if has_more_before {
        expand_desc_page_to_run_start(state, tenant_id, conversation_id, &mut rows).await?;
    }
    rows.reverse();
    let has_more_before =
        has_relevant_events_before(state, tenant_id, conversation_id, &rows).await?;
    Ok((rows, has_more_before, false))
}

async fn load_before_message_page_rows(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    before: i64,
    limit: i64,
) -> Result<(Vec<sqlx::postgres::PgRow>, bool, bool), AppError> {
    let mut rows = sqlx::query(
        r#"
        SELECT id, seq, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
          AND seq < $3
        ORDER BY seq DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(before)
    .bind(limit + 1)
    .fetch_all(&state.connect_pool)
    .await?;
    let has_more_before = trim_desc_page_extra(&mut rows, limit);
    if has_more_before {
        expand_desc_page_to_run_start(state, tenant_id, conversation_id, &mut rows).await?;
    }
    rows.reverse();
    let has_more_before =
        has_relevant_events_before(state, tenant_id, conversation_id, &rows).await?;
    Ok((rows, has_more_before, true))
}

async fn expand_desc_page_to_run_start(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    rows: &mut Vec<sqlx::postgres::PgRow>,
) -> Result<(), AppError> {
    let Some(earliest) = rows.last() else {
        return Ok(());
    };
    let Some(run_id) = earliest.try_get::<Option<Uuid>, _>("run_id")? else {
        return Ok(());
    };
    let earliest_seq: i64 = earliest.try_get("seq")?;
    let mut preceding = sqlx::query(
        r#"
        SELECT id, seq, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND run_id = $3
          AND seq < $4
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
        ORDER BY seq DESC
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(run_id)
    .bind(earliest_seq)
    .fetch_all(&state.connect_pool)
    .await?;
    rows.append(&mut preceding);
    Ok(())
}

async fn has_relevant_events_before(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    rows: &[sqlx::postgres::PgRow],
) -> Result<bool, AppError> {
    let Some(oldest_seq) = rows
        .iter()
        .map(|row| row.try_get::<i64, _>("seq"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .min()
    else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM run_events
            WHERE tenant_id = $1
              AND conversation_id = $2
              AND seq < $3
              AND type IN (
                  'message.completed', 'message.delta', 'run.completed', 'run.failed',
                  'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
              )
        )
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(oldest_seq)
    .fetch_one(&state.connect_pool)
    .await?)
}

async fn load_after_message_page_rows(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    after: i64,
    limit: i64,
) -> Result<(Vec<sqlx::postgres::PgRow>, bool, bool), AppError> {
    let mut rows = sqlx::query(
        r#"
        SELECT id, seq, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
          AND seq > $3
        ORDER BY seq ASC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(after)
    .bind(limit + 1)
    .fetch_all(&state.connect_pool)
    .await?;
    let has_more_after = trim_asc_page_extra(&mut rows, limit);
    Ok((rows, true, has_more_after))
}

async fn load_anchor_message_page_rows(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    anchor_message_id: &str,
    limit: i64,
) -> Result<(Vec<sqlx::postgres::PgRow>, bool, bool), AppError> {
    let anchor_seq =
        resolve_message_anchor_seq(state, tenant_id, conversation_id, anchor_message_id)
            .await?
            .ok_or_else(|| AppError::NotFound("message not found".to_string()))?;
    let before_limit = (limit / 2).max(1);
    let after_limit = (limit - before_limit).max(0);

    let mut before_rows = sqlx::query(
        r#"
        SELECT id, seq, type, run_id, payload, created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
          AND seq <= $3
        ORDER BY seq DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(anchor_seq)
    .bind(before_limit + 1)
    .fetch_all(&state.connect_pool)
    .await?;
    let has_more_before = trim_desc_page_extra(&mut before_rows, before_limit);
    before_rows.reverse();

    let mut after_rows = if after_limit == 0 {
        Vec::new()
    } else {
        sqlx::query(
            r#"
            SELECT id, seq, type, run_id, payload, created_at
            FROM run_events
            WHERE tenant_id = $1
              AND conversation_id = $2
              AND type IN (
                  'message.completed', 'message.delta', 'run.completed', 'run.failed',
                  'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
              )
              AND seq > $3
            ORDER BY seq ASC
            LIMIT $4
            "#,
        )
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(anchor_seq)
        .bind(after_limit + 1)
        .fetch_all(&state.connect_pool)
        .await?
    };
    let has_more_after = if after_limit == 0 {
        message_event_exists_after(state, tenant_id, conversation_id, anchor_seq).await?
    } else {
        trim_asc_page_extra(&mut after_rows, after_limit)
    };

    before_rows.extend(after_rows);
    Ok((before_rows, has_more_before, has_more_after))
}

async fn resolve_message_anchor_seq(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    message_id: &str,
) -> Result<Option<i64>, AppError> {
    let message_id = message_id.trim();
    if message_id.is_empty() {
        return Ok(None);
    }
    let seq = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT seq
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND type IN (
              'message.completed', 'message.delta', 'run.completed', 'run.failed',
              'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
          )
          AND (
              id::text = $3
              OR event_id = $3
              OR payload->>'message_id' = $3
              OR payload->>'tool_call_id' = $3
              OR run_id::text = $3
              OR CONCAT('assistant.', run_id::text) = $3
              OR CONCAT('user.', run_id::text) = $3
              OR CONCAT('error.', run_id::text) = $3
              OR CONCAT('tool.', payload->>'tool_call_id') = $3
          )
        ORDER BY seq ASC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(message_id)
    .fetch_optional(&state.connect_pool)
    .await?;
    Ok(seq)
}

async fn message_event_exists_after(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    seq: i64,
) -> Result<bool, AppError> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM run_events
            WHERE tenant_id = $1
              AND conversation_id = $2
              AND type IN (
                  'message.completed', 'message.delta', 'run.completed', 'run.failed',
                  'tool.call.started', 'tool.call.delta', 'tool.call.completed', 'tool.call.failed'
              )
              AND seq > $3
        )
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(seq)
    .fetch_one(&state.connect_pool)
    .await?;
    Ok(exists)
}

fn trim_desc_page_extra(rows: &mut Vec<sqlx::postgres::PgRow>, limit: i64) -> bool {
    if rows.len() as i64 <= limit {
        return false;
    }
    rows.truncate(limit as usize);
    true
}

fn trim_asc_page_extra(rows: &mut Vec<sqlx::postgres::PgRow>, limit: i64) -> bool {
    if rows.len() as i64 <= limit {
        return false;
    }
    rows.truncate(limit as usize);
    true
}

fn conversation_message_page_payload(
    conversation_id: Uuid,
    rows: Vec<sqlx::postgres::PgRow>,
    has_more_before: bool,
    has_more_after: bool,
) -> Result<Value, AppError> {
    let mut items = Vec::with_capacity(rows.len());
    let mut oldest_cursor = Value::Null;
    let mut newest_cursor = Value::Null;
    let mut pending_delta: Option<PendingDeltaMessage> = None;
    for row in rows {
        let seq: i64 = row.try_get("seq")?;
        if oldest_cursor.is_null() {
            oldest_cursor = json!(seq.to_string());
        }
        newest_cursor = json!(seq.to_string());
        let event_type: String = row.try_get("type")?;
        match event_type.as_str() {
            "message.completed" => {
                reconcile_pending_delta_before_completed(
                    conversation_id,
                    row.try_get::<Option<Uuid>, _>("run_id")?,
                    &mut pending_delta,
                    &mut items,
                );
                if let Some(message) = message_from_event_row(conversation_id, &row)? {
                    items.push(message);
                }
            }
            "message.delta" => {
                append_delta_message(conversation_id, &row, &mut pending_delta, &mut items)?;
            }
            "tool.call.started"
            | "tool.call.delta"
            | "tool.call.completed"
            | "tool.call.failed" => {
                flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
                if let Some(message) = tool_message_from_event_row(conversation_id, &row)? {
                    push_or_replace_tool_message(&mut items, message);
                }
            }
            "run.completed" => {
                flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
            }
            "run.failed" => {
                flush_pending_delta(conversation_id, &mut pending_delta, &mut items);
                items.push(run_failed_message_from_event_row(conversation_id, &row)?);
            }
            _ => {}
        }
    }
    flush_pending_delta(conversation_id, &mut pending_delta, &mut items);

    Ok(json!({
        "items": items,
        "oldest_cursor": oldest_cursor,
        "newest_cursor": newest_cursor,
        "has_more_before": has_more_before,
        "has_more_after": has_more_after,
    }))
}

pub(super) fn biwork_search_result_message_id(
    event_id: Uuid,
    event_type: &str,
    run_id: Option<Uuid>,
    payload: &Value,
) -> String {
    payload
        .get("message_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            (event_type == "message.delta")
                .then_some(run_id)
                .flatten()
                .map(|id| format!("assistant.{id}"))
        })
        .unwrap_or_else(|| event_id.to_string())
}

fn message_from_event_row(
    conversation_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> Result<Option<Value>, AppError> {
    let payload: Value = row.try_get("payload")?;
    let content = payload
        .get("content")
        .and_then(message_content_text)
        .unwrap_or_default();
    if content.trim().is_empty() {
        return Ok(None);
    }
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    let event_id = row.try_get::<Uuid, _>("id")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let run_id: Option<Uuid> = row.try_get("run_id")?;
    let message_id = biwork_completed_message_id(&payload, role, run_id, event_id);

    Ok(Some(biwork_history_text_message_json(
        event_id.to_string(),
        message_id,
        conversation_id,
        run_id,
        content,
        created_at,
        if role == "user" { "right" } else { "left" },
    )))
}

fn tool_message_from_event_row(
    conversation_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> Result<Option<Value>, AppError> {
    let payload: Value = row.try_get("payload")?;
    let event_type: String = row.try_get("type")?;
    let Some(content) =
        biwork_tool_call_update_payload(conversation_id, event_type.as_str(), &payload)
    else {
        return Ok(None);
    };
    let tool_call_id = payload
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let run_id: Option<Uuid> = row.try_get("run_id")?;

    Ok(Some(json!({
        "id": row.try_get::<Uuid, _>("id")?.to_string(),
        "msg_id": format!("tool.{tool_call_id}"),
        "turn_id": run_id.map(|id| id.to_string()),
        "conversation_id": conversation_id.to_string(),
        "type": "acp_tool_call",
        "content": content,
        "created_at": epoch_ms(created_at),
        "position": "left",
        "status": if event_type == "tool.call.completed" || event_type == "tool.call.failed" {
            "finish"
        } else {
            "pending"
        },
        "hidden": false,
    })))
}

fn run_failed_message_from_event_row(
    conversation_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> Result<Value, AppError> {
    let payload: Value = row.try_get("payload")?;
    let event_id: Uuid = row.try_get("id")?;
    let run_id: Option<Uuid> = row.try_get("run_id")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let msg_id = value_string(&payload, "message_id").unwrap_or_else(|| {
        run_id
            .map(|id| format!("error.{id}"))
            .unwrap_or_else(|| format!("error.{event_id}"))
    });
    let detail = run_failed_history_detail(&payload);

    Ok(biwork_history_error_message_json(
        event_id.to_string(),
        msg_id,
        conversation_id,
        run_id,
        detail,
        &payload,
        created_at,
    ))
}

fn run_failed_history_detail(payload: &Value) -> String {
    payload
        .get("error")
        .and_then(|error| {
            error
                .as_str()
                .or_else(|| error.get("message").and_then(Value::as_str))
        })
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .or_else(|| payload.get("reason").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Run failed")
        .to_string()
}

pub(super) fn biwork_history_error_message_json(
    id: String,
    msg_id: String,
    conversation_id: Uuid,
    turn_id: Option<Uuid>,
    detail: String,
    payload: &Value,
    created_at: OffsetDateTime,
) -> Value {
    json!({
        "id": id,
        "msg_id": msg_id,
        "turn_id": turn_id.map(|id| id.to_string()),
        "conversation_id": conversation_id.to_string(),
        "type": "tips",
        "content": biwork_history_error_content(&detail, payload),
        "created_at": epoch_ms(created_at),
        "position": "center",
        "status": "error",
        "hidden": false,
    })
}

fn biwork_history_error_content(detail: &str, payload: &Value) -> Value {
    let mut error = Map::new();
    error.insert("message".to_string(), json!(detail));
    error.insert(
        "detail".to_string(),
        json!(value_string(payload, "detail").unwrap_or_else(|| detail.to_string())),
    );
    error.insert(
        "retryable".to_string(),
        json!(
            payload
                .get("retryable")
                .and_then(Value::as_bool)
                .unwrap_or(true)
        ),
    );

    if let Some(code) =
        value_string(payload, "code").or_else(|| value_string(payload, "error_code"))
    {
        error.insert("code".to_string(), json!(code));
    }
    if let Some(ownership) = value_string(payload, "ownership") {
        error.insert("ownership".to_string(), json!(ownership));
    }
    if let Some(workspace_path) =
        value_string(payload, "workspacePath").or_else(|| value_string(payload, "workspace_path"))
    {
        error.insert("workspacePath".to_string(), json!(workspace_path));
    }
    if let Some(feedback_recommended) = payload.get("feedback_recommended").and_then(Value::as_bool)
    {
        error.insert(
            "feedback_recommended".to_string(),
            json!(feedback_recommended),
        );
    }
    if let Some(resolution) = payload.get("resolution").filter(|value| value.is_object()) {
        error.insert("resolution".to_string(), resolution.clone());
    }
    if let Some(raw_error) = payload.get("rawError").filter(|value| value.is_object()) {
        error.insert("rawError".to_string(), raw_error.clone());
    }

    json!({
        "content": detail,
        "type": "error",
        "error": Value::Object(error),
    })
}

pub(super) fn push_or_replace_tool_message(items: &mut Vec<Value>, message: Value) {
    let Some(tool_call_id) = message
        .pointer("/content/update/tool_call_id")
        .and_then(Value::as_str)
    else {
        items.push(message);
        return;
    };
    if let Some(index) = items.iter().position(|item| {
        item.pointer("/content/update/tool_call_id")
            .and_then(Value::as_str)
            == Some(tool_call_id)
    }) {
        items.remove(index);
    }
    items.push(message);
}

pub(super) fn biwork_tool_call_update_payload(
    conversation_id: Uuid,
    event_type: &str,
    payload: &Value,
) -> Option<Value> {
    let tool_call_id = payload.get("tool_call_id").and_then(Value::as_str)?;
    let tool_name = payload
        .get("tool_name")
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let status = match event_type {
        "tool.call.started" | "tool.call.delta" => "in_progress",
        "tool.call.completed" => "completed",
        "tool.call.failed" => "failed",
        _ => return None,
    };
    let mut raw_input = serde_json::Map::new();
    if let Some(input_summary) = payload.get("input_summary").and_then(Value::as_str) {
        raw_input.insert("input_summary".to_string(), json!(input_summary));
    }
    if let Some(arguments_text) = payload.get("arguments_text").and_then(Value::as_str) {
        raw_input.insert("arguments_text".to_string(), json!(arguments_text));
    }
    if let Some(target) = payload.get("target") {
        raw_input.insert("target".to_string(), target.clone());
    }

    let mut raw_output = serde_json::Map::new();
    for key in ["output_summary", "error_summary", "error_type"] {
        if let Some(value) = payload.get(key) {
            raw_output.insert(key.to_string(), value.clone());
        }
    }
    if let Some(views) = payload.get("views") {
        raw_output.insert("views".to_string(), views.clone());
    }
    if let Some(browser) = payload.get("browser") {
        raw_output.insert("browser".to_string(), browser.clone());
    }
    raw_output.insert("status".to_string(), json!(status));

    Some(json!({
        "session_id": conversation_id.to_string(),
        "update": {
            "sessionUpdate": "tool_call",
            "tool_call_id": tool_call_id,
            "status": status,
            "title": tool_name,
            "kind": biwork_tool_kind(tool_name),
            "rawInput": Value::Object(raw_input),
            "rawOutput": Value::Object(raw_output),
            "content": biwork_tool_call_content(payload),
        },
    }))
}

fn biwork_tool_kind(tool_name: &str) -> &'static str {
    let normalized = tool_name.to_ascii_lowercase();
    if normalized.contains("read")
        || normalized == "ls"
        || normalized.contains("grep")
        || normalized.contains("glob")
    {
        "read"
    } else if normalized.contains("write")
        || normalized.contains("edit")
        || normalized.contains("patch")
    {
        "edit"
    } else {
        "execute"
    }
}

fn biwork_tool_call_content(payload: &Value) -> Value {
    let mut content = Vec::new();
    let Some(views) = payload.get("views").and_then(Value::as_array) else {
        return Value::Array(content);
    };
    for view in views {
        match view.get("kind").and_then(Value::as_str) {
            Some("file_diff") => {
                if let Some(files) = view.get("files").and_then(Value::as_array) {
                    for file in files {
                        let Some(diff) = file.get("file_diff").and_then(Value::as_str) else {
                            continue;
                        };
                        let path = file
                            .get("path")
                            .or_else(|| file.get("file_name"))
                            .and_then(Value::as_str)
                            .unwrap_or("changes.diff");
                        content.push(json!({
                            "type": "content",
                            "content": {
                                "type": "text",
                                "text": format!("```diff\n{diff}\n```\n\n`{path}`"),
                            },
                        }));
                    }
                }
            }
            Some("markdown") => {
                if let Some(text) = view.get("text").and_then(Value::as_str) {
                    content.push(json!({
                        "type": "content",
                        "content": {
                            "type": "text",
                            "text": text,
                        },
                    }));
                }
            }
            _ => {}
        }
    }
    Value::Array(content)
}

pub(super) struct PendingDeltaMessage {
    pub(super) run_id: Uuid,
    pub(super) first_event_id: Uuid,
    pub(super) created_at: OffsetDateTime,
    pub(super) content: String,
}

pub(super) fn reconcile_pending_delta_before_completed(
    conversation_id: Uuid,
    completed_run_id: Option<Uuid>,
    pending: &mut Option<PendingDeltaMessage>,
    items: &mut Vec<Value>,
) {
    let replaces_pending = completed_run_id.is_some_and(|run_id| {
        pending
            .as_ref()
            .is_some_and(|message| message.run_id == run_id)
    });
    if replaces_pending {
        pending.take();
    } else {
        flush_pending_delta(conversation_id, pending, items);
    }
}

fn append_delta_message(
    conversation_id: Uuid,
    row: &sqlx::postgres::PgRow,
    pending: &mut Option<PendingDeltaMessage>,
    items: &mut Vec<Value>,
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
            // Defensive flush for streams that missed run.completed.
            flush_pending_delta(conversation_id, pending, items);
            *pending = Some(PendingDeltaMessage {
                run_id,
                first_event_id,
                created_at,
                content,
            });
        }
        None => {
            *pending = Some(PendingDeltaMessage {
                run_id,
                first_event_id,
                created_at,
                content,
            });
        }
    }
    Ok(())
}

fn flush_pending_delta(
    conversation_id: Uuid,
    pending: &mut Option<PendingDeltaMessage>,
    items: &mut Vec<Value>,
) {
    let Some(message) = pending.take() else {
        return;
    };
    if message.content.trim().is_empty() {
        return;
    }
    items.push(biwork_history_text_message_json(
        message.first_event_id.to_string(),
        format!("assistant.{}", message.run_id),
        conversation_id,
        Some(message.run_id),
        message.content,
        message.created_at,
        "left",
    ));
}

pub(super) fn biwork_history_text_message_json(
    id: String,
    msg_id: String,
    conversation_id: Uuid,
    turn_id: Option<Uuid>,
    content: String,
    created_at: OffsetDateTime,
    position: &str,
) -> Value {
    json!({
        "id": id,
        "msg_id": msg_id,
        "turn_id": turn_id.map(|id| id.to_string()),
        "conversation_id": conversation_id.to_string(),
        "type": "text",
        "content": {
            "content": content,
        },
        "created_at": epoch_ms(created_at),
        "position": position,
        "status": "finish",
        "hidden": false,
    })
}

pub(super) fn biwork_completed_message_id(
    payload: &Value,
    role: &str,
    run_id: Option<Uuid>,
    event_id: Uuid,
) -> String {
    payload
        .get("message_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            run_id.map(|run_id| {
                if role == "user" {
                    format!("user.{run_id}")
                } else {
                    format!("assistant.{run_id}")
                }
            })
        })
        .unwrap_or_else(|| event_id.to_string())
}

pub(super) fn message_content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(message_content_text)
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| object.get("content").and_then(message_content_text)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    #[test]
    fn history_text_message_includes_turn_id_for_biwork_replay() {
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let event_id = Uuid::new_v4().to_string();
        let created_at = OffsetDateTime::UNIX_EPOCH + Duration::seconds(12);

        let message = biwork_history_text_message_json(
            event_id.clone(),
            "assistant-msg".to_string(),
            conversation_id,
            Some(run_id),
            "hello".to_string(),
            created_at,
            "left",
        );

        assert_eq!(message["id"], event_id);
        assert_eq!(message["msg_id"], "assistant-msg");
        assert_eq!(message["turn_id"], run_id.to_string());
        assert_eq!(message["conversation_id"], conversation_id.to_string());
        assert_eq!(message["content"]["content"], "hello");
        assert_eq!(message["created_at"], 12_000);
        assert_eq!(message["position"], "left");
        assert_eq!(message["status"], "finish");
    }

    #[test]
    fn completed_history_message_uses_the_live_stream_identity() {
        let run_id = Uuid::new_v4();
        let event_id = Uuid::new_v4();

        assert_eq!(
            biwork_completed_message_id(&json!({}), "assistant", Some(run_id), event_id),
            format!("assistant.{run_id}")
        );
        assert_eq!(
            biwork_completed_message_id(&json!({}), "user", Some(run_id), event_id),
            format!("user.{run_id}")
        );
        assert_eq!(
            biwork_completed_message_id(
                &json!({ "message_id": "explicit-message" }),
                "assistant",
                Some(run_id),
                event_id,
            ),
            "explicit-message"
        );
    }

    #[test]
    fn completed_message_replaces_pending_deltas_from_the_same_run() {
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let mut pending = Some(PendingDeltaMessage {
            run_id,
            first_event_id: Uuid::new_v4(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            content: "streamed text".to_string(),
        });
        let mut items = Vec::new();

        reconcile_pending_delta_before_completed(
            conversation_id,
            Some(run_id),
            &mut pending,
            &mut items,
        );

        assert!(pending.is_none());
        assert!(items.is_empty());
    }

    #[test]
    fn history_run_failed_message_matches_biwork_tips_error_contract() {
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let event_id = Uuid::new_v4().to_string();
        let created_at = OffsetDateTime::UNIX_EPOCH + Duration::seconds(13);

        let message = biwork_history_error_message_json(
            event_id.clone(),
            format!("error.{run_id}"),
            conversation_id,
            Some(run_id),
            "provider rejected request".to_string(),
            &json!({
                "code": "USER_LLM_PROVIDER_AUTH_FAILED",
                "detail": "provider returned HTTP 401",
                "ownership": "user_llm_provider",
                "retryable": false,
                "feedback_recommended": false,
                "resolution": {"kind": "check_provider_credentials", "target": "provider_settings"},
                "rawError": {"name": "AuthenticationError", "status": 401},
            }),
            created_at,
        );

        assert_eq!(message["id"], event_id);
        assert_eq!(message["msg_id"], format!("error.{run_id}"));
        assert_eq!(message["turn_id"], run_id.to_string());
        assert_eq!(message["conversation_id"], conversation_id.to_string());
        assert_eq!(message["type"], "tips");
        assert_eq!(message["content"]["content"], "provider rejected request");
        assert_eq!(message["content"]["type"], "error");
        assert_eq!(
            message["content"]["error"]["message"],
            "provider rejected request"
        );
        assert_eq!(
            message["content"]["error"]["code"],
            "USER_LLM_PROVIDER_AUTH_FAILED"
        );
        assert_eq!(message["content"]["error"]["retryable"], false);
        assert_eq!(message["content"]["error"]["feedback_recommended"], false);
        assert_eq!(
            message["content"]["error"]["detail"],
            "provider returned HTTP 401"
        );
        assert_eq!(
            message["content"]["error"]["ownership"],
            "user_llm_provider"
        );
        assert_eq!(
            message["content"]["error"]["resolution"]["kind"],
            "check_provider_credentials"
        );
        assert_eq!(message["content"]["error"]["rawError"]["status"], 401);
        assert_eq!(message["created_at"], 13_000);
        assert_eq!(message["position"], "center");
        assert_eq!(message["status"], "error");
    }

    #[test]
    fn tool_call_update_payload_matches_biwork_acp_message_contract() {
        let conversation_id = Uuid::new_v4();
        let tool_call_id = Uuid::new_v4();

        let payload = biwork_tool_call_update_payload(
            conversation_id,
            "tool.call.completed",
            &json!({
                "tool_call_id": tool_call_id.to_string(),
                "tool_name": "write_file",
                "output_summary": "updated report",
                "browser": {
                    "kind": "browser",
                    "action": "snapshot",
                    "url": "https://example.com"
                },
                "views": [{
                    "kind": "file_diff",
                    "files": [{
                        "file_name": "report.md",
                        "path": "/workspace/report.md",
                        "file_diff": "--- a/report.md\n+++ b/report.md\n@@\n-old\n+new\n"
                    }]
                }]
            }),
        )
        .expect("tool call update payload");

        assert_eq!(payload["session_id"], conversation_id.to_string());
        assert_eq!(payload["update"]["sessionUpdate"], "tool_call");
        assert_eq!(payload["update"]["tool_call_id"], tool_call_id.to_string());
        assert_eq!(payload["update"]["status"], "completed");
        assert_eq!(payload["update"]["kind"], "edit");
        assert_eq!(
            payload["update"]["rawOutput"]["views"][0]["kind"],
            "file_diff"
        );
        assert_eq!(
            payload["update"]["rawOutput"]["browser"]["url"],
            "https://example.com"
        );
        assert!(
            payload["update"]["content"][0]["content"]["text"]
                .as_str()
                .expect("content text")
                .contains("```diff")
        );
    }

    #[test]
    fn history_tool_messages_keep_latest_state_per_tool_call_id() {
        let tool_call_id = Uuid::new_v4();
        let mut items = vec![json!({
            "type": "acp_tool_call",
            "content": {
                "update": {
                    "tool_call_id": tool_call_id.to_string(),
                    "status": "in_progress"
                }
            }
        })];

        push_or_replace_tool_message(
            &mut items,
            json!({
                "type": "acp_tool_call",
                "content": {
                    "update": {
                        "tool_call_id": tool_call_id.to_string(),
                        "status": "completed"
                    }
                }
            }),
        );

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["content"]["update"]["status"], "completed");
    }

    #[test]
    fn search_result_message_id_uses_biwork_message_identity() {
        let event_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();

        assert_eq!(
            biwork_search_result_message_id(
                event_id,
                "message.completed",
                Some(run_id),
                &json!({ "message_id": format!("user.{run_id}") }),
            ),
            format!("user.{run_id}")
        );
        assert_eq!(
            biwork_search_result_message_id(
                event_id,
                "message.delta",
                Some(run_id),
                &json!({ "content": "hello" }),
            ),
            format!("assistant.{run_id}")
        );
        assert_eq!(
            biwork_search_result_message_id(
                event_id,
                "message.completed",
                Some(run_id),
                &json!({ "content": "hello" }),
            ),
            event_id.to_string()
        );
    }

    #[test]
    fn conversation_messages_limit_defaults_and_clamps_for_biwork_pages() {
        assert_eq!(conversation_messages_limit(None), 50);
        assert_eq!(conversation_messages_limit(Some(0)), 1);
        assert_eq!(conversation_messages_limit(Some(25)), 25);
        assert_eq!(conversation_messages_limit(Some(500)), 200);
    }

    #[test]
    fn parse_message_cursor_accepts_event_seq_only() {
        assert_eq!(parse_message_cursor(None, "before").unwrap(), None);
        assert_eq!(
            parse_message_cursor(Some(" 42 "), "before").unwrap(),
            Some(42)
        );
        assert!(matches!(
            parse_message_cursor(Some("msg-1"), "before"),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            parse_message_cursor(Some("-1"), "after"),
            Err(AppError::InvalidInput(_))
        ));
    }
}
