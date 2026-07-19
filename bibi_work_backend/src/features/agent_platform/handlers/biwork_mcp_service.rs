use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path, State},
};
use reqwest::Url;
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext,
            mcp_discovery::{self, DiscoveredMcpTool},
            models::AuthzContext,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_compat_service::{
        biwork_failure, epoch_ms, ok, required_string, trimmed_string, value_string,
    },
    support::require_ferriskey_allow,
};

fn required_mcp_oauth_server_url(value: &Value) -> Result<String, AppError> {
    let server_url = required_string(value, "server_url")?;
    let parsed = Url::parse(&server_url)
        .map_err(|_| AppError::InvalidInput("server_url must be a valid URL".to_string()))?;
    if matches!(parsed.scheme(), "http" | "https") {
        Ok(server_url)
    } else {
        Err(AppError::InvalidInput(
            "server_url must use http or https".to_string(),
        ))
    }
}

pub async fn biwork_list_mcp_servers(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, description, transport, config, status,
               health_status, last_health_check_at, last_discovered_at,
               consecutive_failures, health_error, created_at, updated_at
        FROM mcp_servers
        WHERE tenant_id = $1 AND deleted_at IS NULL
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let server_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("id"))
        .collect::<Result<Vec<_>, _>>()?;
    let tools_by_server = load_biwork_mcp_tools(&state, ctx.tenant_id, &server_ids).await?;

    let mut servers = Vec::with_capacity(rows.len());
    for row in rows {
        let server_id: Uuid = row.try_get("id")?;
        servers.push(biwork_mcp_server_from_row(
            &row,
            tools_by_server.get(&server_id).cloned().unwrap_or_default(),
        )?);
    }

    Ok(ok(Value::Array(servers)))
}

pub async fn biwork_create_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = required_string(&payload, "name")?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "create",
        "mcp_server",
        name.clone(),
        None,
    )
    .await?;
    let server = upsert_biwork_mcp_server(&state, ctx.tenant_id, None, &payload).await?;
    Ok(ok(server))
}

pub async fn biwork_import_mcp_servers(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let servers = payload
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("servers is required".to_string()))?;
    let mut imported = Vec::with_capacity(servers.len());
    for server in servers {
        let name = required_string(server, "name")?;
        require_ferriskey_allow(
            &state,
            &ctx,
            ctx.tenant_id,
            "create",
            "mcp_server",
            name,
            None,
        )
        .await?;
        imported.push(upsert_biwork_mcp_server(&state, ctx.tenant_id, None, server).await?);
    }
    Ok(ok(Value::Array(imported)))
}

pub async fn biwork_update_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(server_ref): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let server_id = resolve_biwork_mcp_server_id(&state, ctx.tenant_id, &server_ref).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "mcp_server",
        server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(server_id),
            ..Default::default()
        }),
    )
    .await?;
    let server = upsert_biwork_mcp_server(&state, ctx.tenant_id, Some(server_id), &payload).await?;
    Ok(ok(server))
}

