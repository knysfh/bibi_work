use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::features::core::errors::AppError;

use super::ferriskey_oidc::PlatformRequestContext;

const LOCAL_POLICY_VERSION: &str = "local-policy-v1";
const LOCAL_RISK_POLICY_VERSION: &str = "local-risk-v1";

#[derive(Debug)]
pub struct CompiledRunSnapshot {
    pub agent_id: Option<Uuid>,
    pub agent_version_id: Option<Uuid>,
    pub snapshot: Value,
    pub scope_snapshot: Value,
}

pub struct ConversationRunSnapshotRequest<'a> {
    pub tenant_id: Uuid,
    pub conversation_id: Uuid,
    pub run_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub requested_agent_id: Option<Uuid>,
    pub agent_version_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub thread_id: Option<String>,
    pub client_snapshot: Option<Value>,
    pub ctx: &'a PlatformRequestContext,
}

pub struct WorkflowNodeRunSnapshotRequest<'a> {
    pub tenant_id: Uuid,
    pub run_id: Uuid,
    pub workflow_run_id: Uuid,
    pub workflow_version_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub node_run_id: Uuid,
    pub node_key: &'a str,
    pub node: &'a Value,
    pub node_permission_snapshot: Value,
    pub actor_user_id: Uuid,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
    pub project_id: Option<Uuid>,
    pub thread_id: String,
}

#[derive(Debug)]
struct AgentVersionSnapshot {
    agent_id: Uuid,
    agent_version_id: Uuid,
    policy_version: String,
    schema_hash: Option<String>,
    snapshot: Value,
}

#[derive(Debug)]
struct ModelProfileSnapshot {
    model_profile_id: Uuid,
    provider_id: Uuid,
    provider_key: String,
    provider_name: String,
    base_url: Option<String>,
    credential_id: Option<Uuid>,
    secret_ref: Option<String>,
    model_name: String,
    context_window: Option<i64>,
    max_input_tokens: Option<i64>,
    max_output_tokens: Option<i64>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    reasoning_effort: Option<String>,
    response_format: Value,
    tool_choice_policy: Value,
    rate_limit_policy: Value,
    cost_policy: Value,
}

pub async fn compile_conversation_run_snapshot(
    pool: &PgPool,
    request: ConversationRunSnapshotRequest<'_>,
) -> Result<CompiledRunSnapshot, AppError> {
    let agent_version = load_optional_agent_version(
        pool,
        request.tenant_id,
        request.agent_version_id,
        request.requested_agent_id,
    )
    .await?;
    let mut snapshot = base_snapshot(
        agent_version.as_ref(),
        request.client_snapshot,
        request.requested_agent_id,
        request.agent_version_id,
        request.project_id,
    )?;

    apply_agent_version_snapshot(
        pool,
        request.tenant_id,
        &mut snapshot,
        agent_version.as_ref(),
    )
    .await?;
    apply_default_model_snapshot(
        pool,
        request.tenant_id,
        &mut snapshot,
        agent_version.as_ref(),
    )
    .await?;
    let scope_snapshot = load_workspace_scope_snapshot(
        pool,
        request.tenant_id,
        request.workspace_id,
        request.project_id,
        request.ctx.platform_user_id,
        request.ctx.device_id,
    )
    .await?;
    merge_runtime_options(&mut snapshot, request.thread_id);
    insert_common_runtime_fields(
        &mut snapshot,
        request.tenant_id,
        request.run_id,
        Some(request.conversation_id),
        request.workspace_id,
        request.project_id,
        &scope_snapshot,
    )?;
    insert_actor_from_context(&mut snapshot, request.ctx)?;

    Ok(CompiledRunSnapshot {
        agent_id: agent_version
            .as_ref()
            .map(|version| version.agent_id)
            .or(request.requested_agent_id),
        agent_version_id: agent_version
            .as_ref()
            .map(|version| version.agent_version_id)
            .or(request.agent_version_id),
        snapshot,
        scope_snapshot,
    })
}

