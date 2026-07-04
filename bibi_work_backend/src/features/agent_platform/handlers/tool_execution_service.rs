use axum::{Json, extract::State};
use serde_json::Value;

use crate::{
    features::{
        agent_platform::{models::*, tool_execution},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn call_mcp_tool(
    State(state): State<AppState>,
    Json(payload): Json<McpToolCallRequest>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        payload.actor.clone(),
        "execute",
        "mcp_tool",
        payload
            .mcp_tool_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| payload.tool_name.clone()),
        Some(AuthzContext {
            conversation_id: payload.conversation_id,
            run_id: payload.run_id,
            mcp_server_id: payload.mcp_server_id,
            risk_level: Some(mcp_risk_level(&payload.tool_name).to_string()),
            ..Default::default()
        }),
    )
    .await?;

    tool_execution::execute_mcp_tool(&state, &payload)
        .await
        .map(Json)
}

pub async fn execute_sql_tool(
    State(state): State<AppState>,
    Json(payload): Json<SqlToolExecuteRequest>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        payload.actor.clone(),
        "execute",
        if payload.sql_tool_id.is_some() {
            "sql_tool"
        } else {
            "sql_query"
        },
        payload
            .sql_tool_id
            .map(|id| id.to_string())
            .or_else(|| payload.query_hash.clone())
            .unwrap_or_else(|| "unidentified-query".to_string()),
        Some(AuthzContext {
            conversation_id: payload.conversation_id,
            run_id: payload.run_id,
            args_hash: payload.query_hash.clone(),
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;

    tool_execution::execute_sql_tool(&state, &payload)
        .await
        .map(Json)
}

pub async fn call_third_party_tool(
    State(state): State<AppState>,
    Json(payload): Json<ThirdPartyToolCallRequest>,
) -> Result<Json<Value>, AppError> {
    let resource_id = payload
        .tool_id
        .map(|id| id.to_string())
        .or_else(|| payload.tool_version_id.map(|id| id.to_string()))
        .or_else(|| payload.tool_name.clone())
        .unwrap_or_else(|| "unidentified-tool".to_string());
    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        payload.actor.clone(),
        "execute",
        "tool",
        resource_id,
        Some(AuthzContext {
            conversation_id: payload.conversation_id,
            run_id: payload.run_id,
            tool_id: payload.tool_id,
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;

    tool_execution::execute_third_party_tool(&state, &payload)
        .await
        .map(Json)
}

fn mcp_risk_level(tool_name: &str) -> &'static str {
    let lowered = tool_name.to_lowercase();
    if ["read", "list", "get", "search"]
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
    {
        "medium"
    } else {
        "high"
    }
}
