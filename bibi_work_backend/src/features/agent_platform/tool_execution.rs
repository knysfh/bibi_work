use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sqlx::{
    AssertSqlSafe, PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions, PgRow, PgSslMode},
};
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::{
    local_runtime_queue, mcp_http,
    models::{McpToolCallRequest, SqlToolExecuteRequest, ThirdPartyToolCallRequest},
    secret_resolver,
};

const DEFAULT_TOOL_HTTP_TIMEOUT_MS: u64 = 30_000;
const MAX_SQL_ROWS_HARD_LIMIT: i64 = 10_000;

struct McpExecutionTarget {
    mcp_tool_id: Option<Uuid>,
    server_id: Uuid,
    tool_name: String,
    schema: Value,
    transport: String,
    config: Value,
    secret_ref: Option<String>,
}

pub struct McpExecutionAuthzTarget {
    pub mcp_tool_id: Uuid,
    pub mcp_server_id: Uuid,
    pub risk_level: String,
}

struct SqlExecutionTarget {
    sql_tool_version_id: Uuid,
    sql_tool_id: Uuid,
    connection_id: Uuid,
    operation: String,
    risk_level: String,
    requires_approval: bool,
    sql_template: String,
    query_hash: String,
    max_rows: i32,
    statement_timeout_ms: i32,
    database_kind: String,
    host: Option<String>,
    port: Option<i32>,
    database_name: Option<String>,
    username_ref: Option<String>,
    password_secret_ref: Option<String>,
    tls_config_ref: Option<String>,
}

#[derive(Deserialize)]
struct SqlTlsSecretConfig {
    mode: String,
    root_cert_pem: Option<String>,
    client_cert_pem: Option<String>,
    client_key_pem: Option<String>,
}

pub struct SqlExecutionAuthzTarget {
    pub sql_tool_id: Uuid,
    pub query_hash: String,
    pub risk_level: String,
}

struct ThirdPartyExecutionTarget {
    tool_id: Uuid,
    tool_version_id: Uuid,
    tool_name: String,
    schema_snapshot: Value,
    secret_ref: Option<String>,
}

pub async fn execute_mcp_tool(
    state: &AppState,
    payload: &McpToolCallRequest,
) -> Result<Value, AppError> {
    let target = load_mcp_execution_target(state, payload).await?;
    if target.transport == "stdio" {
        return execute_stdio_mcp_tool(state, payload, target).await;
    }
    if !matches!(
        target.transport.as_str(),
        "http" | "streamable-http" | "streamable_http" | "sse" | "json-rpc"
    ) {
        return Err(AppError::InvalidInput(format!(
            "unsupported MCP transport for Rust executor: {}",
            target.transport
        )));
    }

    let mut value = mcp_http::request(
        &state.secret_resolver,
        &target.transport,
        &target.config,
        target.secret_ref.as_deref(),
        "tools/call",
        json!({
            "name": target.tool_name,
            "arguments": payload.arguments
        }),
    )
    .await?;
    if value.get("error").is_some() {
        return Err(AppError::InvalidInput(format!(
            "MCP tool returned error: {}",
            redact_for_error(&value)
        )));
    }
    if let Value::Object(ref mut map) = value {
        map.insert(
            "mcp_server_id".to_string(),
            Value::String(target.server_id.to_string()),
        );
        if let Some(tool_id) = target.mcp_tool_id {
            map.insert(
                "mcp_tool_id".to_string(),
                Value::String(tool_id.to_string()),
            );
        }
    }
    Ok(value)
}

pub async fn load_mcp_execution_authz_target(
    state: &AppState,
    payload: &McpToolCallRequest,
) -> Result<McpExecutionAuthzTarget, AppError> {
    let target = load_mcp_execution_target(state, payload).await?;
    Ok(McpExecutionAuthzTarget {
        mcp_tool_id: target
            .mcp_tool_id
            .ok_or_else(|| AppError::NotFound("MCP tool not found".to_string()))?,
        mcp_server_id: target.server_id,
        risk_level: trusted_mcp_risk_level(&target.tool_name, &target.schema).to_string(),
    })
}

async fn execute_stdio_mcp_tool(
    state: &AppState,
    payload: &McpToolCallRequest,
    target: McpExecutionTarget,
) -> Result<Value, AppError> {
    let device_id = payload.actor.device_id.ok_or_else(|| {
        AppError::InvalidInput(
            "MCP stdio execution requires the actor desktop device_id".to_string(),
        )
    })?;
    let command = json_string(&target.config, "command")
        .ok_or_else(|| AppError::InvalidInput("MCP stdio command is required".to_string()))?;
    let timeout_ms = json_u64(&target.config, "timeout_ms")
        .unwrap_or(DEFAULT_TOOL_HTTP_TIMEOUT_MS)
        .clamp(1_000, 120_000) as i32;
    let max_output_bytes = json_u64(&target.config, "max_output_bytes")
        .unwrap_or(1_048_576)
        .clamp(1_024, 8 * 1_048_576) as i32;
    let work = json!({
        "protocol": "local_runtime.v1",
        "kind": "mcp_stdio",
        "mcp_server_id": target.server_id,
        "transport": {
            "type": "stdio",
            "command": command,
            "args": target.config.get("args").cloned().unwrap_or_else(|| json!([])),
            "env": target.config.get("env").cloned().unwrap_or_else(|| json!({})),
            "timeout_ms": timeout_ms
        },
        "tool": {
            "name": target.tool_name,
            "arguments": payload.arguments
        }
    });
    let request = local_runtime_queue::enqueue(
        &state.connect_pool,
        local_runtime_queue::EnqueueLocalRuntimeRequest {
            tenant_id: payload.tenant_id,
            device_id: Some(device_id),
            project_id: None,
            run_id: payload.run_id,
            command: work,
            timeout_ms,
            max_output_bytes,
        },
    )
    .await?;
    let result = local_runtime_queue::wait_for_result(
        &state.connect_pool,
        request.id,
        payload.tenant_id,
        timeout_ms,
    )
    .await?;
    if result.status != "completed" {
        let summary = result
            .error
            .unwrap_or_else(|| "desktop local runtime did not complete the request".to_string());
        return Err(AppError::InvalidInput(format!(
            "MCP stdio local runtime request failed: {}",
            summary.chars().take(2_000).collect::<String>()
        )));
    }
    let mut value = result.result.unwrap_or(Value::Null);
    if value.get("error").is_some() {
        return Err(AppError::InvalidInput(format!(
            "MCP tool returned error: {}",
            redact_for_error(&value)
        )));
    }
    if let Value::Object(ref mut map) = value {
        map.insert(
            "mcp_server_id".to_string(),
            Value::String(target.server_id.to_string()),
        );
        if let Some(tool_id) = target.mcp_tool_id {
            map.insert(
                "mcp_tool_id".to_string(),
                Value::String(tool_id.to_string()),
            );
        }
    }
    Ok(value)
}

