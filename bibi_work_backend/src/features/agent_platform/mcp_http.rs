use std::{
    collections::HashMap,
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::{
    Client, RequestBuilder, StatusCode, Url,
    header::{self, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::features::{agent_platform::secret_resolver::SecretResolver, core::errors::AppError};

const DEFAULT_MCP_HTTP_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";
const MAX_MCP_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MCP_REQUEST_HEADERS: usize = 64;
const MAX_SESSION_SLOTS: usize = 256;
const SESSION_TTL_SECONDS: u64 = 15 * 60;

type SessionSlots = Mutex<HashMap<String, Arc<SessionSlot>>>;

static HTTP_CLIENT: OnceLock<Client> = OnceLock::new();
static SESSION_SLOTS: OnceLock<SessionSlots> = OnceLock::new();
static MCP_REQUESTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static MCP_REQUEST_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static MCP_SESSION_REUSES_TOTAL: AtomicU64 = AtomicU64::new(0);
static MCP_SESSION_INITIALIZATIONS_TOTAL: AtomicU64 = AtomicU64::new(0);
static MCP_SESSION_RETRIES_TOTAL: AtomicU64 = AtomicU64::new(0);
static MCP_SESSION_SLOTS: AtomicU64 = AtomicU64::new(0);
static MCP_REQUEST_DURATION_BUCKETS: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];
static MCP_REQUEST_DURATION_MICROS: AtomicU64 = AtomicU64::new(0);

const MCP_REQUEST_DURATION_BOUNDS_MS: [u64; 10] =
    [10, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000];

pub struct McpHttpMetricsSnapshot {
    pub requests_total: u64,
    pub request_failures_total: u64,
    pub session_reuses_total: u64,
    pub session_initializations_total: u64,
    pub session_retries_total: u64,
    pub session_slots: u64,
    pub request_duration_buckets: Vec<(f64, u64)>,
    pub request_duration_sum_seconds: f64,
}

pub fn metrics_snapshot() -> McpHttpMetricsSnapshot {
    McpHttpMetricsSnapshot {
        requests_total: MCP_REQUESTS_TOTAL.load(Ordering::Relaxed),
        request_failures_total: MCP_REQUEST_FAILURES_TOTAL.load(Ordering::Relaxed),
        session_reuses_total: MCP_SESSION_REUSES_TOTAL.load(Ordering::Relaxed),
        session_initializations_total: MCP_SESSION_INITIALIZATIONS_TOTAL.load(Ordering::Relaxed),
        session_retries_total: MCP_SESSION_RETRIES_TOTAL.load(Ordering::Relaxed),
        session_slots: MCP_SESSION_SLOTS.load(Ordering::Relaxed),
        request_duration_buckets: MCP_REQUEST_DURATION_BOUNDS_MS
            .iter()
            .zip(MCP_REQUEST_DURATION_BUCKETS.iter())
            .map(|(bound_ms, count)| {
                (
                    Duration::from_millis(*bound_ms).as_secs_f64(),
                    count.load(Ordering::Relaxed),
                )
            })
            .collect(),
        request_duration_sum_seconds: MCP_REQUEST_DURATION_MICROS.load(Ordering::Relaxed) as f64
            / 1_000_000.0,
    }
}

struct SessionSlot {
    state: Mutex<Option<StreamableSession>>,
    last_used_at: AtomicU64,
}

#[derive(Clone)]
struct StreamableSession {
    id: Option<String>,
    protocol_version: String,
    initialized_at: u64,
}

struct RawMcpResponse {
    status: StatusCode,
    content_type: Option<String>,
    session_id: Option<String>,
    bytes: Vec<u8>,
}

pub async fn request(
    secret_resolver: &SecretResolver,
    transport: &str,
    config: &Value,
    secret_ref: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    MCP_REQUESTS_TOTAL.fetch_add(1, Ordering::Relaxed);
    let started_at = Instant::now();
    let result = match transport {
        "streamable-http" | "streamable_http" => {
            request_streamable(secret_resolver, config, secret_ref, method, params).await
        }
        "http" | "json-rpc" => {
            request_stateless(secret_resolver, config, secret_ref, method, params).await
        }
        "sse" => request_legacy_sse(secret_resolver, config, secret_ref, method, params).await,
        other => Err(AppError::InvalidInput(format!(
            "unsupported MCP HTTP transport: {other}"
        ))),
    };
    observe_request_duration(started_at.elapsed());
    if result.is_err() {
        MCP_REQUEST_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }
    result
}

async fn request_legacy_sse(
    secret_resolver: &SecretResolver,
    config: &Value,
    secret_ref: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    let sse_endpoint = mcp_endpoint(config)?;
    let timeout = request_timeout(config);
    let response = tokio::time::timeout(
        timeout,
        authenticated_request(
            secret_resolver,
            http_client().get(&sse_endpoint),
            config,
            secret_ref,
        )
        .await?
        .header(header::ACCEPT, "text/event-stream")
        .send(),
    )
    .await
    .map_err(|_| AppError::InvalidInput("MCP SSE connection timed out".to_string()))?
    .map_err(|err| AppError::InvalidInput(format!("MCP SSE connection failed: {err}")))?;
    if !response.status().is_success() {
        return Err(http_status_error("SSE connect", response.status()));
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.starts_with("text/event-stream") {
        return Err(AppError::InvalidInput(
            "MCP SSE endpoint did not return text/event-stream".to_string(),
        ));
    }
    let negotiated_base = response.url().clone();
    let mut stream = Box::pin(response.bytes_stream());
    let mut buffer = Vec::new();
    let message_endpoint = loop {
        let event = next_sse_event(&mut stream, &mut buffer, timeout).await?;
        if event.event.as_deref() == Some("endpoint") {
            break resolve_legacy_message_endpoint(&negotiated_base, &event.data)?;
        }
    };

    let protocol_version = json_string(config, "protocol_version")
        .unwrap_or_else(|| DEFAULT_PROTOCOL_VERSION.to_string());
    let initialize_id = Uuid::new_v4().to_string();
    post_legacy_message(
        secret_resolver,
        &message_endpoint,
        config,
        secret_ref,
        &json_rpc_request(
            &initialize_id,
            "initialize",
            json!({
                "protocolVersion": protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "bibi-work-rust-mcp", "version": env!("CARGO_PKG_VERSION")}
            }),
        ),
        "initialize",
    )
    .await?;
    let initialized =
        next_legacy_rpc_message(&mut stream, &mut buffer, timeout, &initialize_id).await?;
    if initialized.get("error").is_some() {
        return Err(AppError::InvalidInput(
            "MCP SSE initialize returned an RPC error".to_string(),
        ));
    }

    post_legacy_message(
        secret_resolver,
        &message_endpoint,
        config,
        secret_ref,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        "notifications/initialized",
    )
    .await?;

    let request_id = Uuid::new_v4().to_string();
    post_legacy_message(
        secret_resolver,
        &message_endpoint,
        config,
        secret_ref,
        &json_rpc_request(&request_id, method, params),
        method,
    )
    .await?;
    next_legacy_rpc_message(&mut stream, &mut buffer, timeout, &request_id).await
}

struct SseEvent {
    event: Option<String>,
    data: String,
}

async fn next_sse_event<S>(
    stream: &mut Pin<Box<S>>,
    buffer: &mut Vec<u8>,
    timeout: Duration,
) -> Result<SseEvent, AppError>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>>,
{
    loop {
        if let Some((event, consumed)) = parse_next_sse_event(buffer)? {
            buffer.drain(..consumed);
            return Ok(event);
        }
        let chunk = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| AppError::InvalidInput("MCP SSE event timed out".to_string()))?
            .ok_or_else(|| AppError::InvalidInput("MCP SSE stream closed".to_string()))?
            .map_err(|err| AppError::InvalidInput(format!("MCP SSE stream read failed: {err}")))?;
        if buffer.len().saturating_add(chunk.len()) > MAX_MCP_RESPONSE_BYTES as usize {
            return Err(AppError::InvalidInput(format!(
                "MCP SSE event exceeds the {} byte limit",
                MAX_MCP_RESPONSE_BYTES
            )));
        }
        buffer.extend_from_slice(&chunk);
    }
}

fn parse_next_sse_event(buffer: &[u8]) -> Result<Option<(SseEvent, usize)>, AppError> {
    let Some((event_end, separator_len)) = find_sse_event_end(buffer) else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&buffer[..event_end])
        .map_err(|_| AppError::InvalidInput("MCP SSE event is not valid UTF-8".to_string()))?;
    let normalized = text.replace("\r\n", "\n");
    let mut event = None;
    let mut data = Vec::new();
    for line in normalized.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim_start().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.trim_start());
        }
    }
    Ok(Some((
        SseEvent {
            event,
            data: data.join("\n"),
        },
        event_end + separator_len,
    )))
}