pub async fn biwork_delete_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(server_ref): Path<String>,
) -> Result<Json<Value>, AppError> {
    let server_id = resolve_biwork_mcp_server_id(&state, ctx.tenant_id, &server_ref).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "delete",
        "mcp_server",
        server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(server_id),
            ..Default::default()
        }),
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE mcp_servers
        SET status = 'deleted',
            deleted_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(server_id)
    .bind(ctx.tenant_id)
    .execute(&state.connect_pool)
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_toggle_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(server_ref): Path<String>,
) -> Result<Json<Value>, AppError> {
    let server_id = resolve_biwork_mcp_server_id(&state, ctx.tenant_id, &server_ref).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "mcp_server",
        server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(server_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE mcp_servers
        SET status = CASE WHEN status = 'active' THEN 'disabled' ELSE 'active' END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, name, description, transport, config, status,
                  health_status, last_health_check_at, last_discovered_at,
                  consecutive_failures, health_error, created_at, updated_at
        "#,
    )
    .bind(server_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?;
    let tools_by_server = load_biwork_mcp_tools(&state, ctx.tenant_id, &[server_id]).await?;
    Ok(ok(biwork_mcp_server_from_row(
        &row,
        tools_by_server.get(&server_id).cloned().unwrap_or_default(),
    )?))
}

pub async fn biwork_test_mcp_connection(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let (server_id, transport_kind, config, secret_ref) =
        resolve_biwork_mcp_test_target(&state, ctx.tenant_id, &payload).await?;
    if transport_kind == "stdio" {
        let command = value_string(&config, "command").unwrap_or_default();
        let result = if command.is_empty() {
            biwork_failure(
                "MCP_STDIO_COMMAND_REQUIRED",
                "MCP stdio command is required",
                json!({}),
            )
        } else {
            biwork_failure(
                "MCP_STDIO_LOCAL_RUNTIME_REQUIRED",
                "MCP stdio live connection tests require a local desktop runtime; the Rust catalog endpoint only owns persisted MCP facts",
                json!({ "transport": "stdio" }),
            )
        };
        if let Some(server_id) = server_id {
            update_biwork_mcp_test_status(
                &state,
                ctx.tenant_id,
                server_id,
                "unsupported",
                Some("stdio connection tests require the local desktop runtime"),
            )
            .await?;
        }
        return Ok(ok(result));
    }

    match crate::features::agent_platform::mcp_discovery::discover_mcp_tools(
        &state.secret_resolver,
        biwork_mcp_discovery_transport(&transport_kind),
        &config,
        secret_ref.as_deref(),
    )
    .await
    {
        Ok(tools) => {
            if let Some(server_id) = server_id {
                replace_biwork_mcp_tools(&state, ctx.tenant_id, server_id, &tools).await?;
                update_biwork_mcp_test_status(&state, ctx.tenant_id, server_id, "healthy", None)
                    .await?;
            }
            Ok(ok(json!({
                "success": true,
                "tools": tools.into_iter().map(discovered_mcp_tool_json).collect::<Vec<_>>(),
            })))
        }
        Err(error) => {
            let error_message = error.to_string();
            if let Some(server_id) = server_id {
                update_biwork_mcp_test_status(
                    &state,
                    ctx.tenant_id,
                    server_id,
                    "unhealthy",
                    Some(&error_message),
                )
                .await?;
            }
            Ok(ok(biwork_mcp_connection_failure(&error_message)))
        }
    }
}

fn biwork_mcp_connection_failure(error: &str) -> Value {
    if let Some(status) = mcp_http_status(error) {
        return biwork_failure(
            "MCP_HTTP_ERROR",
            "MCP server returned an HTTP error",
            json!({ "status": status }),
        );
    }
    if error.to_ascii_lowercase().contains("timed out") {
        return biwork_failure("MCP_TIMEOUT", "MCP connection test timed out", json!({}));
    }
    if error.contains("RPC error") {
        return biwork_failure(
            "MCP_RPC_ERROR",
            "MCP server returned an RPC error",
            json!({ "method": "request" }),
        );
    }
    if error.contains("initialize")
        || error.contains("protocol")
        || error.contains("session")
        || error.contains("SSE")
    {
        return biwork_failure(
            "MCP_PROTOCOL_ERROR",
            "MCP protocol handshake failed",
            json!({}),
        );
    }
    biwork_failure(
        "MCP_CONNECTION_FAILED",
        "MCP connection test failed",
        json!({}),
    )
}

fn mcp_http_status(error: &str) -> Option<u16> {
    error
        .split_once("returned HTTP ")
        .and_then(|(_, tail)| tail.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

pub async fn biwork_report_mcp_local_discovery(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(server_ref): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let server_id = resolve_biwork_mcp_server_id(&state, ctx.tenant_id, &server_ref).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "manage",
        "mcp_server",
        server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(server_id),
            ..Default::default()
        }),
    )
    .await?;
    let transport: String = sqlx::query_scalar(
        r#"
        SELECT transport FROM mcp_servers
        WHERE id = $1 AND tenant_id = $2 AND status = 'active' AND deleted_at IS NULL
        "#,
    )
    .bind(server_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("active mcp server not found".to_string()))?;
    if transport != "stdio" {
        return Err(AppError::Conflict(
            "local MCP discovery reports are only accepted for stdio servers".to_string(),
        ));
    }

    if payload
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let tools = payload
            .get("tools")
            .cloned()
            .filter(Value::is_array)
            .ok_or_else(|| AppError::InvalidInput("tools must be an array".to_string()))?;
        let discovered = mcp_discovery::parse_tools_list_response(json!({ "tools": tools }))?;
        replace_biwork_mcp_tools(&state, ctx.tenant_id, server_id, &discovered).await?;
        update_biwork_mcp_test_status(&state, ctx.tenant_id, server_id, "healthy", None).await?;
        return Ok(ok(json!({
            "reported": true,
            "tools": discovered.into_iter().map(discovered_mcp_tool_json).collect::<Vec<_>>(),
        })));
    }

    let error = payload
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("local MCP stdio discovery failed");
    update_biwork_mcp_test_status(&state, ctx.tenant_id, server_id, "unhealthy", Some(error))
        .await?;
    Ok(ok(json!({ "reported": true })))
}