pub async fn execute_sql_tool(
    state: &AppState,
    payload: &SqlToolExecuteRequest,
) -> Result<Value, AppError> {
    let target = load_sql_execution_target(state, payload).await?;
    if target.database_kind != "postgres" {
        return Err(AppError::InvalidInput(format!(
            "unsupported SQL database kind: {}",
            target.database_kind
        )));
    }
    if target.operation != "read" {
        return Err(AppError::InvalidInput(
            "only read SQL tools can execute in the built-in Rust executor".to_string(),
        ));
    }
    if let Some(query_hash) = payload.query_hash.as_deref()
        && query_hash != target.query_hash
    {
        return Err(AppError::InvalidInput(
            "query_hash does not match the registered SQL tool version".to_string(),
        ));
    }
    validate_read_only_sql(&target.sql_template)?;
    let compiled = compile_named_parameters(&target.sql_template, &payload.parameters)?;
    let max_rows = i64::from(target.max_rows).clamp(1, MAX_SQL_ROWS_HARD_LIMIT);
    let wrapped_sql = format!(
        "WITH result_rows AS ({} LIMIT {}) SELECT COALESCE(jsonb_agg(to_jsonb(result_rows)), '[]'::jsonb) AS rows, COUNT(*)::BIGINT AS row_count FROM result_rows",
        compiled.sql,
        max_rows + 1
    );

    let (rows, row_count) =
        execute_postgres_read_query(state, &target, wrapped_sql, compiled.values).await?;
    let mut rows = rows.as_array().cloned().unwrap_or_default();
    let truncated = i64::try_from(rows.len())? > max_rows;
    if truncated {
        rows.truncate(usize::try_from(max_rows)?);
    }
    Ok(json!({
        "sql_tool_id": target.sql_tool_id,
        "sql_tool_version_id": target.sql_tool_version_id,
        "connection_id": target.connection_id,
        "query_hash": target.query_hash,
        "rows": rows,
        "row_count": rows.len(),
        "matched_row_count": row_count,
        "truncated": truncated
    }))
}

pub async fn load_sql_execution_authz_target(
    state: &AppState,
    payload: &SqlToolExecuteRequest,
) -> Result<SqlExecutionAuthzTarget, AppError> {
    let target = load_sql_execution_target(state, payload).await?;
    Ok(SqlExecutionAuthzTarget {
        sql_tool_id: target.sql_tool_id,
        query_hash: target.query_hash,
        risk_level: sql_execution_risk_level(
            &target.operation,
            &target.risk_level,
            target.requires_approval,
        ),
    })
}