fn find_sse_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2))
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| (index, 4))
        })
}

fn resolve_legacy_message_endpoint(base: &Url, endpoint: &str) -> Result<Url, AppError> {
    let endpoint = base.join(endpoint.trim()).map_err(|_| {
        AppError::InvalidInput("MCP SSE endpoint event contained an invalid URI".to_string())
    })?;
    let same_origin = base.scheme() == endpoint.scheme()
        && base.host_str() == endpoint.host_str()
        && base.port_or_known_default() == endpoint.port_or_known_default();
    if !same_origin
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(AppError::InvalidInput(
            "MCP SSE message endpoint must use the negotiated SSE origin".to_string(),
        ));
    }
    Ok(endpoint)
}

async fn post_legacy_message(
    secret_resolver: &SecretResolver,
    endpoint: &Url,
    config: &Value,
    secret_ref: Option<&str>,
    body: &Value,
    operation: &str,
) -> Result<(), AppError> {
    let response = send(
        authenticated_request(
            secret_resolver,
            http_client().post(endpoint.clone()),
            config,
            secret_ref,
        )
        .await?
        .header(header::ACCEPT, "application/json")
        .timeout(request_timeout(config))
        .json(body),
        operation,
    )
    .await?;
    if response.status.is_success() {
        Ok(())
    } else {
        Err(http_status_error(operation, response.status))
    }
}