pub async fn compile_workflow_node_run_snapshot(
    pool: &PgPool,
    request: WorkflowNodeRunSnapshotRequest<'_>,
) -> Result<Value, AppError> {
    let agent_version =
        load_agent_version(pool, request.tenant_id, request.agent_version_id).await?;
    if agent_version.agent_id != request.agent_id {
        return Err(AppError::InvalidInput(
            "workflow node agent_id does not match agent_version".to_string(),
        ));
    }

    let mut snapshot = base_snapshot(
        Some(&agent_version),
        None,
        Some(request.agent_id),
        Some(request.agent_version_id),
        request.project_id,
    )?;
    apply_agent_version_snapshot(pool, request.tenant_id, &mut snapshot, Some(&agent_version))
        .await?;
    insert_common_runtime_fields(
        &mut snapshot,
        request.tenant_id,
        request.run_id,
        request.conversation_id,
        request.workspace_id,
        request.project_id,
        &empty_workspace_scope(request.workspace_id, request.project_id),
    )?;
    insert_actor_from_user_id(&mut snapshot, request.actor_user_id)?;
    merge_runtime_options(&mut snapshot, Some(request.thread_id));

    let object = snapshot_object_mut(&mut snapshot)?;
    object.insert(
        "workflow".to_string(),
        json!({
            "workflow_run_id": request.workflow_run_id,
            "workflow_version_id": request.workflow_version_id,
            "node_key": request.node_key,
            "node_run_id": request.node_run_id,
            "node_permission_snapshot": request.node_permission_snapshot
        }),
    );
    object.insert("node".to_string(), request.node.clone());

    Ok(snapshot)
}

async fn load_optional_agent_version(
    pool: &PgPool,
    tenant_id: Uuid,
    agent_version_id: Option<Uuid>,
    requested_agent_id: Option<Uuid>,
) -> Result<Option<AgentVersionSnapshot>, AppError> {
    let Some(agent_version_id) = agent_version_id else {
        return Ok(None);
    };
    let version = load_agent_version(pool, tenant_id, agent_version_id).await?;
    if let Some(requested_agent_id) = requested_agent_id
        && requested_agent_id != version.agent_id
    {
        return Err(AppError::InvalidInput(
            "agent_id does not match agent_version_id".to_string(),
        ));
    }
    Ok(Some(version))
}