pub async fn execute_third_party_tool(
    state: &AppState,
    payload: &ThirdPartyToolCallRequest,
) -> Result<Value, AppError> {
    let target = load_third_party_execution_target(state, payload).await?;
    let executor = target
        .schema_snapshot
        .get("executor")
        .ok_or_else(|| AppError::InvalidInput("tool executor is not configured".to_string()))?;
    let executor_type = executor
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("http");
    if executor_type != "http" {
        return Err(AppError::InvalidInput(format!(
            "unsupported third-party tool executor type: {executor_type}"
        )));
    }
    let mut url = json_string(executor, "url")
        .or_else(|| json_string(executor, "endpoint"))
        .ok_or_else(|| AppError::InvalidInput("third-party tool url is required".to_string()))?;
    let method = http_method(executor)?;
    if method == Method::GET {
        url = append_query_params(&url, &scalar_query_params(&payload.arguments)?);
    }
    let timeout_ms = json_u64(executor, "timeout_ms").unwrap_or(DEFAULT_TOOL_HTTP_TIMEOUT_MS);
    let headers = safe_headers(executor.get("headers"))?;
    let http = Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()
        .map_err(|err| {
            AppError::InvalidInput(format!("failed to build HTTP tool client: {err}"))
        })?;

    let mut request = http.request(method.clone(), url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request = apply_http_tool_secret_auth(
        &state.secret_resolver,
        request,
        executor,
        target.secret_ref.as_deref(),
    )
    .await?;
    request = if method == Method::GET {
        request
    } else {
        request.json(&json!({
            "tool_id": target.tool_id,
            "tool_version_id": target.tool_version_id,
            "tool_name": target.tool_name,
            "arguments": payload.arguments
        }))
    };
    let response = request
        .send()
        .await
        .map_err(|err| AppError::InvalidInput(format!("third-party tool call failed: {err}")))?;
    response_json_or_text(response).await
}

async fn load_mcp_execution_target(
    state: &AppState,
    payload: &McpToolCallRequest,
) -> Result<McpExecutionTarget, AppError> {
    let row = if let Some(mcp_tool_id) = payload.mcp_tool_id {
        sqlx::query(
            r#"
            SELECT mt.id AS mcp_tool_id, mt.name AS tool_name, mt.schema,
                   ms.id AS server_id, ms.transport, ms.config, ms.secret_ref
            FROM mcp_tools mt
            JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
            WHERE mt.id = $1
              AND mt.tenant_id = $2
              AND mt.status = 'active'
              AND ms.status = 'active'
            "#,
        )
        .bind(mcp_tool_id)
        .bind(payload.tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
    } else {
        let server_id = payload.mcp_server_id.ok_or_else(|| {
            AppError::InvalidInput("mcp_server_id or mcp_tool_id is required".to_string())
        })?;
        sqlx::query(
            r#"
            SELECT mt.id AS mcp_tool_id, mt.name AS tool_name, mt.schema,
                   ms.id AS server_id, ms.transport, ms.config, ms.secret_ref
            FROM mcp_tools mt
            JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
            WHERE ms.id = $1
              AND mt.tenant_id = $2
              AND mt.name = $3
              AND mt.status = 'active'
              AND ms.status = 'active'
            "#,
        )
        .bind(server_id)
        .bind(payload.tenant_id)
        .bind(&payload.tool_name)
        .fetch_optional(&state.connect_pool)
        .await?
    }
    .ok_or_else(|| AppError::NotFound("MCP tool not found".to_string()))?;

    Ok(McpExecutionTarget {
        mcp_tool_id: row.try_get("mcp_tool_id")?,
        server_id: row.try_get("server_id")?,
        tool_name: row.try_get("tool_name")?,
        schema: row.try_get("schema")?,
        transport: row.try_get("transport")?,
        config: row.try_get("config")?,
        secret_ref: row.try_get("secret_ref")?,
    })
}

fn trusted_mcp_risk_level(tool_name: &str, schema: &Value) -> &'static str {
    let annotations = schema.get("annotations");
    if annotations
        .and_then(|value| value.get("destructiveHint"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return "high";
    }
    if annotations
        .and_then(|value| value.get("readOnlyHint"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return "low";
    }
    let lowered = tool_name.to_ascii_lowercase();
    if ["read", "list", "get", "search"]
        .iter()
        .any(|prefix| lowered.starts_with(prefix))
    {
        "medium"
    } else {
        "high"
    }
}

async fn load_sql_execution_target(
    state: &AppState,
    payload: &SqlToolExecuteRequest,
) -> Result<SqlExecutionTarget, AppError> {
    let row = if let Some(sql_tool_id) = payload.sql_tool_id {
        sqlx::query(AssertSqlSafe(sql_target_query(
            "stv.sql_tool_id = $2",
            "stv.created_at DESC LIMIT 1",
        )))
        .bind(payload.tenant_id)
        .bind(sql_tool_id)
        .fetch_optional(&state.connect_pool)
        .await?
    } else if let Some(query_hash) = payload.query_hash.as_deref() {
        let rows = sqlx::query(AssertSqlSafe(sql_target_query(
            "stv.query_hash = $2",
            "stv.created_at DESC",
        )))
        .bind(payload.tenant_id)
        .bind(query_hash)
        .fetch_all(&state.connect_pool)
        .await?;
        if rows.len() > 1 {
            return Err(AppError::InvalidInput(
                "query_hash matched multiple SQL tool versions; sql_tool_id is required"
                    .to_string(),
            ));
        }
        rows.into_iter().next()
    } else {
        return Err(AppError::InvalidInput(
            "sql_tool_id or query_hash is required".to_string(),
        ));
    }
    .ok_or_else(|| AppError::NotFound("SQL tool version not found".to_string()))?;

    sql_target_from_row(row)
}

fn sql_target_query(predicate: &str, order_by: &str) -> String {
    format!(
        r#"
        SELECT stv.id AS sql_tool_version_id, stv.sql_tool_id, stv.connection_id,
               stv.operation, stv.risk_level, stv.requires_approval,
               stv.sql_template, stv.query_hash,
               sc.max_rows, sc.statement_timeout_ms, sc.database_kind,
               sc.host, sc.port, sc.database_name, sc.username_ref,
               sc.password_secret_ref, sc.tls_config_ref
        FROM sql_tool_versions stv
        JOIN sql_connections sc ON sc.id = stv.connection_id
        JOIN sql_tools st ON st.id = stv.sql_tool_id
        WHERE stv.tenant_id = $1
          AND {predicate}
          AND stv.status = 'published'
          AND sc.status = 'active'
          AND st.status = 'active'
        ORDER BY {order_by}
        "#
    )
}

fn sql_target_from_row(row: PgRow) -> Result<SqlExecutionTarget, AppError> {
    Ok(SqlExecutionTarget {
        sql_tool_version_id: row.try_get("sql_tool_version_id")?,
        sql_tool_id: row.try_get("sql_tool_id")?,
        connection_id: row.try_get("connection_id")?,
        operation: row.try_get("operation")?,
        risk_level: row.try_get("risk_level")?,
        requires_approval: row.try_get("requires_approval")?,
        sql_template: row.try_get("sql_template")?,
        query_hash: row.try_get("query_hash")?,
        max_rows: row.try_get("max_rows")?,
        statement_timeout_ms: row.try_get("statement_timeout_ms")?,
        database_kind: row.try_get("database_kind")?,
        host: row.try_get("host")?,
        port: row.try_get("port")?,
        database_name: row.try_get("database_name")?,
        username_ref: row.try_get("username_ref")?,
        password_secret_ref: row.try_get("password_secret_ref")?,
        tls_config_ref: row.try_get("tls_config_ref")?,
    })
}

async fn execute_postgres_read_query(
    state: &AppState,
    target: &SqlExecutionTarget,
    sql: String,
    values: Vec<Value>,
) -> Result<(Value, i64), AppError> {
    if target.password_secret_ref.is_some() {
        let pool = secret_backed_postgres_pool(&state.secret_resolver, target).await?;
        fetch_postgres_json_rows(&pool, target.statement_timeout_ms, sql, values).await
    } else {
        fetch_postgres_json_rows(
            &state.connect_pool,
            target.statement_timeout_ms,
            sql,
            values,
        )
        .await
    }
}

async fn secret_backed_postgres_pool(
    secret_resolver: &secret_resolver::SecretResolver,
    target: &SqlExecutionTarget,
) -> Result<PgPool, AppError> {
    let host = required_sql_connection_field(target.host.as_deref(), "host")?;
    let database_name =
        required_sql_connection_field(target.database_name.as_deref(), "database_name")?;
    let username_ref =
        required_sql_connection_field(target.username_ref.as_deref(), "username_ref")?;
    let password_secret_ref = required_sql_connection_field(
        target.password_secret_ref.as_deref(),
        "password_secret_ref",
    )?;
    let username = resolve_optional_env_ref(username_ref)?;
    let password = secret_resolver.resolve(password_secret_ref).await?;
    let port = target.port.unwrap_or(5432).clamp(1, 65_535) as u16;
    let options = PgConnectOptions::new()
        .host(host)
        .port(port)
        .database(database_name)
        .username(&username)
        .password(&password);
    let options =
        apply_sql_tls_config(secret_resolver, options, target.tls_config_ref.as_deref()).await?;
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|err| AppError::InvalidInput(format!("SQL connection failed: {err}")))
}

async fn apply_sql_tls_config(
    secret_resolver: &secret_resolver::SecretResolver,
    mut options: PgConnectOptions,
    tls_config_ref: Option<&str>,
) -> Result<PgConnectOptions, AppError> {
    let Some(tls_config_ref) = tls_config_ref else {
        return Ok(options.ssl_mode(PgSslMode::Prefer));
    };
    let encoded = secret_resolver.resolve(tls_config_ref).await?;
    if encoded.len() > 2 * 1024 * 1024 {
        return Err(AppError::InvalidInput(
            "SQL TLS secret config exceeds the size limit".to_string(),
        ));
    }
    let config: SqlTlsSecretConfig = serde_json::from_str(&encoded).map_err(|_| {
        AppError::InvalidInput("SQL TLS secret config must be valid JSON".to_string())
    })?;
    let mode = match config.mode.trim().to_ascii_lowercase().as_str() {
        "require" => PgSslMode::Require,
        "verify-ca" => PgSslMode::VerifyCa,
        "verify-full" => PgSslMode::VerifyFull,
        _ => {
            return Err(AppError::InvalidInput(
                "SQL TLS mode must be require, verify-ca, or verify-full".to_string(),
            ));
        }
    };
    if matches!(mode, PgSslMode::VerifyCa | PgSslMode::VerifyFull)
        && config.root_cert_pem.as_deref().is_none_or(str::is_empty)
    {
        return Err(AppError::InvalidInput(
            "SQL verify TLS mode requires root_cert_pem".to_string(),
        ));
    }
    if config.client_cert_pem.is_some() != config.client_key_pem.is_some() {
        return Err(AppError::InvalidInput(
            "SQL TLS client_cert_pem and client_key_pem must be configured together".to_string(),
        ));
    }
    options = options.ssl_mode(mode);
    if let Some(root) = config.root_cert_pem {
        validate_pem(&root, "CERTIFICATE")?;
        options = options.ssl_root_cert_from_pem(root.into_bytes());
    }
    if let (Some(cert), Some(key)) = (config.client_cert_pem, config.client_key_pem) {
        validate_pem(&cert, "CERTIFICATE")?;
        if !(key.contains("-----BEGIN PRIVATE KEY-----")
            || key.contains("-----BEGIN RSA PRIVATE KEY-----"))
        {
            return Err(AppError::InvalidInput(
                "SQL TLS client key is not PEM encoded".to_string(),
            ));
        }
        options = options
            .ssl_client_cert_from_pem(cert.as_bytes())
            .ssl_client_key_from_pem(key.as_bytes());
    }
    Ok(options)
}

fn validate_pem(value: &str, label: &str) -> Result<(), AppError> {
    if value.len() > 1024 * 1024
        || !value.contains(&format!("-----BEGIN {label}-----"))
        || !value.contains(&format!("-----END {label}-----"))
    {
        return Err(AppError::InvalidInput(
            "SQL TLS certificate is not valid bounded PEM".to_string(),
        ));
    }
    Ok(())
}

async fn fetch_postgres_json_rows(
    pool: &PgPool,
    statement_timeout_ms: i32,
    sql: String,
    values: Vec<Value>,
) -> Result<(Value, i64), AppError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(statement_timeout_ms.max(1).to_string())
        .execute(&mut *tx)
        .await?;
    let mut query = sqlx::query(AssertSqlSafe(sql));
    for value in values {
        query = bind_json_value(query, value)?;
    }
    let row = query.fetch_one(&mut *tx).await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok((row.try_get("rows")?, row.try_get("row_count")?))
}

fn required_sql_connection_field<'a>(
    value: Option<&'a str>,
    field: &str,
) -> Result<&'a str, AppError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("sql connection {field} is required")))
}