pub async fn biwork_mcp_agent_configs() -> Result<Json<Value>, AppError> {
    Ok(ok(json!([])))
}

pub async fn biwork_mcp_oauth_check_status(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let _server_url = required_mcp_oauth_server_url(&payload)?;
    Ok(ok(json!({ "authenticated": false })))
}

pub async fn biwork_mcp_oauth_login(Json(payload): Json<Value>) -> Result<Json<Value>, AppError> {
    let _server_url = required_mcp_oauth_server_url(&payload)?;
    Ok(ok(biwork_failure(
        "MCP_OAUTH_LOCAL_RUNTIME_REQUIRED",
        "MCP OAuth browser login requires the desktop local runtime; the Rust backend only owns persisted MCP catalog facts",
        json!({}),
    )))
}

pub async fn biwork_mcp_oauth_logout(Json(payload): Json<Value>) -> Result<Json<Value>, AppError> {
    let _server_url = required_mcp_oauth_server_url(&payload)?;
    Ok(ok(Value::Null))
}

fn mcp_transport_json(transport: &str, config: &Value) -> Value {
    match transport {
        "stdio" => json!({
            "type": "stdio",
            "command": value_string(config, "command").unwrap_or_default(),
            "args": config.get("args").cloned().unwrap_or_else(|| json!([])),
            "env": config.get("env").cloned().unwrap_or_else(|| json!({})),
        }),
        "sse" => json!({
            "type": "sse",
            "url": value_string(config, "url").unwrap_or_default(),
            "headers": config.get("headers").cloned().unwrap_or_else(|| json!({})),
        }),
        "streamable-http" => json!({
            "type": "streamable_http",
            "url": value_string(config, "url").unwrap_or_default(),
            "headers": config.get("headers").cloned().unwrap_or_else(|| json!({})),
        }),
        _ => json!({
            "type": "http",
            "url": value_string(config, "url").unwrap_or_default(),
            "headers": config.get("headers").cloned().unwrap_or_else(|| json!({})),
        }),
    }
}

fn biwork_mcp_server_from_row(
    row: &sqlx::postgres::PgRow,
    tools: Vec<Value>,
) -> Result<Value, AppError> {
    let server_id: Uuid = row.try_get("id")?;
    let transport_kind: String = row.try_get("transport")?;
    let config: Value = row.try_get("config")?;
    let status: String = row.try_get("status")?;
    let health_status: String = row.try_get("health_status")?;
    let last_health_check_at: Option<OffsetDateTime> = row.try_get("last_health_check_at")?;
    let last_discovered_at: Option<OffsetDateTime> = row.try_get("last_discovered_at")?;
    let transport = mcp_transport_json(&transport_kind, &config);
    let original_json = value_string(&config, "original_json")
        .unwrap_or_else(|| serde_json::to_string(&transport).unwrap_or_else(|_| "{}".to_string()));
    Ok(json!({
        "id": server_id.to_string(),
        "name": row.try_get::<String, _>("name")?,
        "description": row.try_get::<Option<String>, _>("description")?,
        "enabled": status == "active",
        "transport": transport,
        "tools": tools,
        "last_test_status": match health_status.as_str() {
            "healthy" => "connected",
            "unhealthy" => "error",
            _ => "disconnected",
        },
        "last_connected": last_discovered_at.map(epoch_ms),
        "last_health_check": last_health_check_at.map(epoch_ms),
        "health_status": health_status,
        "consecutive_failures": row.try_get::<i32, _>("consecutive_failures")?,
        "has_health_error": row.try_get::<Option<String>, _>("health_error")?.is_some(),
        "created_at": epoch_ms(row.try_get("created_at")?),
        "updated_at": epoch_ms(row.try_get("updated_at")?),
        "original_json": original_json,
        "builtin": config.get("builtin").and_then(Value::as_bool).unwrap_or(false),
    }))
}

