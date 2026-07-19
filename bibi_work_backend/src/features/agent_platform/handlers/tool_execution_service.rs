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
    let authz_target = tool_execution::load_mcp_execution_authz_target(&state, &payload).await?;
    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        payload.actor.clone(),
        "execute",
        "mcp_tool",
        authz_target.mcp_tool_id.to_string(),
        Some(AuthzContext {
            conversation_id: payload.conversation_id,
            run_id: payload.run_id,
            mcp_server_id: Some(authz_target.mcp_server_id),
            risk_level: Some(authz_target.risk_level),
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
    let authz_target = tool_execution::load_sql_execution_authz_target(&state, &payload).await?;
    let (resource_type, resource_id) = if payload.sql_tool_id.is_some() {
        ("sql_tool", authz_target.sql_tool_id.to_string())
    } else {
        ("sql_query", authz_target.query_hash.clone())
    };
    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        payload.actor.clone(),
        "execute",
        resource_type,
        resource_id,
        Some(AuthzContext {
            conversation_id: payload.conversation_id,
            run_id: payload.run_id,
            args_hash: Some(authz_target.query_hash),
            risk_level: Some(authz_target.risk_level),
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

#[cfg(test)]
mod tests {
    use axum::{Json, extract::State};
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::json;
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};
    use uuid::Uuid;

    use super::*;
    use crate::{
        configuration::{
            AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings, ObjectStoreSettings,
        },
        features::{
            agent_platform::{
                authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
                memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient,
                rustfs::RustFsClient,
            },
            core::errors::AppError,
        },
    };

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn sql_write_tool_execute_authz_is_critical_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, sql_tool_id, query_hash) =
            seed_write_sql_tool_context(&state.connect_pool).await?;

        let result = execute_sql_tool(
            State(state.clone()),
            Json(SqlToolExecuteRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                conversation_id: None,
                run_id: None,
                sql_tool_id: Some(sql_tool_id),
                query_hash: Some(query_hash.clone()),
                parameters: json!({"id": 1}),
            }),
        )
        .await;

        let error = match result {
            Ok(_) => panic!("write SQL tool should be denied before execution"),
            Err(error) => error,
        };
        assert!(
            matches!(
                &error,
                AppError::PermissionDenied(message)
                    if message.contains("resource=sql_tool:")
                        && message.contains("critical_risk_requires_explicit_policy")
            ),
            "unexpected error: {error:?}"
        );

        let row = sqlx::query(
            r#"
            SELECT resource_type, resource_id, action, decision, reason_code,
                   context->>'risk_level' AS risk_level,
                   context->>'args_hash' AS args_hash
            FROM authz_decisions
            WHERE tenant_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(row.try_get::<String, _>("resource_type")?, "sql_tool");
        assert_eq!(
            row.try_get::<String, _>("resource_id")?,
            sql_tool_id.to_string()
        );
        assert_eq!(row.try_get::<String, _>("action")?, "execute");
        assert_eq!(row.try_get::<String, _>("decision")?, "deny");
        assert_eq!(
            row.try_get::<String, _>("reason_code")?,
            "critical_risk_requires_explicit_policy"
        );
        assert_eq!(row.try_get::<String, _>("risk_level")?, "critical");
        assert_eq!(row.try_get::<String, _>("args_hash")?, query_hash);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    async fn seed_write_sql_tool_context(
        pool: &PgPool,
    ) -> Result<(Uuid, Uuid, Uuid, String), sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'SQL Authz Test', $2)")
            .bind(tenant_id)
            .bind(format!("sql-authz-test-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, 'sql-authz-subject', 'sql-authz-user', 'active')
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'member')
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;

        let connection_id: Uuid = sqlx::query_scalar(
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
        .bind(format!("sql-authz-conn-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        let sql_tool_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO sql_tools (tenant_id, name, status)
            VALUES ($1, $2, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("sql-write-tool-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        let query_hash = format!("sha256:{}", Uuid::new_v4());
        sqlx::query(
            r#"
            INSERT INTO sql_tool_versions (
                tenant_id, sql_tool_id, connection_id, version_label, operation,
                parameter_schema, sql_template, query_hash, risk_level,
                requires_approval, status
            )
            VALUES (
                $1, $2, $3, 'v1', 'write', '{}'::jsonb,
                'UPDATE platform_users SET username = username WHERE id = :id',
                $4, 'medium', false, 'published'
            )
            "#,
        )
        .bind(tenant_id)
        .bind(sql_tool_id)
        .bind(connection_id)
        .bind(&query_hash)
        .execute(pool)
        .await?;

        Ok((tenant_id, user_id, sql_tool_id, query_hash))
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
            secret_resolver:
                crate::features::agent_platform::secret_resolver::SecretResolver::env_only_for_tests(
                ),
            credential_rotation_worker_enabled: false,
        })
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
