use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::features::{agent_platform::secret_resolver::SecretResolver, core::errors::AppError};

use super::mcp_http;

#[derive(Debug, Clone)]
pub struct DiscoveredMcpTool {
    pub name: String,
    pub description: Option<String>,
    pub schema: Value,
    pub schema_hash: String,
}

pub async fn discover_mcp_tools(
    secret_resolver: &SecretResolver,
    transport: &str,
    config: &Value,
    secret_ref: Option<&str>,
) -> Result<Vec<DiscoveredMcpTool>, AppError> {
    if !matches!(
        transport,
        "http" | "streamable-http" | "streamable_http" | "sse" | "json-rpc"
    ) {
        return Err(AppError::InvalidInput(format!(
            "unsupported MCP transport for discovery: {transport}"
        )));
    }

    let value = mcp_http::request(
        secret_resolver,
        transport,
        config,
        secret_ref,
        "tools/list",
        json!({}),
    )
    .await?;
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

fn schema_hash(schema: &Value) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(schema)
        .map_err(|err| AppError::InvalidInput(format!("failed to encode MCP schema: {err}")))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        extract::Path,
        http::HeaderMap,
        routing::{get, post},
    };
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

        let resolver = SecretResolver::env_only_for_tests();
        let tools =
            discover_mcp_tools(&resolver, "http", &json!({"endpoint": endpoint}), None).await?;
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

        let resolver = SecretResolver::env_only_for_tests();
        let tools = discover_mcp_tools(
            &resolver,
            "http",
            &json!({"endpoint": endpoint}),
            Some("env://BIBI_TEST_MCP_TOKEN"),
        )
        .await?;
        assert_eq!(tools[0].name, "secured_lookup");

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn discovers_tools_with_configured_custom_header()
    -> Result<(), Box<dyn std::error::Error>> {
        async fn tools_list(headers: HeaderMap, Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("x-api-key")
                    .and_then(|value| value.to_str().ok()),
                Some("test-key")
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

        let resolver = SecretResolver::env_only_for_tests();
        let tools = discover_mcp_tools(
            &resolver,
            "http",
            &json!({"endpoint": endpoint, "headers": {"X-API-Key": "test-key"}}),
            None,
        )
        .await?;
        assert_eq!(tools[0].name, "secured_lookup");

        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn discovers_tools_with_vault_backed_bearer_secret()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_MCP_VAULT_TOKEN", "vault-control-token");
        }
        async fn vault_secret(Path(path): Path<String>, headers: HeaderMap) -> Json<Value> {
            assert_eq!(path, "secret/data/mcp/server");
            assert_eq!(
                headers
                    .get("x-vault-token")
                    .and_then(|value| value.to_str().ok()),
                Some("vault-control-token")
            );
            Json(json!({"data": {"data": {"token": "vault-mcp-secret"}}}))
        }
        async fn tools_list(headers: HeaderMap, Json(payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer vault-mcp-secret")
            );
            Json(json!({
                "jsonrpc": "2.0",
                "id": payload["id"].clone(),
                "result": {"tools": [{"name": "vault_lookup", "inputSchema": {"type": "object"}}]}
            }))
        }
        let router = Router::new()
            .route("/v1/{*path}", get(vault_secret))
            .route("/mcp", post(tools_list));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let endpoint = format!("{base_url}/mcp");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let resolver = SecretResolver::new(crate::configuration::SecretResolverSettings {
            timeout_milliseconds: 2_000,
            vault_enabled: true,
            vault_base_url: Some(base_url),
            vault_token_ref: Some("env://BIBI_TEST_MCP_VAULT_TOKEN".to_string()),
            vault_namespace: None,
            kms_enabled: false,
            kms_base_url: None,
            kms_auth_token_ref: None,
            rotation_gateway_enabled: false,
            rotation_gateway_base_url: None,
            rotation_gateway_auth_token_ref: None,
        })?;
        let tools = discover_mcp_tools(
            &resolver,
            "http",
            &json!({"endpoint": endpoint}),
            Some("vault://secret/data/mcp/server#token"),
        )
        .await?;
        assert_eq!(tools[0].name, "vault_lookup");
        server.abort();
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires BIBI_TEST_STREAMABLE_MCP_URL"]
    async fn discovers_tools_from_real_streamable_http_server()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint = std::env::var("BIBI_TEST_STREAMABLE_MCP_URL")?;
        let resolver = SecretResolver::env_only_for_tests();
        let tools = discover_mcp_tools(
            &resolver,
            "streamable-http",
            &json!({"endpoint": endpoint, "timeout_ms": 30_000}),
            None,
        )
        .await?;
        assert!(!tools.is_empty());
        assert!(tools.iter().all(|tool| !tool.name.trim().is_empty()));
        Ok(())
    }
}