async fn next_legacy_rpc_message<S>(
    stream: &mut Pin<Box<S>>,
    buffer: &mut Vec<u8>,
    timeout: Duration,
    expected_id: &str,
) -> Result<Value, AppError>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>>,
{
    loop {
        let event = next_sse_event(stream, buffer, timeout).await?;
        if event
            .event
            .as_deref()
            .is_some_and(|event| event != "message")
            || event.data.is_empty()
        {
            continue;
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|err| AppError::InvalidInput(format!("MCP SSE JSON parse failed: {err}")))?;
        if value.get("id").and_then(Value::as_str) == Some(expected_id) {
            return Ok(value);
        }
    }
}

async fn request_stateless(
    secret_resolver: &SecretResolver,
    config: &Value,
    secret_ref: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    let endpoint = mcp_endpoint(config)?;
    let timeout = request_timeout(config);
    let request_id = Uuid::new_v4().to_string();
    let request = authenticated_request(
        secret_resolver,
        http_client().post(endpoint),
        config,
        secret_ref,
    )
    .await?
    .header(header::ACCEPT, "application/json, text/event-stream")
    .timeout(timeout)
    .json(&json_rpc_request(&request_id, method, params));
    let response = send(request, method).await?;
    response.rpc_value(method, Some(&request_id))
}

async fn request_streamable(
    secret_resolver: &SecretResolver,
    config: &Value,
    secret_ref: Option<&str>,
    method: &str,
    params: Value,
) -> Result<Value, AppError> {
    let endpoint = mcp_endpoint(config)?;
    let key = session_key(&endpoint, config, secret_ref)?;
    let slot = session_slot(key).await;

    for attempt in 0..2 {
        let session = ensure_session(&slot, secret_resolver, &endpoint, config, secret_ref).await?;
        let request_id = Uuid::new_v4().to_string();
        let response = send_session_request(
            secret_resolver,
            &endpoint,
            config,
            secret_ref,
            &session,
            json_rpc_request(&request_id, method, params.clone()),
            method,
        )
        .await?;
        if attempt == 0
            && response.indicates_invalid_session()
            && let Some(session_id) = session.id.as_deref()
        {
            MCP_SESSION_RETRIES_TOTAL.fetch_add(1, Ordering::Relaxed);
            invalidate_session(&slot, session_id).await;
            continue;
        }
        slot.last_used_at.store(now_seconds(), Ordering::Relaxed);
        return response.rpc_value(method, Some(&request_id));
    }

    Err(AppError::InvalidInput(
        "MCP streamable HTTP session could not be established".to_string(),
    ))
}