pub(super) fn normalize_biwork_mcp_transport_payload(
    payload: &Value,
) -> Result<(String, Value), AppError> {
    let transport = payload
        .get("transport")
        .ok_or_else(|| AppError::InvalidInput("transport is required".to_string()))?;
    let transport_type = value_string(transport, "type").unwrap_or_else(|| "http".to_string());
    let transport_kind = match transport_type.as_str() {
        "stdio" => "stdio",
        "sse" => "sse",
        "http" => "http",
        "streamable_http" | "streamable-http" => "streamable-http",
        _ => {
            return Err(AppError::InvalidInput(
                "transport.type is not supported".to_string(),
            ));
        }
    };

    let mut config = transport
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("transport must be a JSON object".to_string()))?;
    config.remove("type");
    config.remove("transport");
    match transport_kind {
        "stdio" => {
            if value_string(&Value::Object(config.clone()), "command").is_none() {
                return Err(AppError::InvalidInput(
                    "transport.command is required".to_string(),
                ));
            }
            if !config.get("args").is_none_or(Value::is_array) {
                return Err(AppError::InvalidInput(
                    "transport.args must be an array".to_string(),
                ));
            }
            if !config.get("env").is_none_or(Value::is_object) {
                return Err(AppError::InvalidInput(
                    "transport.env must be an object".to_string(),
                ));
            }
            if let Some(env) = config.get("env").and_then(Value::as_object) {
                if env.len() > 128 {
                    return Err(AppError::InvalidInput(
                        "transport.env exceeds the supported entry limit".to_string(),
                    ));
                }
                for reference in env.values() {
                    let reference = reference.as_str().ok_or_else(|| {
                        AppError::InvalidInput(
                            "transport.env values must be env:// references".to_string(),
                        )
                    })?;
                    if !reference.starts_with("env://") {
                        return Err(AppError::InvalidInput(
                            "transport.env values must use env:// references".to_string(),
                        ));
                    }
                    crate::features::agent_platform::secret_resolver::validate_secret_ref(
                        reference,
                    )?;
                }
            }
        }
        _ => {
            if value_string(&Value::Object(config.clone()), "url").is_none() {
                return Err(AppError::InvalidInput(
                    "transport.url is required".to_string(),
                ));
            }
            if !config.get("headers").is_none_or(Value::is_object) {
                return Err(AppError::InvalidInput(
                    "transport.headers must be an object".to_string(),
                ));
            }
        }
    }

    config.insert(
        "original_json".to_string(),
        Value::String(value_string(payload, "original_json").unwrap_or_else(|| {
            serde_json::to_string(transport).unwrap_or_else(|_| "{}".to_string())
        })),
    );
    config.insert(
        "builtin".to_string(),
        Value::Bool(
            payload
                .get("builtin")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
    );
    if let Some(biwork_id) = value_string(payload, "id") {
        config.insert("biwork_id".to_string(), Value::String(biwork_id));
    }
    Ok((transport_kind.to_string(), Value::Object(config)))
}