async fn load_agent_version(
    pool: &PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<AgentVersionSnapshot, AppError> {
    let row = sqlx::query(
        r#"
        SELECT agent_id, policy_version, schema_hash, config_snapshot
        FROM agent_versions
        WHERE id = $1
          AND tenant_id = $2
          AND status = 'published'
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent version not found".to_string()))?;

    Ok(AgentVersionSnapshot {
        agent_id: row.try_get("agent_id")?,
        agent_version_id,
        policy_version: row.try_get("policy_version")?,
        schema_hash: row.try_get("schema_hash")?,
        snapshot: row.try_get("config_snapshot")?,
    })
}

fn base_snapshot(
    agent_version: Option<&AgentVersionSnapshot>,
    client_snapshot: Option<Value>,
    requested_agent_id: Option<Uuid>,
    requested_agent_version_id: Option<Uuid>,
    project_id: Option<Uuid>,
) -> Result<Value, AppError> {
    if let Some(version) = agent_version {
        if !version.snapshot.is_object() {
            return Err(AppError::InvalidInput(
                "agent version config_snapshot must be a JSON object".to_string(),
            ));
        }
        let mut snapshot = version.snapshot.clone();
        merge_client_runtime_options(&mut snapshot, client_snapshot);
        return Ok(snapshot);
    }

    let snapshot = client_snapshot.unwrap_or_else(|| {
        json!({
            "agent_id": requested_agent_id,
            "agent_version_id": requested_agent_version_id,
            "project_id": project_id,
            "policy_version": LOCAL_POLICY_VERSION,
            "risk_policy_version": LOCAL_RISK_POLICY_VERSION
        })
    });
    if snapshot.is_object() {
        Ok(snapshot)
    } else {
        Err(AppError::InvalidInput(
            "run_config_snapshot must be a JSON object".to_string(),
        ))
    }
}

async fn apply_agent_version_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    snapshot: &mut Value,
    agent_version: Option<&AgentVersionSnapshot>,
) -> Result<(), AppError> {
    let Some(agent_version) = agent_version else {
        return Ok(());
    };
    let model_profile_id = model_profile_id(snapshot).ok_or_else(|| {
        AppError::InvalidInput("agent version snapshot must include model_profile_id".to_string())
    })?;
    let model = load_model_profile(pool, tenant_id, model_profile_id).await?;
    let tools = load_tool_snapshots(pool, agent_version.agent_version_id).await?;
    let skills = load_skill_snapshots(pool, agent_version.agent_version_id).await?;
    let mcp_tools = load_mcp_tool_snapshots(pool, agent_version.agent_version_id).await?;

    let model_json = model.to_runtime_json();
    let object = snapshot_object_mut(snapshot)?;
    object.insert("model".to_string(), model_json.clone());
    object.insert(
        "model_profile_id".to_string(),
        json!(model.model_profile_id),
    );
    object.insert("tools".to_string(), Value::Array(tools));
    object.insert("skills".to_string(), Value::Array(skills));
    object.insert("mcp_tools".to_string(), Value::Array(mcp_tools));
    object.insert(
        "policy_version".to_string(),
        json!(agent_version.policy_version),
    );
    object.insert(
        "risk_policy_version".to_string(),
        json!(LOCAL_RISK_POLICY_VERSION),
    );

    let mut agent_object = object
        .remove("agent")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    agent_object.insert("agent_id".to_string(), json!(agent_version.agent_id));
    agent_object.insert(
        "agent_version_id".to_string(),
        json!(agent_version.agent_version_id),
    );
    agent_object.insert(
        "model_profile_id".to_string(),
        json!(model.model_profile_id),
    );
    agent_object.insert("model".to_string(), model_json);
    agent_object.insert(
        "policy_version".to_string(),
        json!(agent_version.policy_version),
    );
    agent_object.insert("schema_hash".to_string(), json!(agent_version.schema_hash));
    object.insert("agent".to_string(), Value::Object(agent_object));

    Ok(())
}

async fn apply_default_model_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    snapshot: &mut Value,
    agent_version: Option<&AgentVersionSnapshot>,
) -> Result<(), AppError> {
    if agent_version.is_some() || snapshot_has_model(snapshot) {
        return Ok(());
    }

    let model = if let Some(model_profile_id) = model_profile_id(snapshot) {
        load_model_profile(pool, tenant_id, model_profile_id).await?
    } else {
        let profile_name = default_model_profile_name()?;
        load_model_profile_by_name(pool, tenant_id, &profile_name).await?
    };
    insert_model_snapshot(snapshot, &model)
}

fn snapshot_has_model(snapshot: &Value) -> bool {
    snapshot.get("model").is_some_and(|value| !value.is_null())
        || snapshot
            .pointer("/agent/model")
            .is_some_and(|value| !value.is_null())
}

fn default_model_profile_name() -> Result<String, AppError> {
    let profile_name = std::env::var("DEFAULT_MODEL")
        .map_err(|_| {
            AppError::InvalidInput(
                "run requires agent_version_id, model_profile_id, or DEFAULT_MODEL".to_string(),
            )
        })?
        .trim()
        .to_string();
    if profile_name.is_empty() {
        return Err(AppError::InvalidInput(
            "DEFAULT_MODEL must name an active LLM model profile".to_string(),
        ));
    }
    Ok(profile_name)
}

