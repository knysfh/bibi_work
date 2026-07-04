use std::{convert::Infallible, time::Duration};

use async_stream::stream;
use axum::{
    extract::ws::{Message, WebSocket},
    http::HeaderMap,
    response::sse::{Event, Sse},
};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use tokio::time::{MissedTickBehavior, sleep};
use tracing::warn;
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::models::{RunEventInput, StreamEventResponse};

pub async fn insert_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    conversation_id: Uuid,
    run_id: Option<Uuid>,
    event: RunEventInput,
) -> Result<StreamEventResponse, AppError> {
    let seq = next_event_seq(tx, conversation_id).await?;
    let event_id = event.event_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let payload = event.payload.unwrap_or_else(|| json!({}));

    let row = sqlx::query(
        r#"
        INSERT INTO run_events (
            tenant_id, conversation_id, run_id, seq, event_id, type, payload, trace_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, tenant_id, conversation_id, run_id, seq, event_id, type, payload, trace_id, created_at
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(run_id)
    .bind(seq)
    .bind(event_id)
    .bind(event.event_type)
    .bind(payload)
    .bind(event.trace_id)
    .fetch_one(&mut **tx)
    .await?;

    let response = event_from_row(row)?;
    let outbox_payload = serde_json::to_value(&response)
        .map_err(|_| AppError::InvalidInput("failed to encode event payload".to_string()))?;

    sqlx::query(
        r#"
        INSERT INTO event_outbox (tenant_id, event_row_id, target, payload)
        VALUES ($1, $2, 'redis', $3)
        "#,
    )
    .bind(tenant_id)
    .bind(response.id)
    .bind(outbox_payload)
    .execute(&mut **tx)
    .await?;

    Ok(response)
}

pub async fn update_run_status_from_event(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Option<Uuid>,
    event_type: &str,
) -> Result<(), AppError> {
    let Some(run_id) = run_id else {
        return Ok(());
    };

    let status = match event_type {
        "run.started" => "running",
        "approval.requested" | "interrupt.requested" => "waiting_approval",
        "run.completed" => "completed",
        "run.failed" => "failed",
        "run.cancelled" => "cancelled",
        _ => return Ok(()),
    };

    sqlx::query(
        r#"
        UPDATE runs
        SET status = $1,
            started_at = CASE WHEN $1 = 'running' THEN COALESCE(started_at, CURRENT_TIMESTAMP) ELSE started_at END,
            completed_at = CASE WHEN $1 IN ('completed', 'failed', 'cancelled') THEN CURRENT_TIMESTAMP ELSE completed_at END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(status)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

pub fn is_run_state_event(event_type: &str) -> bool {
    matches!(
        event_type,
        "run.started"
            | "approval.requested"
            | "interrupt.requested"
            | "run.completed"
            | "run.failed"
            | "run.cancelled"
    )
}

pub async fn fetch_events(
    pool: &PgPool,
    conversation_id: Uuid,
    after_seq: i64,
) -> Result<Vec<StreamEventResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, run_id, seq, event_id, type, payload, trace_id, created_at
        FROM run_events
        WHERE conversation_id = $1 AND seq > $2
        ORDER BY seq ASC
        LIMIT 1000
        "#,
    )
    .bind(conversation_id)
    .bind(after_seq)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(event_from_row)
        .collect::<Result<Vec<_>, AppError>>()
}

pub fn events_to_sse(
    events: Vec<StreamEventResponse>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let output = stream! {
        for event in events {
            yield Ok(event_to_sse(event));
        }
    };

    sse_with_keep_alive(output)
}

pub fn live_events_to_sse(
    pool: PgPool,
    redis_client: redis::Client,
    tenant_id: Uuid,
    conversation_id: Uuid,
    after_seq: i64,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let output = stream! {
        let mut cursor = after_seq;
        let mut pubsub = match redis_client.get_async_pubsub().await {
            Ok(pubsub) => Some(pubsub),
            Err(err) => {
                warn!(
                    "failed to open redis pubsub for conversation {}: {}",
                    conversation_id, err
                );
                yield Ok(Event::default()
                    .event("stream.warning")
                    .data(json!({ "message": "redis pubsub unavailable; falling back to polling" }).to_string()));
                None
            }
        };

        if let Some(pubsub_conn) = pubsub.as_mut() {
            let channel = conversation_pubsub_channel(tenant_id, conversation_id);
            if let Err(err) = pubsub_conn.subscribe(&channel).await {
                warn!(
                    "failed to subscribe redis channel {} for conversation {}: {}",
                    channel, conversation_id, err
                );
                yield Ok(Event::default()
                    .event("stream.warning")
                    .data(json!({ "message": "redis subscription unavailable; falling back to polling" }).to_string()));
                pubsub = None;
            }
        }

        match fetch_events_after(&pool, conversation_id, cursor).await {
            Ok((events, next_cursor)) => {
                cursor = next_cursor;
                for event in events {
                    yield Ok(event_to_sse(event));
                }
            }
            Err(err) => {
                warn!(
                    "failed to fetch initial live conversation events {} after seq {}: {}",
                    conversation_id, cursor, err
                );
                yield Ok(Event::default()
                    .event("stream.error")
                    .data(json!({ "message": "failed to fetch events" }).to_string()));
            }
        }

        if let Some(mut pubsub) = pubsub {
            let mut messages = pubsub.on_message();
            let mut backfill = tokio::time::interval(Duration::from_secs(30));
            backfill.set_missed_tick_behavior(MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    message = messages.next() => {
                        if message.is_none() {
                            warn!(
                                "redis pubsub stream ended for conversation {}; falling back to polling",
                                conversation_id
                            );
                            yield Ok(Event::default()
                                .event("stream.warning")
                                .data(json!({ "message": "redis subscription ended; falling back to polling" }).to_string()));
                            break;
                        }
                    }
                    _ = backfill.tick() => {}
                }

                match fetch_events_after(&pool, conversation_id, cursor).await {
                    Ok((events, next_cursor)) => {
                        cursor = next_cursor;
                        for event in events {
                            yield Ok(event_to_sse(event));
                        }
                    }
                    Err(err) => {
                        warn!(
                            "failed to backfill live conversation events {} after seq {}: {}",
                            conversation_id, cursor, err
                        );
                        yield Ok(Event::default()
                            .event("stream.error")
                            .data(json!({ "message": "failed to fetch events" }).to_string()));
                    }
                }
            }
        }

        loop {
            let fetched = match fetch_events_after(&pool, conversation_id, cursor).await {
                Ok((events, next_cursor)) => {
                    let count = events.len();
                    cursor = next_cursor;
                    for event in events {
                        yield Ok(event_to_sse(event));
                    }
                    count
                }
                Err(err) => {
                    warn!(
                        "failed to poll live conversation events {} after seq {}: {}",
                        conversation_id, cursor, err
                    );
                    yield Ok(Event::default()
                        .event("stream.error")
                        .data(json!({ "message": "failed to fetch events" }).to_string()));
                    0
                }
            };

            if fetched < 1000 {
                sleep(Duration::from_secs(1)).await;
            }
        }
    };

    sse_with_keep_alive(output)
}

fn sse_with_keep_alive<S>(stream: S) -> Sse<impl Stream<Item = Result<Event, Infallible>>>
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("heartbeat"),
    )
}

