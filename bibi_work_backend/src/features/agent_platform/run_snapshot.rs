use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::features::core::errors::AppError;

use super::ferriskey_oidc::PlatformRequestContext;

const LOCAL_POLICY_VERSION: &str = "local-policy-v1";
const LOCAL_RISK_POLICY_VERSION: &str = "local-risk-v1";
const EXTENSION_GOVERNANCE_POLICY_VERSION: &str = "biwork-extension-v1";
const RUNTIME_EXTENSION_CONTRIBUTION_TYPES: &[&str] = &[
    "assistant",
    "agent",
    "skill",
    "mcp_server",
    "channel_plugin",
    "acp_adapter",
];
pub const PYTHON_RUNTIME_KIND: &str = "deepagents";
pub const DESKTOP_ACP_RUNTIME_KIND: &str = "biwork_cli";

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
    pub selected_model_profile_id: Option<Uuid>,
    pub selected_mcp_server_ids: Vec<Uuid>,
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
    runtime_id: Uuid,
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
    auth_scheme: String,
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

    if agent_version
        .as_ref()
        .is_some_and(|version| assistant_model_mode(&version.snapshot) == Some("auto"))
        && request.selected_model_profile_id.is_none()
    {
        return Err(AppError::InvalidInput(
            "assistant uses automatic model selection; select an active model for this conversation"
                .to_string(),
        ));
    }
    if let Some(model_profile_id) = request.selected_model_profile_id {
        override_snapshot_model_profile_id(&mut snapshot, model_profile_id)?;
    }

    apply_agent_version_snapshot(
        pool,
        request.tenant_id,
        &mut snapshot,
        agent_version.as_ref(),
    )
    .await?;
    apply_execution_runtime_snapshot(
        pool,
        request.tenant_id,
        &mut snapshot,
        agent_version.as_ref(),
        request.requested_agent_id,
    )
    .await?;
    apply_selected_mcp_server_snapshot(&mut snapshot, &request.selected_mcp_server_ids)?;
    apply_default_model_snapshot(
        pool,
        request.tenant_id,
        &mut snapshot,
        agent_version.as_ref(),
    )
    .await?;
    apply_extension_contribution_snapshot(
        pool,
        request.tenant_id,
        request.ctx.device_id,
        &mut snapshot,
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
    let browser_enabled = agent_browser_capability_enabled(&snapshot);
    insert_actor_from_context(&mut snapshot, request.ctx, browser_enabled)?;
    ensure_runtime_contract_fields(&mut snapshot)?;

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
    apply_execution_runtime_snapshot(
        pool,
        request.tenant_id,
        &mut snapshot,
        Some(&agent_version),
        Some(request.agent_id),
    )
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
    ensure_runtime_contract_fields(&mut snapshot)?;

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

pub fn ensure_python_dispatch_runtime(snapshot: &Value) -> Result<(), AppError> {
    let kind = execution_runtime_kind(snapshot)?;

    match kind {
        PYTHON_RUNTIME_KIND => Ok(()),
        DESKTOP_ACP_RUNTIME_KIND => Err(AppError::Conflict(
            "runtime.kind=biwork_cli is handled by desktop local runtime and must not be dispatched to Python".to_string(),
        )),
        "disabled" => Err(AppError::Conflict(
            "runtime.kind=disabled is catalog-visible but not runnable".to_string(),
        )),
        other => Err(AppError::InvalidInput(format!(
            "runtime.kind={other} is not supported by Python dispatch"
        ))),
    }
}

pub fn execution_runtime_kind(snapshot: &Value) -> Result<&str, AppError> {
    snapshot
        .pointer("/runtime/kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput("run_config_snapshot.runtime.kind is required".to_string())
        })
}