async fn load_model_profile(
    pool: &PgPool,
    tenant_id: Uuid,
    model_profile_id: Uuid,
) -> Result<ModelProfileSnapshot, AppError> {
    let row = sqlx::query(
        r#"
        SELECT mp.id AS model_profile_id, mp.provider_id, p.provider_key,
               p.display_name AS provider_name, p.base_url, mp.credential_id,
               c.secret_ref, mp.model_name, mp.context_window, mp.max_input_tokens,
               mp.max_output_tokens, mp.temperature, mp.top_p, mp.reasoning_effort,
               mp.response_format, mp.tool_choice_policy, mp.rate_limit_policy, mp.cost_policy
        FROM llm_model_profiles mp
        JOIN llm_providers p ON p.id = mp.provider_id
        LEFT JOIN llm_credentials c ON c.id = mp.credential_id
        WHERE mp.id = $1
          AND mp.tenant_id = $2
          AND mp.status = 'active'
          AND p.status = 'active'
          AND (c.id IS NULL OR c.revoked_at IS NULL)
        "#,
    )
    .bind(model_profile_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::InvalidInput("model_profile_id is not active".to_string()))?;

    Ok(ModelProfileSnapshot {
        model_profile_id: row.try_get("model_profile_id")?,
        provider_id: row.try_get("provider_id")?,
        provider_key: row.try_get("provider_key")?,
        provider_name: row.try_get("provider_name")?,
        base_url: row.try_get("base_url")?,
        credential_id: row.try_get("credential_id")?,
        secret_ref: row.try_get("secret_ref")?,
        model_name: row.try_get("model_name")?,
        context_window: row.try_get("context_window")?,
        max_input_tokens: row.try_get("max_input_tokens")?,
        max_output_tokens: row.try_get("max_output_tokens")?,
        temperature: row.try_get("temperature")?,
        top_p: row.try_get("top_p")?,
        reasoning_effort: row.try_get("reasoning_effort")?,
        response_format: row.try_get("response_format")?,
        tool_choice_policy: row.try_get("tool_choice_policy")?,
        rate_limit_policy: row.try_get("rate_limit_policy")?,
        cost_policy: row.try_get("cost_policy")?,
    })
}

async fn load_model_profile_by_name(
    pool: &PgPool,
    tenant_id: Uuid,
    profile_name: &str,
) -> Result<ModelProfileSnapshot, AppError> {
    let row = sqlx::query(
        r#"
        SELECT mp.id AS model_profile_id, mp.provider_id, p.provider_key,
               p.display_name AS provider_name, p.base_url, mp.credential_id,
               c.secret_ref, mp.model_name, mp.context_window, mp.max_input_tokens,
               mp.max_output_tokens, mp.temperature, mp.top_p, mp.reasoning_effort,
               mp.response_format, mp.tool_choice_policy, mp.rate_limit_policy, mp.cost_policy
        FROM llm_model_profiles mp
        JOIN llm_providers p ON p.id = mp.provider_id
        LEFT JOIN llm_credentials c ON c.id = mp.credential_id
        WHERE mp.profile_name = $1
          AND mp.tenant_id = $2
          AND mp.status = 'active'
          AND p.status = 'active'
          AND (c.id IS NULL OR c.revoked_at IS NULL)
        "#,
    )
    .bind(profile_name)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| {
        AppError::InvalidInput(format!(
            "DEFAULT_MODEL '{profile_name}' does not reference an active LLM model profile"
        ))
    })?;

    Ok(model_profile_from_row(row)?)
}

fn model_profile_from_row(row: sqlx::postgres::PgRow) -> Result<ModelProfileSnapshot, sqlx::Error> {
    Ok(ModelProfileSnapshot {
        model_profile_id: row.try_get("model_profile_id")?,
        provider_id: row.try_get("provider_id")?,
        provider_key: row.try_get("provider_key")?,
        provider_name: row.try_get("provider_name")?,
        base_url: row.try_get("base_url")?,
        credential_id: row.try_get("credential_id")?,
        secret_ref: row.try_get("secret_ref")?,
        model_name: row.try_get("model_name")?,
        context_window: row.try_get("context_window")?,
        max_input_tokens: row.try_get("max_input_tokens")?,
        max_output_tokens: row.try_get("max_output_tokens")?,
        temperature: row.try_get("temperature")?,
        top_p: row.try_get("top_p")?,
        reasoning_effort: row.try_get("reasoning_effort")?,
        response_format: row.try_get("response_format")?,
        tool_choice_policy: row.try_get("tool_choice_policy")?,
        rate_limit_policy: row.try_get("rate_limit_policy")?,
        cost_policy: row.try_get("cost_policy")?,
    })
}