async fn ensure_session(
    slot: &SessionSlot,
    secret_resolver: &SecretResolver,
    endpoint: &str,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<StreamableSession, AppError> {
    let now = now_seconds();
    let mut state = slot.state.lock().await;
    if let Some(session) = state.as_ref()
        && now.saturating_sub(session.initialized_at) < SESSION_TTL_SECONDS
    {
        MCP_SESSION_REUSES_TOTAL.fetch_add(1, Ordering::Relaxed);
        return Ok(session.clone());
    }

    if let Some(expired) = state.take() {
        close_session(secret_resolver, endpoint, config, secret_ref, &expired).await;
    }
    let session = initialize_session(secret_resolver, endpoint, config, secret_ref).await?;
    MCP_SESSION_INITIALIZATIONS_TOTAL.fetch_add(1, Ordering::Relaxed);
    *state = Some(session.clone());
    slot.last_used_at.store(now, Ordering::Relaxed);
    Ok(session)
}

async fn initialize_session(
    secret_resolver: &SecretResolver,
    endpoint: &str,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<StreamableSession, AppError> {
    let protocol_version = json_string(config, "protocol_version")
        .unwrap_or_else(|| DEFAULT_PROTOCOL_VERSION.to_string());
    let request_id = Uuid::new_v4().to_string();
    let request = authenticated_request(
        secret_resolver,
        http_client().post(endpoint),
        config,
        secret_ref,
    )
    .await?
    .header(header::ACCEPT, "application/json, text/event-stream")
    .timeout(request_timeout(config))
    .json(&json_rpc_request(
        &request_id,
        "initialize",
        json!({
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": {"name": "bibi-work-rust-mcp", "version": env!("CARGO_PKG_VERSION")}
        }),
    ));
    let response = send(request, "initialize").await?;
    let response_session_id = response
        .session_id
        .clone()
        .filter(|value| !value.trim().is_empty());
    let value = response.rpc_value("initialize", Some(&request_id))?;
    if value.get("error").is_some() {
        return Err(AppError::InvalidInput(
            "MCP streamable HTTP initialize returned an RPC error".to_string(),
        ));
    }
    let negotiated_version = value
        .pointer("/result/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(&protocol_version)
        .to_string();
    let session = StreamableSession {
        id: response_session_id,
        protocol_version: negotiated_version,
        initialized_at: now_seconds(),
    };

    let initialized = apply_streamable_session_headers(
        authenticated_request(
            secret_resolver,
            http_client().post(endpoint),
            config,
            secret_ref,
        )
        .await?,
        &session,
    )
    .header(header::ACCEPT, "application/json, text/event-stream")
    .timeout(request_timeout(config))
    .json(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));
    let initialized_response = send(initialized, "notifications/initialized").await?;
    if !initialized_response.status.is_success() {
        return Err(http_status_error(
            "notifications/initialized",
            initialized_response.status,
        ));
    }
    Ok(session)
}

async fn send_session_request(
    secret_resolver: &SecretResolver,
    endpoint: &str,
    config: &Value,
    secret_ref: Option<&str>,
    session: &StreamableSession,
    body: Value,
    operation: &str,
) -> Result<RawMcpResponse, AppError> {
    let request = apply_streamable_session_headers(
        authenticated_request(
            secret_resolver,
            http_client().post(endpoint),
            config,
            secret_ref,
        )
        .await?,
        session,
    )
    .header(header::ACCEPT, "application/json, text/event-stream")
    .timeout(request_timeout(config))
    .json(&body);
    send(request, operation).await
}

async fn close_session(
    secret_resolver: &SecretResolver,
    endpoint: &str,
    config: &Value,
    secret_ref: Option<&str>,
    session: &StreamableSession,
) {
    let Some(session_id) = session.id.as_deref() else {
        return;
    };
    let Ok(request) = authenticated_request(
        secret_resolver,
        http_client().delete(endpoint),
        config,
        secret_ref,
    )
    .await
    else {
        return;
    };
    let _ = request
        .header("Mcp-Session-Id", session_id)
        .header("MCP-Protocol-Version", &session.protocol_version)
        .timeout(request_timeout(config))
        .send()
        .await;
}

fn apply_streamable_session_headers(
    request: RequestBuilder,
    session: &StreamableSession,
) -> RequestBuilder {
    let request = request.header("MCP-Protocol-Version", &session.protocol_version);
    if let Some(session_id) = session.id.as_deref() {
        request.header("Mcp-Session-Id", session_id)
    } else {
        request
    }
}

async fn invalidate_session(slot: &SessionSlot, expected_id: &str) {
    let mut state = slot.state.lock().await;
    if state
        .as_ref()
        .is_some_and(|session| session.id.as_deref() == Some(expected_id))
    {
        *state = None;
    }
}

async fn session_slot(key: String) -> Arc<SessionSlot> {
    let slots = SESSION_SLOTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut slots = slots.lock().await;
    if let Some(slot) = slots.get(&key) {
        return Arc::clone(slot);
    }
    if slots.len() >= MAX_SESSION_SLOTS
        && let Some(oldest_key) = slots
            .iter()
            .min_by_key(|(_, slot)| slot.last_used_at.load(Ordering::Relaxed))
            .map(|(key, _)| key.clone())
    {
        slots.remove(&oldest_key);
    }
    let slot = Arc::new(SessionSlot {
        state: Mutex::new(None),
        last_used_at: AtomicU64::new(now_seconds()),
    });
    slots.insert(key, Arc::clone(&slot));
    MCP_SESSION_SLOTS.store(slots.len() as u64, Ordering::Relaxed);
    slot
}

fn observe_request_duration(duration: Duration) {
    let millis = duration.as_millis().min(u128::from(u64::MAX)) as u64;
    let micros = duration.as_micros().min(u128::from(u64::MAX)) as u64;
    MCP_REQUEST_DURATION_MICROS.fetch_add(micros, Ordering::Relaxed);
    for (bound_ms, bucket) in MCP_REQUEST_DURATION_BOUNDS_MS
        .iter()
        .zip(MCP_REQUEST_DURATION_BUCKETS.iter())
    {
        if millis <= *bound_ms {
            bucket.fetch_add(1, Ordering::Relaxed);
        }
    }
}

async fn send(request: RequestBuilder, operation: &str) -> Result<RawMcpResponse, AppError> {
    let response = request
        .send()
        .await
        .map_err(|err| AppError::InvalidInput(format!("MCP {operation} request failed: {err}")))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if response
        .content_length()
        .is_some_and(|size| size > MAX_MCP_RESPONSE_BYTES)
    {
        return Err(AppError::InvalidInput(format!(
            "MCP {operation} response exceeds the {} byte limit",
            MAX_MCP_RESPONSE_BYTES
        )));
    }
    let bytes = response.bytes().await.map_err(|err| {
        AppError::InvalidInput(format!("MCP {operation} response read failed: {err}"))
    })?;
    if bytes.len() as u64 > MAX_MCP_RESPONSE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "MCP {operation} response exceeds the {} byte limit",
            MAX_MCP_RESPONSE_BYTES
        )));
    }
    Ok(RawMcpResponse {
        status,
        content_type,
        session_id,
        bytes: bytes.to_vec(),
    })
}

