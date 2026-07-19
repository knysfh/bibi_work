use axum::{
    Extension, Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::AuthzContext},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_agent_support::{biwork_agent_type, normalize_biwork_agent_source, runtime_kind},
    biwork_compat_service::{epoch_ms, ok, value_string},
    support::require_ferriskey_allow,
};

#[derive(Debug, Deserialize)]
pub struct CustomAgentPayload {
    name: String,
    command: String,
    icon: Option<String>,
    args: Option<Vec<String>>,
    env: Option<Vec<Value>>,
    advanced: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct AgentEnabledPayload {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct AgentOverridesPayload {
    command_override: Option<String>,
    env_override: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
pub struct AgentMcpCapabilitiesPayload {
    mcp_tool_ids: Vec<Uuid>,
    #[serde(default)]
    browser_enabled: bool,
}

#[derive(Debug)]
struct AgentMcpBindingRow {
    tool_id: Uuid,
    schema_hash_at_publish: Option<String>,
    current_schema_hash: Option<String>,
}

#[derive(Debug)]
struct LatestAgentVersionRow {
    id: Uuid,
    config_snapshot: Value,
}

pub async fn biwork_list_agents_management(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.name, a.description, a.status, a.metadata, a.draft_config,
               a.created_at, a.updated_at
        FROM agents a
        WHERE a.tenant_id = $1 AND a.deleted_at IS NULL
        ORDER BY a.updated_at DESC, a.created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut agents = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row.try_get("id")?;
        let status: String = row.try_get("status")?;
        let metadata: Value = row.try_get("metadata")?;
        let draft_config: Value = row.try_get("draft_config")?;
        let runtime = runtime_kind(&draft_config, &metadata);
        let raw_source = value_string(&metadata, "source");
        let agent_source = normalize_biwork_agent_source(raw_source.as_deref());
        let agent_type = biwork_agent_type(&runtime, &metadata);
        agents.push(json!({
            "id": id.to_string(),
            "name": row.try_get::<String, _>("name")?,
            "description": row.try_get::<Option<String>, _>("description")?,
            "agent_type": agent_type,
            "agent_source": agent_source,
            "acp_backend": runtime,
            "enabled": status != "disabled",
            "installed": true,
            "status": if status == "disabled" { "offline" } else { "online" },
            "created_at": epoch_ms(row.try_get("created_at")?),
            "updated_at": epoch_ms(row.try_get("updated_at")?),
        }));
    }

    Ok(ok(Value::Array(agents)))
}

pub async fn biwork_refresh_custom_agents() -> Result<Json<Value>, AppError> {
    Ok(ok(Value::Null))
}

pub async fn biwork_test_custom_agent(Json(payload): Json<Value>) -> Result<Json<Value>, AppError> {
    let command = payload
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if command.is_empty() {
        return Ok(ok(json!({
            "step": "fail_cli",
            "error": "command is required",
        })));
    }
    if !command_exists(command) {
        return Ok(ok(json!({
            "step": "fail_cli",
            "error": format!("command not found: {command}"),
        })));
    }
    Ok(ok(json!({
        "step": "desktop_runtime_required",
        "error": "ACP handshake is performed by the authenticated desktop runtime when a conversation run starts",
    })))
}

pub async fn biwork_create_custom_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CustomAgentPayload>,
) -> Result<Json<Value>, AppError> {
    let name = payload.name.trim();
    let command = payload.command.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("name is required".to_string()));
    }
    if command.is_empty() {
        return Err(AppError::InvalidInput("command is required".to_string()));
    }
    let (description, metadata, draft_config) = custom_agent_documents(
        name,
        command,
        payload.icon.as_deref(),
        payload.args,
        payload.env,
        payload.advanced,
    );
    let row = sqlx::query(
        r#"
        INSERT INTO agents (
            tenant_id, owner_user_id, name, description, draft_config, metadata, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'draft')
        RETURNING id, name, description, status, metadata, draft_config, created_at, updated_at
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(name)
    .bind(description)
    .bind(draft_config)
    .bind(metadata)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(ok(custom_agent_from_row(&row)?))
}