fn insert_model_snapshot(
    snapshot: &mut Value,
    model: &ModelProfileSnapshot,
) -> Result<(), AppError> {
    let model_json = model.to_runtime_json();
    let object = snapshot_object_mut(snapshot)?;
    object.insert("model".to_string(), model_json);
    object.insert(
        "model_profile_id".to_string(),
        json!(model.model_profile_id),
    );
    Ok(())
}

async fn load_tool_snapshots(
    pool: &PgPool,
    agent_version_id: Uuid,
) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT tv.id AS tool_version_id, tv.tool_id, t.name, t.tool_type,
               tv.schema_hash, tv.schema_snapshot
        FROM agent_version_tool_bindings b
        JOIN tool_versions tv ON tv.id = b.tool_version_id
        JOIN tools t ON t.id = tv.tool_id
        WHERE b.agent_version_id = $1
          AND tv.status = 'published'
          AND t.status = 'active'
        ORDER BY b.created_at ASC, tv.id ASC
        "#,
    )
    .bind(agent_version_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            Ok(json!({
                "tool_version_id": row.try_get::<Uuid, _>("tool_version_id")?,
                "tool_id": row.try_get::<Uuid, _>("tool_id")?,
                "name": row.try_get::<String, _>("name")?,
                "tool_type": row.try_get::<String, _>("tool_type")?,
                "schema_hash": row.try_get::<Option<String>, _>("schema_hash")?,
                "schema": row.try_get::<Value, _>("schema_snapshot")?,
                "risk_level": "low"
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?)
}

async fn load_skill_snapshots(
    pool: &PgPool,
    agent_version_id: Uuid,
) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT sv.id AS skill_version_id, sv.skill_id, s.name, sv.content_hash,
               sv.source_uri, sv.manifest
        FROM agent_version_skill_bindings b
        JOIN skill_versions sv ON sv.id = b.skill_version_id
        JOIN skills s ON s.id = sv.skill_id
        WHERE b.agent_version_id = $1
          AND sv.status = 'published'
          AND s.status = 'active'
        ORDER BY b.created_at ASC, sv.id ASC
        "#,
    )
    .bind(agent_version_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            Ok(json!({
                "skill_version_id": row.try_get::<Uuid, _>("skill_version_id")?,
                "skill_id": row.try_get::<Uuid, _>("skill_id")?,
                "name": row.try_get::<String, _>("name")?,
                "content_hash": row.try_get::<Option<String>, _>("content_hash")?,
                "source_uri": row.try_get::<Option<String>, _>("source_uri")?,
                "manifest": row.try_get::<Value, _>("manifest")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?)
}

async fn load_mcp_tool_snapshots(
    pool: &PgPool,
    agent_version_id: Uuid,
) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT mt.id AS mcp_tool_id, mt.mcp_server_id, ms.name AS server_name,
               mt.name AS tool_name, mt.schema_hash, mt.schema
        FROM agent_version_mcp_bindings b
        JOIN mcp_tools mt ON mt.id = b.mcp_tool_id
        JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
        WHERE b.agent_version_id = $1
          AND mt.status = 'active'
          AND ms.status = 'active'
        ORDER BY b.created_at ASC, mt.id ASC
        "#,
    )
    .bind(agent_version_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            Ok(json!({
                "mcp_tool_id": row.try_get::<Uuid, _>("mcp_tool_id")?,
                "server_id": row.try_get::<Uuid, _>("mcp_server_id")?,
                "server_name": row.try_get::<String, _>("server_name")?,
                "tool_name": row.try_get::<String, _>("tool_name")?,
                "schema_hash": row.try_get::<Option<String>, _>("schema_hash")?,
                "schema": row.try_get::<Value, _>("schema")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?)
}

fn insert_common_runtime_fields(
    snapshot: &mut Value,
    tenant_id: Uuid,
    run_id: Uuid,
    conversation_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    scope_snapshot: &Value,
) -> Result<(), AppError> {
    let object = snapshot_object_mut(snapshot)?;
    object.insert("tenant_id".to_string(), json!(tenant_id));
    object.insert("run_id".to_string(), json!(run_id));
    object.insert("conversation_id".to_string(), json!(conversation_id));
    object.insert("workspace_id".to_string(), json!(workspace_id));
    object.insert("project_id".to_string(), json!(project_id));
    object.insert("workspace".to_string(), scope_snapshot.clone());
    object.insert(
        "local_mount_ids".to_string(),
        scope_snapshot
            .get("local_mount_ids")
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    object.insert(
        "policy_version".to_string(),
        object
            .get("policy_version")
            .cloned()
            .unwrap_or_else(|| json!(LOCAL_POLICY_VERSION)),
    );
    object.insert(
        "risk_policy_version".to_string(),
        object
            .get("risk_policy_version")
            .cloned()
            .unwrap_or_else(|| json!(LOCAL_RISK_POLICY_VERSION)),
    );
    object.insert("memory_context".to_string(), json!([]));
    object.insert(
        "memory_context_meta".to_string(),
        json!({
            "source": "not_retrieved",
            "enabled": true,
            "reason": "pending"
        }),
    );
    Ok(())
}

async fn load_workspace_scope_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    user_id: Uuid,
    device_id: Uuid,
) -> Result<Value, AppError> {
    let Some(workspace_id) = workspace_id else {
        return Ok(empty_workspace_scope(None, project_id));
    };
    let workspace = sqlx::query(
        r#"
        SELECT id, remote_project_id, file_policy, include_globs, exclude_globs, trust_state
        FROM workspaces
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workspace not found".to_string()))?;
    let mount_rows = sqlx::query(
        r#"
        SELECT id, display_name, virtual_path, capabilities, include_globs, exclude_globs,
               trust_state
        FROM local_mounts
        WHERE tenant_id = $1
          AND workspace_id = $2
          AND user_id = $3
          AND device_id = $4
          AND status = 'active'
        ORDER BY virtual_path ASC
        "#,
    )
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(user_id)
    .bind(device_id)
    .fetch_all(pool)
    .await?;
    let mut local_mount_ids = Vec::with_capacity(mount_rows.len());
    let mut local_mounts = Vec::with_capacity(mount_rows.len());
    for row in mount_rows {
        let local_mount_id: Uuid = row.try_get("id")?;
        local_mount_ids.push(local_mount_id);
        local_mounts.push(json!({
            "local_mount_id": local_mount_id,
            "display_name": row.try_get::<String, _>("display_name")?,
            "virtual_path": row.try_get::<String, _>("virtual_path")?,
            "capabilities": row.try_get::<Value, _>("capabilities")?,
            "include_globs": row.try_get::<Value, _>("include_globs")?,
            "exclude_globs": row.try_get::<Value, _>("exclude_globs")?,
            "trust_state": row.try_get::<String, _>("trust_state")?
        }));
    }
    let workspace_project_id: Option<Uuid> = workspace.try_get("remote_project_id")?;
    let effective_project_id = project_id.or(workspace_project_id);

    Ok(json!({
        "workspace_id": workspace_id,
        "remote_project_id": effective_project_id,
        "local_mount_ids": local_mount_ids.clone(),
        "local_mounts": local_mounts,
        "file_policy": workspace.try_get::<Value, _>("file_policy")?,
        "include_globs": workspace.try_get::<Value, _>("include_globs")?,
        "exclude_globs": workspace.try_get::<Value, _>("exclude_globs")?,
        "trust_state": workspace.try_get::<String, _>("trust_state")?,
        "virtual_roots": virtual_roots(),
        "permission_result": {
            "remote_project_id": effective_project_id,
            "local_mount_ids": local_mount_ids,
            "policy_version": LOCAL_POLICY_VERSION
        }
    }))
}

fn empty_workspace_scope(workspace_id: Option<Uuid>, project_id: Option<Uuid>) -> Value {
    json!({
        "workspace_id": workspace_id,
        "remote_project_id": project_id,
        "local_mount_ids": [],
        "local_mounts": [],
        "file_policy": {},
        "include_globs": [],
        "exclude_globs": [],
        "trust_state": "unscoped",
        "virtual_roots": virtual_roots(),
        "permission_result": {
            "remote_project_id": project_id,
            "local_mount_ids": [],
            "policy_version": LOCAL_POLICY_VERSION
        }
    })
}

fn virtual_roots() -> Value {
    json!({
        "workspace": "/workspace/",
        "local_main": "/local/main/",
        "scratch": "/scratch/",
        "artifacts": "/artifacts/"
    })
}

fn insert_actor_from_context(
    snapshot: &mut Value,
    ctx: &PlatformRequestContext,
) -> Result<(), AppError> {
    let object = snapshot_object_mut(snapshot)?;
    object.insert(
        "actor".to_string(),
        json!({
            "user_id": ctx.platform_user_id,
            "device_id": ctx.device_id,
            "session_id": ctx.session_id,
            "roles": ctx.roles.clone(),
            "ferriskey_subject": ctx.ferriskey_subject.clone(),
            "preferred_username": ctx.preferred_username.clone(),
            "email": ctx.email.clone()
        }),
    );
    Ok(())
}

fn insert_actor_from_user_id(snapshot: &mut Value, user_id: Uuid) -> Result<(), AppError> {
    let object = snapshot_object_mut(snapshot)?;
    object.insert(
        "actor".to_string(),
        json!({
            "user_id": user_id,
            "roles": []
        }),
    );
    Ok(())
}

fn merge_runtime_options(snapshot: &mut Value, thread_id: Option<String>) {
    if let Some(thread_id) = thread_id
        && let Some(object) = snapshot.as_object_mut()
    {
        object.insert("thread_id".to_string(), json!(thread_id));
    }
}

fn merge_client_runtime_options(snapshot: &mut Value, client_snapshot: Option<Value>) {
    let Some(client_object) = client_snapshot.and_then(|value| value.as_object().cloned()) else {
        return;
    };
    let Some(snapshot_object) = snapshot.as_object_mut() else {
        return;
    };
    for key in [
        "memory_retrieval",
        "memory_query",
        "permissions",
        "interrupt_on",
        "file_mounts",
    ] {
        if let Some(value) = client_object.get(key) {
            snapshot_object.insert(key.to_string(), value.clone());
        }
    }
}

fn model_profile_id(snapshot: &Value) -> Option<Uuid> {
    snapshot
        .get("model_profile_id")
        .or_else(|| {
            snapshot
                .get("agent")
                .and_then(|agent| agent.get("model_profile_id"))
        })
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn snapshot_object_mut(snapshot: &mut Value) -> Result<&mut Map<String, Value>, AppError> {
    snapshot.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("run_config_snapshot must be a JSON object".to_string())
    })
}

impl ModelProfileSnapshot {
    fn to_runtime_json(&self) -> Value {
        json!({
            "model_profile_id": self.model_profile_id,
            "provider_id": self.provider_id,
            "provider": self.provider_key,
            "provider_name": self.provider_name,
            "model_name": self.model_name,
            "base_url": self.base_url,
            "context_window": self.context_window,
            "max_input_tokens": self.max_input_tokens,
            "max_output_tokens": self.max_output_tokens,
            "parameters": {
                "temperature": self.temperature,
                "top_p": self.top_p,
                "max_output_tokens": self.max_output_tokens,
                "reasoning_effort": self.reasoning_effort,
                "response_format": self.response_format,
                "tool_choice_policy": self.tool_choice_policy
            },
            "credential": {
                "credential_id": self.credential_id,
                "has_secret_ref": self.secret_ref.is_some(),
                "runtime_credential_id": Value::Null
            },
            "rate_limit_policy": self.rate_limit_policy,
            "cost_policy": self.cost_policy
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_profile_id_supports_top_level_and_agent_snapshot() {
        let id = Uuid::new_v4();
        assert_eq!(
            model_profile_id(&json!({"model_profile_id": id.to_string()})),
            Some(id)
        );
        assert_eq!(
            model_profile_id(&json!({"agent": {"model_profile_id": id.to_string()}})),
            Some(id)
        );
    }

    #[test]
    fn base_snapshot_rejects_non_object_client_snapshot() {
        let err = base_snapshot(None, Some(json!("bad")), None, None, None).unwrap_err();
        assert!(err.to_string().contains("run_config_snapshot"));
    }

    #[test]
    fn model_profile_runtime_json_redacts_secret_ref() {
        let snapshot = ModelProfileSnapshot {
            model_profile_id: Uuid::new_v4(),
            provider_id: Uuid::new_v4(),
            provider_key: "openai-compatible".to_string(),
            provider_name: "OpenAI Compatible".to_string(),
            base_url: Some("https://llm.example.test".to_string()),
            credential_id: Some(Uuid::new_v4()),
            secret_ref: Some("vault://tenant/provider/key".to_string()),
            model_name: "test-model".to_string(),
            context_window: Some(8192),
            max_input_tokens: Some(4096),
            max_output_tokens: Some(1024),
            temperature: Some(0.2),
            top_p: Some(1.0),
            reasoning_effort: None,
            response_format: json!({}),
            tool_choice_policy: json!({}),
            rate_limit_policy: json!({}),
            cost_policy: json!({}),
        };

        let runtime = snapshot.to_runtime_json();
        let credential = runtime
            .get("credential")
            .and_then(Value::as_object)
            .expect("credential object");

        assert_eq!(credential.get("has_secret_ref"), Some(&json!(true)));
        assert_eq!(credential.get("runtime_credential_id"), Some(&Value::Null));
        assert!(!credential.contains_key("secret_ref"));
    }

    #[test]
    fn insert_model_snapshot_sets_top_level_runtime_model() {
        let model = ModelProfileSnapshot {
            model_profile_id: Uuid::new_v4(),
            provider_id: Uuid::new_v4(),
            provider_key: "openai-compatible".to_string(),
            provider_name: "OpenAI Compatible".to_string(),
            base_url: Some("http://llm.example.test".to_string()),
            credential_id: Some(Uuid::new_v4()),
            secret_ref: Some("env://COMPATIBLE_API_KEY".to_string()),
            model_name: "minimax-m2.5".to_string(),
            context_window: None,
            max_input_tokens: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            reasoning_effort: None,
            response_format: json!({}),
            tool_choice_policy: json!({}),
            rate_limit_policy: json!({}),
            cost_policy: json!({}),
        };
        let mut snapshot = json!({});

        insert_model_snapshot(&mut snapshot, &model).unwrap();

        assert_eq!(snapshot["model_profile_id"], json!(model.model_profile_id));
        assert_eq!(snapshot["model"]["provider"], json!("openai-compatible"));
        assert_eq!(snapshot["model"]["model_name"], json!("minimax-m2.5"));
        assert_eq!(
            snapshot["model"]["credential"]["credential_id"],
            json!(model.credential_id)
        );
    }

    #[test]
    fn insert_common_runtime_fields_records_workspace_scope() {
        let tenant_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let workspace_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let local_mount_id = Uuid::new_v4();
        let scope = json!({
            "workspace_id": workspace_id,
            "remote_project_id": project_id,
            "local_mount_ids": [local_mount_id],
            "local_mounts": [{
                "local_mount_id": local_mount_id,
                "virtual_path": "/local/main/",
                "capabilities": ["read"]
            }],
            "virtual_roots": virtual_roots()
        });
        let mut snapshot = json!({});

        insert_common_runtime_fields(
            &mut snapshot,
            tenant_id,
            run_id,
            Some(conversation_id),
            Some(workspace_id),
            Some(project_id),
            &scope,
        )
        .unwrap();

        assert_eq!(snapshot["workspace_id"], json!(workspace_id));
        assert_eq!(snapshot["project_id"], json!(project_id));
        assert_eq!(snapshot["local_mount_ids"], json!([local_mount_id]));
        assert_eq!(snapshot["workspace"], scope);
    }
}
