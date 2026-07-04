use std::time::Duration;

use reqwest::{Client, RequestBuilder};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::features::{agent_platform::secret_resolver, core::errors::AppError};

const DEFAULT_MCP_HTTP_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct DiscoveredMcpTool {
    pub name: String,
    pub description: Option<String>,
    pub schema: Value,
    pub schema_hash: String,
}

pub async fn discover_mcp_tools(
    transport: &str,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<Vec<DiscoveredMcpTool>, AppError> {
    if !matches!(transport, "http" | "streamable-http" | "sse" | "json-rpc") {
        return Err(AppError::InvalidInput(format!(
            "unsupported MCP transport for discovery: {transport}"
        )));
    }

    let endpoint = mcp_endpoint(config)?;
    let timeout_ms = json_u64(config, "timeout_ms").unwrap_or(DEFAULT_MCP_HTTP_TIMEOUT_MS);
    let http = Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|err| AppError::InvalidInput(format!("failed to build MCP client: {err}")))?;
    let request_id = Uuid::new_v4().to_string();
    let request_body = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/list",
        "params": {}
    });

    let request = apply_secret_auth(http.post(endpoint), config, secret_ref)?.json(&request_body);
    let response = request
        .send()
        .await
        .map_err(|err| AppError::InvalidInput(format!("MCP tools/list failed: {err}")))?;
    let value = response_json(response).await?;
    parse_tools_list_response(value)
}

pub fn parse_tools_list_response(value: Value) -> Result<Vec<DiscoveredMcpTool>, AppError> {
    if value.get("error").is_some() {
        return Err(AppError::InvalidInput(
            "MCP tools/list returned an error".to_string(),
        ));
    }

    let tools = value
        .pointer("/result/tools")
        .or_else(|| value.get("tools"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AppError::InvalidInput("MCP tools/list response must contain result.tools".to_string())
        })?;

    tools
        .iter()
        .map(discovered_tool_from_value)
        .collect::<Result<Vec<_>, AppError>>()
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

fn discovered_tool_from_value(value: &Value) -> Result<DiscoveredMcpTool, AppError> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("MCP tool entry must be an object".to_string()))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("MCP tool name is required".to_string()))?
        .to_string();
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    let input_schema = object
        .get("inputSchema")
        .or_else(|| object.get("input_schema"))
        .or_else(|| object.get("schema"))
        .or_else(|| object.get("parameters"))
        .cloned()
        .unwrap_or_else(default_input_schema);
    if !matches!(input_schema, Value::Object(_) | Value::Bool(_)) {
        return Err(AppError::InvalidInput(format!(
            "MCP tool {name} input schema must be a JSON schema object or boolean"
        )));
    }

    let mut schema = Map::new();
    schema.insert("inputSchema".to_string(), input_schema);
    if let Some(annotations) = object.get("annotations") {
        schema.insert("annotations".to_string(), annotations.clone());
    }
    let schema = Value::Object(schema);
    let schema_hash = schema_hash(&schema)?;

    Ok(DiscoveredMcpTool {
        name,
        description,
        schema,
        schema_hash,
    })
}

fn default_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {}
    })
}

async fn response_json(response: reqwest::Response) -> Result<Value, AppError> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| AppError::InvalidInput(format!("MCP response read failed: {err}")))?;
    if !status.is_success() {
        return Err(AppError::InvalidInput(format!(
            "MCP tools/list returned HTTP {}",
            status.as_u16()
        )));
    }
    serde_json::from_slice(&bytes)
        .map_err(|err| AppError::InvalidInput(format!("MCP tools/list JSON parse failed: {err}")))
}

fn schema_hash(schema: &Value) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(schema)
        .map_err(|err| AppError::InvalidInput(format!("failed to encode MCP schema: {err}")))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn json_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn json_u64(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn apply_secret_auth(
    request: RequestBuilder,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<RequestBuilder, AppError> {
    let Some(secret_ref) = secret_ref else {
        return Ok(request);
    };
    let secret = secret_resolver::resolve_secret_ref(secret_ref)?;
    let header_name = json_string(config, "auth_header")
        .or_else(|| json_string(config, "secret_header"))
        .unwrap_or_else(|| "Authorization".to_string());
    if header_name.trim().is_empty()
        || header_name
            .bytes()
            .any(|byte| byte <= 31 || byte == 127 || byte == b':')
    {
        return Err(AppError::InvalidInput(
            "MCP auth header name is invalid".to_string(),
        ));
    }
    let scheme = json_string(config, "auth_scheme")
        .or_else(|| json_string(config, "secret_scheme"))
        .unwrap_or_else(|| "Bearer".to_string());
    let header_value = if scheme.eq_ignore_ascii_case("none") {
        secret
    } else {
        format!("{} {}", scheme.trim(), secret)
    };
    Ok(request.header(header_name, header_value))
}

#[cfg(test)]
mod tests {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn parses_tools_list_response_and_hashes_normalized_schema() {
        let tools = parse_tools_list_response(json!({
            "jsonrpc": "2.0",
            "id": "test",
            "result": {
                "tools": [
                    {
                        "name": "lookup_sales",
                        "description": "Lookup sales",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "q": {"type": "string"}
                            },
                            "required": ["q"]
                        },
                        "annotations": {"readOnlyHint": true}
                    }
                ]
            }
        }))
        .expect("tools parsed");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "lookup_sales");
        assert_eq!(tools[0].description.as_deref(), Some("Lookup sales"));
        assert_eq!(
            tools[0].schema["inputSchema"]["properties"]["q"]["type"],
            "string"
        );
        assert_eq!(tools[0].schema["annotations"]["readOnlyHint"], true);
        assert!(tools[0].schema_hash.starts_with("sha256:"));
    }

    #[test]
    fn rejects_error_and_invalid_schema_response() {
        assert!(parse_tools_list_response(json!({"error": {"message": "denied"}})).is_err());
        assert!(parse_tools_list_response(json!({"result": {"tools": {}}})).is_err());
        assert!(
            parse_tools_list_response(json!({
                "result": {"tools": [{"name": "bad", "inputSchema": "not-json-schema"}]}
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn discovers_tools_via_json_rpc_http() -> Result<(), Box<dyn std::error::Error>> {
        async fn tools_list(Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(payload["method"], "tools/list");
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload["id"].clone(),
                "result": {
                    "tools": [{
                        "name": "lookup_sales",
                        "description": "Lookup sales",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]
                }
            }))
        }

        let router = Router::new().route("/mcp", post(tools_list));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let tools = discover_mcp_tools("http", &json!({"endpoint": endpoint}), None).await?;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "lookup_sales");

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn discovers_tools_with_env_backed_bearer_secret()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_MCP_TOKEN", "mcp-secret");
        }

        async fn tools_list(headers: HeaderMap, Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(payload["method"], "tools/list");
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer mcp-secret")
            );
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload["id"].clone(),
                "result": {
                    "tools": [{
                        "name": "secured_lookup",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]
                }
            }))
        }

        let router = Router::new().route("/mcp", post(tools_list));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let tools = discover_mcp_tools(
            "http",
            &json!({"endpoint": endpoint}),
            Some("env://BIBI_TEST_MCP_TOKEN"),
        )
        .await?;
        assert_eq!(tools[0].name, "secured_lookup");

        server.abort();
        Ok(())
    }
}