pub async fn biwork_update_custom_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<CustomAgentPayload>,
) -> Result<Json<Value>, AppError> {
    let name = payload.name.trim();
    let command = payload.command.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("name is required".to_string()));
    }
    if command.is_empty() {
        return Err(AppError::InvalidInput("command is required".to_string()));
    }
    let (description, metadata, draft_config) = custom_agent_documents(
        name,
        command,
        payload.icon.as_deref(),
        payload.args,
        payload.env,
        payload.advanced,
    );
    let row = sqlx::query(
        r#"
        UPDATE agents
        SET name = $4,
            description = $5,
            draft_config = $6,
            metadata = metadata || $7,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
          AND metadata->>'source' = 'custom'
        RETURNING id, name, description, status, metadata, draft_config, created_at, updated_at
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(name)
    .bind(description)
    .bind(draft_config)
    .bind(metadata)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("custom agent not found".to_string()))?;

    Ok(ok(custom_agent_from_row(&row)?))
}

pub async fn biwork_delete_custom_agent(
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
          AND metadata->>'source' = 'custom'
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .execute(&state.connect_pool)
    .await?;

    Ok(ok(json!({ "deleted": result.rows_affected() > 0 })))
}

pub async fn biwork_set_agent_enabled(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<AgentEnabledPayload>,
) -> Result<Json<Value>, AppError> {
    let status = if payload.enabled { "draft" } else { "disabled" };
    let row = sqlx::query(
        r#"
        UPDATE agents
        SET status = $4,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
        RETURNING id, name, description, status, metadata, draft_config, created_at, updated_at
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(status)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;

    Ok(ok(custom_agent_from_row(&row)?))
}

pub async fn biwork_get_agent_overrides(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT draft_config
        FROM agents
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;
    let config: Value = row.try_get("draft_config")?;
    Ok(ok(json!({
        "command_override": config.get("command_override")
            .or_else(|| config.get("command"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "env_override": config.get("env_override")
            .or_else(|| config.get("env"))
            .cloned()
            .unwrap_or_else(|| json!([])),
    })))
}

pub async fn biwork_set_agent_overrides(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<AgentOverridesPayload>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query(
        r#"
        UPDATE agents
        SET draft_config = jsonb_set(
                jsonb_set(
                    draft_config,
                    '{command_override}',
                    to_jsonb($4::text),
                    true
                ),
                '{env_override}',
                $5,
                true
            ),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND owner_user_id = $3
          AND deleted_at IS NULL
        RETURNING id, name, description, status, metadata, draft_config, created_at, updated_at
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(payload.command_override.unwrap_or_default())
    .bind(Value::Array(payload.env_override.unwrap_or_default()))
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;

    Ok(ok(custom_agent_from_row(&row)?))
}

pub async fn biwork_get_agent_mcp_capabilities(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    ensure_agent_manage_access(&state, &ctx, agent_id).await?;

    let version = latest_agent_version(&state.connect_pool, ctx.tenant_id, agent_id).await?;
    let version_id = version.as_ref().map(|version| version.id);
    let browser_enabled = version
        .as_ref()
        .is_some_and(|version| browser_capability_enabled(&version.config_snapshot));
    let bindings = if let Some(version_id) = version_id {
        load_agent_mcp_bindings(&state.connect_pool, ctx.tenant_id, version_id).await?
    } else {
        Vec::new()
    };
    let selected_ids = bindings
        .iter()
        .map(|binding| binding.tool_id)
        .collect::<Vec<_>>();
    let stale_ids = bindings
        .iter()
        .filter(|binding| {
            binding.schema_hash_at_publish.is_none()
                || binding.schema_hash_at_publish != binding.current_schema_hash
        })
        .map(|binding| binding.tool_id)
        .collect::<Vec<_>>();

    let rows = sqlx::query(
        r#"
        SELECT ms.id AS server_id, ms.name AS server_name, ms.description AS server_description,
               mt.id AS tool_id, mt.name AS tool_name, mt.description AS tool_description,
               mt.schema, mt.schema_hash
        FROM mcp_servers ms
        JOIN mcp_tools mt ON mt.mcp_server_id = ms.id
        WHERE ms.tenant_id = $1
          AND ms.status = 'active'
          AND ms.deleted_at IS NULL
          AND mt.tenant_id = $1
          AND mt.status = 'active'
        ORDER BY ms.name ASC, mt.name ASC, mt.id ASC
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut servers: Vec<Value> = Vec::new();
    let selected = selected_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let stale = stale_ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    for row in rows {
        let server_id: Uuid = row.try_get("server_id")?;
        let server_id_string = server_id.to_string();
        let tool_id: Uuid = row.try_get("tool_id")?;
        let schema: Value = row.try_get("schema")?;
        let (risk_level, risk_source, read_only, destructive) =
            mcp_tool_risk(&schema, row.try_get("tool_name")?);
        let tool = json!({
            "id": tool_id,
            "name": row.try_get::<String, _>("tool_name")?,
            "description": row.try_get::<Option<String>, _>("tool_description")?,
            "schema_hash": row.try_get::<Option<String>, _>("schema_hash")?,
            "risk_level": risk_level,
            "risk_source": risk_source,
            "read_only": read_only,
            "destructive": destructive,
            "selected": selected.contains(&tool_id),
            "stale": stale.contains(&tool_id),
        });

        if let Some(server) = servers.iter_mut().find(|server| {
            server.get("id").and_then(Value::as_str) == Some(server_id_string.as_str())
        }) {
            server
                .get_mut("tools")
                .and_then(Value::as_array_mut)
                .expect("agent MCP tools response must keep tools as an array")
                .push(tool);
        } else {
            servers.push(json!({
                "id": server_id,
                "name": row.try_get::<String, _>("server_name")?,
                "description": row.try_get::<Option<String>, _>("server_description")?,
                "tools": [tool],
            }));
        }
    }

    Ok(ok(json!({
        "agent_id": agent_id,
        "agent_version_id": version_id,
        "browser_enabled": browser_enabled,
        "selected_mcp_tool_ids": selected_ids,
        "stale_mcp_tool_ids": stale_ids,
        "servers": servers,
    })))
}

pub async fn biwork_publish_agent_mcp_capabilities(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<AgentMcpCapabilitiesPayload>,
) -> Result<Json<Value>, AppError> {
    ensure_agent_manage_access(&state, &ctx, agent_id).await?;
    let mut selected_ids = payload.mcp_tool_ids;
    let browser_enabled = payload.browser_enabled;
    selected_ids.sort_unstable();
    selected_ids.dedup();

    let mut tx = state.connect_pool.begin().await?;
    let agent_row = sqlx::query(
        r#"
        SELECT draft_config
        FROM agents
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        FOR UPDATE
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;
    let draft_config: Value = agent_row.try_get("draft_config")?;

    let previous = sqlx::query(
        r#"
        SELECT id, config_snapshot, policy_version, schema_hash
        FROM agent_versions
        WHERE tenant_id = $1 AND agent_id = $2 AND status = 'published'
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(agent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let previous_version_id = previous
        .as_ref()
        .map(|row| row.try_get::<Uuid, _>("id"))
        .transpose()?;
    let previous_browser_enabled = previous
        .as_ref()
        .map(|row| row.try_get::<Value, _>("config_snapshot"))
        .transpose()?
        .as_ref()
        .is_some_and(browser_capability_enabled);
    let previous_bindings = if let Some(previous_version_id) = previous_version_id {
        load_agent_mcp_bindings_tx(&mut tx, ctx.tenant_id, previous_version_id).await?
    } else {
        Vec::new()
    };
    let mut previous_ids = previous_bindings
        .iter()
        .map(|binding| binding.tool_id)
        .collect::<Vec<_>>();
    previous_ids.sort_unstable();
    let has_stale_binding = previous_bindings.iter().any(|binding| {
        binding.schema_hash_at_publish.is_none()
            || binding.schema_hash_at_publish != binding.current_schema_hash
    });
    if let Some(previous_version_id) = previous_version_id
        && previous_ids == selected_ids
        && previous_browser_enabled == browser_enabled
        && !has_stale_binding
    {
        let revoked_versions = revoke_unsafe_published_agent_versions(
            &mut tx,
            ctx.tenant_id,
            agent_id,
            previous_version_id,
            &selected_ids,
        )
        .await?;
        tx.commit()
            .await
            .map_err(|_| AppError::DatabaseTransaction)?;
        return Ok(ok(json!({
            "changed": false,
            "agent_id": agent_id,
            "agent_version_id": previous_version_id,
            "browser_enabled": browser_enabled,
            "selected_mcp_tool_ids": selected_ids,
            "previous_version_revoked": revoked_versions > 0,
        })));
    }

    let selected_tools = load_bindable_mcp_tools_tx(&mut tx, ctx.tenant_id, &selected_ids).await?;
    if selected_tools.len() != selected_ids.len() {
        return Err(AppError::InvalidInput(
            "one or more MCP tools are not active in the current tenant".to_string(),
        ));
    }

    let mut config_snapshot = previous
        .as_ref()
        .map(|row| row.try_get::<Value, _>("config_snapshot"))
        .transpose()?
        .unwrap_or(draft_config);
    set_browser_capability(&mut config_snapshot, browser_enabled)?;
    let policy_version = previous
        .as_ref()
        .map(|row| row.try_get::<String, _>("policy_version"))
        .transpose()?
        .unwrap_or_else(|| "local-policy-v1".to_string());
    let schema_hash = previous
        .as_ref()
        .map(|row| row.try_get::<Option<String>, _>("schema_hash"))
        .transpose()?
        .flatten();
    let version_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO agent_versions (
            tenant_id, agent_id, version_label, config_snapshot, policy_version, schema_hash, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'published')
        RETURNING id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(agent_id)
    .bind(format!("agent-capabilities-{}", Uuid::new_v4().simple()))
    .bind(config_snapshot)
    .bind(policy_version)
    .bind(schema_hash)
    .fetch_one(&mut *tx)
    .await?;

    if let Some(previous_version_id) = previous_version_id {
        copy_non_mcp_agent_bindings(&mut tx, previous_version_id, version_id).await?;
    }
    for (tool_id, tool_schema_hash) in selected_tools {
        sqlx::query(
            r#"
            INSERT INTO agent_version_mcp_bindings (
                agent_version_id, mcp_tool_id, schema_hash_at_publish, binding_mode
            )
            VALUES ($1, $2, $3, 'optional')
            "#,
        )
        .bind(version_id)
        .bind(tool_id)
        .bind(tool_schema_hash)
        .execute(&mut *tx)
        .await?;
    }

    let revoked_versions = revoke_unsafe_published_agent_versions(
        &mut tx,
        ctx.tenant_id,
        agent_id,
        version_id,
        &selected_ids,
    )
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(json!({
        "changed": true,
        "agent_id": agent_id,
        "agent_version_id": version_id,
        "browser_enabled": browser_enabled,
        "selected_mcp_tool_ids": selected_ids,
        "previous_version_revoked": revoked_versions > 0,
    })))
}

async fn ensure_agent_manage_access(
    state: &AppState,
    ctx: &PlatformRequestContext,
    agent_id: Uuid,
) -> Result<(), AppError> {
    require_ferriskey_allow(
        state,
        ctx,
        ctx.tenant_id,
        "manage",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await
    .map(|_| ())
}

async fn latest_agent_version(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    agent_id: Uuid,
) -> Result<Option<LatestAgentVersionRow>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, config_snapshot
        FROM agent_versions
        WHERE tenant_id = $1 AND agent_id = $2 AND status = 'published'
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(agent_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(LatestAgentVersionRow {
        id: row.try_get("id")?,
        config_snapshot: row.try_get("config_snapshot")?,
    }))
}

fn browser_capability_enabled(config_snapshot: &Value) -> bool {
    config_snapshot
        .pointer("/capabilities/browser/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn set_browser_capability(config_snapshot: &mut Value, enabled: bool) -> Result<(), AppError> {
    let root = config_snapshot.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("agent config_snapshot must be a JSON object".to_string())
    })?;
    let capabilities = root
        .entry("capabilities")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            AppError::InvalidInput("agent capabilities must be a JSON object".to_string())
        })?;
    let browser = capabilities
        .entry("browser")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            AppError::InvalidInput("agent browser capability must be a JSON object".to_string())
        })?;
    browser.insert("enabled".to_string(), json!(enabled));
    Ok(())
}

async fn load_agent_mcp_bindings(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    version_id: Uuid,
) -> Result<Vec<AgentMcpBindingRow>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT b.mcp_tool_id, b.schema_hash_at_publish, mt.schema_hash AS current_schema_hash
        FROM agent_version_mcp_bindings b
        JOIN mcp_tools mt ON mt.id = b.mcp_tool_id AND mt.tenant_id = $2
        WHERE b.agent_version_id = $1
        ORDER BY b.created_at ASC, b.mcp_tool_id ASC
        "#,
    )
    .bind(version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    binding_rows(rows)
}

async fn load_agent_mcp_bindings_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    version_id: Uuid,
) -> Result<Vec<AgentMcpBindingRow>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT b.mcp_tool_id, b.schema_hash_at_publish, mt.schema_hash AS current_schema_hash
        FROM agent_version_mcp_bindings b
        JOIN mcp_tools mt ON mt.id = b.mcp_tool_id AND mt.tenant_id = $2
        WHERE b.agent_version_id = $1
        ORDER BY b.created_at ASC, b.mcp_tool_id ASC
        "#,
    )
    .bind(version_id)
    .bind(tenant_id)
    .fetch_all(&mut **tx)
    .await?;
    binding_rows(rows)
}

