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
use time::OffsetDateTime;
use tokio::time::{MissedTickBehavior, sleep};
use tracing::warn;
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::{
    ferriskey_oidc::PlatformRequestContext,
    models::{RunEventInput, RunEventKind, StreamEventResponse},
};

pub const STREAM_SESSION_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub async fn insert_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    conversation_id: Uuid,
    run_id: Option<Uuid>,
    event: RunEventInput,
) -> Result<StreamEventResponse, AppError> {
    let event_id = event.event_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let (event_type, payload) = normalize_event(event.event_type, event.payload)?;

    if let Some(existing) =
        find_existing_event_tx(tx, tenant_id, conversation_id, &event_id).await?
    {
        return Ok(existing);
    }

    let seq = next_event_seq(tx, conversation_id).await?;

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
    .bind(event_type)
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

fn normalize_event(
    event_type: String,
    payload: Option<Value>,
) -> Result<(String, Value), AppError> {
    let payload = payload.unwrap_or_else(|| json!({}));
    if let Some(kind) = RunEventKind::parse(&event_type) {
        kind.validate_payload(&payload)
            .map_err(AppError::InvalidInput)?;
        return Ok((event_type, payload));
    }

    Ok((
        "activity.raw".to_string(),
        json!({
            "original_type": event_type,
            "payload": payload
        }),
    ))
}

async fn find_existing_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    conversation_id: Uuid,
    event_id: &str,
) -> Result<Option<StreamEventResponse>, AppError> {
    let maybe_row = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, run_id, seq, event_id, type, payload, trace_id,
               created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND event_id = $3
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await?;

    maybe_row.map(event_from_row).transpose()
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

    update_scheduled_job_run_status_from_event(tx, Some(run_id), event_type).await?;

    Ok(())
}

