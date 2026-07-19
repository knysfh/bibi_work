use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{agent_platform::ferriskey_oidc::PlatformRequestContext, core::errors::AppError},
    startup::AppState,
};

use super::biwork_compat_service::{biwork_failure, epoch_ms, ok, required_string, value_string};

#[derive(Debug, Deserialize)]
pub struct RemoteAgentPayload {
    name: Option<String>,
    protocol: Option<String>,
    url: Option<String>,
    auth_type: Option<String>,
    auth_token: Option<String>,
    allow_insecure: Option<bool>,
    avatar: Option<String>,
    description: Option<String>,
}

pub async fn biwork_list_remote_agents(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, description, status, metadata, draft_config, created_at, updated_at
        FROM agents
        WHERE tenant_id = $1
          AND owner_user_id = $2
          AND deleted_at IS NULL
          AND metadata->>'source' = 'remote'
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let items = rows
        .iter()
        .map(remote_agent_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(ok(json!(items)))
}

pub async fn biwork_get_remote_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, name, description, status, metadata, draft_config, created_at, updated_at
        FROM agents
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
          AND metadata->>'source' = 'remote'
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("remote agent not found".to_string()))?;
    Ok(ok(remote_agent_from_row(&row)?))
}

pub async fn biwork_create_remote_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<RemoteAgentPayload>,
) -> Result<Json<Value>, AppError> {
    let docs = remote_agent_documents_for_create(payload)?;
    let row = sqlx::query(
        r#"
        INSERT INTO agents (
            tenant_id, owner_user_id, name, description, draft_config, metadata, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'active')
        RETURNING id, name, description, status, metadata, draft_config, created_at, updated_at
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(docs.name)
    .bind(docs.description)
    .bind(docs.config)
    .bind(docs.metadata)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(ok(remote_agent_from_row(&row)?))
}

pub async fn biwork_update_remote_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<RemoteAgentPayload>,
) -> Result<Json<Value>, AppError> {
    let current = sqlx::query(
        r#"
        SELECT name, description, metadata, draft_config
        FROM agents
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
          AND metadata->>'source' = 'remote'
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("remote agent not found".to_string()))?;

    let docs = remote_agent_documents_for_update(&current, payload)?;
    let row = sqlx::query(
        r#"
        UPDATE agents
        SET name = $4,
            description = $5,
            draft_config = $6,
            metadata = $7,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
          AND metadata->>'source' = 'remote'
        RETURNING id, name, description, status, metadata, draft_config, created_at, updated_at
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(docs.name)
    .bind(docs.description)
    .bind(docs.config)
    .bind(docs.metadata)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("remote agent not found".to_string()))?;

    Ok(ok(remote_agent_from_row(&row)?))
}

pub async fn biwork_delete_remote_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE agents
        SET deleted_at = CURRENT_TIMESTAMP,
            status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
          AND metadata->>'source' = 'remote'
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;

    Ok(ok(json!(result.rows_affected() > 0)))
}

pub async fn biwork_test_remote_agent_connection(
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let url = required_string(&payload, "url")?;
    let auth_type = required_string(&payload, "auth_type")?;
    validate_remote_url(&url)?;
    validate_remote_auth_type(&auth_type)?;
    if auth_type != "none" {
        let token = payload
            .get("auth_token")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if token.is_empty() {
            return Ok(ok(biwork_failure(
                "VALIDATION_ERROR",
                "auth_token is required for authenticated remote agents",
                json!({ "field": "auth_token" }),
            )));
        }
    }
    Ok(ok(biwork_failure(
        "REMOTE_AGENT_LOCAL_RUNTIME_REQUIRED",
        "remote agent live connection probe requires the desktop/network runtime",
        json!({ "auth_type": auth_type }),
    )))
}

pub async fn biwork_remote_agent_handshake(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agents
            WHERE id = $1
              AND tenant_id = $2
              AND owner_user_id = $3
              AND deleted_at IS NULL
              AND metadata->>'source' = 'remote'
        )
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if !exists {
        return Err(AppError::NotFound("remote agent not found".to_string()));
    }
    Ok(ok(json!({
        "status": "error",
        "error": "remote agent handshake requires the desktop/network runtime",
    })))
}

struct RemoteAgentDocuments {
    name: String,
    description: Option<String>,
    metadata: Value,
    config: Value,
}

fn remote_agent_documents_for_create(
    payload: RemoteAgentPayload,
) -> Result<RemoteAgentDocuments, AppError> {
    let name = payload
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("name is required".to_string()))?
        .to_string();
    let protocol = payload
        .protocol
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("protocol is required".to_string()))?
        .to_string();
    let url = payload
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("url is required".to_string()))?
        .to_string();
    let auth_type = payload
        .auth_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("auth_type is required".to_string()))?
        .to_string();
    validate_remote_protocol(&protocol)?;
    validate_remote_auth_type(&auth_type)?;
    validate_remote_url(&url)?;
    Ok(build_remote_agent_documents(RemoteAgentDocumentInput {
        name,
        description: payload.description,
        protocol,
        url,
        auth_type,
        auth_token: payload.auth_token,
        allow_insecure: payload.allow_insecure.unwrap_or(false),
        avatar: payload.avatar,
        status: None,
    }))
}