impl RawMcpResponse {
    fn indicates_invalid_session(&self) -> bool {
        self.status == StatusCode::NOT_FOUND
            || (self.status == StatusCode::BAD_REQUEST
                && String::from_utf8_lossy(&self.bytes)
                    .to_ascii_lowercase()
                    .contains("session"))
    }

    fn rpc_value(&self, operation: &str, expected_id: Option<&str>) -> Result<Value, AppError> {
        if !self.status.is_success() {
            return Err(http_status_error(operation, self.status));
        }
        if self.bytes.is_empty() {
            return Ok(Value::Null);
        }
        let is_sse = self
            .content_type
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
        if is_sse {
            parse_sse_json(&self.bytes, expected_id).ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "MCP {operation} SSE response did not contain the matching JSON-RPC message"
                ))
            })
        } else {
            serde_json::from_slice(&self.bytes).map_err(|err| {
                AppError::InvalidInput(format!("MCP {operation} JSON parse failed: {err}"))
            })
        }
    }
}

fn parse_sse_json(bytes: &[u8], expected_id: Option<&str>) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut fallback = None;
    let normalized = text.replace("\r\n", "\n");
    for event in normalized.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if expected_id.is_some_and(|id| value.get("id").and_then(Value::as_str) == Some(id)) {
            return Some(value);
        }
        fallback = Some(value);
    }
    expected_id.is_none().then_some(fallback).flatten()
}

fn json_rpc_request(id: &str, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
}

fn http_client() -> &'static Client {
    HTTP_CLIENT.get_or_init(Client::new)
}

fn request_timeout(config: &Value) -> Duration {
    Duration::from_millis(
        config
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MCP_HTTP_TIMEOUT_MS)
            .clamp(1_000, 120_000),
    )
}