fn binding_rows(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<AgentMcpBindingRow>, AppError> {
    rows.into_iter()
        .map(|row| {
            Ok(AgentMcpBindingRow {
                tool_id: row.try_get("mcp_tool_id")?,
                schema_hash_at_publish: row.try_get("schema_hash_at_publish")?,
                current_schema_hash: row.try_get("current_schema_hash")?,
            })
        })
        .collect()
}

async fn load_bindable_mcp_tools_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    tool_ids: &[Uuid],
) -> Result<Vec<(Uuid, Option<String>)>, AppError> {
    if tool_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"
        SELECT mt.id, mt.schema_hash
        FROM mcp_tools mt
        JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
        WHERE mt.tenant_id = $1
          AND mt.id = ANY($2::uuid[])
          AND mt.status = 'active'
          AND ms.tenant_id = $1
          AND ms.status = 'active'
          AND ms.deleted_at IS NULL
        ORDER BY mt.id ASC
        "#,
    )
    .bind(tenant_id)
    .bind(tool_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| Ok((row.try_get("id")?, row.try_get("schema_hash")?)))
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(AppError::from)
}

async fn copy_non_mcp_agent_bindings(
    tx: &mut Transaction<'_, Postgres>,
    previous_version_id: Uuid,
    version_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO agent_version_skill_bindings (agent_version_id, skill_version_id)
        SELECT $2, skill_version_id
        FROM agent_version_skill_bindings
        WHERE agent_version_id = $1
        "#,
    )
    .bind(previous_version_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO agent_version_tool_bindings (agent_version_id, tool_version_id)
        SELECT $2, tool_version_id
        FROM agent_version_tool_bindings
        WHERE agent_version_id = $1
        "#,
    )
    .bind(previous_version_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO agent_version_sql_tool_bindings (agent_version_id, sql_tool_version_id)
        SELECT $2, sql_tool_version_id
        FROM agent_version_sql_tool_bindings
        WHERE agent_version_id = $1
        "#,
    )
    .bind(previous_version_id)
    .bind(version_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn revoke_unsafe_published_agent_versions(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    agent_id: Uuid,
    safe_version_id: Uuid,
    selected_tool_ids: &[Uuid],
) -> Result<u64, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE agent_versions AS version
        SET status = 'disabled'
        WHERE version.tenant_id = $1
          AND version.agent_id = $2
          AND version.id <> $3
          AND version.status = 'published'
          AND EXISTS (
              SELECT 1
              FROM agent_version_mcp_bindings binding
              LEFT JOIN mcp_tools tool
                ON tool.id = binding.mcp_tool_id
               AND tool.tenant_id = $1
              LEFT JOIN mcp_servers server
                ON server.id = tool.mcp_server_id
               AND server.tenant_id = $1
              WHERE binding.agent_version_id = version.id
                AND (
                    NOT (binding.mcp_tool_id = ANY($4::uuid[]))
                    OR tool.id IS NULL
                    OR tool.status <> 'active'
                    OR server.id IS NULL
                    OR server.status <> 'active'
                    OR server.deleted_at IS NOT NULL
                    OR binding.schema_hash_at_publish IS NULL
                    OR binding.schema_hash_at_publish IS DISTINCT FROM tool.schema_hash
                )
          )
        "#,
    )
    .bind(tenant_id)
    .bind(agent_id)
    .bind(safe_version_id)
    .bind(selected_tool_ids)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