fn remote_agent_documents_for_update(
    row: &sqlx::postgres::PgRow,
    payload: RemoteAgentPayload,
) -> Result<RemoteAgentDocuments, AppError> {
    let current_name: String = row.try_get("name")?;
    let current_description: Option<String> = row.try_get("description")?;
    let current_config: Value = row.try_get("draft_config")?;
    let current_metadata: Value = row.try_get("metadata")?;
    let name = payload
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or(current_name);
    let protocol = payload
        .protocol
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| value_string(&current_config, "protocol"))
        .unwrap_or_else(|| "acp".to_string());
    let url = payload
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| value_string(&current_config, "url"))
        .ok_or_else(|| AppError::InvalidInput("url is required".to_string()))?;
    let auth_type = payload
        .auth_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| value_string(&current_config, "auth_type"))
        .unwrap_or_else(|| "none".to_string());
    validate_remote_protocol(&protocol)?;
    validate_remote_auth_type(&auth_type)?;
    validate_remote_url(&url)?;
    let auth_token = payload
        .auth_token
        .or_else(|| value_string(&current_config, "auth_token"));
    let allow_insecure = payload.allow_insecure.unwrap_or_else(|| {
        current_config
            .get("allow_insecure")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    });
    let avatar = payload
        .avatar
        .or_else(|| value_string(&current_metadata, "avatar"));
    let description = if payload.description.is_some() {
        payload.description
    } else {
        current_description
    };
    let status = value_string(&current_config, "status");
    Ok(build_remote_agent_documents(RemoteAgentDocumentInput {
        name,
        description,
        protocol,
        url,
        auth_type,
        auth_token,
        allow_insecure,
        avatar,
        status,
    }))
}

struct RemoteAgentDocumentInput {
    name: String,
    description: Option<String>,
    protocol: String,
    url: String,
    auth_type: String,
    auth_token: Option<String>,
    allow_insecure: bool,
    avatar: Option<String>,
    status: Option<String>,
}

fn build_remote_agent_documents(input: RemoteAgentDocumentInput) -> RemoteAgentDocuments {
    let RemoteAgentDocumentInput {
        name,
        description,
        protocol,
        url,
        auth_type,
        auth_token,
        allow_insecure,
        avatar,
        status,
    } = input;
    let avatar = avatar
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("🌐")
        .to_string();
    let status = status.unwrap_or_else(|| "unknown".to_string());
    RemoteAgentDocuments {
        name,
        description,
        metadata: json!({
            "source": "remote",
            "assistant_source": "remote",
            "avatar": avatar,
            "icon": avatar,
            "runtime_kind": "remote",
        }),
        config: json!({
            "protocol": protocol,
            "url": url,
            "auth_type": auth_type,
            "auth_token": auth_token,
            "allow_insecure": allow_insecure,
            "avatar": avatar,
            "status": status,
        }),
    }
}

fn remote_agent_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let id: Uuid = row.try_get("id")?;
    let name: String = row.try_get("name")?;
    let description: Option<String> = row.try_get("description")?;
    let metadata: Value = row.try_get("metadata")?;
    let config: Value = row.try_get("draft_config")?;
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(json!({
        "id": id.to_string(),
        "name": name,
        "protocol": value_string(&config, "protocol").unwrap_or_else(|| "acp".to_string()),
        "url": value_string(&config, "url").unwrap_or_default(),
        "auth_type": value_string(&config, "auth_type").unwrap_or_else(|| "none".to_string()),
        "auth_token": config.get("auth_token").cloned().unwrap_or(Value::Null),
        "allow_insecure": config.get("allow_insecure").and_then(Value::as_bool).unwrap_or(false),
        "avatar": value_string(&config, "avatar")
            .or_else(|| value_string(&metadata, "avatar"))
            .unwrap_or_else(|| "🌐".to_string()),
        "description": description.unwrap_or_default(),
        "device_id": config.get("device_id").cloned().unwrap_or(Value::Null),
        "device_public_key": config.get("device_public_key").cloned().unwrap_or(Value::Null),
        "device_private_key": config.get("device_private_key").cloned().unwrap_or(Value::Null),
        "device_token": config.get("device_token").cloned().unwrap_or(Value::Null),
        "status": value_string(&config, "status").unwrap_or_else(|| "unknown".to_string()),
        "last_connected_at": config.get("last_connected_at").cloned().unwrap_or(Value::Null),
        "created_at": epoch_ms(created_at),
        "updated_at": epoch_ms(updated_at),
    }))
}

fn validate_remote_protocol(protocol: &str) -> Result<(), AppError> {
    match protocol {
        "openclaw" | "zeroclaw" | "acp" => Ok(()),
        _ => Err(AppError::InvalidInput(format!(
            "unsupported remote agent protocol: {protocol}"
        ))),
    }
}

fn validate_remote_auth_type(auth_type: &str) -> Result<(), AppError> {
    match auth_type {
        "bearer" | "password" | "none" => Ok(()),
        _ => Err(AppError::InvalidInput(format!(
            "unsupported remote agent auth_type: {auth_type}"
        ))),
    }
}

fn validate_remote_url(url: &str) -> Result<(), AppError> {
    let url = url.trim();
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("ws://")
        || url.starts_with("wss://")
    {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "remote agent url must start with http://, https://, ws://, or wss://".to_string(),
        ))
    }
}
