use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::HeaderMap,
    response::IntoResponse,
};
use serde_json::{Map, Value, json};
use sqlx::{Row, postgres::PgRow};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::{AtomicI64, AtomicU64, Ordering},
    time::{Duration, Instant},
};
use time::OffsetDateTime;
use tokio::time::{MissedTickBehavior, timeout};
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            event_store,
            ferriskey_oidc::{PlatformRequestContext, request_trace_id},
            models::StreamEventResponse,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

const WS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const WS_HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
const WS_SEND_DURATION_BOUNDS_MS: [u64; 8] = [1, 5, 10, 25, 50, 100, 250, 1_000];

static WS_CONNECTIONS_ACTIVE: AtomicI64 = AtomicI64::new(0);
static WS_CONNECTIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static WS_AUTH_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static WS_SUBSCRIPTIONS_ACTIVE: AtomicI64 = AtomicI64::new(0);
static WS_MESSAGES_SENT_TOTAL: AtomicU64 = AtomicU64::new(0);
static WS_SEND_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static WS_SEND_DURATION_BUCKETS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static WS_SEND_DURATION_MICROS: AtomicU64 = AtomicU64::new(0);

pub(super) struct BiWorkWsMetricsSnapshot {
    pub connections_active: i64,
    pub connections_total: u64,
    pub auth_failures_total: u64,
    pub subscriptions_active: i64,
    pub messages_sent_total: u64,
    pub send_failures_total: u64,
    pub send_duration_buckets: Vec<(f64, u64)>,
    pub send_duration_sum_seconds: f64,
}

pub(super) fn metrics_snapshot() -> BiWorkWsMetricsSnapshot {
    BiWorkWsMetricsSnapshot {
        connections_active: WS_CONNECTIONS_ACTIVE.load(Ordering::Relaxed),
        connections_total: WS_CONNECTIONS_TOTAL.load(Ordering::Relaxed),
        auth_failures_total: WS_AUTH_FAILURES_TOTAL.load(Ordering::Relaxed),
        subscriptions_active: WS_SUBSCRIPTIONS_ACTIVE.load(Ordering::Relaxed),
        messages_sent_total: WS_MESSAGES_SENT_TOTAL.load(Ordering::Relaxed),
        send_failures_total: WS_SEND_FAILURES_TOTAL.load(Ordering::Relaxed),
        send_duration_buckets: WS_SEND_DURATION_BOUNDS_MS
            .iter()
            .zip(WS_SEND_DURATION_BUCKETS.iter())
            .map(|(bound_ms, count)| {
                (
                    Duration::from_millis(*bound_ms).as_secs_f64(),
                    count.load(Ordering::Relaxed),
                )
            })
            .collect(),
        send_duration_sum_seconds: WS_SEND_DURATION_MICROS.load(Ordering::Relaxed) as f64
            / 1_000_000.0,
    }
}

struct WsConnectionMetricsGuard {
    subscriptions: i64,
}

impl WsConnectionMetricsGuard {
    fn authenticated() -> Self {
        WS_CONNECTIONS_TOTAL.fetch_add(1, Ordering::Relaxed);
        WS_CONNECTIONS_ACTIVE.fetch_add(1, Ordering::Relaxed);
        Self { subscriptions: 0 }
    }

    fn set_subscription_count(&mut self, count: usize) {
        let count = count as i64;
        let delta = count - self.subscriptions;
        if delta != 0 {
            WS_SUBSCRIPTIONS_ACTIVE.fetch_add(delta, Ordering::Relaxed);
            self.subscriptions = count;
        }
    }
}

impl Drop for WsConnectionMetricsGuard {
    fn drop(&mut self) {
        WS_CONNECTIONS_ACTIVE.fetch_sub(1, Ordering::Relaxed);
        if self.subscriptions != 0 {
            WS_SUBSCRIPTIONS_ACTIVE.fetch_sub(self.subscriptions, Ordering::Relaxed);
        }
    }
}

pub async fn biwork_global_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        handle_biwork_global_ws(socket, state, headers).await;
    })
}