fn resolve_optional_env_ref(value: &str) -> Result<String, AppError> {
    if value.starts_with("env://") || value.starts_with("env:") {
        secret_resolver::resolve_env_secret_ref(value)
    } else {
        Ok(value.to_string())
    }
}

fn sql_execution_risk_level(
    operation: &str,
    configured_risk_level: &str,
    requires_approval: bool,
) -> String {
    let configured = normalize_risk_level(configured_risk_level).unwrap_or("medium");
    let operation = operation.trim().to_ascii_lowercase();
    if matches!(operation.as_str(), "write" | "ddl") {
        return "critical".to_string();
    }
    if requires_approval && risk_rank(configured) < risk_rank("high") {
        return "high".to_string();
    }
    configured.to_string()
}

fn normalize_risk_level(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "critical" => Some("critical"),
        _ => None,
    }
}

fn risk_rank(value: &str) -> u8 {
    match value {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        "critical" => 3,
        _ => 1,
    }
}

async fn load_third_party_execution_target(
    state: &AppState,
    payload: &ThirdPartyToolCallRequest,
) -> Result<ThirdPartyExecutionTarget, AppError> {
    let row = if let Some(tool_version_id) = payload.tool_version_id {
        sqlx::query(
            r#"
            SELECT t.id AS tool_id, tv.id AS tool_version_id, t.name AS tool_name,
                   tv.schema_snapshot, tv.secret_ref
            FROM tool_versions tv
            JOIN tools t ON t.id = tv.tool_id
            WHERE tv.id = $1
              AND tv.tenant_id = $2
              AND tv.status = 'published'
              AND t.status = 'active'
            "#,
        )
        .bind(tool_version_id)
        .bind(payload.tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
    } else if let Some(tool_id) = payload.tool_id {
        sqlx::query(
            r#"
            SELECT t.id AS tool_id, tv.id AS tool_version_id, t.name AS tool_name,
                   tv.schema_snapshot, tv.secret_ref
            FROM tool_versions tv
            JOIN tools t ON t.id = tv.tool_id
            WHERE t.id = $1
              AND tv.tenant_id = $2
              AND tv.status = 'published'
              AND t.status = 'active'
            ORDER BY tv.created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tool_id)
        .bind(payload.tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
    } else if let Some(tool_name) = payload.tool_name.as_deref() {
        sqlx::query(
            r#"
            SELECT t.id AS tool_id, tv.id AS tool_version_id, t.name AS tool_name,
                   tv.schema_snapshot, tv.secret_ref
            FROM tool_versions tv
            JOIN tools t ON t.id = tv.tool_id
            WHERE t.name = $1
              AND tv.tenant_id = $2
              AND tv.status = 'published'
              AND t.status = 'active'
            ORDER BY tv.created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tool_name)
        .bind(payload.tenant_id)
        .fetch_optional(&state.connect_pool)
        .await?
    } else {
        return Err(AppError::InvalidInput(
            "tool_id, tool_version_id or tool_name is required".to_string(),
        ));
    }
    .ok_or_else(|| AppError::NotFound("third-party tool version not found".to_string()))?;

    Ok(ThirdPartyExecutionTarget {
        tool_id: row.try_get("tool_id")?,
        tool_version_id: row.try_get("tool_version_id")?,
        tool_name: row.try_get("tool_name")?,
        schema_snapshot: row.try_get("schema_snapshot")?,
        secret_ref: row.try_get("secret_ref")?,
    })
}

struct CompiledSql {
    sql: String,
    values: Vec<Value>,
}

fn compile_named_parameters(
    sql_template: &str,
    parameters: &Value,
) -> Result<CompiledSql, AppError> {
    let parameter_map = parameters.as_object().ok_or_else(|| {
        AppError::InvalidInput("SQL tool parameters must be a JSON object".to_string())
    })?;
    let mut sql = String::with_capacity(sql_template.len());
    let mut values = Vec::new();
    let chars = sql_template.chars().collect::<Vec<_>>();
    let mut index = 0_usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            sql.push(current);
            index += 1;
            continue;
        }
        if current == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            sql.push(current);
            index += 1;
            continue;
        }
        if current == ':'
            && !in_single_quote
            && !in_double_quote
            && chars
                .get(index + 1)
                .is_some_and(|next| is_identifier_start(*next))
            && index.checked_sub(1).and_then(|prev| chars.get(prev)) != Some(&':')
        {
            let start = index + 1;
            let mut end = start;
            while chars.get(end).is_some_and(|ch| is_identifier_continue(*ch)) {
                end += 1;
            }
            let name = chars[start..end].iter().collect::<String>();
            let value = parameter_map
                .get(&name)
                .cloned()
                .ok_or_else(|| AppError::InvalidInput(format!("missing SQL parameter: {name}")))?;
            values.push(value);
            sql.push('$');
            sql.push_str(&values.len().to_string());
            index = end;
            continue;
        }
        sql.push(current);
        index += 1;
    }
    Ok(CompiledSql { sql, values })
}

fn bind_json_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: Value,
) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, AppError> {
    Ok(match value {
        Value::Null => query.bind(Option::<String>::None),
        Value::Bool(value) => query.bind(value),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                query.bind(value)
            } else if let Some(value) = number.as_f64() {
                query.bind(value)
            } else {
                return Err(AppError::InvalidInput(
                    "unsigned integer SQL parameters above i64 are not supported".to_string(),
                ));
            }
        }
        Value::String(value) => query.bind(value),
        other => query.bind(sqlx::types::Json(other)),
    })
}

fn validate_read_only_sql(sql: &str) -> Result<(), AppError> {
    let trimmed = sql.trim();
    let lowered = trimmed.to_ascii_lowercase();
    if !(lowered.starts_with("select ") || lowered.starts_with("with ")) {
        return Err(AppError::InvalidInput(
            "read SQL tool template must start with SELECT or WITH".to_string(),
        ));
    }
    if lowered.contains(';') || lowered.contains("--") || lowered.contains("/*") {
        return Err(AppError::InvalidInput(
            "SQL tool template must contain a single read statement without comments".to_string(),
        ));
    }
    for keyword in [
        "insert", "update", "delete", "drop", "alter", "create", "truncate", "grant", "revoke",
        "copy", "call", "execute",
    ] {
        if contains_sql_keyword(&lowered, keyword) {
            return Err(AppError::InvalidInput(format!(
                "read SQL tool template contains forbidden keyword: {keyword}"
            )));
        }
    }
    Ok(())
}

fn contains_sql_keyword(sql: &str, keyword: &str) -> bool {
    sql.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .any(|token| token == keyword)
}

fn is_identifier_start(value: char) -> bool {
    value.is_ascii_alphabetic() || value == '_'
}

fn is_identifier_continue(value: char) -> bool {
    value.is_ascii_alphanumeric() || value == '_'
}

fn http_method(executor: &Value) -> Result<Method, AppError> {
    let method = json_string(executor, "method")
        .unwrap_or_else(|| "POST".to_string())
        .to_ascii_uppercase();
    match method.as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        other => Err(AppError::InvalidInput(format!(
            "unsupported HTTP tool method: {other}"
        ))),
    }
}