pub fn mcp_endpoint(config: &Value) -> Result<String, AppError> {
    if let Some(url) = json_string(config, "tools_list_url")
        .or_else(|| json_string(config, "discovery_url"))
        .or_else(|| json_string(config, "tool_call_url"))
        .or_else(|| json_string(config, "endpoint"))
        .or_else(|| json_string(config, "url"))
    {
        return Ok(url);
    }
    let base_url = json_string(config, "base_url").ok_or_else(|| {
        AppError::InvalidInput("MCP server endpoint/base_url is required".to_string())
    })?;
    let path = json_string(config, "path").unwrap_or_else(|| "/".to_string());
    Ok(format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

async fn authenticated_request(
    secret_resolver: &SecretResolver,
    request: RequestBuilder,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<RequestBuilder, AppError> {
    let mut headers = configured_headers(config)?;
    if let Some(secret_ref) = secret_ref {
        let secret = secret_resolver.resolve(secret_ref).await?;
        let header_name = json_string(config, "auth_header")
            .or_else(|| json_string(config, "secret_header"))
            .unwrap_or_else(|| "Authorization".to_string());
        let header_name = parse_header_name(&header_name, "MCP auth header name is invalid")?;
        let scheme = json_string(config, "auth_scheme")
            .or_else(|| json_string(config, "secret_scheme"))
            .unwrap_or_else(|| "Bearer".to_string());
        let header_value = if scheme.eq_ignore_ascii_case("none") {
            secret
        } else {
            format!("{} {}", scheme.trim(), secret)
        };
        let header_value = HeaderValue::from_str(&header_value)
            .map_err(|_| AppError::InvalidInput("MCP auth header value is invalid".to_string()))?;
        headers.insert(header_name, header_value);
    }
    Ok(request.headers(headers))
}

fn configured_headers(config: &Value) -> Result<HeaderMap, AppError> {
    let Some(headers) = config.get("headers") else {
        return Ok(HeaderMap::new());
    };
    let headers = headers.as_object().ok_or_else(|| {
        AppError::InvalidInput("MCP transport headers must be an object".to_string())
    })?;
    if headers.len() > MAX_MCP_REQUEST_HEADERS {
        return Err(AppError::InvalidInput(format!(
            "MCP transport headers exceed the {MAX_MCP_REQUEST_HEADERS} entry limit"
        )));
    }

    let mut result = HeaderMap::with_capacity(headers.len());
    for (name, value) in headers {
        let name = parse_header_name(name, "MCP transport header name is invalid")?;
        if is_runtime_owned_header(&name) {
            return Err(AppError::InvalidInput(format!(
                "MCP transport header {} is managed by the runtime",
                name.as_str()
            )));
        }
        let value = value.as_str().ok_or_else(|| {
            AppError::InvalidInput("MCP transport header values must be strings".to_string())
        })?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            AppError::InvalidInput("MCP transport header value is invalid".to_string())
        })?;
        result.insert(name, value);
    }
    Ok(result)
}

fn parse_header_name(value: &str, error: &str) -> Result<HeaderName, AppError> {
    HeaderName::from_bytes(value.as_bytes()).map_err(|_| AppError::InvalidInput(error.to_string()))
}

fn is_runtime_owned_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "upgrade"
            | "mcp-session-id"
            | "mcp-protocol-version"
    )
}