pub async fn update_scheduled_job_run_status_from_event(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Option<Uuid>,
    event_type: &str,
) -> Result<(), AppError> {
    let Some(run_id) = run_id else {
        return Ok(());
    };
    let Some(status) = scheduled_job_status_from_event(event_type) else {
        return Ok(());
    };
    sqlx::query(
        r#"
        UPDATE scheduled_job_runs
        SET status = $1,
            completed_at = CASE
                WHEN $1 IN ('completed', 'failed', 'cancelled')
                THEN COALESCE(completed_at, CURRENT_TIMESTAMP)
                ELSE completed_at
            END
        WHERE run_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(status)
    .bind(run_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn scheduled_job_status_from_event(event_type: &str) -> Option<&'static str> {
    match event_type {
        "run.started" => Some("running"),
        "run.completed" => Some("completed"),
        "run.failed" => Some("failed"),
        "run.cancelled" => Some("cancelled"),
        _ => None,
    }
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
    ctx: PlatformRequestContext,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let output = stream! {
        let mut cursor = after_seq;
        let mut session_refresh = tokio::time::interval(STREAM_SESSION_REFRESH_INTERVAL);
        session_refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
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
                    _ = session_refresh.tick() => {
                        if let Some(event) = stream_session_validation_event(&pool, &ctx, conversation_id).await {
                            yield Ok(event);
                            return;
                        }
                    }
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

        let mut poll_immediately = true;
        loop {
            if poll_immediately {
                if let Some(event) = stream_session_validation_event(&pool, &ctx, conversation_id).await {
                    yield Ok(event);
                    return;
                }
            } else {
                tokio::select! {
                    _ = session_refresh.tick() => {
                        if let Some(event) = stream_session_validation_event(&pool, &ctx, conversation_id).await {
                            yield Ok(event);
                            return;
                        }
                    }
                    _ = sleep(Duration::from_secs(1)) => {}
                }
            }

            poll_immediately = match fetch_events_after(&pool, conversation_id, cursor).await {
                Ok((events, next_cursor)) => {
                    let fetched = events.len();
                    cursor = next_cursor;
                    for event in events {
                        yield Ok(event_to_sse(event));
                    }
                    fetched >= 1000
                }
                Err(err) => {
                    warn!(
                        "failed to poll live conversation events {} after seq {}: {}",
                        conversation_id, cursor, err
                    );
                    yield Ok(Event::default()
                        .event("stream.error")
                        .data(json!({ "message": "failed to fetch events" }).to_string()));
                    false
                }
            };
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
    ctx: PlatformRequestContext,
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
        let mut session_refresh = tokio::time::interval(STREAM_SESSION_REFRESH_INTERVAL);
        let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
        backfill.set_missed_tick_behavior(MissedTickBehavior::Delay);
        session_refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = session_refresh.tick() => {
                    if !refresh_socket_session_or_close(&mut socket, &pool, &ctx, conversation_id).await {
                        return;
                    }
                }
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
    let mut session_refresh = tokio::time::interval(STREAM_SESSION_REFRESH_INTERVAL);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    session_refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = poll.tick() => {
                if !send_new_socket_events(&mut socket, &pool, conversation_id, &mut cursor).await {
                    return;
                }
            }
            _ = session_refresh.tick() => {
                if !refresh_socket_session_or_close(&mut socket, &pool, &ctx, conversation_id).await {
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

pub async fn refresh_stream_session_state(
    pool: &PgPool,
    ctx: &PlatformRequestContext,
) -> Result<(), AppError> {
    let row = sqlx::query(
        r#"
        SELECT s.revoked_at AS session_revoked_at,
               d.revoked_at AS device_revoked_at,
               s.token_exp,
               s.idle_expires_at,
               s.client_kind
        FROM platform_sessions s
        JOIN devices d
          ON d.id = s.device_id
         AND d.tenant_id = s.tenant_id
         AND d.user_id = s.user_id
        WHERE s.id = $1
          AND s.tenant_id = $2
          AND s.user_id = $3
          AND d.id = $4
        "#,
    )
    .bind(ctx.session_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(ctx.device_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(AppError::Unauthorized(
            "platform session projection is missing".to_string(),
        ));
    };

    let session_revoked_at: Option<OffsetDateTime> = row.try_get("session_revoked_at")?;
    let device_revoked_at: Option<OffsetDateTime> = row.try_get("device_revoked_at")?;
    let token_exp: OffsetDateTime = row.try_get("token_exp")?;
    let idle_expires_at: OffsetDateTime = row.try_get("idle_expires_at")?;
    let client_kind: String = row.try_get("client_kind")?;
    classify_stream_session_state(
        session_revoked_at,
        device_revoked_at,
        token_exp,
        (client_kind == "desktop").then_some(idle_expires_at),
        OffsetDateTime::now_utc(),
    )?;

    sqlx::query(
        r#"
        UPDATE platform_sessions
        SET last_seen_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND user_id = $3
          AND revoked_at IS NULL
        "#,
    )
    .bind(ctx.session_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE devices
        SET last_seen_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND user_id = $3
          AND revoked_at IS NULL
        "#,
    )
    .bind(ctx.device_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(pool)
    .await?;

    Ok(())
}

fn classify_stream_session_state(
    session_revoked_at: Option<OffsetDateTime>,
    device_revoked_at: Option<OffsetDateTime>,
    token_exp: OffsetDateTime,
    idle_expires_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Result<(), AppError> {
    if session_revoked_at.is_some() {
        return Err(AppError::Unauthorized(
            "platform session has been revoked".to_string(),
        ));
    }
    if device_revoked_at.is_some() {
        return Err(AppError::Unauthorized(
            "platform device has been revoked".to_string(),
        ));
    }
    if token_exp <= now {
        return Err(AppError::Unauthorized(
            "FerrisKey access token has expired".to_string(),
        ));
    }
    if idle_expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(AppError::Unauthorized(
            "platform session idle timeout expired".to_string(),
        ));
    }
    Ok(())
}

async fn stream_session_validation_event(
    pool: &PgPool,
    ctx: &PlatformRequestContext,
    conversation_id: Uuid,
) -> Option<Event> {
    match refresh_stream_session_state(pool, ctx).await {
        Ok(()) => None,
        Err(AppError::Unauthorized(reason)) => Some(
            Event::default()
                .event("auth.revoked")
                .data(json!({ "reason": reason }).to_string()),
        ),
        Err(err) => {
            warn!(
                "failed to validate SSE session for conversation {}: {}",
                conversation_id, err
            );
            Some(
                Event::default()
                    .event("stream.error")
                    .data(json!({ "message": "failed to validate session" }).to_string()),
            )
        }
    }
}

async fn refresh_socket_session_or_close(
    socket: &mut WebSocket,
    pool: &PgPool,
    ctx: &PlatformRequestContext,
    conversation_id: Uuid,
) -> bool {
    match refresh_stream_session_state(pool, ctx).await {
        Ok(()) => true,
        Err(AppError::Unauthorized(reason)) => {
            let _ = socket
                .send(Message::Text(
                    json!({ "type": "auth.revoked", "reason": reason })
                        .to_string()
                        .into(),
                ))
                .await;
            let _ = socket.send(Message::Close(None)).await;
            false
        }
        Err(err) => {
            warn!(
                "failed to validate websocket session for conversation {}: {}",
                conversation_id, err
            );
            let _ = socket
                .send(Message::Text(
                    json!({ "type": "stream.error", "message": "failed to validate session" })
                        .to_string()
                        .into(),
                ))
                .await;
            false
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
    use time::{Duration as TimeDuration, OffsetDateTime};
    use uuid::Uuid;

    use crate::features::core::errors::AppError;

    use super::{
        STREAM_SESSION_REFRESH_INTERVAL, classify_stream_session_state,
        conversation_pubsub_channel, resolve_after_seq, run_pubsub_channel,
        scheduled_job_status_from_event,
    };

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

    #[test]
    fn stream_session_refresh_interval_is_fast_enough_for_revoke() {
        assert!(STREAM_SESSION_REFRESH_INTERVAL <= std::time::Duration::from_secs(5));
    }

    #[test]
    fn scheduled_job_status_tracks_run_terminal_events() {
        assert_eq!(
            scheduled_job_status_from_event("run.started"),
            Some("running")
        );
        assert_eq!(
            scheduled_job_status_from_event("run.completed"),
            Some("completed")
        );
        assert_eq!(
            scheduled_job_status_from_event("run.failed"),
            Some("failed")
        );
        assert_eq!(
            scheduled_job_status_from_event("run.cancelled"),
            Some("cancelled")
        );
        assert_eq!(scheduled_job_status_from_event("approval.requested"), None);
    }

    #[test]
    fn stream_session_state_allows_active_session() {
        let now = OffsetDateTime::UNIX_EPOCH;

        assert!(
            classify_stream_session_state(
                None,
                None,
                now + TimeDuration::seconds(60),
                Some(now + TimeDuration::seconds(60)),
                now,
            )
            .is_ok()
        );
    }

    #[test]
    fn stream_session_state_rejects_revoked_or_expired_session() {
        let now = OffsetDateTime::UNIX_EPOCH;

        let session_revoked = classify_stream_session_state(
            Some(now),
            None,
            now + TimeDuration::seconds(60),
            Some(now + TimeDuration::seconds(60)),
            now,
        )
        .expect_err("session revoke should fail");
        assert!(
            matches!(session_revoked, AppError::Unauthorized(reason) if reason.contains("session"))
        );

        let device_revoked = classify_stream_session_state(
            None,
            Some(now),
            now + TimeDuration::seconds(60),
            Some(now + TimeDuration::seconds(60)),
            now,
        )
        .expect_err("device revoke should fail");
        assert!(
            matches!(device_revoked, AppError::Unauthorized(reason) if reason.contains("device"))
        );

        let expired = classify_stream_session_state(
            None,
            None,
            now,
            Some(now + TimeDuration::seconds(60)),
            now,
        )
        .expect_err("expired token should fail");
        assert!(matches!(expired, AppError::Unauthorized(reason) if reason.contains("expired")));

        let idle_expired = classify_stream_session_state(
            None,
            None,
            now + TimeDuration::seconds(60),
            Some(now),
            now,
        )
        .expect_err("idle timeout should fail");
        assert!(matches!(idle_expired, AppError::Unauthorized(reason) if reason.contains("idle")));

        assert!(
            classify_stream_session_state(None, None, now + TimeDuration::seconds(60), None, now,)
                .is_ok()
        );
    }
}