fn safe_headers(value: Option<&Value>) -> Result<Vec<(String, String)>, AppError> {
    let Some(Value::Object(headers)) = value else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for (name, value) in headers {
        let lowered = name.to_ascii_lowercase();
        if lowered.contains("authorization")
            || lowered.contains("token")
            || lowered.contains("secret")
            || lowered.contains("key")
        {
            return Err(AppError::InvalidInput(
                "third-party tool headers may not contain secret-bearing fields without a resolver"
                    .to_string(),
            ));
        }
        let header_value = value.as_str().ok_or_else(|| {
            AppError::InvalidInput("third-party tool header values must be strings".to_string())
        })?;
        result.push((name.clone(), header_value.to_string()));
    }
    Ok(result)
}

async fn apply_http_tool_secret_auth(
    secret_resolver: &secret_resolver::SecretResolver,
    request: RequestBuilder,
    executor: &Value,
    secret_ref: Option<&str>,
) -> Result<RequestBuilder, AppError> {
    let Some(secret_ref) = secret_ref else {
        return Ok(request);
    };
    let secret = secret_resolver.resolve(secret_ref).await?;
    let header_name = json_string(executor, "auth_header")
        .or_else(|| json_string(executor, "secret_header"))
        .or_else(|| json_string(executor, "api_key_header"))
        .unwrap_or_else(|| "Authorization".to_string());
    if header_name.trim().is_empty()
        || header_name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-'))
    {
        return Err(AppError::InvalidInput(
            "third-party tool auth header name is invalid".to_string(),
        ));
    }
    let scheme = json_string(executor, "auth_scheme").unwrap_or_else(|| "Bearer".to_string());
    let header_value = if scheme.eq_ignore_ascii_case("none") {
        secret
    } else {
        format!("{scheme} {secret}")
    };
    Ok(request.header(header_name, header_value))
}

fn scalar_query_params(arguments: &Value) -> Result<Vec<(String, String)>, AppError> {
    let object = arguments.as_object().ok_or_else(|| {
        AppError::InvalidInput("GET tool arguments must be a JSON object".to_string())
    })?;
    object
        .iter()
        .map(|(key, value)| {
            let value = match value {
                Value::String(value) => value.clone(),
                Value::Bool(value) => value.to_string(),
                Value::Number(value) => value.to_string(),
                Value::Null => String::new(),
                _ => {
                    return Err(AppError::InvalidInput(
                        "GET tool arguments must be scalar values".to_string(),
                    ));
                }
            };
            Ok((key.clone(), value))
        })
        .collect()
}

fn append_query_params(url: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return url.to_string();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    let query = params
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{url}{separator}{query}")
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            b' ' => vec!['%', '2', '0'],
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

async fn response_json_or_text(response: reqwest::Response) -> Result<Value, AppError> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| AppError::InvalidInput(format!("tool response read failed: {err}")))?;
    if !status.is_success() {
        return Err(AppError::InvalidInput(format!(
            "tool executor returned HTTP {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
        )));
    }
    if content_type.contains("json") {
        serde_json::from_slice(&bytes).map_err(|err| {
            AppError::InvalidInput(format!("tool response JSON parse failed: {err}"))
        })
    } else {
        Ok(json!({
            "content_type": content_type,
            "text": String::from_utf8_lossy(&bytes)
        }))
    }
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

fn redact_for_error(value: &Value) -> String {
    let mut redacted = value.clone();
    redact_value(&mut redacted);
    redacted.to_string()
}

fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => redact_map(map),
        Value::Array(items) => {
            for item in items {
                redact_value(item);
            }
        }
        _ => {}
    }
}