fn session_key(
    endpoint: &str,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(endpoint.as_bytes());
    hasher.update([0]);
    hasher
        .update(serde_json::to_vec(config).map_err(|err| {
            AppError::InvalidInput(format!("failed to encode MCP config: {err}"))
        })?);
    hasher.update([0]);
    hasher.update(secret_ref.unwrap_or_default().as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

fn json_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn http_status_error(operation: &str, status: StatusCode) -> AppError {
    AppError::InvalidInput(format!("MCP {operation} returned HTTP {}", status.as_u16()))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{
            HeaderMap as AxumHeaderMap, HeaderValue as AxumHeaderValue,
            StatusCode as AxumStatusCode,
        },
        response::{
            IntoResponse, Response,
            sse::{Event, Sse},
        },
        routing::{get, post},
    };
    use futures_util::stream;
    use std::{convert::Infallible, sync::Arc};
    use tokio::sync::{Mutex as TokioMutex, mpsc};

    #[test]
    fn parses_matching_json_rpc_message_from_sse() {
        let value = parse_sse_json(
            b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"wanted\",\"result\":{\"ok\":true}}\n\n",
            Some("wanted"),
        )
        .expect("matching SSE data");
        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn rejects_non_matching_json_rpc_message_from_sse() {
        assert!(
            parse_sse_json(
                b"data: {\"jsonrpc\":\"2.0\",\"id\":\"other\",\"result\":{}}\n\n",
                Some("wanted"),
            )
            .is_none()
        );
    }

    #[test]
    fn incremental_sse_parser_handles_crlf_and_multiple_data_lines() {
        let bytes = b"event: message\r\ndata: {\"first\":\r\ndata: true}\r\n\r\nremaining";
        let (event, consumed) = parse_next_sse_event(bytes)
            .expect("valid event")
            .expect("complete event");
        assert_eq!(event.event.as_deref(), Some("message"));
        assert_eq!(event.data, "{\"first\":\ntrue}");
        assert_eq!(&bytes[consumed..], b"remaining");
    }

    #[test]
    fn legacy_message_endpoint_must_remain_same_origin() {
        let base = Url::parse("https://mcp.example/sse").expect("base URL");
        assert_eq!(
            resolve_legacy_message_endpoint(&base, "/messages?session=1")
                .expect("same origin")
                .as_str(),
            "https://mcp.example/messages?session=1"
        );
        assert!(resolve_legacy_message_endpoint(&base, "https://evil.example/messages").is_err());
        assert!(resolve_legacy_message_endpoint(&base, "//evil.example/messages").is_err());
    }

    #[derive(Clone, Default)]
    struct LegacyFixtureState {
        sender: Arc<TokioMutex<Option<mpsc::UnboundedSender<String>>>>,
    }

    async fn legacy_sse(
        State(state): State<LegacyFixtureState>,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
        let (sender, receiver) = mpsc::unbounded_channel();
        *state.sender.lock().await = Some(sender);
        let endpoint =
            stream::once(async { Ok(Event::default().event("endpoint").data("/messages")) });
        let messages = stream::unfold(receiver, |mut receiver| async {
            receiver
                .recv()
                .await
                .map(|data| (Ok(Event::default().event("message").data(data)), receiver))
        });
        Sse::new(endpoint.chain(messages))
    }

    async fn legacy_message(
        State(state): State<LegacyFixtureState>,
        Json(payload): Json<Value>,
    ) -> AxumStatusCode {
        let Some(id) = payload.get("id").and_then(Value::as_str) else {
            return AxumStatusCode::ACCEPTED;
        };
        let result = match payload.get("method").and_then(Value::as_str) {
            Some("initialize") => json!({
                "protocolVersion": DEFAULT_PROTOCOL_VERSION,
                "capabilities": {},
                "serverInfo": {"name": "legacy-fixture", "version": "1"}
            }),
            Some("tools/list") => json!({
                "tools": [{"name": "legacy_echo", "description": "fixture", "inputSchema": {"type": "object"}}]
            }),
            _ => json!({}),
        };
        if let Some(sender) = state.sender.lock().await.as_ref() {
            let _ = sender.send(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string());
        }
        AxumStatusCode::ACCEPTED
    }

    #[tokio::test]
    async fn legacy_sse_negotiates_endpoint_initializes_and_lists_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = LegacyFixtureState::default();
        let app = Router::new()
            .route("/sse", get(legacy_sse))
            .route("/messages", post(legacy_message))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let result = request(
            &SecretResolver::env_only_for_tests(),
            "sse",
            &json!({"endpoint": format!("http://{address}/sse"), "timeout_ms": 5_000}),
            None,
            "tools/list",
            json!({}),
        )
        .await?;
        assert_eq!(
            result
                .pointer("/result/tools/0/name")
                .and_then(Value::as_str),
            Some("legacy_echo")
        );
        server.abort();
        Ok(())
    }

    async fn streamable_with_custom_header(
        headers: AxumHeaderMap,
        Json(payload): Json<Value>,
    ) -> Response {
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("test-key")
        );
        let method = payload
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method != "initialize" {
            assert_eq!(
                headers
                    .get("mcp-session-id")
                    .and_then(|value| value.to_str().ok()),
                Some("test-session")
            );
        }

        match method {
            "initialize" => {
                let mut response = Json(json!({
                    "jsonrpc": "2.0",
                    "id": payload["id"].clone(),
                    "result": {
                        "protocolVersion": DEFAULT_PROTOCOL_VERSION,
                        "capabilities": {},
                        "serverInfo": {"name": "header-fixture", "version": "1"}
                    }
                }))
                .into_response();
                response.headers_mut().insert(
                    "Mcp-Session-Id",
                    AxumHeaderValue::from_static("test-session"),
                );
                response
            }
            "notifications/initialized" => AxumStatusCode::ACCEPTED.into_response(),
            "tools/list" => Json(json!({
                "jsonrpc": "2.0",
                "id": payload["id"].clone(),
                "result": {
                    "tools": [{
                        "name": "secured_streamable_tool",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]
                }
            }))
            .into_response(),
            _ => AxumStatusCode::BAD_REQUEST.into_response(),
        }
    }

    #[tokio::test]
    async fn streamable_http_sends_custom_headers_throughout_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let app = Router::new().route("/mcp", post(streamable_with_custom_header));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let result = request(
            &SecretResolver::env_only_for_tests(),
            "streamable-http",
            &json!({
                "endpoint": format!("http://{address}/mcp"),
                "headers": {"X-API-Key": "test-key"},
                "timeout_ms": 5_000
            }),
            None,
            "tools/list",
            json!({}),
        )
        .await?;
        assert_eq!(
            result
                .pointer("/result/tools/0/name")
                .and_then(Value::as_str),
            Some("secured_streamable_tool")
        );

        server.abort();
        Ok(())
    }

    async fn sessionless_streamable_with_custom_header(
        headers: AxumHeaderMap,
        Json(payload): Json<Value>,
    ) -> Response {
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("test-key")
        );
        assert!(headers.get("mcp-session-id").is_none());
        let method = payload
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match method {
            "initialize" => Json(json!({
                "jsonrpc": "2.0",
                "id": payload["id"].clone(),
                "result": {
                    "protocolVersion": DEFAULT_PROTOCOL_VERSION,
                    "capabilities": {},
                    "serverInfo": {"name": "sessionless-fixture", "version": "1"}
                }
            }))
            .into_response(),
            "notifications/initialized" => AxumStatusCode::ACCEPTED.into_response(),
            "tools/list" => Json(json!({
                "jsonrpc": "2.0",
                "id": payload["id"].clone(),
                "result": {
                    "tools": [{
                        "name": "sessionless_tool",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]
                }
            }))
            .into_response(),
            _ => AxumStatusCode::BAD_REQUEST.into_response(),
        }
    }

    #[tokio::test]
    async fn streamable_http_accepts_servers_without_session_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let app = Router::new().route("/mcp", post(sessionless_streamable_with_custom_header));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let result = request(
            &SecretResolver::env_only_for_tests(),
            "streamable-http",
            &json!({
                "endpoint": format!("http://{address}/mcp"),
                "headers": {"X-API-Key": "test-key"},
                "timeout_ms": 5_000
            }),
            None,
            "tools/list",
            json!({}),
        )
        .await?;
        assert_eq!(
            result
                .pointer("/result/tools/0/name")
                .and_then(Value::as_str),
            Some("sessionless_tool")
        );

        server.abort();
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires BIBI_TEST_LEGACY_MCP_SSE_URL"]
    async fn official_sdk_legacy_sse_lists_and_calls_tool() -> Result<(), Box<dyn std::error::Error>>
    {
        let endpoint = std::env::var("BIBI_TEST_LEGACY_MCP_SSE_URL")?;
        let resolver = SecretResolver::env_only_for_tests();
        let config = json!({"endpoint": endpoint, "timeout_ms": 10_000});
        let listed = request(&resolver, "sse", &config, None, "tools/list", json!({})).await?;
        assert_eq!(
            listed
                .pointer("/result/tools/0/name")
                .and_then(Value::as_str),
            Some("legacy_echo")
        );
        let called = request(
            &resolver,
            "sse",
            &config,
            None,
            "tools/call",
            json!({"name": "legacy_echo", "arguments": {}}),
        )
        .await?;
        assert_eq!(
            called
                .pointer("/result/content/0/text")
                .and_then(Value::as_str),
            Some("legacy-ok")
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires BIBI_TEST_STREAMABLE_MCP_URL"]
    async fn real_streamable_http_session_lists_and_calls_tool()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint = std::env::var("BIBI_TEST_STREAMABLE_MCP_URL")?;
        let resolver = SecretResolver::env_only_for_tests();
        let config = json!({"endpoint": endpoint, "timeout_ms": 30_000});

        let first = request(
            &resolver,
            "streamable-http",
            &config,
            None,
            "tools/list",
            json!({}),
        )
        .await?;
        let second = request(
            &resolver,
            "streamable-http",
            &config,
            None,
            "tools/list",
            json!({}),
        )
        .await?;
        assert_eq!(
            first
                .pointer("/result/tools")
                .and_then(Value::as_array)
                .map(Vec::len),
            second
                .pointer("/result/tools")
                .and_then(Value::as_array)
                .map(Vec::len)
        );

        let result = request(
            &resolver,
            "streamable-http",
            &config,
            None,
            "tools/call",
            json!({
                "name": "maps_geocode",
                "arguments": {"address": "Shanghai Tower"}
            }),
        )
        .await?;
        assert!(result.get("error").is_none());
        assert!(
            result
                .pointer("/result/content")
                .and_then(Value::as_array)
                .is_some_and(|content| !content.is_empty())
        );
        Ok(())
    }
}