pub fn resolve_after_seq(headers: &HeaderMap, explicit: Option<i64>) -> i64 {
    explicit
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<i64>().ok())
        })
        .unwrap_or(0)
}

pub async fn handle_conversation_socket(
    mut socket: WebSocket,
    pool: PgPool,
    redis_client: redis::Client,
    tenant_id: Uuid,
    conversation_id: Uuid,
    after_seq: i64,
) {
    let mut cursor = after_seq;
    if !send_new_socket_events(&mut socket, &pool, conversation_id, &mut cursor).await {
        return;
    }

    let mut pubsub = match redis_client.get_async_pubsub().await {
        Ok(pubsub) => Some(pubsub),
        Err(err) => {
            warn!(
                "failed to open redis pubsub for websocket conversation {}: {}",
                conversation_id, err
            );
            if socket
                .send(Message::Text(
                    json!({
                        "type": "stream.warning",
                        "message": "redis pubsub unavailable; falling back to polling"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .is_err()
            {
                return;
            }
            None
        }
    };

    if let Some(pubsub_conn) = pubsub.as_mut() {
        let channel = conversation_pubsub_channel(tenant_id, conversation_id);
        if let Err(err) = pubsub_conn.subscribe(&channel).await {
            warn!(
                "failed to subscribe redis channel {} for websocket conversation {}: {}",
                channel, conversation_id, err
            );
            if socket
                .send(Message::Text(
                    json!({
                        "type": "stream.warning",
                        "message": "redis subscription unavailable; falling back to polling"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .is_err()
            {
                return;
            }
            pubsub = None;
        }
    }

    if let Some(mut pubsub) = pubsub {
        let mut messages = pubsub.on_message();
        let mut backfill = tokio::time::interval(Duration::from_secs(30));
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        backfill.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                message = messages.next() => {
                    if message.is_none() {
                        warn!(
                            "redis pubsub stream ended for websocket conversation {}; falling back to polling",
                            conversation_id
                        );
                        if socket
                            .send(Message::Text(
                                json!({
                                    "type": "stream.warning",
                                    "message": "redis subscription ended; falling back to polling"
                                })
                                .to_string()
                                .into(),
                            ))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        break;
                    }
                    if !send_new_socket_events(&mut socket, &pool, conversation_id, &mut cursor).await {
                        return;
                    }
                }
                _ = backfill.tick() => {
                    if !send_new_socket_events(&mut socket, &pool, conversation_id, &mut cursor).await {
                        return;
                    }
                }
                _ = heartbeat.tick() => {
                    if send_socket_heartbeat(&mut socket).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    let mut poll = tokio::time::interval(Duration::from_secs(1));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = poll.tick() => {
                if !send_new_socket_events(&mut socket, &pool, conversation_id, &mut cursor).await {
                    return;
                }
            }
            _ = heartbeat.tick() => {
                if send_socket_heartbeat(&mut socket).await.is_err() {
                    return;
                }
            }
        }
    }
}

pub async fn publish_single_event(state: &AppState, event: &StreamEventResponse) {
    match publish_event_to_redis(state, event).await {
        Ok(()) => {
            if let Err(err) =
                mark_pending_outbox_delivered_for_event(&state.connect_pool, event.id).await
            {
                warn!("failed to mark event {} delivered: {}", event.id, err);
            }
        }
        Err(err) => {
            warn!("failed to publish event {} to redis: {}", event.id, err);
        }
    }
}

pub async fn publish_pending_outbox(state: &AppState) -> Result<usize, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT o.id, o.payload
        FROM event_outbox o
        WHERE o.status = 'pending' AND o.next_attempt_at <= CURRENT_TIMESTAMP
        ORDER BY o.created_at ASC
        LIMIT 100
        "#,
    )
    .fetch_all(&state.connect_pool)
    .await?;

    let mut published = 0;
    for row in rows {
        let outbox_id: Uuid = row.try_get("id")?;
        let payload: Value = row.try_get("payload")?;
        match serde_json::from_value::<StreamEventResponse>(payload) {
            Ok(event) => {
                if publish_event_to_redis(state, &event).await.is_ok() {
                    mark_outbox_delivered(&state.connect_pool, outbox_id).await?;
                    published += 1;
                } else {
                    increment_outbox_attempt(
                        &state.connect_pool,
                        outbox_id,
                        Some("redis publish failed".to_string()),
                    )
                    .await?;
                }
            }
            Err(err) => {
                warn!("invalid outbox payload {}: {}", outbox_id, err);
                increment_outbox_attempt(&state.connect_pool, outbox_id, Some(err.to_string()))
                    .await?;
            }
        }
    }

    Ok(published)
}

pub fn spawn_outbox_publisher(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            if let Err(err) = publish_pending_outbox(&state).await {
                warn!("background outbox publish failed: {}", err);
            }
        }
    });
}

async fn next_event_seq(
    tx: &mut Transaction<'_, Postgres>,
    conversation_id: Uuid,
) -> Result<i64, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO conversation_event_sequences (conversation_id, next_seq)
        VALUES ($1, 2)
        ON CONFLICT (conversation_id)
        DO UPDATE SET next_seq = conversation_event_sequences.next_seq + 1
        RETURNING next_seq - 1 AS seq
        "#,
    )
    .bind(conversation_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok(row.try_get("seq")?)
}

async fn publish_event_to_redis(
    state: &AppState,
    event: &StreamEventResponse,
) -> Result<(), AppError> {
    let mut conn = state
        .redis_client
        .get_multiplexed_async_connection()
        .await?;
    let payload = serde_json::to_string(event)
        .map_err(|_| AppError::InvalidInput("failed to encode redis event".to_string()))?;

    let conversation_key = format!(
        "stream:tenant:{}:conversation:{}",
        event.tenant_id, event.conversation_id
    );
    let _: String = redis::cmd("XADD")
        .arg(conversation_key)
        .arg("*")
        .arg("event")
        .arg(&payload)
        .query_async(&mut conn)
        .await?;
    let conversation_channel = conversation_pubsub_channel(event.tenant_id, event.conversation_id);
    let _: i64 = redis::cmd("PUBLISH")
        .arg(conversation_channel)
        .arg(&payload)
        .query_async(&mut conn)
        .await?;

    if let Some(run_id) = event.run_id {
        let run_key = format!("stream:tenant:{}:run:{}", event.tenant_id, run_id);
        let _: String = redis::cmd("XADD")
            .arg(run_key)
            .arg("*")
            .arg("event")
            .arg(&payload)
            .query_async(&mut conn)
            .await?;
        let run_channel = run_pubsub_channel(event.tenant_id, run_id);
        let _: i64 = redis::cmd("PUBLISH")
            .arg(run_channel)
            .arg(payload)
            .query_async(&mut conn)
            .await?;
    }

    Ok(())
}

async fn mark_outbox_delivered(pool: &PgPool, outbox_id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE event_outbox
        SET status = 'delivered', published_at = CURRENT_TIMESTAMP, attempts = attempts + 1
        WHERE id = $1 AND status = 'pending'
        "#,
    )
    .bind(outbox_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_pending_outbox_delivered_for_event(
    pool: &PgPool,
    event_row_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE event_outbox
        SET status = 'delivered', published_at = CURRENT_TIMESTAMP, attempts = attempts + 1
        WHERE event_row_id = $1 AND status = 'pending'
        "#,
    )
    .bind(event_row_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn increment_outbox_attempt(
    pool: &PgPool,
    outbox_id: Uuid,
    last_error: Option<String>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE event_outbox
        SET attempts = attempts + 1,
            last_error = COALESCE($2, last_error),
            next_attempt_at = CURRENT_TIMESTAMP + (attempts + 1) * INTERVAL '10 seconds'
        WHERE id = $1
        "#,
    )
    .bind(outbox_id)
    .bind(last_error)
    .execute(pool)
    .await?;
    Ok(())
}

async fn fetch_events_after(
    pool: &PgPool,
    conversation_id: Uuid,
    cursor: i64,
) -> Result<(Vec<StreamEventResponse>, i64), AppError> {
    let events = fetch_events(pool, conversation_id, cursor).await?;
    let next_cursor = events.last().map(|event| event.seq).unwrap_or(cursor);
    Ok((events, next_cursor))
}

async fn send_new_socket_events(
    socket: &mut WebSocket,
    pool: &PgPool,
    conversation_id: Uuid,
    cursor: &mut i64,
) -> bool {
    match fetch_events_after(pool, conversation_id, *cursor).await {
        Ok((events, next_cursor)) => {
            *cursor = next_cursor;
            for event in events {
                let Ok(data) = serde_json::to_string(&event) else {
                    continue;
                };
                if socket.send(Message::Text(data.into())).await.is_err() {
                    return false;
                }
            }
            true
        }
        Err(err) => {
            warn!(
                "failed to fetch websocket conversation events {} after seq {}: {}",
                conversation_id, *cursor, err
            );
            socket
                .send(Message::Text(
                    json!({ "type": "stream.error", "message": "failed to fetch events" })
                        .to_string()
                        .into(),
                ))
                .await
                .is_ok()
        }
    }
}

async fn send_socket_heartbeat(socket: &mut WebSocket) -> Result<(), axum::Error> {
    socket
        .send(Message::Text(
            json!({ "type": "stream.heartbeat" }).to_string().into(),
        ))
        .await
}

fn conversation_pubsub_channel(tenant_id: Uuid, conversation_id: Uuid) -> String {
    format!("pubsub:tenant:{tenant_id}:conversation:{conversation_id}")
}

fn run_pubsub_channel(tenant_id: Uuid, run_id: Uuid) -> String {
    format!("pubsub:tenant:{tenant_id}:run:{run_id}")
}

fn event_from_row(row: PgRow) -> Result<StreamEventResponse, AppError> {
    Ok(StreamEventResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        conversation_id: row.try_get("conversation_id")?,
        run_id: row.try_get("run_id")?,
        seq: row.try_get("seq")?,
        event_id: row.try_get("event_id")?,
        event_type: row.try_get("type")?,
        payload: row.try_get("payload")?,
        trace_id: row.try_get("trace_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn event_to_sse(event: StreamEventResponse) -> Event {
    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
    Event::default()
        .id(event.seq.to_string())
        .event(event.event_type)
        .data(data)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use uuid::Uuid;

    use super::{conversation_pubsub_channel, resolve_after_seq, run_pubsub_channel};

    #[test]
    fn resolve_after_seq_prefers_explicit_query_value() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("10"));

        assert_eq!(resolve_after_seq(&headers, Some(42)), 42);
    }

    #[test]
    fn resolve_after_seq_uses_last_event_id_header() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("10"));

        assert_eq!(resolve_after_seq(&headers, None), 10);
    }

    #[test]
    fn resolve_after_seq_falls_back_to_zero_for_invalid_cursor() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", HeaderValue::from_static("invalid"));

        assert_eq!(resolve_after_seq(&headers, None), 0);
    }

    #[test]
    fn pubsub_channels_are_tenant_scoped() {
        let tenant_id = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let conversation_id = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let run_id = Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap();

        assert_eq!(
            conversation_pubsub_channel(tenant_id, conversation_id),
            "pubsub:tenant:00000000-0000-0000-0000-000000000001:conversation:00000000-0000-0000-0000-000000000002"
        );
        assert_eq!(
            run_pubsub_channel(tenant_id, run_id),
            "pubsub:tenant:00000000-0000-0000-0000-000000000001:run:00000000-0000-0000-0000-000000000003"
        );
    }
}
