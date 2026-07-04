use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext, mcp_discovery,
            mcp_discovery::DiscoveredMcpTool, models::*,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn list_mcp_servers(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<ResourceResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status,
               jsonb_build_object(
                   'transport', transport,
                   'has_config', config <> '{}'::jsonb,
                   'has_secret_ref', secret_ref IS NOT NULL
               ) AS metadata,
               created_at, updated_at
        FROM mcp_servers
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND ($2::text IS NULL OR status = $2)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;

    let servers = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(servers))
}

pub async fn get_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_server_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status,
               jsonb_build_object(
                   'transport', transport,
                   'has_config', config <> '{}'::jsonb,
                   'has_secret_ref', secret_ref IS NOT NULL
               ) AS metadata,
               created_at, updated_at
        FROM mcp_servers
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(mcp_server_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_server_id): Path<Uuid>,
    Json(payload): Json<UpdateMcpServerRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "mcp_server",
        mcp_server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(mcp_server_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE mcp_servers
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            transport = COALESCE($5, transport),
            config = COALESCE($6, config),
            secret_ref = COALESCE($7, secret_ref),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status,
                  jsonb_build_object(
                      'transport', transport,
                      'has_config', config <> '{}'::jsonb,
                      'has_secret_ref', secret_ref IS NOT NULL
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(mcp_server_id)
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.transport)
    .bind(payload.config)
    .bind(payload.secret_ref)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_server_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "mcp_server",
        mcp_server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(mcp_server_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE mcp_servers
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status,
                  jsonb_build_object(
                      'transport', transport,
                      'has_config', config <> '{}'::jsonb,
                      'has_secret_ref', secret_ref IS NOT NULL
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(mcp_server_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_mcp_server(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateMcpServerRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "mcp_server").await?;
    let row = sqlx::query(
        r#"
        INSERT INTO mcp_servers (
            tenant_id, name, description, transport, config, secret_ref
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, name, description, status,
                  jsonb_build_object(
                      'transport', transport,
                      'has_config', config <> '{}'::jsonb,
                      'has_secret_ref', secret_ref IS NOT NULL
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.transport.unwrap_or_else(|| "http".to_string()))
    .bind(payload.config.unwrap_or_else(|| json!({})))
    .bind(payload.secret_ref)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_mcp_tools(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_server_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<Vec<ResourceResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status,
               jsonb_build_object(
                   'mcp_server_id', mcp_server_id,
                   'schema', schema,
                   'schema_hash', schema_hash
               ) AS metadata,
               created_at, updated_at
        FROM mcp_tools
        WHERE tenant_id = $1
          AND mcp_server_id = $2
          AND ($3::text IS NULL OR status = $3)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT $4
        "#,
    )
    .bind(query.tenant_id)
    .bind(mcp_server_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;
    let tools = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(tools))
}

pub async fn get_mcp_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_tool_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status,
               jsonb_build_object(
                   'mcp_server_id', mcp_server_id,
                   'schema', schema,
                   'schema_hash', schema_hash
               ) AS metadata,
               created_at, updated_at
        FROM mcp_tools
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(mcp_tool_id)
    .bind(query.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp tool not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn publish_mcp_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_server_id): Path<Uuid>,
    Json(payload): Json<UpsertMcpToolRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "mcp_server",
        mcp_server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(mcp_server_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO mcp_tools (
            tenant_id, mcp_server_id, name, description, schema, schema_hash
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (mcp_server_id, name)
        DO UPDATE SET
            description = EXCLUDED.description,
            schema = EXCLUDED.schema,
            schema_hash = EXCLUDED.schema_hash,
            updated_at = CURRENT_TIMESTAMP
        RETURNING id, tenant_id, name, description, status,
                  jsonb_build_object('mcp_server_id', mcp_server_id, 'schema', schema, 'schema_hash', schema_hash) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(mcp_server_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.schema.unwrap_or_else(|| json!({})))
    .bind(payload.schema_hash)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_mcp_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_tool_id): Path<Uuid>,
    Json(payload): Json<UpdateMcpToolRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let server_id = mcp_tool_server_id(&state, payload.tenant_id, mcp_tool_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "mcp_tool",
        mcp_tool_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(server_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE mcp_tools
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            schema = COALESCE($5, schema),
            schema_hash = COALESCE($6, schema_hash),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, name, description, status,
                  jsonb_build_object(
                      'mcp_server_id', mcp_server_id,
                      'schema', schema,
                      'schema_hash', schema_hash
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(mcp_tool_id)
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.schema)
    .bind(payload.schema_hash)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp tool not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_mcp_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_tool_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let server_id = mcp_tool_server_id(&state, payload.tenant_id, mcp_tool_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "mcp_tool",
        mcp_tool_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(server_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE mcp_tools
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, name, description, status,
                  jsonb_build_object(
                      'mcp_server_id', mcp_server_id,
                      'schema', schema,
                      'schema_hash', schema_hash
                  ) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(mcp_tool_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp tool not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn discover_mcp_tools(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(mcp_server_id): Path<Uuid>,
    Json(payload): Json<DiscoverMcpToolsRequest>,
) -> Result<Json<Vec<ResourceResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "mcp_server",
        mcp_server_id.to_string(),
        Some(AuthzContext {
            mcp_server_id: Some(mcp_server_id),
            ..Default::default()
        }),
    )
    .await?;

    let row = sqlx::query(
        r#"
        SELECT transport, config, secret_ref
        FROM mcp_servers
        WHERE id = $1
          AND tenant_id = $2
          AND status = 'active'
          AND deleted_at IS NULL
        "#,
    )
    .bind(mcp_server_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp server not found".to_string()))?;
    let transport: String = row.try_get("transport")?;
    let config: serde_json::Value = row.try_get("config")?;
    let secret_ref: Option<String> = row.try_get("secret_ref")?;

    let discovered =
        mcp_discovery::discover_mcp_tools(&transport, &config, secret_ref.as_deref()).await?;
    let tools = upsert_discovered_mcp_tools(
        &state.connect_pool,
        payload.tenant_id,
        mcp_server_id,
        &discovered,
    )
    .await?;

    Ok(Json(tools))
}

async fn mcp_tool_server_id(
    state: &AppState,
    tenant_id: Uuid,
    mcp_tool_id: Uuid,
) -> Result<Uuid, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT mcp_server_id
        FROM mcp_tools
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(mcp_tool_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("mcp tool not found".to_string()))
}

async fn upsert_discovered_mcp_tools(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    mcp_server_id: Uuid,
    tools: &[DiscoveredMcpTool],
) -> Result<Vec<ResourceResponse>, AppError> {
    let mut tx = pool.begin().await?;
    let mut responses = Vec::with_capacity(tools.len());
    for tool in tools {
        let row = sqlx::query(
            r#"
            INSERT INTO mcp_tools (
                tenant_id, mcp_server_id, name, description, schema, schema_hash, status
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'active')
            ON CONFLICT (mcp_server_id, name)
            DO UPDATE SET
                description = EXCLUDED.description,
                schema = EXCLUDED.schema,
                schema_hash = EXCLUDED.schema_hash,
                status = 'active',
                updated_at = CURRENT_TIMESTAMP
            RETURNING id, tenant_id, name, description, status,
                      jsonb_build_object(
                          'mcp_server_id', mcp_server_id,
                          'schema', schema,
                          'schema_hash', schema_hash,
                          'discovered', true
                      ) AS metadata,
                      created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(mcp_server_id)
        .bind(&tool.name)
        .bind(&tool.description)
        .bind(&tool.schema)
        .bind(&tool.schema_hash)
        .fetch_one(&mut *tx)
        .await?;
        responses.push(resource_from_row(row)?);
    }

    sqlx::query(
        r#"
        UPDATE mcp_servers
        SET updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(mcp_server_id)
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(responses)
}

#[cfg(test)]
mod tests {
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::json;
    use sqlx::{PgPool, postgres::PgPoolOptions};

    use super::*;
    use crate::{
        configuration::{
            AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings, ObjectStoreSettings,
        },
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
    };

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn discovered_mcp_tools_are_upserted_with_schema_hash()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let tenant_id = seed_tenant(&state.connect_pool).await?;
        let mcp_server_id = seed_mcp_server(&state.connect_pool, tenant_id).await?;
        let first = DiscoveredMcpTool {
            name: "lookup_sales".to_string(),
            description: Some("Lookup sales".to_string()),
            schema: json!({"inputSchema": {"type": "object", "properties": {}}}),
            schema_hash: "sha256:first".to_string(),
        };
        let second = DiscoveredMcpTool {
            name: "lookup_sales".to_string(),
            description: Some("Updated lookup sales".to_string()),
            schema: json!({"inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}}),
            schema_hash: "sha256:second".to_string(),
        };

        let created =
            upsert_discovered_mcp_tools(&state.connect_pool, tenant_id, mcp_server_id, &[first])
                .await?;
        let updated =
            upsert_discovered_mcp_tools(&state.connect_pool, tenant_id, mcp_server_id, &[second])
                .await?;

        assert_eq!(created[0].id, updated[0].id);
        assert_eq!(
            updated[0].description.as_deref(),
            Some("Updated lookup sales")
        );
        assert_eq!(updated[0].metadata["schema_hash"], "sha256:second");

        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM mcp_tools
            WHERE tenant_id = $1 AND mcp_server_id = $2 AND name = 'lookup_sales'
            "#,
        )
        .bind(tenant_id)
        .bind(mcp_server_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(count, 1);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
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
        .bind(format!("MCP catalog test {suffix}"))
        .bind(format!("mcp-catalog-test-{suffix}"))
        .fetch_one(pool)
        .await
    }

    async fn seed_mcp_server(pool: &PgPool, tenant_id: Uuid) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            INSERT INTO mcp_servers (tenant_id, name, transport, config, status)
            VALUES ($1, $2, 'http', '{}'::jsonb, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("mcp-server-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}