fn redact_map(map: &mut Map<String, Value>) {
    for (key, value) in map.iter_mut() {
        let lowered = key.to_ascii_lowercase();
        if lowered.contains("authorization")
            || lowered.contains("secret")
            || lowered.contains("token")
            || lowered.contains("password")
            || lowered.contains("api_key")
        {
            *value = Value::String("[redacted]".to_string());
        } else {
            redact_value(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::{Json, Router, http::HeaderMap, routing::post};
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::json;
    use sqlx::{PgPool, postgres::PgPoolOptions};
    use tokio::{net::TcpListener, task::JoinHandle};

    use super::*;
    use crate::{
        configuration::{
            AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings, ObjectStoreSettings,
        },
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, models::ActorRef, runtime::AgentRuntimeClient,
            rustfs::RustFsClient,
        },
    };

    #[test]
    fn compile_named_parameters_rewrites_outside_literals_and_preserves_casts() {
        let compiled = compile_named_parameters(
            "SELECT :name::text AS name, ':ignored' AS literal, :limit AS limit",
            &json!({"name": "sales", "limit": 3}),
        )
        .expect("compiled sql");

        assert_eq!(
            compiled.sql,
            "SELECT $1::text AS name, ':ignored' AS literal, $2 AS limit"
        );
        assert_eq!(compiled.values, vec![json!("sales"), json!(3)]);
    }

    #[test]
    fn validate_read_only_sql_rejects_mutation_and_multiple_statements() {
        assert!(validate_read_only_sql("SELECT * FROM memory_items").is_ok());
        assert!(validate_read_only_sql("WITH rows AS (SELECT 1) SELECT * FROM rows").is_ok());
        assert!(validate_read_only_sql("DELETE FROM memory_items").is_err());
        assert!(validate_read_only_sql("SELECT 1; SELECT 2").is_err());
        assert!(validate_read_only_sql("SELECT * FROM users -- comment").is_err());
    }

    #[test]
    fn sql_execution_risk_level_uses_registered_operation_and_approval_flag() {
        assert_eq!(sql_execution_risk_level("read", "medium", false), "medium");
        assert_eq!(sql_execution_risk_level("read", "low", true), "high");
        assert_eq!(
            sql_execution_risk_level("write", "medium", false),
            "critical"
        );
        assert_eq!(sql_execution_risk_level("ddl", "low", true), "critical");
        assert_eq!(
            sql_execution_risk_level("read", "unexpected", false),
            "medium"
        );
    }

    #[test]
    fn safe_headers_reject_secret_bearing_fields() {
        assert!(safe_headers(Some(&json!({"X-Trace": "ok"}))).is_ok());
        assert!(safe_headers(Some(&json!({"Authorization": "Bearer secret"}))).is_err());
        assert!(safe_headers(Some(&json!({"X-Api-Key": "secret"}))).is_err());
    }

    #[tokio::test]
    async fn http_tool_secret_auth_uses_env_backed_secret_ref_without_exposing_secret_in_schema() {
        unsafe {
            std::env::set_var("BIBI_TEST_HTTP_TOOL_TOKEN", "http-tool-secret");
        }
        let http = Client::new();
        let resolver = secret_resolver::SecretResolver::env_only_for_tests();
        let request = apply_http_tool_secret_auth(
            &resolver,
            http.get("http://tools.invalid/test"),
            &json!({"auth_header": "X-Api-Key", "auth_scheme": "none"}),
            Some("env://BIBI_TEST_HTTP_TOOL_TOKEN"),
        )
        .await
        .expect("env-backed tool secret should resolve")
        .build()
        .expect("request should build");

        assert_eq!(
            request
                .headers()
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("http-tool-secret")
        );
    }

    #[tokio::test]
    async fn sql_tls_config_is_resolved_as_bounded_json_and_fails_closed() {
        unsafe {
            std::env::set_var(
                "BIBI_TEST_SQL_TLS_CONFIG",
                serde_json::to_string(&json!({
                    "mode": "verify-full",
                    "root_cert_pem": "-----BEGIN CERTIFICATE-----\nTEST\n-----END CERTIFICATE-----"
                }))
                .unwrap(),
            );
            std::env::set_var(
                "BIBI_TEST_SQL_TLS_INVALID",
                serde_json::to_string(&json!({"mode": "verify-full"})).unwrap(),
            );
        }
        let resolver = secret_resolver::SecretResolver::env_only_for_tests();
        assert!(
            apply_sql_tls_config(
                &resolver,
                PgConnectOptions::new(),
                Some("env://BIBI_TEST_SQL_TLS_CONFIG")
            )
            .await
            .is_ok()
        );
        assert!(
            apply_sql_tls_config(
                &resolver,
                PgConnectOptions::new(),
                Some("env://BIBI_TEST_SQL_TLS_INVALID")
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn sql_username_ref_allows_plain_or_env_value() {
        unsafe {
            std::env::set_var("BIBI_TEST_SQL_USERNAME", "postgres");
        }

        assert_eq!(
            resolve_optional_env_ref("plain_user").expect("plain username"),
            "plain_user"
        );
        assert_eq!(
            resolve_optional_env_ref("env://BIBI_TEST_SQL_USERNAME").expect("env username"),
            "postgres"
        );
    }

    #[test]
    fn scalar_query_params_only_accept_scalars() {
        assert_eq!(
            scalar_query_params(&json!({"q": "sales", "limit": 3})).expect("params"),
            vec![
                ("limit".to_string(), "3".to_string()),
                ("q".to_string(), "sales".to_string())
            ]
        );
        assert!(scalar_query_params(&json!({"nested": {"x": 1}})).is_err());
    }

    #[test]
    fn mcp_risk_uses_trusted_annotations_before_client_visible_name() {
        assert_eq!(
            trusted_mcp_risk_level(
                "delete_everything",
                &json!({"annotations": {"readOnlyHint": true, "destructiveHint": false}})
            ),
            "low"
        );
        assert_eq!(
            trusted_mcp_risk_level(
                "read_records",
                &json!({"annotations": {"readOnlyHint": true, "destructiveHint": true}})
            ),
            "high"
        );
        assert_eq!(trusted_mcp_risk_level("read_records", &json!({})), "medium");
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn registered_read_sql_tool_executes_on_rust_side()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let actor = ActorRef {
            user_id: Uuid::new_v4(),
            device_id: None,
            session_id: None,
            roles: Vec::new(),
        };
        let connection_id = seed_sql_connection(&state.connect_pool, tenant_id).await?;
        let (sql_tool_id, query_hash) =
            seed_sql_tool(&state.connect_pool, tenant_id, connection_id).await?;

        let result = execute_sql_tool(
            &state,
            &SqlToolExecuteRequest {
                tenant_id,
                actor,
                conversation_id: None,
                run_id: None,
                sql_tool_id: Some(sql_tool_id),
                query_hash: Some(query_hash.clone()),
                parameters: json!({"needle": "销售额数据"}),
            },
        )
        .await?;

        assert_eq!(result["query_hash"], query_hash);
        assert_eq!(result["rows"][0]["value"], "销售额数据");
        assert_eq!(result["truncated"], false);
        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn secret_backed_read_sql_tool_executes_with_env_backed_password()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var(
                "BIBI_TEST_SQL_PASSWORD",
                postgres_password_from_database_url(),
            );
        }

        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let actor = ActorRef {
            user_id: Uuid::new_v4(),
            device_id: None,
            session_id: None,
            roles: Vec::new(),
        };
        let connection_id = seed_secret_sql_connection(&state.connect_pool, tenant_id).await?;
        let (sql_tool_id, query_hash) =
            seed_sql_tool(&state.connect_pool, tenant_id, connection_id).await?;

        let result = execute_sql_tool(
            &state,
            &SqlToolExecuteRequest {
                tenant_id,
                actor,
                conversation_id: None,
                run_id: None,
                sql_tool_id: Some(sql_tool_id),
                query_hash: Some(query_hash.clone()),
                parameters: json!({"needle": "secret-backed-sql"}),
            },
        )
        .await?;

        assert_eq!(result["query_hash"], query_hash);
        assert_eq!(result["rows"][0]["value"], "secret-backed-sql");
        assert_eq!(result["truncated"], false);
        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn registered_mcp_and_third_party_tools_execute_via_rust_http_adapter()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let (base_url, server) = spawn_tool_server().await?;
        let actor = ActorRef {
            user_id: Uuid::new_v4(),
            device_id: None,
            session_id: None,
            roles: Vec::new(),
        };
        let (mcp_server_id, mcp_tool_id) =
            seed_mcp_tool(&state.connect_pool, tenant_id, &format!("{base_url}/mcp")).await?;
        let (tool_id, tool_version_id) =
            seed_third_party_tool(&state.connect_pool, tenant_id, &format!("{base_url}/third"))
                .await?;

        let mcp_result = execute_mcp_tool(
            &state,
            &McpToolCallRequest {
                tenant_id,
                actor: actor.clone(),
                conversation_id: None,
                run_id: None,
                mcp_server_id: Some(mcp_server_id),
                mcp_tool_id: Some(mcp_tool_id),
                tool_name: "lookup_sales".to_string(),
                arguments: json!({"q": "sales"}),
            },
        )
        .await?;
        assert_eq!(mcp_result["result"]["content"][0]["text"], "mcp-result");

        let third_party_result = execute_third_party_tool(
            &state,
            &ThirdPartyToolCallRequest {
                tenant_id,
                actor,
                conversation_id: None,
                run_id: None,
                tool_id: Some(tool_id),
                tool_version_id: Some(tool_version_id),
                tool_name: None,
                arguments: json!({"q": "sales"}),
            },
        )
        .await?;
        assert_eq!(third_party_result["status"], "ok");
        assert_eq!(third_party_result["content"], "third-party-result");

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        server.abort();
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn stdio_mcp_tool_dispatches_to_the_actor_device_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let (user_id, device_id) = seed_actor_device(&state.connect_pool, tenant_id).await?;
        let (mcp_server_id, mcp_tool_id) =
            seed_stdio_mcp_tool(&state.connect_pool, tenant_id).await?;
        let pool = state.connect_pool.clone();

        let simulator = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(row) = sqlx::query(
                    r#"
                    UPDATE local_exec_requests
                    SET status = 'dispatching', updated_at = CURRENT_TIMESTAMP
                    WHERE id = (
                        SELECT id FROM local_exec_requests
                        WHERE tenant_id = $1 AND device_id = $2 AND status = 'queued'
                          AND command->>'kind' = 'mcp_stdio'
                        ORDER BY created_at ASC LIMIT 1
                        FOR UPDATE SKIP LOCKED
                    )
                    RETURNING id, command
                    "#,
                )
                .bind(tenant_id)
                .bind(device_id)
                .fetch_optional(&pool)
                .await?
                {
                    let request_id: Uuid = row.try_get("id")?;
                    let command: Value = row.try_get("command")?;
                    assert_eq!(command["protocol"], "local_runtime.v1");
                    assert_eq!(command["tool"]["name"], "stdio_fixture_health");
                    assert_eq!(
                        command["transport"]["env"]["API_TOKEN"],
                        "env://MCP_TEST_TOKEN"
                    );
                    let mut tx = pool.begin().await?;
                    sqlx::query(
                        "UPDATE local_exec_requests SET status = 'completed', updated_at = CURRENT_TIMESTAMP WHERE id = $1",
                    )
                    .bind(request_id)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO local_exec_events
                            (tenant_id, local_exec_request_id, type, payload)
                        VALUES ($1, $2, 'local_exec.completed', $3)
                        "#,
                    )
                    .bind(tenant_id)
                    .bind(request_id)
                    .bind(json!({
                        "status": "completed",
                        "result": {
                            "content": [{"type": "text", "text": "stdio-ok"}],
                            "structuredContent": {"status": "ok"},
                            "isError": false
                        }
                    }))
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok::<(), sqlx::Error>(());
                }
                if tokio::time::Instant::now() >= deadline {
                    panic!("stdio MCP work item was not queued");
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });

        let result = execute_mcp_tool(
            &state,
            &McpToolCallRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: Some(device_id),
                    session_id: None,
                    roles: Vec::new(),
                },
                conversation_id: None,
                run_id: None,
                mcp_server_id: Some(mcp_server_id),
                mcp_tool_id: Some(mcp_tool_id),
                tool_name: "stdio_fixture_health".to_string(),
                arguments: json!({"probe": true}),
            },
        )
        .await?;

        simulator.await??;
        assert_eq!(result["content"][0]["text"], "stdio-ok");
        assert_eq!(result["mcp_server_id"], mcp_server_id.to_string());
        assert_eq!(result["mcp_tool_id"], mcp_tool_id.to_string());
        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn secret_backed_mcp_tool_executes_with_env_backed_auth_header()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_MCP_EXEC_TOKEN", "mcp-exec-secret");
        }

        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let (base_url, server) = spawn_secret_mcp_tool_server().await?;
        let actor = ActorRef {
            user_id: Uuid::new_v4(),
            device_id: None,
            session_id: None,
            roles: Vec::new(),
        };
        let (mcp_server_id, mcp_tool_id) = seed_secret_mcp_tool(
            &state.connect_pool,
            tenant_id,
            &format!("{base_url}/mcp-secret"),
        )
        .await?;

        let mcp_result = execute_mcp_tool(
            &state,
            &McpToolCallRequest {
                tenant_id,
                actor,
                conversation_id: None,
                run_id: None,
                mcp_server_id: Some(mcp_server_id),
                mcp_tool_id: Some(mcp_tool_id),
                tool_name: "secured_lookup".to_string(),
                arguments: json!({"q": "sales"}),
            },
        )
        .await?;

        assert_eq!(
            mcp_result["result"]["content"][0]["text"],
            "secret-mcp-result"
        );

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        server.abort();
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn secret_backed_third_party_tool_executes_with_env_backed_auth_header()
    -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            std::env::set_var("BIBI_TEST_HTTP_TOOL_TOKEN", "http-tool-secret");
        }

        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let (base_url, server) = spawn_secret_third_party_tool_server().await?;
        let actor = ActorRef {
            user_id: Uuid::new_v4(),
            device_id: None,
            session_id: None,
            roles: Vec::new(),
        };
        let (tool_id, tool_version_id) = seed_secret_third_party_tool(
            &state.connect_pool,
            tenant_id,
            &format!("{base_url}/third-secret"),
        )
        .await?;

        let third_party_result = execute_third_party_tool(
            &state,
            &ThirdPartyToolCallRequest {
                tenant_id,
                actor,
                conversation_id: None,
                run_id: None,
                tool_id: Some(tool_id),
                tool_version_id: Some(tool_version_id),
                tool_name: None,
                arguments: json!({"q": "sales"}),
            },
        )
        .await?;

        assert_eq!(third_party_result["status"], "ok");
        assert_eq!(third_party_result["content"], "secret-third-party-result");

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        server.abort();
        Ok(())
    }

    async fn test_state() -> Result<AppState, Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(AppState {
            connect_pool: pool.clone(),
            redis_client: RedisClient::open(redis_url)?,
            ferriskey_oidc: FerrisKeyOidcVerifier::new(FerrisKeySettings {
                issuer: "http://localhost:3333/realms/bibi-work".to_string(),
                audience: "bibi-work-backend".to_string(),
                trusted_authorized_parties: Vec::new(),
                discovery_url:
                    "http://localhost:3333/realms/bibi-work/.well-known/openid-configuration"
                        .to_string(),
                jwks_uri: None,
                default_tenant_slug: "bibi-work".to_string(),
                timeout_milliseconds: 1000,
            })?,
            authz_service: ResourceAuthzService::new(pool),
            agent_runtime_client: AgentRuntimeClient::new(AgentRuntimeSettings {
                base_url: None,
                shared_token: secret("test-internal-token"),
                timeout_milliseconds: 1000,
            })?,
            rustfs_client: RustFsClient::new(ObjectStoreSettings {
                enabled: false,
                endpoint: "http://127.0.0.1:9000".to_string(),
                access_key: secret("test"),
                secret_key: secret("test"),
                region: "local".to_string(),
                files_bucket: "test-files".to_string(),
                audit_bucket: "test-audit".to_string(),
                timeout_milliseconds: 1000,
            })?,
            memory_vector_client: MemoryVectorClient::new(MemoryVectorSettings {
                enabled: false,
                embedding_endpoint: None,
                qdrant_rest_url: None,
                qdrant_collection: "test_memories".to_string(),
                timeout_milliseconds: 1000,
                max_context_chars: 1200,
                worker_interval_milliseconds: 1000,
                worker_batch_size: 1,
                worker_max_attempts: 1,
            })?,
            internal_shared_token: "test-internal-token".to_string(),
            audit_partition_cleanup_enabled: false,
            secret_resolver: secret_resolver::SecretResolver::env_only_for_tests(),
            credential_rotation_worker_enabled: false,
        })
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }

    async fn seed_tenant(pool: &PgPool) -> Result<Uuid, sqlx::Error> {
        let suffix = Uuid::new_v4();
        sqlx::query_scalar(
            r#"
            INSERT INTO tenants (name, slug, metadata)
            VALUES ($1, $2, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(format!("Tool Execution Test {suffix}"))
        .bind(format!("tool-exec-test-{suffix}"))
        .fetch_one(pool)
        .await
    }

    async fn seed_actor_device(
        pool: &PgPool,
        tenant_id: Uuid,
    ) -> Result<(Uuid, Uuid), sqlx::Error> {
        let suffix = Uuid::new_v4();
        let user_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO platform_users (tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("stdio-user-{suffix}"))
        .bind(format!("stdio-user-{suffix}"))
        .fetch_one(pool)
        .await?;
        let device_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO devices
                (tenant_id, user_id, device_fingerprint, device_name, platform)
            VALUES ($1, $2, $3, 'stdio-test-device', 'test')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("stdio-device-{suffix}"))
        .fetch_one(pool)
        .await?;
        Ok((user_id, device_id))
    }

    async fn seed_sql_connection(pool: &PgPool, tenant_id: Uuid) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            INSERT INTO sql_connections (
                tenant_id, name, database_kind, host, port, database_name,
                max_rows, statement_timeout_ms, status
            )
            VALUES ($1, $2, 'postgres', '127.0.0.1', 5433, 'bibi_work', 10, 1000, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("local-postgres-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
    }

    async fn seed_secret_sql_connection(
        pool: &PgPool,
        tenant_id: Uuid,
    ) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            INSERT INTO sql_connections (
                tenant_id, name, database_kind, host, port, database_name,
                username_ref, password_secret_ref, max_rows, statement_timeout_ms, status
            )
            VALUES (
                $1, $2, 'postgres', '127.0.0.1', 5433, 'bibi_work',
                'postgres', 'env://BIBI_TEST_SQL_PASSWORD', 10, 1000, 'active'
            )
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("secret-local-postgres-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
    }

    async fn seed_sql_tool(
        pool: &PgPool,
        tenant_id: Uuid,
        connection_id: Uuid,
    ) -> Result<(Uuid, String), sqlx::Error> {
        let suffix = Uuid::new_v4();
        let sql_tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO sql_tools (tenant_id, name, status)
            VALUES ($1, $2, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("sales-lookup-{suffix}"))
        .fetch_one(pool)
        .await?;
        let query_hash = format!("sha256:{suffix}");
        sqlx::query(
            r#"
            INSERT INTO sql_tool_versions (
                tenant_id, sql_tool_id, connection_id, version_label, operation,
                parameter_schema, sql_template, query_hash, requires_approval, status
            )
            VALUES ($1, $2, $3, 'v1', 'read', '{}'::jsonb, $4, $5, false, 'published')
            "#,
        )
        .bind(tenant_id)
        .bind(sql_tool_id)
        .bind(connection_id)
        .bind("SELECT :needle::text AS value")
        .bind(&query_hash)
        .execute(pool)
        .await?;
        Ok((sql_tool_id, query_hash))
    }

    async fn seed_mcp_tool(
        pool: &PgPool,
        tenant_id: Uuid,
        endpoint: &str,
    ) -> Result<(Uuid, Uuid), sqlx::Error> {
        let mcp_server_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mcp_servers (tenant_id, name, transport, config, status)
            VALUES ($1, $2, 'http', $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("mcp-server-{}", Uuid::new_v4()))
        .bind(json!({"endpoint": endpoint}))
        .fetch_one(pool)
        .await?;
        let mcp_tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mcp_tools (tenant_id, mcp_server_id, name, status)
            VALUES ($1, $2, 'lookup_sales', 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(mcp_server_id)
        .fetch_one(pool)
        .await?;
        Ok((mcp_server_id, mcp_tool_id))
    }

    async fn seed_stdio_mcp_tool(
        pool: &PgPool,
        tenant_id: Uuid,
    ) -> Result<(Uuid, Uuid), sqlx::Error> {
        let mcp_server_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mcp_servers (tenant_id, name, transport, config, status)
            VALUES ($1, $2, 'stdio', $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("stdio-mcp-server-{}", Uuid::new_v4()))
        .bind(json!({
            "command": "/usr/bin/node",
            "args": ["fixture.mjs"],
            "env": {"API_TOKEN": "env://MCP_TEST_TOKEN"},
            "timeout_ms": 5_000
        }))
        .fetch_one(pool)
        .await?;
        let mcp_tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mcp_tools (tenant_id, mcp_server_id, name, status)
            VALUES ($1, $2, 'stdio_fixture_health', 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(mcp_server_id)
        .fetch_one(pool)
        .await?;
        Ok((mcp_server_id, mcp_tool_id))
    }

    async fn seed_secret_mcp_tool(
        pool: &PgPool,
        tenant_id: Uuid,
        endpoint: &str,
    ) -> Result<(Uuid, Uuid), sqlx::Error> {
        let mcp_server_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mcp_servers (
                tenant_id, name, transport, config, secret_ref, status
            )
            VALUES ($1, $2, 'http', $3, 'env://BIBI_TEST_MCP_EXEC_TOKEN', 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("secret-mcp-server-{}", Uuid::new_v4()))
        .bind(json!({
            "endpoint": endpoint,
            "auth_header": "Authorization",
            "auth_scheme": "Bearer"
        }))
        .fetch_one(pool)
        .await?;
        let mcp_tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mcp_tools (tenant_id, mcp_server_id, name, status)
            VALUES ($1, $2, 'secured_lookup', 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(mcp_server_id)
        .fetch_one(pool)
        .await?;
        Ok((mcp_server_id, mcp_tool_id))
    }

    async fn seed_third_party_tool(
        pool: &PgPool,
        tenant_id: Uuid,
        endpoint: &str,
    ) -> Result<(Uuid, Uuid), sqlx::Error> {
        let suffix = Uuid::new_v4();
        let tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tools (tenant_id, name, tool_type, schema, status)
            VALUES ($1, $2, 'third_party', '{}'::jsonb, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("third-party-{suffix}"))
        .fetch_one(pool)
        .await?;
        let tool_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tool_versions (
                tenant_id, tool_id, version_label, schema_snapshot, status
            )
            VALUES ($1, $2, 'v1', $3, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(tool_id)
        .bind(json!({
            "executor": {
                "type": "http",
                "url": endpoint,
                "method": "POST"
            }
        }))
        .fetch_one(pool)
        .await?;
        Ok((tool_id, tool_version_id))
    }

    async fn seed_secret_third_party_tool(
        pool: &PgPool,
        tenant_id: Uuid,
        endpoint: &str,
    ) -> Result<(Uuid, Uuid), sqlx::Error> {
        let suffix = Uuid::new_v4();
        let tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tools (tenant_id, name, tool_type, schema, status)
            VALUES ($1, $2, 'third_party', '{}'::jsonb, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("secret-third-party-{suffix}"))
        .fetch_one(pool)
        .await?;
        let tool_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tool_versions (
                tenant_id, tool_id, version_label, schema_snapshot, secret_ref, status
            )
            VALUES ($1, $2, 'v1', $3, 'env://BIBI_TEST_HTTP_TOOL_TOKEN', 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(tool_id)
        .bind(json!({
            "executor": {
                "type": "http",
                "url": endpoint,
                "method": "POST",
                "auth_header": "Authorization",
                "auth_scheme": "Bearer"
            }
        }))
        .fetch_one(pool)
        .await?;
        Ok((tool_id, tool_version_id))
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn spawn_tool_server() -> Result<(String, JoinHandle<()>), std::io::Error> {
        async fn mcp(Json(_payload): Json<Value>) -> Json<Value> {
            Json(json!({
                "jsonrpc": "2.0",
                "id": "test",
                "result": {
                    "content": [{"type": "text", "text": "mcp-result"}]
                }
            }))
        }
        async fn third(Json(_payload): Json<Value>) -> Json<Value> {
            Json(json!({
                "status": "ok",
                "content": "third-party-result"
            }))
        }

        let router = Router::new()
            .route("/mcp", post(mcp))
            .route("/third", post(third));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok((base_url, handle))
    }

    async fn spawn_secret_mcp_tool_server() -> Result<(String, JoinHandle<()>), std::io::Error> {
        async fn mcp_secret(headers: HeaderMap, Json(_payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer mcp-exec-secret")
            );
            Json(json!({
                "jsonrpc": "2.0",
                "id": "test",
                "result": {
                    "content": [{"type": "text", "text": "secret-mcp-result"}]
                }
            }))
        }

        let router = Router::new().route("/mcp-secret", post(mcp_secret));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok((base_url, handle))
    }

    async fn spawn_secret_third_party_tool_server()
    -> Result<(String, JoinHandle<()>), std::io::Error> {
        async fn third_secret(headers: HeaderMap, Json(_payload): Json<Value>) -> Json<Value> {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer http-tool-secret")
            );
            Json(json!({
                "status": "ok",
                "content": "secret-third-party-result"
            }))
        }

        let router = Router::new().route("/third-secret", post(third_secret));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok((base_url, handle))
    }

    fn postgres_password_from_database_url() -> String {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        database_url
            .split('@')
            .next()
            .and_then(|auth| auth.rsplit_once(':').map(|(_, password)| password))
            .filter(|password| !password.is_empty())
            .unwrap_or("password")
            .to_string()
    }
}
