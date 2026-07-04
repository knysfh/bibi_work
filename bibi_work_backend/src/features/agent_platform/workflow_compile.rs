use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::features::core::errors::AppError;

use super::workflow_plan;

const WORKFLOW_COMPILATION_KEY: &str = "platform_compilation";
const WORKFLOW_COMPILATION_VERSION: i32 = 1;
const LOCAL_POLICY_VERSION: &str = "local-policy-v1";

#[derive(Debug, Clone)]
struct NodeAgentVersion {
    agent_id: Uuid,
    policy_version: String,
    schema_hash: Option<String>,
    config_snapshot: Value,
}

#[derive(Debug, Clone)]
pub struct NodePermission {
    pub node_key: String,
    pub agent_id: Uuid,
    pub agent_version_id: Uuid,
}

pub async fn compile_plan(pool: &PgPool, tenant_id: Uuid, plan: &Value) -> Result<Value, AppError> {
    workflow_plan::validate(plan)?;
    let mut compiled_plan = plan.clone();
    let mut compiled_nodes = Map::new();

    for (node_key, node) in workflow_plan::nodes(plan)? {
        let agent_version_id = workflow_plan::node_agent_version_id(&node)?;
        let agent_version =
            fetch_published_agent_version(pool, tenant_id, agent_version_id).await?;
        let skill_version_ids = fetch_bound_ids(
            pool,
            r#"
            SELECT skill_version_id AS id
            FROM agent_version_skill_bindings
            WHERE agent_version_id = $1
            ORDER BY created_at ASC, skill_version_id ASC
            "#,
            agent_version_id,
        )
        .await?;
        let tool_version_ids = fetch_bound_ids(
            pool,
            r#"
            SELECT tool_version_id AS id
            FROM agent_version_tool_bindings
            WHERE agent_version_id = $1
            ORDER BY created_at ASC, tool_version_id ASC
            "#,
            agent_version_id,
        )
        .await?;
        let mcp_tool_ids = fetch_bound_ids(
            pool,
            r#"
            SELECT mcp_tool_id AS id
            FROM agent_version_mcp_bindings
            WHERE agent_version_id = $1
            ORDER BY created_at ASC, mcp_tool_id ASC
            "#,
            agent_version_id,
        )
        .await?;

        compiled_nodes.insert(
            node_key,
            json!({
                "agent_id": agent_version.agent_id,
                "agent_version_id": agent_version_id,
                "agent_policy_version": agent_version.policy_version,
                "agent_schema_hash": agent_version.schema_hash,
                "required_permissions": [
                    {
                        "action": "run",
                        "resource_type": "agent",
                        "resource_id": agent_version.agent_id
                    }
                ],
                "capabilities": {
                    "model_profile_id": model_profile_id(&agent_version.config_snapshot),
                    "skill_version_ids": skill_version_ids,
                    "tool_version_ids": tool_version_ids,
                    "mcp_tool_ids": mcp_tool_ids
                }
            }),
        );
    }

    let plan_object = compiled_plan.as_object_mut().ok_or_else(|| {
        AppError::InvalidInput("compiled workflow plan must be an object".to_string())
    })?;
    plan_object.insert(
        WORKFLOW_COMPILATION_KEY.to_string(),
        json!({
            "version": WORKFLOW_COMPILATION_VERSION,
            "policy_version": LOCAL_POLICY_VERSION,
            "nodes": compiled_nodes
        }),
    );

    Ok(compiled_plan)
}

pub fn node_permissions(plan: &Value) -> Result<Vec<NodePermission>, AppError> {
    workflow_plan::nodes(plan)?
        .into_iter()
        .map(|(node_key, node)| {
            let agent_version_id = workflow_plan::node_agent_version_id(&node)?;
            let agent_id = node_compilation(plan, &node_key)
                .and_then(|snapshot| snapshot.get("agent_id"))
                .and_then(Value::as_str)
                .map(Uuid::parse_str)
                .transpose()
                .map_err(|_| {
                    AppError::InvalidInput(format!(
                        "workflow node {node_key} has invalid compiled agent_id"
                    ))
                })?
                .ok_or_else(|| {
                    AppError::InvalidInput(format!(
                        "workflow node {node_key} is missing compiled agent permission snapshot"
                    ))
                })?;
            Ok(NodePermission {
                node_key,
                agent_id,
                agent_version_id,
            })
        })
        .collect()
}

pub fn node_permission_snapshot(plan: &Value, node_key: &str) -> Result<Value, AppError> {
    node_compilation(plan, node_key).cloned().ok_or_else(|| {
        AppError::InvalidInput(format!(
            "workflow node {node_key} is missing compiled permission snapshot"
        ))
    })
}

fn node_compilation<'a>(plan: &'a Value, node_key: &str) -> Option<&'a Value> {
    plan.get(WORKFLOW_COMPILATION_KEY)?
        .get("nodes")?
        .get(node_key)
}

async fn fetch_published_agent_version(
    pool: &PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<NodeAgentVersion, AppError> {
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
    .ok_or_else(|| {
        AppError::InvalidInput(format!(
            "workflow node references unpublished agent_version_id {agent_version_id}"
        ))
    })?;

    Ok(NodeAgentVersion {
        agent_id: row.try_get("agent_id")?,
        policy_version: row.try_get("policy_version")?,
        schema_hash: row.try_get("schema_hash")?,
        config_snapshot: row.try_get("config_snapshot")?,
    })
}

async fn fetch_bound_ids(
    pool: &PgPool,
    sql: &'static str,
    agent_version_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    let rows = sqlx::query(sql)
        .bind(agent_version_id)
        .fetch_all(pool)
        .await?;

    rows.into_iter()
        .map(|row| row.try_get("id").map_err(AppError::from))
        .collect()
}

fn model_profile_id(snapshot: &Value) -> Option<String> {
    snapshot
        .get("model_profile_id")
        .or_else(|| {
            snapshot
                .get("agent")
                .and_then(|agent| agent.get("model_profile_id"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
}