async fn handle_biwork_global_ws(mut socket: WebSocket, state: AppState, headers: HeaderMap) {
    if send_ws_event(
        &mut socket,
        "connection.ready",
        json!({ "requires_auth": true }),
    )
    .await
    .is_err()
    {
        return;
    }

    let Some(ctx) = authenticate_ws(&mut socket, &state, &headers).await else {
        WS_AUTH_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    let mut connection_metrics = WsConnectionMetricsGuard::authenticated();

    if send_ws_event(
        &mut socket,
        "auth.ok",
        json!({
            "tenant_id": ctx.tenant_id,
            "user_id": ctx.platform_user_id,
            "session_id": ctx.session_id,
        }),
    )
    .await
    .is_err()
    {
        return;
    }

    let mut cursor = match current_global_event_offset(&state).await {
        Ok(offset) => GlobalEventCursor { offset },
        Err(err) => {
            warn!("failed to initialize BiWork websocket cursor: {err}");
            let _ = send_ws_event(
                &mut socket,
                "stream.error",
                json!({ "message": "failed to initialize stream cursor" }),
            )
            .await;
            return;
        }
    };

    let mut poll = tokio::time::interval(WS_POLL_INTERVAL);
    let mut session_refresh = tokio::time::interval(event_store::STREAM_SESSION_REFRESH_INTERVAL);
    let mut heartbeat = tokio::time::interval(WS_HEARTBEAT_INTERVAL);
    let mut projector = BiWorkEventProjector::default();
    let mut subscriptions = BiWorkWsSubscriptions::default();
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    session_refresh.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if handle_client_message(
                            &mut socket,
                            text.as_str(),
                            &mut subscriptions,
                            &mut connection_metrics,
                        ).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => return,
                }
            }
            _ = poll.tick() => {
                if send_new_global_events(&mut socket, &state, &ctx, &mut cursor, &mut projector, &subscriptions).await.is_err() {
                    return;
                }
            }
            _ = session_refresh.tick() => {
                match event_store::refresh_stream_session_state(&state.connect_pool, &ctx).await {
                    Ok(()) => {}
                    Err(AppError::Unauthorized(reason)) => {
                        let _ = send_ws_event(
                            &mut socket,
                            "auth.revoked",
                            json!({ "reason": reason }),
                        )
                        .await;
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                    Err(err) => {
                        warn!("failed to validate BiWork websocket session: {err}");
                        let _ = send_ws_event(
                            &mut socket,
                            "stream.error",
                            json!({ "message": "failed to validate session" }),
                        )
                        .await;
                        return;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if send_ws_event(&mut socket, "stream.heartbeat", json!({})).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn authenticate_ws(
    socket: &mut WebSocket,
    state: &AppState,
    headers: &HeaderMap,
) -> Option<PlatformRequestContext> {
    let message = timeout(std::time::Duration::from_secs(10), socket.recv())
        .await
        .ok()??;
    let Ok(Message::Text(text)) = message else {
        let _ = send_ws_event(
            socket,
            "auth.failed",
            json!({ "code": "AUTH_FRAME_REQUIRED" }),
        )
        .await;
        return None;
    };
    let payload: Value = match serde_json::from_str(text.as_str()) {
        Ok(payload) => payload,
        Err(_) => {
            let _ = send_ws_event(
                socket,
                "auth.failed",
                json!({ "code": "INVALID_AUTH_FRAME" }),
            )
            .await;
            return None;
        }
    };
    let token = payload
        .get("access_token")
        .or_else(|| payload.pointer("/payload/access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(token) = token else {
        let _ = send_ws_event(
            socket,
            "auth.failed",
            json!({ "code": "MISSING_ACCESS_TOKEN" }),
        )
        .await;
        return None;
    };

    match state
        .ferriskey_oidc
        .authenticate(
            &state.connect_pool,
            headers,
            token,
            request_trace_id(headers),
        )
        .await
    {
        Ok(ctx) => Some(ctx),
        Err(err) => {
            warn!("BiWork websocket authentication failed: {err}");
            let _ = send_ws_event(socket, "auth.failed", json!({ "code": "UNAUTHORIZED" })).await;
            None
        }
    }
}

async fn handle_client_message(
    socket: &mut WebSocket,
    text: &str,
    subscriptions: &mut BiWorkWsSubscriptions,
    connection_metrics: &mut WsConnectionMetricsGuard,
) -> Result<(), axum::Error> {
    let payload: Value = match serde_json::from_str(text) {
        Ok(payload) => payload,
        Err(_) => return Ok(()),
    };
    let op = payload
        .get("event")
        .or_else(|| payload.get("op"))
        .and_then(Value::as_str);
    match op {
        Some("ping") => {
            send_ws_event(socket, "pong", json!({})).await?;
        }
        Some("subscribe") => match parse_ws_subscription(&payload) {
            Ok(subscription) => {
                subscriptions.subscribe(subscription.clone());
                connection_metrics.set_subscription_count(subscriptions.entries.len());
                send_ws_event(
                    socket,
                    "subscription.ok",
                    subscription.to_payload("subscribe"),
                )
                .await?;
            }
            Err(code) => {
                send_ws_event(
                    socket,
                    "subscription.error",
                    json!({
                        "code": code,
                        "op": "subscribe",
                    }),
                )
                .await?;
            }
        },
        Some("unsubscribe") => match parse_ws_subscription(&payload) {
            Ok(subscription) => {
                subscriptions.unsubscribe(&subscription);
                connection_metrics.set_subscription_count(subscriptions.entries.len());
                send_ws_event(
                    socket,
                    "subscription.removed",
                    subscription.to_payload("unsubscribe"),
                )
                .await?;
            }
            Err(code) => {
                send_ws_event(
                    socket,
                    "subscription.error",
                    json!({
                        "code": code,
                        "op": "unsubscribe",
                    }),
                )
                .await?;
            }
        },
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BiWorkWsSubscription {
    scope: String,
    id: Option<String>,
}

impl BiWorkWsSubscription {
    fn key(&self) -> String {
        format!("{}:{}", self.scope, self.id.as_deref().unwrap_or("*"))
    }

    fn matches(&self, scope: &str, event: &StreamEventResponse, projected_payload: &Value) -> bool {
        if self.scope != scope {
            return false;
        }
        let Some(id) = self.id.as_deref() else {
            return true;
        };
        projected_event_identifiers(scope, event, projected_payload)
            .iter()
            .any(|candidate| candidate == id)
    }

    fn to_payload(&self, op: &str) -> Value {
        json!({
            "op": op,
            "scope": self.scope,
            "id": self.id,
        })
    }
}

#[derive(Default)]
struct BiWorkWsSubscriptions {
    entries: BTreeMap<String, BiWorkWsSubscription>,
}

impl BiWorkWsSubscriptions {
    fn subscribe(&mut self, subscription: BiWorkWsSubscription) {
        self.entries.insert(subscription.key(), subscription);
    }

    fn unsubscribe(&mut self, subscription: &BiWorkWsSubscription) {
        self.entries.remove(&subscription.key());
    }

    fn allows_projected_event(
        &self,
        event: &StreamEventResponse,
        projected_name: &str,
        projected_payload: &Value,
    ) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let Some(scope) = scope_for_projected_event(projected_name) else {
            return false;
        };
        self.entries
            .values()
            .any(|subscription| subscription.matches(scope, event, projected_payload))
    }
}

fn scope_for_projected_event(event_name: &str) -> Option<&'static str> {
    if event_name.starts_with("message.")
        || event_name.starts_with("turn.")
        || event_name.starts_with("conversation.")
        || event_name.starts_with("confirmation.")
        || event_name.starts_with("fileStream.")
        || event_name == "platform.runEvent"
    {
        return Some("conversation");
    }
    if event_name.starts_with("team.") {
        return Some("team");
    }
    if event_name.starts_with("cron.") {
        return Some("cron");
    }
    if event_name.starts_with("channel.") {
        return Some("channel");
    }
    if event_name.starts_with("extensions.") {
        return Some("extensions");
    }
    if event_name.starts_with("hub.") {
        return Some("hub");
    }
    None
}

fn projected_event_identifiers(
    scope: &str,
    event: &StreamEventResponse,
    projected_payload: &Value,
) -> Vec<String> {
    let mut identifiers = Vec::new();
    match scope {
        "conversation" => {
            push_identifier(&mut identifiers, Some(event.conversation_id.to_string()));
            for key in ["conversation_id", "conversationId", "session_id"] {
                push_identifier(&mut identifiers, payload_string(projected_payload, key));
            }
        }
        "team" => {
            for key in ["team_id", "teamId"] {
                push_identifier(&mut identifiers, payload_string(projected_payload, key));
            }
        }
        "cron" => {
            for key in ["cron_job_id", "job_id", "id"] {
                push_identifier(&mut identifiers, payload_string(projected_payload, key));
            }
        }
        "channel" => {
            for key in ["platform_type", "plugin_id", "id"] {
                push_identifier(&mut identifiers, payload_string(projected_payload, key));
            }
        }
        "extensions" | "hub" => {
            for key in ["extension_id", "package_id", "name", "id"] {
                push_identifier(&mut identifiers, payload_string(projected_payload, key));
            }
        }
        _ => {}
    }
    identifiers
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn push_identifier(identifiers: &mut Vec<String>, value: Option<String>) {
    let Some(value) = value else {
        return;
    };
    if !identifiers.iter().any(|existing| existing == &value) {
        identifiers.push(value);
    }
}

fn parse_ws_subscription(payload: &Value) -> Result<BiWorkWsSubscription, &'static str> {
    let scope = payload
        .get("scope")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("SCOPE_REQUIRED")?;
    if !matches!(
        scope,
        "conversation" | "team" | "cron" | "channel" | "extensions" | "hub"
    ) {
        return Err("UNSUPPORTED_SCOPE");
    }

    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Ok(BiWorkWsSubscription {
        scope: scope.to_string(),
        id,
    })
}

struct GlobalEventCursor {
    offset: i64,
}

async fn current_global_event_offset(state: &AppState) -> Result<i64, AppError> {
    sqlx::query_scalar("SELECT COALESCE(MAX(stream_offset), 0) FROM run_events")
        .fetch_one(&state.connect_pool)
        .await
        .map_err(Into::into)
}

async fn send_new_global_events(
    socket: &mut WebSocket,
    state: &AppState,
    ctx: &PlatformRequestContext,
    cursor: &mut GlobalEventCursor,
    projector: &mut BiWorkEventProjector,
    subscriptions: &BiWorkWsSubscriptions,
) -> Result<(), axum::Error> {
    let events = match fetch_global_events(state, ctx, cursor).await {
        Ok(events) => events,
        Err(err) => {
            warn!("failed to fetch BiWork websocket events: {err}");
            send_ws_event(
                socket,
                "stream.error",
                json!({ "message": "failed to fetch events" }),
            )
            .await?;
            return Ok(());
        }
    };

    for (stream_offset, event) in events {
        cursor.offset = stream_offset;
        for (name, payload) in projector.project(&event) {
            let allowed = subscriptions.allows_projected_event(&event, &name, &payload);
            if !allowed {
                continue;
            }
            send_ws_event(socket, &name, payload).await?;
        }
    }
    Ok(())
}

async fn fetch_global_events(
    state: &AppState,
    ctx: &PlatformRequestContext,
    cursor: &GlobalEventCursor,
) -> Result<Vec<(i64, StreamEventResponse)>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT e.stream_offset, e.id, e.tenant_id, e.conversation_id, e.run_id, e.seq, e.event_id,
               e.type, e.payload, e.trace_id, e.created_at
        FROM run_events e
        JOIN conversations c
          ON c.id = e.conversation_id
         AND c.tenant_id = e.tenant_id
        WHERE e.tenant_id = $1
          AND c.created_by_user_id = $2
          AND c.deleted_at IS NULL
          AND e.stream_offset > $3
        ORDER BY e.stream_offset ASC
        LIMIT 200
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(cursor.offset)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let stream_offset = row.try_get("stream_offset")?;
            Ok((stream_offset, event_from_row(row)?))
        })
        .collect()
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

fn project_biwork_event(event: &StreamEventResponse) -> Vec<(String, Value)> {
    let mut projected = vec![(
        "platform.runEvent".to_string(),
        json!({
            "id": event.id,
            "conversation_id": event.conversation_id,
            "run_id": event.run_id,
            "seq": event.seq,
            "type": event.event_type,
            "payload": event.payload,
            "trace_id": event.trace_id,
            "created_at": epoch_ms(event.created_at),
        }),
    )];

    match event.event_type.as_str() {
        "message.completed" => {
            projected.extend(project_message_completed(event));
        }
        "message.delta" => {
            projected.push((
                "message.stream".to_string(),
                message_stream_payload(event, false),
            ));
        }
        "tool.call.started" | "tool.call.delta" | "tool.call.completed" | "tool.call.failed" => {
            if let Some(payload) = tool_call_stream_payload(event) {
                projected.push(("message.stream".to_string(), payload));
            }
        }
        "run.completed" => {
            projected.push((
                "turn.completed".to_string(),
                turn_completed_payload(event, "finished"),
            ));
            projected.push((
                "conversation.listChanged".to_string(),
                json!({
                    "conversation_id": event.conversation_id,
                    "action": "updated",
                }),
            ));
        }
        "run.failed" => {
            projected.push((
                "message.stream".to_string(),
                run_failed_stream_payload(event),
            ));
            projected.push(("turn.completed".to_string(), turn_failed_payload(event)));
            projected.push((
                "conversation.listChanged".to_string(),
                json!({
                    "conversation_id": event.conversation_id,
                    "action": "updated",
                }),
            ));
        }
        "run.cancelled" => {
            projected.push(("turn.completed".to_string(), turn_cancelled_payload(event)));
            projected.push((
                "conversation.listChanged".to_string(),
                json!({
                    "conversation_id": event.conversation_id,
                    "action": "updated",
                }),
            ));
        }
        "approval.requested" => {
            projected.push(("confirmation.add".to_string(), confirmation_payload(event)));
        }
        "approval.updated" => {
            projected.push((
                "confirmation.update".to_string(),
                confirmation_payload(event),
            ));
        }
        "approval.decided" | "approval.completed" => {
            projected.push((
                "confirmation.remove".to_string(),
                confirmation_remove_payload(event),
            ));
        }
        "team.run.started" => {
            projected.push((
                "team.runStarted".to_string(),
                team_run_event_payload(event, "running"),
            ));
        }
        "team.run.updated" => {
            projected.push((
                "team.runUpdated".to_string(),
                team_run_updated_event_payload(event),
            ));
        }
        "team.run.completed" => {
            projected.push((
                "team.runCompleted".to_string(),
                team_run_event_payload(event, "completed"),
            ));
        }
        "team.run.cancelled" => {
            projected.push((
                "team.runCancelled".to_string(),
                team_run_event_payload(event, "cancelled"),
            ));
        }
        "team.run.failed" => {
            projected.push((
                "team.runFailed".to_string(),
                team_run_event_payload(event, "failed"),
            ));
        }
        "team.member.queued" => {
            projected.push((
                "team.childTurnStarted".to_string(),
                team_child_event_payload(event, "running"),
            ));
        }
        "team.member.updated" => {
            projected.push(team_child_updated_projected_event(event));
        }
        "team.member.completed" => {
            projected.push((
                "team.childTurnCompleted".to_string(),
                team_child_event_payload(event, "completed"),
            ));
        }
        "team.member.cancelled" => {
            projected.push((
                "team.childTurnCancelled".to_string(),
                team_child_event_payload(event, "cancelled"),
            ));
        }
        "team.member.failed" => {
            projected.push((
                "team.childTurnCompleted".to_string(),
                team_child_event_payload(event, "failed"),
            ));
        }
        _ => {}
    }

    projected
}

#[derive(Default)]
struct BiWorkEventProjector {
    artifact_drafts: HashMap<String, ArtifactDraftBuffer>,
}

#[derive(Default)]
struct ArtifactDraftBuffer {
    path: String,
    workspace: String,
    operation: String,
    chunks: BTreeMap<i64, String>,
}

impl BiWorkEventProjector {
    fn project(&mut self, event: &StreamEventResponse) -> Vec<(String, Value)> {
        let mut projected = project_biwork_event(event);
        let (event_type, payload) = effective_event_type_payload(event);

        if let Some(cron) = cron_projection(event_type, payload) {
            projected.push(cron);
        }

        if let Some(artifact) = cron_trigger_artifact_projection(event, event_type, payload) {
            projected.push(artifact);
        }

        if let Some(list_changed) =
            conversation_list_changed_projection(event.conversation_id, event_type, payload)
        {
            projected.push(list_changed);
        }

        if let Some(channel) = channel_projection(event_type, payload) {
            projected.push(channel);
        }

        if let Some(extension_or_hub) = extension_or_hub_projection(event_type, payload) {
            projected.push(extension_or_hub);
        }

        if let Some(file_stream) = self.artifact_draft_projection(event_type, payload) {
            projected.push(file_stream);
        } else if let Some(file_stream) = file_changed_projection(payload, event_type) {
            projected.push(file_stream);
        }

        projected
    }

    fn artifact_draft_projection(
        &mut self,
        event_type: &str,
        payload: &Value,
    ) -> Option<(String, Value)> {
        let draft_id = payload.get("draft_id").and_then(Value::as_str)?;
        match event_type {
            "artifact.draft.started" => {
                self.artifact_drafts.insert(
                    draft_id.to_string(),
                    ArtifactDraftBuffer {
                        path: payload_path(payload),
                        workspace: payload_workspace(payload),
                        operation: payload_operation(payload),
                        chunks: BTreeMap::new(),
                    },
                );
                None
            }
            "artifact.draft.delta" => {
                let buffer = self
                    .artifact_drafts
                    .entry(draft_id.to_string())
                    .or_insert_with(|| ArtifactDraftBuffer {
                        path: payload_path(payload),
                        workspace: payload_workspace(payload),
                        operation: payload_operation(payload),
                        chunks: BTreeMap::new(),
                    });
                let chunk_index = payload
                    .get("chunk_index")
                    .and_then(Value::as_i64)
                    .unwrap_or(buffer.chunks.len() as i64);
                let delta = payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                buffer.chunks.insert(chunk_index, delta);
                None
            }
            "artifact.draft.completed" => {
                let buffer =
                    self.artifact_drafts
                        .remove(draft_id)
                        .unwrap_or_else(|| ArtifactDraftBuffer {
                            path: payload_path(payload),
                            workspace: payload_workspace(payload),
                            operation: payload_operation(payload),
                            chunks: BTreeMap::new(),
                        });
                let content = buffer.chunks.values().cloned().collect::<String>();
                let path = if buffer.path.is_empty() {
                    payload_path(payload)
                } else {
                    buffer.path
                };
                let workspace = if buffer.workspace.is_empty() {
                    payload_workspace(payload)
                } else {
                    buffer.workspace
                };
                let operation = if buffer.operation.is_empty() {
                    payload_operation(payload)
                } else {
                    buffer.operation
                };
                Some(file_stream_content_update(
                    path, workspace, operation, content,
                ))
            }
            "artifact.draft.failed" => {
                self.artifact_drafts.remove(draft_id);
                None
            }
            _ => None,
        }
    }
}

fn effective_event_type_payload(event: &StreamEventResponse) -> (&str, &Value) {
    if event.event_type == "activity.raw"
        && let Some(original_type) = event.payload.get("original_type").and_then(Value::as_str)
    {
        let payload = event.payload.get("payload").unwrap_or(&event.payload);
        return (original_type, payload);
    }
    (event.event_type.as_str(), &event.payload)
}

fn cron_projection(event_type: &str, payload: &Value) -> Option<(String, Value)> {
    match event_type {
        "cron.job-created" | "cron.job.created" => Some((
            "cron.job-created".to_string(),
            payload
                .get("job")
                .cloned()
                .unwrap_or_else(|| payload.clone()),
        )),
        "cron.job-updated" | "cron.job.updated" => Some((
            "cron.job-updated".to_string(),
            payload
                .get("job")
                .cloned()
                .unwrap_or_else(|| payload.clone()),
        )),
        "cron.job-removed" | "cron.job.removed" => Some((
            "cron.job-removed".to_string(),
            json!({
                "job_id": payload.get("job_id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            }),
        )),
        "cron.job-executed" | "cron.job.executed" => Some((
            "cron.job-executed".to_string(),
            cron_job_executed_payload(payload),
        )),
        _ => None,
    }
}

fn cron_job_executed_payload(payload: &Value) -> Value {
    let job_id = payload
        .get("job_id")
        .or_else(|| payload.get("cron_job_id"))
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let cron_job_id = payload
        .get("cron_job_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(job_id.as_str())
        .to_string();

    let mut object = Map::new();
    object.insert("job_id".to_string(), Value::String(job_id));
    if !cron_job_id.is_empty() {
        object.insert("cron_job_id".to_string(), Value::String(cron_job_id));
    }
    object.insert(
        "status".to_string(),
        Value::String(
            payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("ok")
                .to_string(),
        ),
    );
    object.insert(
        "error".to_string(),
        payload
            .get("error")
            .and_then(Value::as_str)
            .map(|value| Value::String(value.to_string()))
            .unwrap_or(Value::Null),
    );

    for key in ["conversation_id", "run_id"] {
        if let Some(value) = payload
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            object.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    if let Some(value) = payload
        .get("cron_job_name")
        .or_else(|| payload.get("job_name"))
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        object.insert(
            "cron_job_name".to_string(),
            Value::String(value.to_string()),
        );
    }
    if let Some(value) = payload
        .get("triggered_at")
        .filter(|value| value.is_number())
    {
        object.insert("triggered_at".to_string(), value.clone());
    }

    Value::Object(object)
}

fn cron_trigger_artifact_projection(
    event: &StreamEventResponse,
    event_type: &str,
    payload: &Value,
) -> Option<(String, Value)> {
    if !matches!(event_type, "cron.job-executed" | "cron.job.executed") {
        return None;
    }
    if payload
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status != "ok")
    {
        return None;
    }
    let cron_job_id = payload
        .get("cron_job_id")
        .or_else(|| payload.get("job_id"))
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let cron_job_name = payload
        .get("cron_job_name")
        .or_else(|| payload.get("job_name"))
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Scheduled task");
    let artifact_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or(event.id);
    let created_at = payload
        .get("triggered_at")
        .and_then(Value::as_i64)
        .map(i128::from)
        .unwrap_or_else(|| epoch_ms(event.created_at));
    let conversation_id = payload
        .get("conversation_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| event.conversation_id.to_string());

    Some((
        "conversation.artifact".to_string(),
        json!({
            "id": artifact_id.to_string(),
            "conversation_id": conversation_id,
            "cron_job_id": cron_job_id,
            "kind": "cron_trigger",
            "status": "active",
            "payload": {
                "cron_job_id": cron_job_id,
                "cron_job_name": cron_job_name,
                "triggered_at": created_at,
            },
            "created_at": created_at,
            "updated_at": created_at,
        }),
    ))
}

fn conversation_list_changed_projection(
    fallback_conversation_id: Uuid,
    event_type: &str,
    payload: &Value,
) -> Option<(String, Value)> {
    if !matches!(
        event_type,
        "conversation.listChanged" | "conversation.list_changed"
    ) {
        return None;
    }
    let action = match payload.get("action").and_then(Value::as_str) {
        Some("created") => "created",
        Some("deleted") => "deleted",
        _ => "updated",
    };
    let conversation_id = payload
        .get("conversation_id")
        .or_else(|| payload.get("conversationId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fallback_conversation_id.to_string());
    let mut data = json!({
        "conversation_id": conversation_id,
        "action": action,
    });
    if let Some(source) = payload.get("source").and_then(Value::as_str)
        && let Some(object) = data.as_object_mut()
    {
        object.insert("source".to_string(), json!(source));
    }
    Some(("conversation.listChanged".to_string(), data))
}

fn channel_projection(event_type: &str, payload: &Value) -> Option<(String, Value)> {
    match event_type {
        "channel.plugin-status-changed" | "channel.plugin.status_changed" => {
            let plugin_id = payload
                .get("plugin_id")
                .or_else(|| payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let status = payload
                .get("status")
                .cloned()
                .unwrap_or_else(|| payload.clone());
            Some((
                "channel.plugin-status-changed".to_string(),
                json!({
                    "plugin_id": plugin_id,
                    "status": status,
                }),
            ))
        }
        "channel.user-authorized" | "channel.user.authorized" => {
            Some(("channel.user-authorized".to_string(), payload.clone()))
        }
        "channel.pairing-requested" | "channel.pairing.requested" => {
            Some(("channel.pairing-requested".to_string(), payload.clone()))
        }
        _ => None,
    }
}

fn extension_or_hub_projection(event_type: &str, payload: &Value) -> Option<(String, Value)> {
    match event_type {
        "extensions.state-changed" | "extensions.state_changed" => {
            Some(("extensions.state-changed".to_string(), payload.clone()))
        }
        "hub.state-changed" | "hub.state_changed" => {
            Some(("hub.state-changed".to_string(), payload.clone()))
        }
        _ => None,
    }
}

fn file_changed_projection(payload: &Value, event_type: &str) -> Option<(String, Value)> {
    if event_type != "file.changed" {
        return None;
    }
    let path = payload_path(payload);
    if path.is_empty() {
        return None;
    }
    let content = payload
        .get("content")
        .or_else(|| payload.get("inline_content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some(file_stream_content_update(
        path,
        payload_workspace(payload),
        payload_operation(payload),
        content,
    ))
}

fn file_stream_content_update(
    path: String,
    workspace: String,
    operation: String,
    content: String,
) -> (String, Value) {
    let operation = match operation.as_str() {
        "delete" | "delete_file" | "remove" | "removed" => "delete",
        _ => "write",
    };
    let relative_path = relative_file_path(&path);
    (
        "fileStream.contentUpdate".to_string(),
        json!({
            "file_path": path,
            "relative_path": relative_path,
            "workspace": workspace,
            "content": content,
            "operation": operation,
        }),
    )
}

fn payload_path(payload: &Value) -> String {
    payload
        .get("path")
        .or_else(|| payload.get("file_path"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn payload_workspace(payload: &Value) -> String {
    payload
        .get("workspace")
        .or_else(|| payload.get("project_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn payload_operation(payload: &Value) -> String {
    payload
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("write")
        .to_string()
}

fn relative_file_path(path: &str) -> String {
    path.strip_prefix("/workspace/")
        .or_else(|| path.strip_prefix("/local/main/"))
        .unwrap_or_else(|| path.trim_start_matches('/'))
        .to_string()
}

fn project_message_completed(event: &StreamEventResponse) -> Vec<(String, Value)> {
    let role = event
        .payload
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("assistant");
    if role == "user" {
        return vec![(
            "message.userCreated".to_string(),
            json!({
                "conversation_id": event.conversation_id,
                "msg_id": event.payload.get("message_id")
                    .and_then(Value::as_str)
                    .unwrap_or(event.event_id.as_str()),
                "content": message_content_text(&event.payload),
                "position": "right",
                "status": "finish",
                "hidden": false,
                "created_at": epoch_ms(event.created_at),
            }),
        )];
    }

    let mut projected = vec![
        (
            "message.stream".to_string(),
            message_stream_payload(event, true),
        ),
        (
            "turn.completed".to_string(),
            turn_completed_payload(event, "finished"),
        ),
    ];
    if let Some(artifact) = skill_suggest_artifact_projection(event) {
        projected.push(artifact);
    }
    projected
}

fn skill_suggest_artifact_projection(event: &StreamEventResponse) -> Option<(String, Value)> {
    let cron_job_id = event
        .payload
        .get("cron_job_id")
        .or_else(|| event.payload.pointer("/cron/job_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let content = message_content_text(&event.payload);
    let parsed = parse_skill_suggest_artifact(&content)?;
    let skill_content = parsed.skill_content.clone();
    Some((
        "conversation.artifact".to_string(),
        json!({
            "id": event.id.to_string(),
            "conversation_id": event.conversation_id.to_string(),
            "cron_job_id": cron_job_id,
            "kind": "skill_suggest",
            "status": "pending",
            "payload": {
                "cron_job_id": cron_job_id,
                "name": parsed.name,
                "description": parsed.description,
                "skill_content": parsed.skill_content,
                "skillContent": skill_content,
            },
            "created_at": epoch_ms(event.created_at),
            "updated_at": epoch_ms(event.created_at),
        }),
    ))
}

struct ParsedSkillSuggestArtifact {
    name: String,
    description: String,
    skill_content: String,
}

fn parse_skill_suggest_artifact(content: &str) -> Option<ParsedSkillSuggestArtifact> {
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

fn message_stream_payload(event: &StreamEventResponse, finished: bool) -> Value {
    let content = message_content_text(&event.payload);
    let message_id = event
        .payload
        .get("message_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| event.run_id.map(|run_id| format!("assistant.{run_id}")))
        .unwrap_or_else(|| event.event_id.clone());
    json!({
        "type": "text",
        "data": content,
        "msg_id": message_id,
        "turn_id": event.run_id.map(|id| id.to_string()),
        "conversation_id": event.conversation_id,
        "created_at": epoch_ms(event.created_at),
        "position": "left",
        "status": if finished { "finish" } else { "pending" },
        "replace": event.event_type == "message.completed",
    })
}

fn tool_call_stream_payload(event: &StreamEventResponse) -> Option<Value> {
    let data = biwork_tool_call_update_payload(
        event.conversation_id,
        event.event_type.as_str(),
        &event.payload,
    )?;
    let msg_id = data
        .pointer("/update/tool_call_id")
        .and_then(Value::as_str)
        .map(|id| format!("tool.{id}"))
        .unwrap_or_else(|| event.event_id.clone());
    Some(json!({
        "type": "acp_tool_call",
        "data": data,
        "msg_id": msg_id,
        "turn_id": event.run_id.map(|id| id.to_string()),
        "conversation_id": event.conversation_id,
        "created_at": epoch_ms(event.created_at),
        "position": "left",
        "status": if event.event_type == "tool.call.completed" || event.event_type == "tool.call.failed" {
            "finish"
        } else {
            "pending"
        },
    }))
}

fn biwork_tool_call_update_payload(
    conversation_id: Uuid,
    event_type: &str,
    payload: &Value,
) -> Option<Value> {
    let tool_call_id = payload
        .get("ui_tool_call_id")
        .or_else(|| payload.get("tool_call_id"))
        .and_then(Value::as_str)?;
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

    let content = biwork_tool_call_content(payload);
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
            "content": content,
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

fn turn_completed_payload(event: &StreamEventResponse, status: &str) -> Value {
    turn_completed_payload_with_state(
        event,
        status,
        if status == "finished" {
            "ai_waiting_input"
        } else {
            "unknown"
        },
        "",
    )
}

fn turn_cancelled_payload(event: &StreamEventResponse) -> Value {
    turn_completed_payload_with_state(event, "finished", "stopped", "cancelled")
}

fn turn_failed_payload(event: &StreamEventResponse) -> Value {
    turn_completed_payload_with_state(
        event,
        "finished",
        "error",
        run_failed_detail(event).as_str(),
    )
}

fn turn_completed_payload_with_state(
    event: &StreamEventResponse,
    status: &str,
    state: &str,
    detail: &str,
) -> Value {
    let turn_id = event
        .run_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| event.event_id.clone());
    json!({
        "conversation_id": event.conversation_id,
        "session_id": event.conversation_id.to_string(),
        "turn_id": turn_id,
        "status": status,
        "state": state,
        "detail": detail,
        "can_send_message": status == "finished",
        "runtime": {
            "state": if status == "finished" { "idle" } else { "running" },
            "can_send_message": status == "finished",
            "has_task": status != "finished",
            "task_status": if status == "finished" { "finished" } else { status },
            "is_processing": status != "finished",
            "pending_confirmations": 0,
            "turn_id": event.run_id.map(|id| id.to_string()),
        },
        "last_message": {
            "id": event.payload.get("message_id")
                .and_then(Value::as_str)
                .unwrap_or(event.event_id.as_str()),
            "type": "text",
            "content": message_content_text(&event.payload),
            "status": "finish",
            "created_at": epoch_ms(event.created_at),
        },
    })
}

fn confirmation_payload(event: &StreamEventResponse) -> Value {
    let approval_id = event
        .payload
        .get("approval_id")
        .or_else(|| event.payload.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(event.event_id.as_str());
    let tool_name = event
        .payload
        .get("tool_name")
        .or_else(|| event.payload.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or("tool execution");
    json!({
        "conversation_id": event.conversation_id,
        "id": approval_id,
        "approval_id": approval_id,
        "call_id": event.payload.get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or(approval_id),
        "title": format!("Approve {tool_name}"),
        "action": "exec",
        "description": event.payload.get("reason")
            .or_else(|| event.payload.get("summary"))
            .and_then(Value::as_str)
            .unwrap_or("Policy requires approval"),
        "created_at": epoch_ms(event.created_at),
        "options": [
            { "label": "Allow once", "value": "proceed_once" },
            { "label": "Allow always", "value": "proceed_always" },
            { "label": "Cancel", "value": "cancel" }
        ],
    })
}

fn run_failed_stream_payload(event: &StreamEventResponse) -> Value {
    let detail = run_failed_detail(event);
    json!({
        "type": "error",
        "data": {
            "message": detail,
            "code": event.payload.get("code").and_then(Value::as_str),
            "retryable": event.payload.get("retryable").and_then(Value::as_bool).unwrap_or(true),
            "rawError": {
                "message": detail,
                "code": event.payload.get("code").and_then(Value::as_str),
                "traceId": event.trace_id,
            },
        },
        "msg_id": event.payload.get("message_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("error.{}", event.run_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| event.event_id.clone()))),
        "turn_id": event.run_id.map(|id| id.to_string()),
        "conversation_id": event.conversation_id,
        "created_at": epoch_ms(event.created_at),
        "position": "left",
        "status": "error",
    })
}

fn run_failed_detail(event: &StreamEventResponse) -> String {
    event
        .payload
        .get("error")
        .or_else(|| event.payload.get("message"))
        .or_else(|| event.payload.get("reason"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Run failed")
        .to_string()
}

fn confirmation_remove_payload(event: &StreamEventResponse) -> Value {
    let approval_id = event
        .payload
        .get("approval_id")
        .or_else(|| event.payload.get("id"))
        .and_then(Value::as_str)
        .unwrap_or(event.event_id.as_str());
    json!({
        "conversation_id": event.conversation_id,
        "id": approval_id,
        "approval_id": approval_id,
        "call_id": event.payload.get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or(approval_id),
    })
}

fn team_run_event_payload(event: &StreamEventResponse, status: &str) -> Value {
    let team_run_id = event
        .payload
        .get("team_run_id")
        .and_then(Value::as_str)
        .unwrap_or(event.event_id.as_str());
    let team_id = event
        .payload
        .get("team_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({
        "team_id": team_id,
        "team_run_id": team_run_id,
        "target_slot_id": event.payload.get("target_slot_id").and_then(Value::as_str).unwrap_or(""),
        "target_role": event.payload.get("target_role").and_then(Value::as_str).unwrap_or("lead"),
        "status": status,
        "active_child_count": event.payload.get("active_child_count").and_then(Value::as_i64).unwrap_or(0),
        "pending_wake_count": event.payload.get("pending_wake_count").and_then(Value::as_i64).unwrap_or(0),
        "starting_child_count": 0,
        "slot_work": [],
    })
}

fn team_run_updated_event_payload(event: &StreamEventResponse) -> Value {
    let status = event
        .payload
        .get("status")
        .and_then(Value::as_str)
        .map(biwork_team_run_ws_status)
        .unwrap_or("running");
    team_run_event_payload(event, status)
}

fn biwork_team_run_ws_status(status: &str) -> &'static str {
    match status.trim().to_ascii_lowercase().as_str() {
        "accepted" => "accepted",
        "running" | "queued" | "pending" => "running",
        "cancelling" | "canceling" => "cancelling",
        "completed" => "completed",
        "cancelled" | "canceled" => "cancelled",
        "failed" => "failed",
        _ => "running",
    }
}

fn team_child_updated_projected_event(event: &StreamEventResponse) -> (String, Value) {
    let status = event
        .payload
        .get("status")
        .and_then(Value::as_str)
        .map(biwork_team_run_ws_status)
        .unwrap_or("running");
    let name = match status {
        "completed" | "failed" => "team.childTurnCompleted",
        "cancelled" => "team.childTurnCancelled",
        _ => "team.childTurnStarted",
    };
    (name.to_string(), team_child_event_payload(event, status))
}

fn team_child_event_payload(event: &StreamEventResponse, status: &str) -> Value {
    json!({
        "team_id": event.payload.get("team_id").and_then(Value::as_str).unwrap_or(""),
        "team_run_id": event.payload.get("team_run_id").and_then(Value::as_str).unwrap_or(""),
        "slot_id": event.payload.get("team_member_id").and_then(Value::as_str).unwrap_or(""),
        "role": event.payload.get("role").and_then(Value::as_str).unwrap_or("teammate"),
        "conversation_id": event.conversation_id,
        "turn_id": event.payload.get("run_id").and_then(Value::as_str).unwrap_or(event.event_id.as_str()),
        "status": status,
    })
}

fn message_content_text(payload: &Value) -> String {
    match payload.get("content") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.as_str()
                    .map(str::to_string)
                    .or_else(|| part.get("text").and_then(Value::as_str).map(str::to_string))
            })
            .collect::<Vec<_>>()
            .join(""),
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

async fn send_ws_event(
    socket: &mut WebSocket,
    event: &str,
    payload: Value,
) -> Result<(), axum::Error> {
    let started_at = Instant::now();
    let result = socket
        .send(Message::Text(
            json!({
                "event": event,
                "name": event,
                "payload": payload,
                "data": payload,
            })
            .to_string()
            .into(),
        ))
        .await;
    observe_ws_send(started_at.elapsed(), result.is_ok());
    result
}

fn observe_ws_send(duration: Duration, succeeded: bool) {
    let millis = duration.as_millis().min(u128::from(u64::MAX)) as u64;
    let micros = duration.as_micros().min(u128::from(u64::MAX)) as u64;
    WS_SEND_DURATION_MICROS.fetch_add(micros, Ordering::Relaxed);
    for (bound_ms, bucket) in WS_SEND_DURATION_BOUNDS_MS
        .iter()
        .zip(WS_SEND_DURATION_BUCKETS.iter())
    {
        if millis <= *bound_ms {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }
    if succeeded {
        WS_MESSAGES_SENT_TOTAL.fetch_add(1, Ordering::Relaxed);
    } else {
        WS_SEND_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
}

fn epoch_ms(value: OffsetDateTime) -> i128 {
    i128::from(value.unix_timestamp()) * 1000 + i128::from(value.millisecond())
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn projects_assistant_message_to_stream_and_turn_completed() {
        let event = stream_event(
            "message.completed",
            json!({
                "message_id": "assistant-msg",
                "role": "assistant",
                "content": "hello",
            }),
        );

        let projected = project_biwork_event(&event);

        assert!(projected.iter().any(|(name, payload)| {
            name == "message.stream"
                && payload.get("msg_id").and_then(Value::as_str) == Some("assistant-msg")
        }));
        assert!(projected.iter().any(|(name, _)| name == "turn.completed"));
    }

    #[test]
    fn projects_message_deltas_and_completion_to_one_stable_assistant_message() {
        let run_id = Uuid::new_v4();
        let mut delta = stream_event(
            "message.delta",
            json!({
                "run_id": run_id,
                "content": "hel",
            }),
        );
        delta.run_id = Some(run_id);
        let mut completed = stream_event(
            "message.completed",
            json!({
                "run_id": run_id,
                "role": "assistant",
                "content": "hello",
            }),
        );
        completed.run_id = Some(run_id);

        let delta_payload = project_biwork_event(&delta)
            .into_iter()
            .find_map(|(name, payload)| (name == "message.stream").then_some(payload))
            .expect("delta stream payload");
        let completed_payload = project_biwork_event(&completed)
            .into_iter()
            .find_map(|(name, payload)| (name == "message.stream").then_some(payload))
            .expect("completed stream payload");

        assert_eq!(delta_payload["msg_id"], format!("assistant.{run_id}"));
        assert_eq!(completed_payload["msg_id"], delta_payload["msg_id"]);
        assert_eq!(delta_payload["replace"], false);
        assert_eq!(completed_payload["replace"], true);
    }

    #[test]
    fn projects_skill_suggest_message_to_conversation_artifact() {
        let event = stream_event(
            "message.completed",
            json!({
                "message_id": "assistant-msg",
                "role": "assistant",
                "cron_job_id": "cron-1",
                "content": r#"
[SKILL_SUGGEST]
name: Daily Summary
description: Summarize daily work
content:
---
name: daily-summary
description: Summarize daily work
---

Use this skill for daily reports.
[/SKILL_SUGGEST]
"#,
            }),
        );

        let projected = project_biwork_event(&event);
        let artifact = projected
            .iter()
            .find(|(name, _)| name == "conversation.artifact")
            .expect("conversation artifact projection");

        assert_eq!(artifact.1["id"], event.id.to_string());
        assert_eq!(
            artifact.1["conversation_id"],
            event.conversation_id.to_string()
        );
        assert_eq!(artifact.1["cron_job_id"], "cron-1");
        assert_eq!(artifact.1["kind"], "skill_suggest");
        assert_eq!(artifact.1["status"], "pending");
        assert_eq!(artifact.1["payload"]["name"], "Daily Summary");
        assert_eq!(
            artifact.1["payload"]["skill_content"],
            artifact.1["payload"]["skillContent"]
        );
    }

    #[test]
    fn projects_user_message_to_user_created_only() {
        let event = stream_event(
            "message.completed",
            json!({
                "message_id": "user-msg",
                "role": "user",
                "content": "hi",
            }),
        );

        let projected = project_biwork_event(&event);

        assert!(projected.iter().any(|(name, payload)| {
            name == "message.userCreated"
                && payload.get("msg_id").and_then(Value::as_str) == Some("user-msg")
        }));
        assert!(!projected.iter().any(|(name, _)| name == "turn.completed"));
    }

    #[test]
    fn projects_approval_requested_to_confirmation_add() {
        let approval_id = Uuid::new_v4();
        let tool_call_id = Uuid::new_v4();
        let event = stream_event(
            "approval.requested",
            json!({
                "approval_id": approval_id,
                "tool_call_id": tool_call_id,
                "tool_name": "write_file",
                "reason": "Allow write_file to modify report.md?",
            }),
        );

        let projected = project_biwork_event(&event);
        let confirmation = projected
            .iter()
            .find(|(name, _)| name == "confirmation.add")
            .expect("confirmation add projection");

        assert_eq!(
            confirmation.1.get("id").and_then(Value::as_str),
            Some(approval_id.to_string().as_str())
        );
        assert_eq!(
            confirmation.1.get("call_id").and_then(Value::as_str),
            Some(tool_call_id.to_string().as_str())
        );
        assert_eq!(
            confirmation.1.get("description").and_then(Value::as_str),
            Some("Allow write_file to modify report.md?")
        );
    }

    #[test]
    fn projects_approval_updated_to_confirmation_update() {
        let approval_id = Uuid::new_v4();
        let tool_call_id = Uuid::new_v4();
        let event = stream_event(
            "approval.updated",
            json!({
                "approval_id": approval_id,
                "tool_call_id": tool_call_id,
                "tool_name": "local_exec",
                "summary": "Approval context changed",
            }),
        );

        let projected = project_biwork_event(&event);
        let confirmation = projected
            .iter()
            .find(|(name, _)| name == "confirmation.update")
            .expect("confirmation update projection");

        assert_eq!(
            confirmation.1.get("id").and_then(Value::as_str),
            Some(approval_id.to_string().as_str())
        );
        assert_eq!(
            confirmation.1.get("call_id").and_then(Value::as_str),
            Some(tool_call_id.to_string().as_str())
        );
        assert_eq!(
            confirmation.1.get("description").and_then(Value::as_str),
            Some("Approval context changed")
        );
        assert!(confirmation.1.get("options").is_some_and(Value::is_array));
    }

    #[test]
    fn projects_run_cancelled_to_stopped_turn_completed() {
        let run_id = Uuid::new_v4();
        let mut event = stream_event(
            "run.cancelled",
            json!({
                "run_id": run_id,
                "status": "cancelled",
                "reason": "conversation_cancelled",
            }),
        );
        event.run_id = Some(run_id);

        let projected = project_biwork_event(&event);
        let turn_completed = projected
            .iter()
            .find(|(name, _)| name == "turn.completed")
            .expect("turn completed projection");
        let runtime = &turn_completed.1["runtime"];

        assert_eq!(
            turn_completed.1.get("turn_id").and_then(Value::as_str),
            Some(run_id.to_string().as_str())
        );
        assert_eq!(
            turn_completed.1.get("status").and_then(Value::as_str),
            Some("finished")
        );
        assert_eq!(
            turn_completed.1.get("state").and_then(Value::as_str),
            Some("stopped")
        );
        assert_eq!(
            turn_completed.1.get("detail").and_then(Value::as_str),
            Some("cancelled")
        );
        assert_eq!(
            turn_completed
                .1
                .get("can_send_message")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(runtime.get("state").and_then(Value::as_str), Some("idle"));
        assert_eq!(
            runtime.get("task_status").and_then(Value::as_str),
            Some("finished")
        );
        assert_eq!(
            runtime.get("is_processing").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            runtime.get("turn_id").and_then(Value::as_str),
            Some(run_id.to_string().as_str())
        );

        assert!(projected.iter().any(|(name, payload)| {
            name == "conversation.listChanged"
                && payload.get("action").and_then(Value::as_str) == Some("updated")
        }));
    }

    #[test]
    fn projects_run_failed_to_error_stream_and_turn_completed() {
        let run_id = Uuid::new_v4();
        let mut event = stream_event(
            "run.failed",
            json!({
                "run_id": run_id,
                "error": "model provider timeout",
                "code": "MODEL_TIMEOUT",
                "retryable": true,
            }),
        );
        event.run_id = Some(run_id);

        let projected = project_biwork_event(&event);
        let error_stream = projected
            .iter()
            .find(|(name, payload)| {
                name == "message.stream"
                    && payload.get("type").and_then(Value::as_str) == Some("error")
            })
            .expect("error stream projection");
        let turn_completed = projected
            .iter()
            .find(|(name, _)| name == "turn.completed")
            .expect("turn completed projection");

        assert_eq!(
            error_stream
                .1
                .pointer("/data/message")
                .and_then(Value::as_str),
            Some("model provider timeout")
        );
        assert_eq!(
            error_stream.1.pointer("/data/code").and_then(Value::as_str),
            Some("MODEL_TIMEOUT")
        );
        assert_eq!(
            error_stream
                .1
                .pointer("/data/rawError/traceId")
                .and_then(Value::as_str),
            Some("trace")
        );
        assert_eq!(
            error_stream
                .1
                .pointer("/data/rawError/code")
                .and_then(Value::as_str),
            Some("MODEL_TIMEOUT")
        );
        assert_eq!(
            error_stream.1.get("status").and_then(Value::as_str),
            Some("error")
        );
        assert_eq!(
            turn_completed.1.get("state").and_then(Value::as_str),
            Some("error")
        );
        assert_eq!(
            turn_completed.1.get("detail").and_then(Value::as_str),
            Some("model provider timeout")
        );
        assert_eq!(
            turn_completed
                .1
                .pointer("/runtime/state")
                .and_then(Value::as_str),
            Some("idle")
        );
        assert!(projected.iter().any(|(name, payload)| {
            name == "conversation.listChanged"
                && payload.get("action").and_then(Value::as_str) == Some("updated")
        }));
    }

    #[test]
    fn projects_approval_decided_to_confirmation_remove() {
        let approval_id = Uuid::new_v4();
        let tool_call_id = Uuid::new_v4();
        let event = stream_event(
            "approval.decided",
            json!({
                "approval_id": approval_id,
                "tool_call_id": tool_call_id,
                "decision": "approved",
            }),
        );

        let projected = project_biwork_event(&event);
        let confirmation = projected
            .iter()
            .find(|(name, _)| name == "confirmation.remove")
            .expect("confirmation remove projection");

        assert_eq!(
            confirmation.1.get("id").and_then(Value::as_str),
            Some(approval_id.to_string().as_str())
        );
        assert_eq!(
            confirmation.1.get("approval_id").and_then(Value::as_str),
            Some(approval_id.to_string().as_str())
        );
        assert_eq!(
            confirmation.1.get("call_id").and_then(Value::as_str),
            Some(tool_call_id.to_string().as_str())
        );
    }

    #[test]
    fn projects_team_run_started_to_team_event() {
        let team_id = Uuid::new_v4();
        let team_run_id = Uuid::new_v4();
        let event = stream_event(
            "team.run.started",
            json!({
                "team_id": team_id,
                "team_run_id": team_run_id,
                "status": "running",
            }),
        );

        let projected = project_biwork_event(&event);

        assert!(projected.iter().any(|(name, payload)| {
            name == "team.runStarted"
                && payload.get("team_run_id").and_then(Value::as_str)
                    == Some(team_run_id.to_string().as_str())
        }));
    }

    #[test]
    fn projects_team_run_updated_preserves_biwork_status() {
        let team_id = Uuid::new_v4();
        let team_run_id = Uuid::new_v4();
        let event = stream_event(
            "team.run.updated",
            json!({
                "team_id": team_id,
                "team_run_id": team_run_id,
                "status": "cancelling",
            }),
        );

        let projected = project_biwork_event(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "team.runUpdated")
            .expect("team run updated projection");

        assert_eq!(
            payload.get("team_run_id").and_then(Value::as_str),
            Some(team_run_id.to_string().as_str())
        );
        assert_eq!(
            payload.get("status").and_then(Value::as_str),
            Some("cancelling")
        );
    }

    #[test]
    fn projects_team_member_updated_cancelling_to_child_turn_started() {
        let team_id = Uuid::new_v4();
        let team_run_id = Uuid::new_v4();
        let member_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let event = stream_event(
            "team.member.updated",
            json!({
                "team_id": team_id,
                "team_run_id": team_run_id,
                "team_member_id": member_id,
                "run_id": run_id,
                "role": "member",
                "status": "cancelling",
            }),
        );

        let projected = project_biwork_event(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "team.childTurnStarted")
            .expect("team child turn updated projection");

        assert_eq!(
            payload.get("team_id").and_then(Value::as_str),
            Some(team_id.to_string().as_str())
        );
        assert_eq!(
            payload.get("slot_id").and_then(Value::as_str),
            Some(member_id.to_string().as_str())
        );
        assert_eq!(
            payload.get("status").and_then(Value::as_str),
            Some("cancelling")
        );
    }

    #[test]
    fn projects_artifact_draft_to_file_stream_content_update() {
        let mut projector = BiWorkEventProjector::default();
        let project_id = Uuid::new_v4();
        let draft_id = "draft-1";

        let started = stream_event(
            "artifact.draft.started",
            json!({
                "draft_id": draft_id,
                "path": "/local/main/report.md",
                "project_id": project_id,
                "operation": "write_file",
            }),
        );
        let delta_0 = stream_event(
            "artifact.draft.delta",
            json!({
                "draft_id": draft_id,
                "path": "/local/main/report.md",
                "project_id": project_id,
                "chunk_index": 0,
                "delta": "# Title\n",
            }),
        );
        let delta_1 = stream_event(
            "artifact.draft.delta",
            json!({
                "draft_id": draft_id,
                "path": "/local/main/report.md",
                "project_id": project_id,
                "chunk_index": 1,
                "delta": "Body",
            }),
        );
        let completed = stream_event(
            "artifact.draft.completed",
            json!({
                "draft_id": draft_id,
                "path": "/local/main/report.md",
                "project_id": project_id,
                "operation": "write_file",
            }),
        );

        assert!(
            !projector
                .project(&started)
                .iter()
                .any(|(name, _)| name == "fileStream.contentUpdate")
        );
        assert!(
            !projector
                .project(&delta_0)
                .iter()
                .any(|(name, _)| name == "fileStream.contentUpdate")
        );
        assert!(
            !projector
                .project(&delta_1)
                .iter()
                .any(|(name, _)| name == "fileStream.contentUpdate")
        );

        let projected = projector.project(&completed);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "fileStream.contentUpdate")
            .expect("file stream content update");

        assert_eq!(payload["file_path"], "/local/main/report.md");
        assert_eq!(payload["relative_path"], "report.md");
        assert_eq!(payload["workspace"], project_id.to_string());
        assert_eq!(payload["operation"], "write");
        assert_eq!(payload["content"], "# Title\nBody");
    }

    #[test]
    fn projects_tool_completed_to_acp_tool_call_message_stream() {
        let mut projector = BiWorkEventProjector::default();
        let tool_call_id = Uuid::new_v4();
        let event = stream_event(
            "tool.call.completed",
            json!({
                "tool_call_id": tool_call_id.to_string(),
                "tool_name": "write_file",
                "status": "completed",
                "output_summary": "updated report",
                "browser": {
                    "kind": "browser",
                    "action": "snapshot",
                    "url": "https://example.com"
                },
                "views": [{
                    "kind": "file_diff",
                    "title": "Patch preview",
                    "files": [{
                        "file_name": "report.md",
                        "path": "/workspace/report.md",
                        "file_diff": "--- a/report.md\n+++ b/report.md\n@@\n-old\n+new\n"
                    }]
                }]
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, payload)| {
                name == "message.stream"
                    && payload.get("type").and_then(Value::as_str) == Some("acp_tool_call")
            })
            .expect("tool call message stream projection");

        assert_eq!(payload["msg_id"], format!("tool.{tool_call_id}"));
        assert_eq!(
            payload["data"]["update"]["tool_call_id"],
            tool_call_id.to_string()
        );
        assert_eq!(payload["data"]["update"]["status"], "completed");
        assert_eq!(payload["data"]["update"]["kind"], "edit");
        assert_eq!(
            payload["data"]["update"]["rawOutput"]["views"][0]["kind"],
            "file_diff"
        );
        assert_eq!(
            payload["data"]["update"]["rawOutput"]["views"][0]["title"],
            "Patch preview"
        );
        assert_eq!(
            payload["data"]["update"]["rawOutput"]["browser"]["url"],
            "https://example.com"
        );
        assert!(
            payload["data"]["update"]["content"][0]["content"]["text"]
                .as_str()
                .expect("content text")
                .contains("```diff")
        );
    }

    #[test]
    fn platform_tool_completion_reuses_the_stream_tool_call_identity() {
        let mut projector = BiWorkEventProjector::default();
        let authorization_tool_call_id = Uuid::new_v4();
        let stream_tool_call_id = "call-stream-write";
        let event = stream_event(
            "tool.call.completed",
            json!({
                "tool_call_id": authorization_tool_call_id.to_string(),
                "ui_tool_call_id": stream_tool_call_id,
                "tool_name": "write_file",
                "status": "completed",
                "output_summary": "updated report"
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, payload)| {
                name == "message.stream"
                    && payload.get("type").and_then(Value::as_str) == Some("acp_tool_call")
            })
            .expect("tool call message stream projection");

        assert_eq!(payload["msg_id"], format!("tool.{stream_tool_call_id}"));
        assert_eq!(
            payload["data"]["update"]["tool_call_id"],
            stream_tool_call_id
        );
    }

    #[test]
    fn projects_file_changed_to_file_stream_content_update() {
        let mut projector = BiWorkEventProjector::default();
        let event = stream_event(
            "file.changed",
            json!({
                "path": "/workspace/src/lib.rs",
                "workspace": "workspace-1",
                "operation": "write",
                "content": "fn main() {}",
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "fileStream.contentUpdate")
            .expect("file stream content update");

        assert_eq!(payload["file_path"], "/workspace/src/lib.rs");
        assert_eq!(payload["relative_path"], "src/lib.rs");
        assert_eq!(payload["workspace"], "workspace-1");
        assert_eq!(payload["content"], "fn main() {}");
        assert_eq!(payload["operation"], "write");
    }

    #[test]
    fn projects_activity_raw_cron_event_to_biwork_cron_event() {
        let mut projector = BiWorkEventProjector::default();
        let event = stream_event(
            "activity.raw",
            json!({
                "original_type": "cron.job-executed",
                "payload": {
                    "job_id": "job-1",
                    "status": "error",
                    "error": "dispatch failed",
                },
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "cron.job-executed")
            .expect("cron job executed projection");

        assert_eq!(payload["job_id"], "job-1");
        assert_eq!(payload["cron_job_id"], "job-1");
        assert_eq!(payload["status"], "error");
        assert_eq!(payload["error"], "dispatch failed");
    }

    #[test]
    fn projects_cron_executed_to_trigger_artifact() {
        let mut projector = BiWorkEventProjector::default();
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let mut event = stream_event(
            "cron.job-executed",
            json!({
                "job_id": "job-1",
                "cron_job_name": "Daily report",
                "status": "ok",
                "conversation_id": conversation_id.to_string(),
                "run_id": run_id.to_string(),
                "triggered_at": 12_000,
            }),
        );
        event.conversation_id = conversation_id;

        let projected = projector.project(&event);
        let (_, cron_event) = projected
            .iter()
            .find(|(name, _)| name == "cron.job-executed")
            .expect("cron job executed projection");
        assert_eq!(cron_event["job_id"], "job-1");
        assert_eq!(cron_event["cron_job_id"], "job-1");
        assert_eq!(cron_event["cron_job_name"], "Daily report");
        assert_eq!(cron_event["conversation_id"], conversation_id.to_string());
        assert_eq!(cron_event["run_id"], run_id.to_string());
        assert_eq!(cron_event["triggered_at"], 12_000);

        let (_, artifact) = projected
            .iter()
            .find(|(name, _)| name == "conversation.artifact")
            .expect("cron trigger artifact projection");

        assert_eq!(artifact["id"], run_id.to_string());
        assert_eq!(artifact["conversation_id"], conversation_id.to_string());
        assert_eq!(artifact["kind"], "cron_trigger");
        assert_eq!(artifact["status"], "active");
        assert_eq!(artifact["payload"]["cron_job_id"], "job-1");
        assert_eq!(artifact["payload"]["cron_job_name"], "Daily report");
        assert_eq!(artifact["payload"]["triggered_at"], 12_000);
    }

    #[test]
    fn projects_activity_raw_conversation_list_changed_to_biwork_event() {
        let mut projector = BiWorkEventProjector::default();
        let conversation_id = Uuid::new_v4();
        let mut event = stream_event(
            "activity.raw",
            json!({
                "original_type": "conversation.listChanged",
                "payload": {
                    "conversation_id": conversation_id.to_string(),
                    "action": "created",
                    "source": "conversation.create",
                },
            }),
        );
        event.conversation_id = conversation_id;

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "conversation.listChanged")
            .expect("conversation list changed projection");

        assert_eq!(payload["conversation_id"], conversation_id.to_string());
        assert_eq!(payload["action"], "created");
        assert_eq!(payload["source"], "conversation.create");
        assert!(payload.get("payload").is_none());
    }

    #[test]
    fn projects_activity_raw_channel_plugin_status_to_biwork_event() {
        let mut projector = BiWorkEventProjector::default();
        let event = stream_event(
            "activity.raw",
            json!({
                "original_type": "channel.plugin-status-changed",
                "payload": {
                    "plugin_id": "telegram",
                    "status": {
                        "id": "telegram",
                        "type": "telegram",
                        "name": "Telegram",
                        "enabled": true,
                        "connected": false,
                        "active_users": 0,
                    },
                },
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "channel.plugin-status-changed")
            .expect("channel plugin status projection");

        assert_eq!(payload["plugin_id"], "telegram");
        assert_eq!(payload["status"]["id"], "telegram");
        assert_eq!(payload["status"]["type"], "telegram");
        assert_eq!(payload["status"]["name"], "Telegram");
        assert_eq!(payload["status"]["enabled"], true);
        assert!(payload.get("payload").is_none());
    }

    #[test]
    fn projects_activity_raw_channel_pairing_requested_to_biwork_event() {
        let mut projector = BiWorkEventProjector::default();
        let event = stream_event(
            "activity.raw",
            json!({
                "original_type": "channel.pairing-requested",
                "payload": {
                    "code": "PAIR1234",
                    "platform_type": "telegram",
                    "platform_user_id": "platform-user-1",
                    "display_name": "Alice",
                    "requested_at": 42_000,
                    "expires_at": 642_000,
                },
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "channel.pairing-requested")
            .expect("channel pairing requested projection");

        assert_eq!(payload["code"], "PAIR1234");
        assert_eq!(payload["platform_type"], "telegram");
        assert_eq!(payload["platform_user_id"], "platform-user-1");
        assert_eq!(payload["display_name"], "Alice");
        assert_eq!(payload["requested_at"], 42_000);
        assert_eq!(payload["expires_at"], 642_000);
        assert!(payload.get("payload").is_none());
    }

    #[test]
    fn projects_activity_raw_channel_user_authorized_to_biwork_event() {
        let mut projector = BiWorkEventProjector::default();
        let event = stream_event(
            "activity.raw",
            json!({
                "original_type": "channel.user-authorized",
                "payload": {
                    "id": "user-1",
                    "platform_type": "telegram",
                    "platform_user_id": "platform-user-1",
                    "display_name": "Alice",
                    "authorized_at": 42_000,
                    "last_active": null,
                    "session_id": null,
                },
            }),
        );

        let projected = projector.project(&event);
        let (_, payload) = projected
            .iter()
            .find(|(name, _)| name == "channel.user-authorized")
            .expect("channel user authorized projection");

        assert_eq!(payload["id"], "user-1");
        assert_eq!(payload["platform_type"], "telegram");
        assert_eq!(payload["platform_user_id"], "platform-user-1");
        assert_eq!(payload["display_name"], "Alice");
        assert_eq!(payload["authorized_at"], 42_000);
        assert!(payload.get("payload").is_none());
    }

    #[test]
    fn projects_activity_raw_extension_and_hub_state_to_biwork_events() {
        let mut projector = BiWorkEventProjector::default();
        let extension = stream_event(
            "activity.raw",
            json!({
                "original_type": "extensions.state-changed",
                "payload": {
                    "name": "theme-pack",
                    "enabled": false,
                    "reason": "policy_sync",
                },
            }),
        );
        let hub = stream_event(
            "activity.raw",
            json!({
                "original_type": "hub.state-changed",
                "payload": {
                    "name": "theme-pack",
                    "status": "installed",
                    "error": null,
                },
            }),
        );

        let projected_extension = projector.project(&extension);
        let (_, extension_payload) = projected_extension
            .iter()
            .find(|(name, _)| name == "extensions.state-changed")
            .expect("extension state projection");
        assert_eq!(extension_payload["name"], "theme-pack");
        assert_eq!(extension_payload["enabled"], false);
        assert_eq!(extension_payload["reason"], "policy_sync");
        assert!(extension_payload.get("payload").is_none());

        let projected_hub = projector.project(&hub);
        let (_, hub_payload) = projected_hub
            .iter()
            .find(|(name, _)| name == "hub.state-changed")
            .expect("hub state projection");
        assert_eq!(hub_payload["name"], "theme-pack");
        assert_eq!(hub_payload["status"], "installed");
        assert!(hub_payload["error"].is_null());
        assert!(hub_payload.get("payload").is_none());
    }

    #[test]
    fn parses_supported_ws_subscriptions() {
        let subscription = parse_ws_subscription(&json!({
            "op": "subscribe",
            "scope": "conversation",
            "id": "conversation-1",
        }))
        .expect("valid subscription");

        assert_eq!(
            subscription,
            BiWorkWsSubscription {
                scope: "conversation".to_string(),
                id: Some("conversation-1".to_string()),
            }
        );
        assert_eq!(
            subscription.to_payload("subscribe")["scope"],
            "conversation"
        );

        let broad_subscription = parse_ws_subscription(&json!({
            "op": "subscribe",
            "scope": "cron",
        }))
        .expect("valid broad subscription");
        assert_eq!(broad_subscription.id, None);
    }

    #[test]
    fn rejects_unsupported_ws_subscriptions() {
        assert_eq!(
            parse_ws_subscription(&json!({ "op": "subscribe" })).unwrap_err(),
            "SCOPE_REQUIRED"
        );
        assert_eq!(
            parse_ws_subscription(&json!({ "op": "subscribe", "scope": "billing" })).unwrap_err(),
            "UNSUPPORTED_SCOPE"
        );
    }

    #[test]
    fn ws_subscriptions_filter_conversation_events_by_scope_and_id() {
        let conversation_id = Uuid::new_v4();
        let other_conversation_id = Uuid::new_v4();
        let mut event = stream_event(
            "message.delta",
            json!({
                "message_id": "msg-1",
                "content": "hello",
            }),
        );
        event.conversation_id = conversation_id;
        let projected = project_biwork_event(&event);
        let (name, payload) = projected
            .iter()
            .find(|(name, _)| name == "message.stream")
            .expect("message stream projection");

        let mut subscriptions = BiWorkWsSubscriptions::default();
        assert!(!subscriptions.allows_projected_event(&event, name, payload));

        subscriptions.subscribe(BiWorkWsSubscription {
            scope: "conversation".to_string(),
            id: Some(other_conversation_id.to_string()),
        });
        assert!(!subscriptions.allows_projected_event(&event, name, payload));

        subscriptions.subscribe(BiWorkWsSubscription {
            scope: "conversation".to_string(),
            id: Some(conversation_id.to_string()),
        });
        assert!(subscriptions.allows_projected_event(&event, name, payload));
    }

    #[test]
    fn ws_subscriptions_allow_broad_and_specific_non_conversation_events() {
        let mut projector = BiWorkEventProjector::default();
        let event = stream_event(
            "cron.job-executed",
            json!({
                "job_id": "job-1",
                "status": "ok",
            }),
        );
        let projected = projector.project(&event);
        let (name, payload) = projected
            .iter()
            .find(|(name, _)| name == "cron.job-executed")
            .expect("cron projection");

        let mut specific = BiWorkWsSubscriptions::default();
        specific.subscribe(BiWorkWsSubscription {
            scope: "cron".to_string(),
            id: Some("job-2".to_string()),
        });
        assert!(!specific.allows_projected_event(&event, name, payload));

        specific.subscribe(BiWorkWsSubscription {
            scope: "cron".to_string(),
            id: Some("job-1".to_string()),
        });
        assert!(specific.allows_projected_event(&event, name, payload));

        let mut broad = BiWorkWsSubscriptions::default();
        broad.subscribe(BiWorkWsSubscription {
            scope: "cron".to_string(),
            id: None,
        });
        assert!(broad.allows_projected_event(&event, name, payload));
    }

    #[test]
    fn ws_unsubscribe_removes_subscription_match() {
        let event = stream_event(
            "run.completed",
            json!({
                "content": "done",
            }),
        );
        let projected = project_biwork_event(&event);
        let (name, payload) = projected
            .iter()
            .find(|(name, _)| name == "turn.completed")
            .expect("turn completed projection");
        let subscription = BiWorkWsSubscription {
            scope: "conversation".to_string(),
            id: None,
        };

        let mut subscriptions = BiWorkWsSubscriptions::default();
        subscriptions.subscribe(subscription.clone());
        assert!(subscriptions.allows_projected_event(&event, name, payload));

        subscriptions.unsubscribe(&subscription);
        assert!(!subscriptions.allows_projected_event(&event, name, payload));
    }

    #[test]
    fn ws_session_refresh_interval_is_fast_enough_for_revoke() {
        assert!(event_store::STREAM_SESSION_REFRESH_INTERVAL <= std::time::Duration::from_secs(5));
        assert!(WS_HEARTBEAT_INTERVAL >= event_store::STREAM_SESSION_REFRESH_INTERVAL);
    }

    fn stream_event(event_type: &str, payload: Value) -> StreamEventResponse {
        StreamEventResponse {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            run_id: Some(Uuid::new_v4()),
            seq: 1,
            event_id: format!("{event_type}.test"),
            event_type: event_type.to_string(),
            payload,
            trace_id: Some("trace".to_string()),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }
}