async fn upsert_biwork_mcp_server(
    state: &AppState,
    tenant_id: Uuid,
    target_id: Option<Uuid>,
    payload: &Value,
) -> Result<Value, AppError> {
    let name = trimmed_string(payload, "name");
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let normalized_transport = if payload.get("transport").is_some() {
        Some(normalize_biwork_mcp_transport_payload(payload)?)
    } else {
        None
    };

    let server_id = if target_id.is_some() {
        target_id
    } else if let Some(name) = name.as_deref() {
        sqlx::query_scalar(
            r#"
            SELECT id
            FROM mcp_servers
            WHERE tenant_id = $1 AND name = $2 AND deleted_at IS NULL
            ORDER BY updated_at DESC, created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .bind(name)
        .fetch_optional(&state.connect_pool)
        .await?
    } else {
        None
    };

    let row = if let Some(server_id) = server_id {
        let (transport_kind, config) = normalized_transport
            .map(|(transport_kind, config)| (Some(transport_kind), Some(config)))
            .unwrap_or((None, None));
        sqlx::query(
            r#"
            UPDATE mcp_servers
            SET name = COALESCE($3, name),
                description = COALESCE($4, description),
                transport = COALESCE($5, transport),
                config = COALESCE($6, config),
                status = CASE WHEN status = 'deleted' THEN 'active' ELSE status END,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
            RETURNING id, name, description, transport, config, status,
                      health_status, last_health_check_at, last_discovered_at,
                      consecutive_failures, health_error, created_at, updated_at
            "#,
        )
        .bind(server_id)
        .bind(tenant_id)
        .bind(name)
        .bind(description)
        .bind(transport_kind)
        .bind(config)
        .fetch_optional(&state.connect_pool)
        .await?
        .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?
    } else {
        let name = name.ok_or_else(|| AppError::InvalidInput("name is required".to_string()))?;
        let (transport_kind, config) = normalized_transport
            .ok_or_else(|| AppError::InvalidInput("transport is required".to_string()))?;
        sqlx::query(
            r#"
            INSERT INTO mcp_servers (tenant_id, name, description, transport, config, status)
            VALUES ($1, $2, $3, $4, $5, 'active')
            RETURNING id, name, description, transport, config, status,
                      health_status, last_health_check_at, last_discovered_at,
                      consecutive_failures, health_error, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(name)
        .bind(description)
        .bind(transport_kind)
        .bind(config)
        .fetch_one(&state.connect_pool)
        .await?
    };

    let server_id: Uuid = row.try_get("id")?;
    let tools_by_server = load_biwork_mcp_tools(state, tenant_id, &[server_id]).await?;
    biwork_mcp_server_from_row(
        &row,
        tools_by_server.get(&server_id).cloned().unwrap_or_default(),
    )
}

async fn resolve_biwork_mcp_server_id(
    state: &AppState,
    tenant_id: Uuid,
    server_ref: &str,
) -> Result<Uuid, AppError> {
    if let Ok(server_id) = Uuid::parse_str(server_ref) {
        return Ok(server_id);
    }
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM mcp_servers
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND (config->>'biwork_id' = $2 OR name = $2)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(server_ref)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))
}