async fn apply_execution_runtime_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    snapshot: &mut Value,
    agent_version: Option<&AgentVersionSnapshot>,
    requested_agent_id: Option<Uuid>,
) -> Result<(), AppError> {
    let runtime_id = if let Some(version) = agent_version {
        Some(version.runtime_id)
    } else if let Some(assistant_id) = requested_agent_id {
        sqlx::query_scalar(
            r#"
            SELECT runtime_id
            FROM assistants
            WHERE id = $1
              AND tenant_id = $2
              AND status <> 'disabled'
              AND deleted_at IS NULL
            "#,
        )
        .bind(assistant_id)
        .bind(tenant_id)
        .fetch_optional(pool)
        .await?
    } else {
        None
    };
    let Some(runtime_id) = runtime_id else {
        return Ok(());
    };
    let row = sqlx::query(
        r#"
        SELECT runtime_kind, draft_config, capabilities, metadata, status
        FROM agent_runtimes
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(runtime_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("execution runtime not found".to_string()))?;
    let status: String = row.try_get("status")?;
    if status == "disabled" {
        return Err(AppError::Conflict(
            "execution runtime is disabled".to_string(),
        ));
    }
    let runtime_kind: String = row.try_get("runtime_kind")?;
    let draft_config: Value = row.try_get("draft_config")?;
    let capabilities: Value = row.try_get("capabilities")?;
    let mut runtime = draft_config
        .get("runtime")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    runtime
        .as_object_mut()
        .expect("runtime snapshot is an object")
        .insert("kind".to_string(), json!(runtime_kind));
    if runtime_kind == DESKTOP_ACP_RUNTIME_KIND {
        let command = draft_config
            .get("command_override")
            .or_else(|| draft_config.get("command"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidInput("desktop ACP runtime command is required".to_string())
            })?;
        let args = draft_config
            .get("args")
            .cloned()
            .filter(Value::is_array)
            .unwrap_or_else(|| json!([]));
        let env = draft_config
            .get("env_override")
            .or_else(|| draft_config.get("env"))
            .cloned()
            .filter(Value::is_array)
            .unwrap_or_else(|| json!([]));
        let runtime_object = runtime
            .as_object_mut()
            .expect("runtime snapshot is an object");
        runtime_object.insert("command".to_string(), json!(command));
        runtime_object.insert("args".to_string(), args);
        runtime_object.insert("env".to_string(), sanitize_snapshot_value(env));
    }
    let runtime_mcp_tools = load_runtime_mcp_tool_snapshots(pool, tenant_id, &capabilities).await?;
    let object = snapshot_object_mut(snapshot)?;
    object.insert("runtime_id".to_string(), json!(runtime_id));
    object.insert("runtime".to_string(), runtime);
    object.insert("capabilities".to_string(), capabilities);
    let mut mcp_tools = object
        .remove("mcp_tools")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    for tool in runtime_mcp_tools {
        let tool_id = tool.get("mcp_tool_id").cloned();
        if !mcp_tools
            .iter()
            .any(|existing| existing.get("mcp_tool_id") == tool_id.as_ref())
        {
            mcp_tools.push(tool);
        }
    }
    object.insert("mcp_tools".to_string(), Value::Array(mcp_tools));
    let agent = object
        .entry("agent")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            AppError::InvalidInput("run snapshot agent must be an object".to_string())
        })?;
    agent.insert("runtime_id".to_string(), json!(runtime_id));
    Ok(())
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
        SELECT assistant_id, runtime_id, policy_version, schema_hash, config_snapshot
        FROM assistant_versions
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
        agent_id: row.try_get("assistant_id")?,
        agent_version_id,
        runtime_id: row.try_get("runtime_id")?,
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

    let mut snapshot = client_snapshot
        .map(sanitize_snapshot_value)
        .unwrap_or_else(|| {
            json!({
                "agent_id": requested_agent_id,
                "agent_version_id": requested_agent_version_id,
                "project_id": project_id,
                "policy_version": LOCAL_POLICY_VERSION,
                "risk_policy_version": LOCAL_RISK_POLICY_VERSION
            })
        });
    if !snapshot.is_object() {
        Err(AppError::InvalidInput(
            "run_config_snapshot must be a JSON object".to_string(),
        ))?;
    }
    insert_server_run_identity(
        &mut snapshot,
        requested_agent_id,
        requested_agent_version_id,
        project_id,
    )?;
    Ok(snapshot)
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
    let sql_tools = load_sql_tool_snapshots(pool, agent_version.agent_version_id).await?;

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
    object.insert("sql_tools".to_string(), Value::Array(sql_tools));
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
    if agent_version.is_some()
        || snapshot_has_model(snapshot)
        || snapshot.pointer("/runtime/kind").and_then(Value::as_str)
            == Some(DESKTOP_ACP_RUNTIME_KIND)
    {
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
               p.display_name AS provider_name, p.base_url, p.auth_scheme, mp.credential_id,
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
        auth_scheme: row.try_get("auth_scheme")?,
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

pub async fn resolve_active_model_profile_reference(
    pool: &PgPool,
    tenant_id: Uuid,
    reference: &str,
) -> Result<Uuid, AppError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(AppError::InvalidInput(
            "conversation model selection is empty".to_string(),
        ));
    }
    if let Ok(model_profile_id) = Uuid::parse_str(reference) {
        load_model_profile(pool, tenant_id, model_profile_id).await?;
        return Ok(model_profile_id);
    }

    let rows = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT mp.id
        FROM llm_model_profiles mp
        JOIN llm_providers provider
          ON provider.id = mp.provider_id
         AND provider.tenant_id = mp.tenant_id
         AND provider.status = 'active'
        LEFT JOIN llm_credentials credential ON credential.id = mp.credential_id
        WHERE mp.tenant_id = $1
          AND mp.status = 'active'
          AND (credential.id IS NULL OR credential.revoked_at IS NULL)
          AND (mp.profile_name = $2 OR mp.model_name = $2)
        ORDER BY (mp.profile_name = $2) DESC, mp.created_at ASC, mp.id ASC
        LIMIT 2
        "#,
    )
    .bind(tenant_id)
    .bind(reference)
    .fetch_all(pool)
    .await?;
    match rows.as_slice() {
        [] => Err(AppError::InvalidInput(format!(
            "selected model '{reference}' is not active; choose another configured model"
        ))),
        [model_profile_id] => Ok(*model_profile_id),
        _ => Err(AppError::InvalidInput(format!(
            "selected model '{reference}' is ambiguous; choose a provider-specific model"
        ))),
    }
}