fn mcp_tool_risk(schema: &Value, tool_name: String) -> (&'static str, &'static str, bool, bool) {
    let annotations = schema.get("annotations");
    let destructive = annotations
        .and_then(|value| value.get("destructiveHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let read_only = annotations
        .and_then(|value| value.get("readOnlyHint"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if destructive {
        return ("high", "server_annotation", read_only, true);
    }
    if read_only {
        return ("low", "server_annotation", true, false);
    }
    let lowered = tool_name.to_ascii_lowercase();
    if [
        "delete", "drop", "remove", "truncate", "execute", "write", "update",
    ]
    .iter()
    .any(|needle| lowered.contains(needle))
    {
        ("high", "name_heuristic", false, false)
    } else if ["get", "list", "read", "search", "find", "status"]
        .iter()
        .any(|needle| lowered.starts_with(needle))
    {
        ("medium", "name_heuristic", false, false)
    } else {
        ("high", "default_untrusted", false, false)
    }
}

pub async fn biwork_check_managed_agent_health(
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
          AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;
    let mut agent = custom_agent_from_row(&row)?;
    let status: String = row.try_get("status")?;
    let config: Value = row.try_get("draft_config")?;
    let command = value_string(&config, "command")
        .or_else(|| value_string(&config, "command_override"))
        .unwrap_or_default();
    let check = agent_health_snapshot(&status, &command);
    merge_json_object(&mut agent, check);
    Ok(ok(agent))
}

fn custom_agent_documents(
    name: &str,
    command: &str,
    icon: Option<&str>,
    args: Option<Vec<String>>,
    env: Option<Vec<Value>>,
    advanced: Option<Value>,
) -> (Option<String>, Value, Value) {
    let advanced = advanced
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let description = advanced
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let icon = icon
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("🤖");
    let env = Value::Array(env.unwrap_or_default());
    let args = json!(args.unwrap_or_default());

    let metadata = json!({
        "source": "custom",
        "assistant_source": "custom",
        "avatar": icon,
        "icon": icon,
        "runtime_kind": "biwork_cli",
        "custom_agent": true,
    });
    let draft_config = json!({
        "name": name,
        "acp_backend": "biwork_cli",
        "runtime": { "kind": "biwork_cli" },
        "command": command,
        "args": args,
        "env": env,
        "advanced": advanced,
    });
    (description, metadata, draft_config)
}

fn custom_agent_from_row(row: &sqlx::postgres::PgRow) -> Result<Value, AppError> {
    let id: Uuid = row.try_get("id")?;
    let name: String = row.try_get("name")?;
    let description: Option<String> = row.try_get("description")?;
    let status: String = row.try_get("status")?;
    let metadata: Value = row.try_get("metadata")?;
    let config: Value = row.try_get("draft_config")?;
    let advanced = config
        .get("advanced")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let command = value_string(&config, "command_override")
        .or_else(|| value_string(&config, "command"))
        .unwrap_or_default();
    let env = config
        .get("env_override")
        .or_else(|| config.get("env"))
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let args = config
        .get("args")
        .cloned()
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([]));
    let enabled = status != "disabled";
    let installed = !command.is_empty() && command_exists(&command);
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    let raw_source = value_string(&metadata, "source");
    let agent_source = normalize_biwork_agent_source(raw_source.as_deref().or(Some("custom")));

    Ok(json!({
        "id": id.to_string(),
        "custom_agent_id": id.to_string(),
        "name": name,
        "description": description.unwrap_or_default(),
        "icon": metadata.get("icon")
            .or_else(|| metadata.get("avatar"))
            .cloned()
            .unwrap_or_else(|| json!("🤖")),
        "avatar": metadata.get("avatar")
            .or_else(|| metadata.get("icon"))
            .cloned()
            .unwrap_or_else(|| json!("🤖")),
        "backend": runtime_kind(&config, &metadata),
        "acp_backend": runtime_kind(&config, &metadata),
        "agent_type": "acp",
        "agent_source": agent_source,
        "enabled": enabled,
        "available": installed,
        "installed": installed,
        "status": if enabled && installed { "unchecked" } else { "offline" },
        "command": command,
        "args": args,
        "env": env,
        "native_skills_dirs": advanced
            .get("native_skills_dirs")
            .cloned()
            .unwrap_or_else(|| json!([])),
        "behavior_policy": advanced
            .get("behavior_policy")
            .cloned()
            .unwrap_or_else(|| json!({})),
        "yolo_id": advanced.get("yolo_id").cloned().unwrap_or(Value::Null),
        "team_capable": false,
        "has_command_override": config
            .get("command_override")
            .and_then(Value::as_str)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false),
        "env_override_key_count": config
            .get("env_override")
            .and_then(Value::as_array)
            .map(|values| values.len())
            .unwrap_or(0),
        "last_check_status": if installed { Value::Null } else { json!("offline") },
        "created_at": epoch_ms(created_at),
        "updated_at": epoch_ms(updated_at),
    }))
}

fn agent_health_snapshot(status: &str, command: &str) -> Value {
    let checked_at = epoch_ms(OffsetDateTime::now_utc());
    if status == "disabled" {
        return json!({
            "status": "offline",
            "last_check_status": "offline",
            "last_check_kind": "manual",
            "last_check_error_code": "disabled",
            "last_check_error_message": "agent is disabled",
            "last_check_at": checked_at,
            "last_failure_at": checked_at,
        });
    }
    if command.trim().is_empty() {
        return json!({
            "status": "offline",
            "last_check_status": "offline",
            "last_check_kind": "manual",
            "last_check_error_code": "no_command",
            "last_check_error_message": "agent command is empty",
            "last_check_at": checked_at,
            "last_failure_at": checked_at,
        });
    }
    if !command_exists(command) {
        return json!({
            "status": "missing",
            "installed": false,
            "available": false,
            "last_check_status": "offline",
            "last_check_kind": "manual",
            "last_check_error_code": "command_not_found",
            "last_check_error_message": format!("command not found: {command}"),
            "last_check_error_details": { "command": command },
            "last_check_at": checked_at,
            "last_failure_at": checked_at,
        });
    }
    json!({
        "status": "unchecked",
        "installed": true,
        "available": true,
        "last_check_status": "unchecked",
        "last_check_kind": "manual",
        "last_check_error_code": Value::Null,
        "last_check_error_message": Value::Null,
        "last_check_error_details": { "command": command },
        "last_check_at": checked_at,
        "last_failure_at": checked_at,
    })
}

fn merge_json_object(target: &mut Value, patch: Value) {
    let Some(target_object) = target.as_object_mut() else {
        return;
    };
    if let Some(patch_object) = patch.as_object() {
        for (key, value) in patch_object {
            target_object.insert(key.clone(), value.clone());
        }
    }
}

fn command_exists(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }
    let command_path = std::path::Path::new(command);
    if command_path.components().count() > 1 {
        return command_path.is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join(command).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_risk_prefers_server_annotations_and_fails_closed() {
        assert_eq!(
            mcp_tool_risk(
                &json!({"annotations": {"readOnlyHint": true}}),
                "read_rows".to_string()
            ),
            ("low", "server_annotation", true, false)
        );
        assert_eq!(
            mcp_tool_risk(
                &json!({"annotations": {"destructiveHint": true}}),
                "cleanup".to_string()
            ),
            ("high", "server_annotation", false, true)
        );
        assert_eq!(
            mcp_tool_risk(&json!({}), "delete_database".to_string()),
            ("high", "name_heuristic", false, false)
        );
        assert_eq!(
            mcp_tool_risk(&json!({}), "custom_operation".to_string()),
            ("high", "default_untrusted", false, false)
        );
    }

    #[test]
    fn browser_capability_defaults_off_and_preserves_other_config() {
        let mut snapshot = json!({
            "runtime": {"kind": "deepagents"},
            "capabilities": {"memory": {"enabled": true}}
        });
        assert!(!browser_capability_enabled(&snapshot));

        set_browser_capability(&mut snapshot, true).unwrap();

        assert!(browser_capability_enabled(&snapshot));
        assert_eq!(snapshot["runtime"]["kind"], json!("deepagents"));
        assert_eq!(snapshot["capabilities"]["memory"]["enabled"], json!(true));
    }
}