async fn try_resolve_biwork_mcp_server_id(
    state: &AppState,
    tenant_id: Uuid,
    server_ref: &str,
) -> Result<Option<Uuid>, AppError> {
    match resolve_biwork_mcp_server_id(state, tenant_id, server_ref).await {
        Ok(server_id) => Ok(Some(server_id)),
        Err(AppError::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn resolve_biwork_mcp_test_target(
    state: &AppState,
    tenant_id: Uuid,
    payload: &Value,
) -> Result<(Option<Uuid>, String, Value, Option<String>), AppError> {
    let server_ref =
        value_string(payload, "runtime_scope_id").or_else(|| value_string(payload, "id"));
    if let Some(server_ref) = server_ref.as_deref()
        && let Some(server_id) =
            try_resolve_biwork_mcp_server_id(state, tenant_id, server_ref).await?
    {
        let row = sqlx::query(
            r#"
                SELECT transport, config, secret_ref
                FROM mcp_servers
                WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
                "#,
        )
        .bind(server_id)
        .bind(tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
        .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?;
        return Ok((
            Some(server_id),
            row.try_get("transport")?,
            row.try_get("config")?,
            row.try_get("secret_ref")?,
        ));
    }

    let (transport_kind, config) = normalize_biwork_mcp_transport_payload(payload)?;
    Ok((None, transport_kind, config, None))
}

fn biwork_mcp_discovery_transport(transport_kind: &str) -> &str {
    if transport_kind == "streamable_http" {
        "streamable-http"
    } else {
        transport_kind
    }
}

async fn update_biwork_mcp_test_status(
    state: &AppState,
    tenant_id: Uuid,
    server_id: Uuid,
    status: &str,
    error: Option<&str>,
) -> Result<(), AppError> {
    let error = error.map(|value| value.chars().take(2_000).collect::<String>());
    sqlx::query(
        r#"
        UPDATE mcp_servers
        SET health_status = $3,
            last_health_check_at = CURRENT_TIMESTAMP,
            last_discovered_at = CASE WHEN $3 = 'healthy' THEN CURRENT_TIMESTAMP ELSE last_discovered_at END,
            consecutive_failures = CASE WHEN $3 = 'unhealthy' THEN consecutive_failures + 1 ELSE 0 END,
            health_error = CASE WHEN $3 = 'unhealthy' THEN $4 ELSE NULL END,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(server_id)
    .bind(tenant_id)
    .bind(status)
    .bind(error)
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

async fn load_biwork_mcp_tools(
    state: &AppState,
    tenant_id: Uuid,
    server_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Value>>, AppError> {
    if server_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT mcp_server_id, name, description, schema
        FROM mcp_tools
        WHERE tenant_id = $1
          AND mcp_server_id = ANY($2::uuid[])
          AND status = 'active'
        ORDER BY name ASC
        "#,
    )
    .bind(tenant_id)
    .bind(server_ids)
    .fetch_all(&state.connect_pool)
    .await?;
    let mut tools: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for row in rows {
        let server_id: Uuid = row.try_get("mcp_server_id")?;
        let schema: Value = row.try_get("schema")?;
        tools.entry(server_id).or_default().push(json!({
            "name": row.try_get::<String, _>("name")?,
            "description": row.try_get::<Option<String>, _>("description")?,
            "input_schema": schema
                .get("inputSchema")
                .or_else(|| schema.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
            "_meta": schema.get("_meta").cloned().unwrap_or_else(|| json!({})),
        }));
    }
    Ok(tools)
}

async fn replace_biwork_mcp_tools(
    state: &AppState,
    tenant_id: Uuid,
    server_id: Uuid,
    tools: &[DiscoveredMcpTool],
) -> Result<(), AppError> {
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE mcp_tools
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1 AND mcp_server_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(server_id)
    .execute(&mut *tx)
    .await?;
    for tool in tools {
        sqlx::query(
            r#"
            INSERT INTO mcp_tools
                (tenant_id, mcp_server_id, name, description, schema, schema_hash, status)
            VALUES ($1, $2, $3, $4, $5, $6, 'active')
            ON CONFLICT (mcp_server_id, name)
            DO UPDATE SET description = EXCLUDED.description,
                          schema = EXCLUDED.schema,
                          schema_hash = EXCLUDED.schema_hash,
                          status = 'active',
                          updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(tenant_id)
        .bind(server_id)
        .bind(&tool.name)
        .bind(&tool.description)
        .bind(&tool.schema)
        .bind(&tool.schema_hash)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await.map_err(|_| AppError::DatabaseTransaction)
}

fn discovered_mcp_tool_json(tool: DiscoveredMcpTool) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool
            .schema
            .get("inputSchema")
            .or_else(|| tool.schema.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
        "_meta": tool.schema.get("_meta").cloned().unwrap_or_else(|| json!({})),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_normalization_preserves_original_json() {
        let payload = json!({
            "id": "builtin-image-gen",
            "name": "BiWork Image Generation",
            "transport": {
                "type": "streamable_http",
                "url": "https://example.test/mcp",
                "headers": {"X-Test": "1"}
            },
            "original_json": "{\"url\":\"https://example.test/mcp\"}",
            "builtin": true
        });

        let (transport, config) = normalize_biwork_mcp_transport_payload(&payload).unwrap();
        assert_eq!(transport, "streamable-http");
        assert!(config.get("type").is_none());
        assert_eq!(config["url"], "https://example.test/mcp");
        assert_eq!(config["headers"]["X-Test"], "1");
        assert_eq!(
            config["original_json"],
            "{\"url\":\"https://example.test/mcp\"}"
        );
        assert_eq!(config["builtin"], true);
        assert_eq!(config["biwork_id"], "builtin-image-gen");
    }

    #[test]
    fn connection_failures_preserve_safe_error_categories() {
        let http = biwork_mcp_connection_failure("MCP initialize returned HTTP 400");
        assert_eq!(http["code"], "MCP_HTTP_ERROR");
        assert_eq!(http["details"]["status"], 400);

        let protocol = biwork_mcp_connection_failure(
            "MCP streamable HTTP initialize response did not provide Mcp-Session-Id",
        );
        assert_eq!(protocol["code"], "MCP_PROTOCOL_ERROR");

        let timeout = biwork_mcp_connection_failure("MCP request timed out");
        assert_eq!(timeout["code"], "MCP_TIMEOUT");
    }

    #[test]
    fn oauth_server_url_accepts_only_http_urls() {
        assert_eq!(
            required_mcp_oauth_server_url(&json!({ "server_url": "https://mcp.example.test" }))
                .unwrap(),
            "https://mcp.example.test"
        );
        assert!(matches!(
            required_mcp_oauth_server_url(&json!({ "server_url": "file:///tmp/socket" })),
            Err(AppError::InvalidInput(_))
        ));
        assert!(matches!(
            required_mcp_oauth_server_url(&json!({ "server_url": "not-a-url" })),
            Err(AppError::InvalidInput(_))
        ));
    }
}