async fn load_model_profile_by_name(
    pool: &PgPool,
    tenant_id: Uuid,
    profile_name: &str,
) -> Result<ModelProfileSnapshot, AppError> {
    let row = sqlx::query(
        r#"
        SELECT mp.id AS model_profile_id, mp.provider_id, p.provider_key,
               p.display_name AS provider_name, p.base_url, p.auth_scheme, mp.credential_id,
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
        auth_scheme: row.try_get("auth_scheme")?,
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
            let schema = row.try_get::<Value, _>("schema_snapshot")?;
            let risk_level = tool_runtime_risk_level(&schema);
            let requires_approval = schema
                .get("requires_approval")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(json!({
                "tool_version_id": row.try_get::<Uuid, _>("tool_version_id")?,
                "tool_id": row.try_get::<Uuid, _>("tool_id")?,
                "name": row.try_get::<String, _>("name")?,
                "tool_type": row.try_get::<String, _>("tool_type")?,
                "schema_hash": row.try_get::<Option<String>, _>("schema_hash")?,
                "schema": schema,
                "risk_level": risk_level,
                "requires_approval": requires_approval
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?)
}

fn tool_runtime_risk_level(schema: &Value) -> &'static str {
    let configured = schema.get("risk_level").and_then(Value::as_str);
    let risk_level = match configured {
        None | Some("low") => "low",
        Some("medium") => "medium",
        Some("high") => "high",
        Some("critical") => "critical",
        Some(_) => "high",
    };
    if schema
        .get("requires_approval")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && matches!(risk_level, "low" | "medium")
    {
        "high"
    } else {
        risk_level
    }
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
          AND b.schema_hash_at_publish IS NOT NULL
          AND b.schema_hash_at_publish IS NOT DISTINCT FROM mt.schema_hash
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

async fn load_runtime_mcp_tool_snapshots(
    pool: &PgPool,
    tenant_id: Uuid,
    capabilities: &Value,
) -> Result<Vec<Value>, AppError> {
    let bindings = capabilities
        .get("mcp_tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|binding| {
            let tool_id = binding
                .get("id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())?;
            let schema_hash = binding
                .get("schema_hash")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Some((tool_id, schema_hash))
        })
        .collect::<std::collections::HashMap<_, _>>();
    if bindings.is_empty() {
        return Ok(Vec::new());
    }
    let tool_ids = bindings.keys().copied().collect::<Vec<_>>();
    let rows = sqlx::query(
        r#"
        SELECT tool.id AS mcp_tool_id, tool.mcp_server_id,
               server.name AS server_name, tool.name AS tool_name,
               tool.schema_hash, tool.schema
        FROM mcp_tools tool
        JOIN mcp_servers server
          ON server.id = tool.mcp_server_id
         AND server.tenant_id = tool.tenant_id
        WHERE tool.tenant_id = $1
          AND tool.id = ANY($2::uuid[])
          AND tool.status = 'active'
          AND server.status = 'active'
          AND server.deleted_at IS NULL
        ORDER BY tool.id ASC
        "#,
    )
    .bind(tenant_id)
    .bind(&tool_ids)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .filter_map(|row| {
            let tool_id = row.try_get::<Uuid, _>("mcp_tool_id").ok()?;
            let current_hash = row.try_get::<Option<String>, _>("schema_hash").ok()?;
            (bindings.get(&tool_id) == Some(&current_hash)).then_some((row, tool_id))
        })
        .map(|(row, tool_id)| {
            Ok(json!({
                "mcp_tool_id": tool_id,
                "server_id": row.try_get::<Uuid, _>("mcp_server_id")?,
                "server_name": row.try_get::<String, _>("server_name")?,
                "tool_name": row.try_get::<String, _>("tool_name")?,
                "schema_hash": row.try_get::<Option<String>, _>("schema_hash")?,
                "schema": row.try_get::<Value, _>("schema")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(AppError::from)
}

fn apply_selected_mcp_server_snapshot(
    snapshot: &mut Value,
    server_ids: &[Uuid],
) -> Result<(), AppError> {
    if server_ids.is_empty() {
        return Ok(());
    }

    let object = snapshot_object_mut(snapshot)?;
    let allowed_tools = object
        .remove("mcp_tools")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|tool| {
            tool.get("server_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_some_and(|server_id| server_ids.contains(&server_id))
        })
        .collect::<Vec<_>>();
    object.insert("mcp_tools".to_string(), Value::Array(allowed_tools));
    object.insert("selected_mcp_server_ids".to_string(), json!(server_ids));
    Ok(())
}

async fn load_sql_tool_snapshots(
    pool: &PgPool,
    agent_version_id: Uuid,
) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT stv.id AS sql_tool_version_id, stv.sql_tool_id, st.name,
               stv.connection_id, stv.operation, stv.query_hash,
               stv.parameter_schema, stv.risk_level, stv.requires_approval
        FROM agent_version_sql_tool_bindings b
        JOIN sql_tool_versions stv ON stv.id = b.sql_tool_version_id
        JOIN sql_tools st ON st.id = stv.sql_tool_id
        JOIN sql_connections sc ON sc.id = stv.connection_id
        WHERE b.agent_version_id = $1
          AND stv.status = 'published'
          AND st.status = 'active'
          AND sc.status = 'active'
        ORDER BY b.created_at ASC, st.name ASC
        "#,
    )
    .bind(agent_version_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            Ok(json!({
                "sql_tool_id": row.try_get::<Uuid, _>("sql_tool_id")?,
                "sql_tool_version_id": row.try_get::<Uuid, _>("sql_tool_version_id")?,
                "connection_id": row.try_get::<Uuid, _>("connection_id")?,
                "name": row.try_get::<String, _>("name")?,
                "operation": row.try_get::<String, _>("operation")?,
                "query_hash": row.try_get::<String, _>("query_hash")?,
                "parameter_schema": row.try_get::<Value, _>("parameter_schema")?,
                "risk_level": row.try_get::<String, _>("risk_level")?,
                "requires_approval": row.try_get::<bool, _>("requires_approval")?
            }))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?)
}

async fn apply_extension_contribution_snapshot(
    pool: &PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    snapshot: &mut Value,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        r#"
        SELECT p.id AS extension_package_id, p.extension_name, p.source, p.version,
               p.risk_level, c.contribution_type, c.contribution_key, c.manifest
        FROM extension_contributions c
        JOIN extension_packages p
          ON p.id = c.extension_package_id AND p.tenant_id = c.tenant_id
        JOIN device_extension_states s
          ON s.extension_package_id = c.extension_package_id
         AND s.tenant_id = c.tenant_id
         AND s.device_id = $2
        WHERE c.tenant_id = $1
          AND c.enabled = TRUE
          AND c.contribution_type IN (
              'assistant', 'agent', 'skill', 'mcp_server', 'channel_plugin', 'acp_adapter'
          )
          AND s.installed = TRUE
          AND s.enabled = TRUE
          AND s.install_status = 'installed'
          AND p.status IN ('discovered', 'approved')
        ORDER BY p.extension_name, c.contribution_type, c.contribution_key
        LIMIT 500
        "#,
    )
    .bind(tenant_id)
    .bind(device_id)
    .fetch_all(pool)
    .await?;

    let contributions = rows
        .into_iter()
        .map(|row| {
            Ok(runtime_extension_contribution_json(
                RuntimeExtensionContribution {
                    extension_package_id: row.try_get("extension_package_id")?,
                    extension_name: row.try_get("extension_name")?,
                    source: row.try_get("source")?,
                    version: row.try_get("version")?,
                    risk_level: row.try_get("risk_level")?,
                    contribution_type: row.try_get("contribution_type")?,
                    contribution_key: row.try_get("contribution_key")?,
                    manifest: row.try_get("manifest")?,
                },
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    let object = snapshot_object_mut(snapshot)?;
    object.insert(
        "extension_contributions".to_string(),
        Value::Array(contributions),
    );
    object.insert(
        "extension_contribution_meta".to_string(),
        json!({
            "source": "rust_governance",
            "policy_version": EXTENSION_GOVERNANCE_POLICY_VERSION
        }),
    );
    Ok(())
}

struct RuntimeExtensionContribution {
    extension_package_id: Uuid,
    extension_name: String,
    source: String,
    version: Option<String>,
    risk_level: String,
    contribution_type: String,
    contribution_key: String,
    manifest: Value,
}

fn runtime_extension_contribution_json(
    contribution: RuntimeExtensionContribution,
) -> Option<Value> {
    if !is_runtime_extension_contribution_type(&contribution.contribution_type) {
        return None;
    }

    Some(json!({
        "extension_package_id": contribution.extension_package_id,
        "extension_name": contribution.extension_name,
        "source": contribution.source,
        "version": contribution.version,
        "risk_level": contribution.risk_level,
        "type": contribution.contribution_type,
        "key": contribution.contribution_key,
        "manifest": sanitize_extension_contribution_manifest(contribution.manifest)
    }))
}

fn is_runtime_extension_contribution_type(contribution_type: &str) -> bool {
    RUNTIME_EXTENSION_CONTRIBUTION_TYPES.contains(&contribution_type)
}

fn sanitize_extension_contribution_manifest(value: Value) -> Value {
    sanitize_snapshot_value(value)
}

fn sanitize_snapshot_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sanitized = Map::new();
            for (key, item) in map {
                if is_sensitive_snapshot_key(&key) {
                    continue;
                }
                sanitized.insert(key, sanitize_snapshot_value(item));
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(sanitize_snapshot_value).collect())
        }
        other => other,
    }
}

fn is_sensitive_snapshot_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    matches!(
        normalized.as_str(),
        "apikey"
            | "api_key"
            | "accesstoken"
            | "access_token"
            | "authorization"
            | "password"
            | "refreshtoken"
            | "refresh_token"
            | "secret"
            | "secretref"
            | "secret_ref"
            | "token"
    )
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
    object.entry("runtime".to_string()).or_insert_with(|| {
        json!({
            "kind": "deepagents"
        })
    });
    object.insert("workspace".to_string(), scope_snapshot.clone());
    object.entry("ui".to_string()).or_insert_with(|| {
        json!({
            "client": "biwork",
            "conversation_type": "acp"
        })
    });
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
    browser_enabled: bool,
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
    object.remove("browser");
    if browser_enabled {
        object.insert(
            "browser".to_string(),
            json!({
                "enabled": true,
                "execution": "local",
                "visible": true,
                "device_id": ctx.device_id
            }),
        );
    }
    Ok(())
}

fn agent_browser_capability_enabled(snapshot: &Value) -> bool {
    snapshot
        .pointer("/capabilities/browser/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
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
        "ui",
        "cron",
        "channel",
        "team",
    ] {
        if let Some(value) = client_object.get(key) {
            snapshot_object.insert(key.to_string(), sanitize_snapshot_value(value.clone()));
        }
    }
}

fn ensure_runtime_contract_fields(snapshot: &mut Value) -> Result<(), AppError> {
    let object = snapshot_object_mut(snapshot)?;
    let mut agent_default = Map::new();
    for key in ["agent_id", "agent_version_id", "model_profile_id"] {
        if let Some(value) = object.get(key).cloned()
            && !value.is_null()
        {
            agent_default.insert(key.to_string(), value);
        }
    }
    match object.entry("agent".to_string()) {
        serde_json::map::Entry::Vacant(entry) => {
            entry.insert(Value::Object(agent_default));
        }
        serde_json::map::Entry::Occupied(mut entry) => {
            let Some(agent_object) = entry.get_mut().as_object_mut() else {
                return Err(AppError::InvalidInput(
                    "run_config_snapshot.agent must be a JSON object".to_string(),
                ));
            };
            for (key, value) in agent_default {
                agent_object.insert(key, value);
            }
        }
    }
    for key in ["tools", "skills", "mcp_tools", "sql_tools"] {
        object
            .entry(key.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
    }
    Ok(())
}

fn insert_server_run_identity(
    snapshot: &mut Value,
    requested_agent_id: Option<Uuid>,
    requested_agent_version_id: Option<Uuid>,
    project_id: Option<Uuid>,
) -> Result<(), AppError> {
    let object = snapshot_object_mut(snapshot)?;
    object.insert("agent_id".to_string(), json!(requested_agent_id));
    object.insert(
        "agent_version_id".to_string(),
        json!(requested_agent_version_id),
    );
    object.insert("project_id".to_string(), json!(project_id));
    object.insert("policy_version".to_string(), json!(LOCAL_POLICY_VERSION));
    object.insert(
        "risk_policy_version".to_string(),
        json!(LOCAL_RISK_POLICY_VERSION),
    );
    Ok(())
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

fn assistant_model_mode(snapshot: &Value) -> Option<&str> {
    snapshot
        .pointer("/defaults/model/mode")
        .and_then(Value::as_str)
}

fn override_snapshot_model_profile_id(
    snapshot: &mut Value,
    model_profile_id: Uuid,
) -> Result<(), AppError> {
    let object = snapshot_object_mut(snapshot)?;
    object.remove("model");
    object.insert("model_profile_id".to_string(), json!(model_profile_id));
    if let Some(agent) = object.get_mut("agent").and_then(Value::as_object_mut) {
        agent.remove("model");
        agent.insert("model_profile_id".to_string(), json!(model_profile_id));
    }
    Ok(())
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
            "auth_scheme": self.auth_scheme,
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
    use sqlx::{PgPool, postgres::PgPoolOptions};

    #[test]
    fn selected_mcp_servers_only_filter_agent_allowed_tools() {
        let selected_server_id = Uuid::new_v4();
        let other_server_id = Uuid::new_v4();
        let mut snapshot = json!({
            "mcp_tools": [
                {"mcp_tool_id": Uuid::new_v4(), "server_id": selected_server_id, "tool_name": "allowed"},
                {"mcp_tool_id": Uuid::new_v4(), "server_id": other_server_id, "tool_name": "filtered"}
            ]
        });

        apply_selected_mcp_server_snapshot(&mut snapshot, &[selected_server_id]).unwrap();

        assert_eq!(snapshot["mcp_tools"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["mcp_tools"][0]["tool_name"], json!("allowed"));
        assert_eq!(
            snapshot["selected_mcp_server_ids"],
            json!([selected_server_id])
        );
    }

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
    fn conversation_model_override_replaces_stale_assistant_model_identity() {
        let stale_id = Uuid::new_v4();
        let selected_id = Uuid::new_v4();
        let mut snapshot = json!({
            "model_profile_id": stale_id,
            "model": {"model_name": "stale"},
            "agent": {
                "model_profile_id": stale_id,
                "model": {"model_name": "stale"}
            }
        });

        override_snapshot_model_profile_id(&mut snapshot, selected_id).unwrap();

        assert_eq!(model_profile_id(&snapshot), Some(selected_id));
        assert_eq!(snapshot["agent"]["model_profile_id"], json!(selected_id));
        assert!(snapshot.get("model").is_none());
        assert!(snapshot["agent"].get("model").is_none());
    }

    #[test]
    fn base_snapshot_rejects_non_object_client_snapshot() {
        let err = base_snapshot(None, Some(json!("bad")), None, None, None).unwrap_err();
        assert!(err.to_string().contains("run_config_snapshot"));
    }

    #[test]
    fn tool_runtime_risk_uses_published_version_and_fails_closed() {
        assert_eq!(tool_runtime_risk_level(&json!({})), "low");
        assert_eq!(
            tool_runtime_risk_level(&json!({"risk_level": "medium"})),
            "medium"
        );
        assert_eq!(
            tool_runtime_risk_level(&json!({"risk_level": "low", "requires_approval": true})),
            "high"
        );
        assert_eq!(
            tool_runtime_risk_level(&json!({"risk_level": "unexpected"})),
            "high"
        );
    }

    #[test]
    fn agent_version_snapshot_preserves_biwork_runtime_namespaces() {
        let version = AgentVersionSnapshot {
            agent_id: Uuid::new_v4(),
            agent_version_id: Uuid::new_v4(),
            runtime_id: Uuid::new_v4(),
            policy_version: "policy-v1".to_string(),
            schema_hash: None,
            snapshot: json!({
                "agent": {"name": "published-agent"},
                "model_profile_id": Uuid::new_v4(),
            }),
        };

        let snapshot = base_snapshot(
            Some(&version),
            Some(json!({
                "ui": {"client": "biwork", "conversation_type": "team"},
                "cron": {"job_id": "cron-1"},
                "channel": {"platform_type": "telegram"},
                "team": {
                    "team_run_id": "team-run-1",
                    "token": "client-team-token",
                    "member": {"secret": "client-member-secret"}
                },
                "model": {"provider": "client-must-not-override"},
                "tools": [{"name": "client-must-not-inject"}],
                "access_token": "client-access-token",
            })),
            Some(version.agent_id),
            Some(version.agent_version_id),
            None,
        )
        .unwrap();

        assert_eq!(snapshot["ui"]["conversation_type"], json!("team"));
        assert_eq!(snapshot["cron"]["job_id"], json!("cron-1"));
        assert_eq!(snapshot["channel"]["platform_type"], json!("telegram"));
        assert_eq!(snapshot["team"]["team_run_id"], json!("team-run-1"));
        assert!(snapshot.get("model").is_none());
        assert!(snapshot.get("tools").is_none());
        let serialized = snapshot.to_string();
        assert!(!serialized.contains("client-access-token"));
        assert!(!serialized.contains("client-team-token"));
        assert!(!serialized.contains("client-member-secret"));
    }

    #[test]
    fn client_snapshot_keeps_server_resolved_run_identity() {
        let agent_id = Uuid::parse_str("00000000-0000-0000-0000-000000000301").unwrap();
        let project_id = Uuid::parse_str("00000000-0000-0000-0000-000000000302").unwrap();
        let spoofed_agent_id = Uuid::parse_str("00000000-0000-0000-0000-000000000303").unwrap();

        let snapshot = base_snapshot(
            None,
            Some(json!({
                "agent_id": spoofed_agent_id,
                "project_id": Uuid::new_v4(),
                "policy_version": "client-policy",
                "risk_policy_version": "client-risk-policy",
                "ui": {"client": "biwork", "conversation_type": "acp"},
                "cron": {"job_id": "cron-1"},
            })),
            Some(agent_id),
            None,
            Some(project_id),
        )
        .unwrap();

        assert_eq!(snapshot["agent_id"], json!(agent_id));
        assert_eq!(snapshot["agent_version_id"], Value::Null);
        assert_eq!(snapshot["project_id"], json!(project_id));
        assert_eq!(snapshot["ui"]["client"], json!("biwork"));
        assert_eq!(snapshot["cron"]["job_id"], json!("cron-1"));
        assert_eq!(snapshot["policy_version"], json!(LOCAL_POLICY_VERSION));
        assert_eq!(
            snapshot["risk_policy_version"],
            json!(LOCAL_RISK_POLICY_VERSION)
        );
    }

    #[test]
    fn client_snapshot_strips_secret_material_before_persistence() {
        let agent_id = Uuid::parse_str("00000000-0000-0000-0000-000000000311").unwrap();
        let project_id = Uuid::parse_str("00000000-0000-0000-0000-000000000312").unwrap();

        let snapshot = base_snapshot(
            None,
            Some(json!({
                "agent_id": Uuid::new_v4(),
                "secret": "raw-client-secret",
                "api_key": "sk-client",
                "ui": {
                    "client": "biwork",
                    "token": "ui-token",
                    "conversation_type": "acp"
                },
                "team": {
                    "team_run_id": "team-run-1",
                    "members": [
                        {"slot_id": "lead", "authorization": "Bearer raw"}
                    ]
                },
                "extension_contributions": [
                    {
                        "type": "mcp_server",
                        "key": "acme",
                        "manifest": {"label": "Acme", "refresh_token": "refresh"}
                    }
                ]
            })),
            Some(agent_id),
            None,
            Some(project_id),
        )
        .unwrap();

        assert_eq!(snapshot["agent_id"], json!(agent_id));
        assert_eq!(snapshot["project_id"], json!(project_id));
        assert_eq!(snapshot["ui"]["client"], json!("biwork"));
        assert_eq!(snapshot["team"]["team_run_id"], json!("team-run-1"));
        assert_eq!(
            snapshot["extension_contributions"][0]["manifest"]["label"],
            json!("Acme")
        );

        let serialized = snapshot.to_string();
        assert!(!serialized.contains("raw-client-secret"));
        assert!(!serialized.contains("sk-client"));
        assert!(!serialized.contains("ui-token"));
        assert!(!serialized.contains("Bearer raw"));
        assert!(!serialized.contains("refresh"));
        assert!(snapshot.get("secret").is_none());
        assert!(snapshot.get("api_key").is_none());
        assert!(snapshot["ui"].get("token").is_none());
        assert!(
            snapshot["team"]["members"][0]
                .get("authorization")
                .is_none()
        );
        assert!(
            snapshot["extension_contributions"][0]["manifest"]
                .get("refresh_token")
                .is_none()
        );
    }

    #[test]
    fn client_snapshot_cannot_spoof_runtime_actor() {
        let tenant_id = Uuid::parse_str("00000000-0000-0000-0000-000000000321").unwrap();
        let server_user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000322").unwrap();
        let server_device_id = Uuid::parse_str("00000000-0000-0000-0000-000000000323").unwrap();
        let server_session_id = Uuid::parse_str("00000000-0000-0000-0000-000000000324").unwrap();
        let spoofed_user_id = Uuid::parse_str("00000000-0000-0000-0000-000000000325").unwrap();
        let mut snapshot = base_snapshot(
            None,
            Some(json!({
                "actor": {
                    "user_id": spoofed_user_id,
                    "device_id": Uuid::new_v4(),
                    "roles": ["admin"]
                },
                "ui": {"client": "biwork"}
            })),
            None,
            None,
            None,
        )
        .unwrap();
        let ctx = PlatformRequestContext {
            tenant_id,
            platform_user_id: server_user_id,
            ferriskey_subject: "ferriskey-subject".to_string(),
            preferred_username: Some("alice".to_string()),
            email: Some("alice@example.test".to_string()),
            roles: vec!["tenant_member".to_string()],
            session_id: server_session_id,
            device_id: server_device_id,
            trace_id: "trace-1".to_string(),
            token_jti: None,
            token_exp: time::OffsetDateTime::now_utc(),
        };

        insert_actor_from_context(&mut snapshot, &ctx, false).unwrap();

        assert_eq!(snapshot["actor"]["user_id"], json!(server_user_id));
        assert_eq!(snapshot["actor"]["device_id"], json!(server_device_id));
        assert_eq!(snapshot["actor"]["session_id"], json!(server_session_id));
        assert_eq!(snapshot["actor"]["roles"], json!(["tenant_member"]));
        assert_eq!(snapshot["actor"]["preferred_username"], json!("alice"));
        assert_ne!(snapshot["actor"]["user_id"], json!(spoofed_user_id));
        assert!(snapshot.get("browser").is_none());
    }

    #[test]
    fn published_browser_capability_controls_runtime_browser_tools() {
        let mut snapshot = json!({
            "capabilities": {"browser": {"enabled": true}},
            "browser": {"enabled": false, "device_id": "spoofed"}
        });
        let ctx = PlatformRequestContext {
            tenant_id: Uuid::new_v4(),
            platform_user_id: Uuid::new_v4(),
            ferriskey_subject: "subject".to_string(),
            preferred_username: None,
            email: None,
            roles: vec![],
            session_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            trace_id: "trace".to_string(),
            token_jti: None,
            token_exp: time::OffsetDateTime::now_utc(),
        };

        assert!(agent_browser_capability_enabled(&snapshot));
        insert_actor_from_context(&mut snapshot, &ctx, true).unwrap();

        assert_eq!(snapshot["browser"]["enabled"], json!(true));
        assert_eq!(snapshot["browser"]["execution"], json!("local"));
        assert_eq!(snapshot["browser"]["visible"], json!(true));
        assert_eq!(snapshot["browser"]["device_id"], json!(ctx.device_id));
    }

    #[test]
    fn runtime_contract_defaults_agent_and_capability_arrays() {
        let agent_id = Uuid::new_v4();
        let model_profile_id = Uuid::new_v4();
        let mut snapshot = json!({
            "agent_id": agent_id,
            "model_profile_id": model_profile_id,
        });

        ensure_runtime_contract_fields(&mut snapshot).unwrap();

        assert_eq!(snapshot["agent"]["agent_id"], json!(agent_id));
        assert_eq!(
            snapshot["agent"]["model_profile_id"],
            json!(model_profile_id)
        );
        assert_eq!(snapshot["tools"], json!([]));
        assert_eq!(snapshot["skills"], json!([]));
        assert_eq!(snapshot["mcp_tools"], json!([]));
        assert_eq!(snapshot["sql_tools"], json!([]));
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn sql_tool_bindings_enter_runtime_snapshot_without_sql_template()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let tenant_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let agent_version_id = Uuid::new_v4();
        let connection_id = Uuid::new_v4();
        let sql_tool_id = Uuid::new_v4();
        let sql_tool_version_id = Uuid::new_v4();
        let query_hash = "sha256:runtime-sql-tool";

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Runtime SQL Snapshot', $2)")
            .bind(tenant_id)
            .bind(format!("runtime-sql-snapshot-{tenant_id}"))
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO agents (id, tenant_id, name, status)
            VALUES ($1, $2, 'runtime-sql-agent', 'active')
            "#,
        )
        .bind(agent_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_versions (
                id, tenant_id, agent_id, version_label, config_snapshot, status
            )
            VALUES ($1, $2, $3, 'v1', '{}'::jsonb, 'published')
            "#,
        )
        .bind(agent_version_id)
        .bind(tenant_id)
        .bind(agent_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sql_connections (
                id, tenant_id, name, database_kind, host, port, database_name, status
            )
            VALUES ($1, $2, 'runtime-sql-conn', 'postgres', '127.0.0.1', 5433, 'bibi_work', 'active')
            "#,
        )
        .bind(connection_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sql_tools (id, tenant_id, name, description, status)
            VALUES ($1, $2, 'sales-summary', 'Sales summary', 'active')
            "#,
        )
        .bind(sql_tool_id)
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sql_tool_versions (
                id, tenant_id, sql_tool_id, connection_id, version_label,
                operation, parameter_schema, sql_template, query_hash,
                risk_level, requires_approval, status
            )
            VALUES (
                $1, $2, $3, $4, 'v1', 'read',
                '{"type":"object","properties":{"region":{"type":"string"}}}'::jsonb,
                'SELECT :region::text AS region', $5, 'medium', false, 'published'
            )
            "#,
        )
        .bind(sql_tool_version_id)
        .bind(tenant_id)
        .bind(sql_tool_id)
        .bind(connection_id)
        .bind(query_hash)
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_version_sql_tool_bindings (
                agent_version_id, sql_tool_version_id
            )
            VALUES ($1, $2)
            "#,
        )
        .bind(agent_version_id)
        .bind(sql_tool_version_id)
        .execute(&pool)
        .await?;

        let sql_tools = load_sql_tool_snapshots(&pool, agent_version_id).await?;

        assert_eq!(sql_tools.len(), 1);
        assert_eq!(sql_tools[0]["sql_tool_id"], json!(sql_tool_id));
        assert_eq!(
            sql_tools[0]["sql_tool_version_id"],
            json!(sql_tool_version_id)
        );
        assert_eq!(sql_tools[0]["connection_id"], json!(connection_id));
        assert_eq!(sql_tools[0]["name"], json!("sales-summary"));
        assert_eq!(sql_tools[0]["operation"], json!("read"));
        assert_eq!(sql_tools[0]["query_hash"], json!(query_hash));
        assert_eq!(sql_tools[0]["risk_level"], json!("medium"));
        assert_eq!(sql_tools[0]["requires_approval"], json!(false));
        assert_eq!(
            sql_tools[0]["parameter_schema"]["properties"]["region"]["type"],
            json!("string")
        );
        assert!(sql_tools[0].get("sql_template").is_none());

        cleanup_tenant(&pool, tenant_id).await?;
        Ok(())
    }

    #[test]
    fn runtime_contract_overwrites_existing_agent_identity_with_server_truth() {
        let agent_id = Uuid::parse_str("00000000-0000-0000-0000-000000000401").unwrap();
        let agent_version_id = Uuid::parse_str("00000000-0000-0000-0000-000000000402").unwrap();
        let model_profile_id = Uuid::parse_str("00000000-0000-0000-0000-000000000403").unwrap();

        let mut snapshot = json!({
            "agent_id": agent_id,
            "agent_version_id": agent_version_id,
            "model_profile_id": model_profile_id,
            "agent": {
                "agent_id": Uuid::new_v4(),
                "name": "client label"
            }
        });

        ensure_runtime_contract_fields(&mut snapshot).unwrap();

        assert_eq!(snapshot["agent"]["agent_id"], json!(agent_id));
        assert_eq!(
            snapshot["agent"]["agent_version_id"],
            json!(agent_version_id)
        );
        assert_eq!(
            snapshot["agent"]["model_profile_id"],
            json!(model_profile_id)
        );
        assert_eq!(snapshot["agent"]["name"], json!("client label"));
    }

    #[test]
    fn python_dispatch_runtime_accepts_deepagents_only() {
        assert!(
            ensure_python_dispatch_runtime(&json!({
                "runtime": {"kind": "deepagents"}
            }))
            .is_ok()
        );
    }

    #[test]
    fn python_dispatch_runtime_rejects_local_or_disabled_runtime() {
        let local_err = ensure_python_dispatch_runtime(&json!({
            "runtime": {"kind": "biwork_cli"}
        }))
        .unwrap_err();
        assert!(matches!(local_err, AppError::Conflict(_)));
        assert!(local_err.to_string().contains("desktop local runtime"));

        let disabled_err = ensure_python_dispatch_runtime(&json!({
            "runtime": {"kind": "disabled"}
        }))
        .unwrap_err();
        assert!(matches!(disabled_err, AppError::Conflict(_)));
        assert!(disabled_err.to_string().contains("not runnable"));
    }

    #[test]
    fn python_dispatch_runtime_rejects_unknown_runtime_kind() {
        let err = ensure_python_dispatch_runtime(&json!({
            "runtime": {"kind": "experimental"}
        }))
        .unwrap_err();
        assert!(matches!(err, AppError::InvalidInput(_)));
        assert!(err.to_string().contains("runtime.kind=experimental"));
    }

    #[test]
    fn model_profile_runtime_json_redacts_secret_ref() {
        let snapshot = ModelProfileSnapshot {
            model_profile_id: Uuid::new_v4(),
            provider_id: Uuid::new_v4(),
            provider_key: "openai-compatible".to_string(),
            provider_name: "OpenAI Compatible".to_string(),
            base_url: Some("https://llm.example.test".to_string()),
            auth_scheme: "bearer".to_string(),
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
        assert_eq!(runtime.get("auth_scheme"), Some(&json!("bearer")));
    }

    #[test]
    fn insert_model_snapshot_sets_top_level_runtime_model() {
        let model = ModelProfileSnapshot {
            model_profile_id: Uuid::new_v4(),
            provider_id: Uuid::new_v4(),
            provider_key: "openai-compatible".to_string(),
            provider_name: "OpenAI Compatible".to_string(),
            base_url: Some("http://llm.example.test".to_string()),
            auth_scheme: "bearer".to_string(),
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

    async fn test_pool() -> Result<PgPool, Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(pool)
    }

    async fn cleanup_tenant(
        pool: &PgPool,
        tenant_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error>> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn active_model_reference_resolves_provider_specific_profile_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let tenant_id = Uuid::new_v4();
        let provider_id = Uuid::new_v4();
        let profile_id = Uuid::new_v4();
        let profile_name = format!("{provider_id}:gpt-test");
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Conversation model resolver test")
            .bind(format!("conversation-model-resolver-{tenant_id}"))
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_providers (id, tenant_id, provider_key, display_name, status)
            VALUES ($1, $2, 'custom', $3, 'active')
            "#,
        )
        .bind(provider_id)
        .bind(tenant_id)
        .bind(format!("Provider {provider_id}"))
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO llm_model_profiles (
                id, tenant_id, provider_id, profile_name, model_name, status
            )
            VALUES ($1, $2, $3, $4, 'gpt-test', 'active')
            "#,
        )
        .bind(profile_id)
        .bind(tenant_id)
        .bind(provider_id)
        .bind(&profile_name)
        .execute(&pool)
        .await?;

        let resolved =
            resolve_active_model_profile_reference(&pool, tenant_id, &profile_name).await?;

        assert_eq!(resolved, profile_id);
        cleanup_tenant(&pool, tenant_id).await?;
        Ok(())
    }

    #[test]
    fn runtime_extension_contribution_json_sanitizes_secret_manifest_keys() {
        let package_id = Uuid::new_v4();
        let contribution = runtime_extension_contribution_json(RuntimeExtensionContribution {
            extension_package_id: package_id,
            extension_name: "acme-extension".to_string(),
            source: "hub".to_string(),
            version: Some("1.2.3".to_string()),
            risk_level: "moderate".to_string(),
            contribution_type: "mcp_server".to_string(),
            contribution_key: "acme-mcp".to_string(),
            manifest: json!({
                "label": "Acme MCP",
                "token": "plain-token",
                "nested": {
                    "secret_ref": "vault://tenant/secret",
                    "safe": true
                },
                "items": [
                    {"apiKey": "sk-test", "name": "kept"}
                ]
            }),
        })
        .expect("mcp_server is a runtime contribution type");

        assert_eq!(contribution["extension_package_id"], json!(package_id));
        assert_eq!(contribution["type"], json!("mcp_server"));
        assert_eq!(contribution["key"], json!("acme-mcp"));
        assert_eq!(contribution["manifest"]["label"], json!("Acme MCP"));
        assert!(contribution["manifest"].get("token").is_none());
        assert!(
            contribution["manifest"]["nested"]
                .get("secret_ref")
                .is_none()
        );
        assert_eq!(contribution["manifest"]["nested"]["safe"], json!(true));
        assert!(contribution["manifest"]["items"][0].get("apiKey").is_none());
        assert_eq!(contribution["manifest"]["items"][0]["name"], json!("kept"));
    }

    #[test]
    fn runtime_extension_contribution_json_rejects_ui_only_contribution_types() {
        for contribution_type in ["theme", "settings_tab", "webui"] {
            let contribution = runtime_extension_contribution_json(RuntimeExtensionContribution {
                extension_package_id: Uuid::new_v4(),
                extension_name: "acme-extension".to_string(),
                source: "hub".to_string(),
                version: Some("1.2.3".to_string()),
                risk_level: "moderate".to_string(),
                contribution_type: contribution_type.to_string(),
                contribution_key: "ui-only".to_string(),
                manifest: json!({
                    "label": "UI only",
                    "token": "plain-token"
                }),
            });

            assert!(
                contribution.is_none(),
                "{contribution_type} must not enter Python runtime snapshots"
            );
        }
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
        assert_eq!(snapshot["runtime"], json!({"kind": "deepagents"}));
        assert_eq!(
            snapshot["ui"],
            json!({
                "client": "biwork",
                "conversation_type": "acp"
            })
        );
        assert_eq!(snapshot["local_mount_ids"], json!([local_mount_id]));
        assert_eq!(snapshot["workspace"], scope);
    }

    #[test]
    fn insert_common_runtime_fields_preserves_explicit_runtime_and_ui() {
        let tenant_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let scope = empty_workspace_scope(None, None);
        let mut snapshot = json!({
            "runtime": {"kind": "custom-test"},
            "ui": {"client": "test-client", "conversation_type": "test"}
        });

        insert_common_runtime_fields(&mut snapshot, tenant_id, run_id, None, None, None, &scope)
            .unwrap();

        assert_eq!(snapshot["runtime"], json!({"kind": "custom-test"}));
        assert_eq!(
            snapshot["ui"],
            json!({"client": "test-client", "conversation_type": "test"})
        );
    }
}
