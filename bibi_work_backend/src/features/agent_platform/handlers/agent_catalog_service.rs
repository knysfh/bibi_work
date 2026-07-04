use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::json;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::{errors::AppError, models::GenericResponse},
    },
    startup::AppState,
};

use super::support::*;

pub async fn list_agents(
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
        SELECT id, tenant_id, name, description, status, metadata, created_at, updated_at
        FROM agents
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

    let agents = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(agents))
}

pub async fn get_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status, metadata, created_at, updated_at
        FROM agents
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<UpdateAgentRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE agents
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            draft_config = COALESCE($5, draft_config),
            metadata = COALESCE($6, metadata),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(agent_id)
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.draft_config)
    .bind(payload.metadata)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE agents
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(agent_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateAgentRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "create",
        "agent",
        "new".to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO agents (tenant_id, owner_user_id, name, description, draft_config, metadata)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.draft_config.unwrap_or_else(|| json!({})))
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_agent_versions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<Vec<VersionResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, agent_id AS parent_id, version_label,
               config_snapshot AS snapshot, policy_version, status, created_at
        FROM agent_versions
        WHERE tenant_id = $1
          AND agent_id = $2
          AND ($3::text IS NULL OR status = $3)
        ORDER BY created_at DESC
        LIMIT $4
        "#,
    )
    .bind(query.tenant_id)
    .bind(agent_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;

    let versions = rows
        .into_iter()
        .map(version_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(versions))
}

pub async fn get_agent_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_version_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, agent_id AS parent_id, version_label,
               config_snapshot AS snapshot, policy_version, status, created_at
        FROM agent_versions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(agent_version_id)
    .bind(query.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent version not found".to_string()))?;

    Ok(Json(version_from_row(row)?))
}

pub async fn publish_agent_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<PublishVersionRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "agent",
        agent_id.to_string(),
        Some(AuthzContext {
            agent_id: Some(agent_id),
            ..Default::default()
        }),
    )
    .await?;
    let snapshot = payload.snapshot.unwrap_or_else(|| json!({}));
    validate_agent_version_model_profile(&state.connect_pool, payload.tenant_id, &snapshot).await?;
    let row = sqlx::query(
        r#"
        INSERT INTO agent_versions (
            tenant_id, agent_id, version_label, config_snapshot, policy_version, schema_hash
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, agent_id AS parent_id, version_label,
                  config_snapshot AS snapshot, policy_version, status, created_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(agent_id)
    .bind(payload.version_label)
    .bind(snapshot)
    .bind(
        payload
            .policy_version
            .unwrap_or_else(|| LOCAL_POLICY_VERSION.to_string()),
    )
    .bind(payload.schema_hash)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(version_from_row(row)?))
}

pub async fn disable_agent_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_version_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "agent_version",
        agent_version_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE agent_versions
        SET status = 'disabled'
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, agent_id AS parent_id, version_label,
                  config_snapshot AS snapshot, policy_version, status, created_at
        "#,
    )
    .bind(agent_version_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent version not found".to_string()))?;

    Ok(Json(version_from_row(row)?))
}

pub async fn get_agent_version_effective_capabilities(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_version_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<AgentVersionCapabilitiesResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        query.tenant_id,
        "read",
        "agent_version",
        agent_version_id.to_string(),
        None,
    )
    .await?;

    let version_row = sqlx::query(
        r#"
        SELECT id, tenant_id, agent_id, version_label, config_snapshot,
               policy_version, status
        FROM agent_versions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(agent_version_id)
    .bind(query.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent version not found".to_string()))?;

    Ok(Json(AgentVersionCapabilitiesResponse {
        agent_version_id,
        tenant_id: version_row.try_get("tenant_id")?,
        agent_id: version_row.try_get("agent_id")?,
        version_label: version_row.try_get("version_label")?,
        status: version_row.try_get("status")?,
        policy_version: version_row.try_get("policy_version")?,
        config_snapshot: version_row.try_get("config_snapshot")?,
        skills: load_agent_version_skill_capabilities(
            &state.connect_pool,
            query.tenant_id,
            agent_version_id,
        )
        .await?,
        tools: load_agent_version_tool_capabilities(
            &state.connect_pool,
            query.tenant_id,
            agent_version_id,
        )
        .await?,
        mcp_tools: load_agent_version_mcp_capabilities(
            &state.connect_pool,
            query.tenant_id,
            agent_version_id,
        )
        .await?,
    }))
}

pub async fn validate_agent_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_version_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ValidationResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "read",
        "agent_version",
        agent_version_id.to_string(),
        None,
    )
    .await?;

    let row = sqlx::query(
        r#"
        SELECT av.status AS version_status, av.config_snapshot,
               a.deleted_at AS agent_deleted_at
        FROM agent_versions av
        JOIN agents a ON a.id = av.agent_id AND a.tenant_id = av.tenant_id
        WHERE av.id = $1 AND av.tenant_id = $2
        "#,
    )
    .bind(agent_version_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent version not found".to_string()))?;

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let version_status: String = row.try_get("version_status")?;
    if version_status != "published" {
        errors.push(format!(
            "agent version status is {version_status}, expected published"
        ));
    }
    if row
        .try_get::<Option<time::OffsetDateTime>, _>("agent_deleted_at")?
        .is_some()
    {
        errors.push("agent has been deleted".to_string());
    }
    let snapshot: serde_json::Value = row.try_get("config_snapshot")?;
    if let Err(error) =
        validate_agent_version_model_profile(&state.connect_pool, payload.tenant_id, &snapshot)
            .await
    {
        errors.push(error.to_string());
    }
    if capability_integrity_issue_count(&state.connect_pool, payload.tenant_id, agent_version_id)
        .await?
        > 0
    {
        errors.push("agent version has inactive or cross-tenant capability bindings".to_string());
    }
    if load_agent_version_skill_capabilities(
        &state.connect_pool,
        payload.tenant_id,
        agent_version_id,
    )
    .await?
    .is_empty()
    {
        warnings.push("agent version has no skill bindings".to_string());
    }

    Ok(Json(ValidationResponse {
        valid: errors.is_empty(),
        errors,
        warnings,
    }))
}

pub async fn bind_agent_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(agent_version_id): Path<Uuid>,
    Json(payload): Json<BindAgentVersionRequest>,
) -> Result<Json<GenericResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "agent_version",
        agent_version_id.to_string(),
        None,
    )
    .await?;
    let mut tx = state.connect_pool.begin().await?;
    ensure_agent_version_bindings_mutable(&mut tx, payload.tenant_id, agent_version_id).await?;

    if let Some(skill_version_ids) = payload.skill_version_ids {
        for skill_version_id in skill_version_ids {
            ensure_skill_version_bindable(&mut tx, payload.tenant_id, skill_version_id).await?;
            sqlx::query(
                r#"
                INSERT INTO agent_version_skill_bindings (agent_version_id, skill_version_id)
                VALUES ($1, $2)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(agent_version_id)
            .bind(skill_version_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    if let Some(tool_version_ids) = payload.tool_version_ids {
        for tool_version_id in tool_version_ids {
            ensure_tool_version_bindable(&mut tx, payload.tenant_id, tool_version_id).await?;
            sqlx::query(
                r#"
                INSERT INTO agent_version_tool_bindings (agent_version_id, tool_version_id)
                VALUES ($1, $2)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(agent_version_id)
            .bind(tool_version_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    if let Some(mcp_tool_ids) = payload.mcp_tool_ids {
        for mcp_tool_id in mcp_tool_ids {
            ensure_mcp_tool_bindable(&mut tx, payload.tenant_id, mcp_tool_id).await?;
            sqlx::query(
                r#"
                INSERT INTO agent_version_mcp_bindings (agent_version_id, mcp_tool_id)
                VALUES ($1, $2)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(agent_version_id)
            .bind(mcp_tool_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Json(GenericResponse {
        code: "AGENT_VERSION_BINDINGS_UPDATED".to_string(),
        message: "Agent version bindings updated".to_string(),
    }))
}

async fn load_agent_version_skill_capabilities(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<Vec<CapabilityResourceResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT s.id AS resource_id, sv.id AS version_id, sv.skill_id AS parent_id,
               s.name, s.description, sv.status, sv.manifest AS snapshot,
               NULL::text AS schema_hash, sv.content_hash, sv.source_uri
        FROM agent_version_skill_bindings b
        JOIN skill_versions sv ON sv.id = b.skill_version_id
        JOIN skills s ON s.id = sv.skill_id
        WHERE b.agent_version_id = $1
          AND sv.tenant_id = $2
          AND s.tenant_id = $2
        ORDER BY b.created_at ASC, s.name ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| capability_resource_from_row(row, "skill"))
        .collect()
}

async fn load_agent_version_tool_capabilities(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<Vec<CapabilityResourceResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT t.id AS resource_id, tv.id AS version_id, tv.tool_id AS parent_id,
               t.name, t.description, tv.status, tv.schema_snapshot AS snapshot,
               tv.schema_hash, NULL::text AS content_hash, NULL::text AS source_uri
        FROM agent_version_tool_bindings b
        JOIN tool_versions tv ON tv.id = b.tool_version_id
        JOIN tools t ON t.id = tv.tool_id
        WHERE b.agent_version_id = $1
          AND tv.tenant_id = $2
          AND t.tenant_id = $2
        ORDER BY b.created_at ASC, t.name ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| capability_resource_from_row(row, "tool"))
        .collect()
}

async fn load_agent_version_mcp_capabilities(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<Vec<CapabilityResourceResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT mt.id AS resource_id, NULL::uuid AS version_id,
               mt.mcp_server_id AS parent_id, mt.name, mt.description, mt.status,
               mt.schema AS snapshot, mt.schema_hash,
               NULL::text AS content_hash, NULL::text AS source_uri
        FROM agent_version_mcp_bindings b
        JOIN mcp_tools mt ON mt.id = b.mcp_tool_id
        JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
        WHERE b.agent_version_id = $1
          AND mt.tenant_id = $2
          AND ms.tenant_id = $2
        ORDER BY b.created_at ASC, mt.name ASC
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| capability_resource_from_row(row, "mcp_tool"))
        .collect()
}

fn capability_resource_from_row(
    row: sqlx::postgres::PgRow,
    resource_type: &str,
) -> Result<CapabilityResourceResponse, AppError> {
    Ok(CapabilityResourceResponse {
        resource_type: resource_type.to_string(),
        resource_id: row.try_get("resource_id")?,
        version_id: row.try_get("version_id")?,
        parent_id: row.try_get("parent_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        snapshot: row.try_get("snapshot")?,
        schema_hash: row.try_get("schema_hash")?,
        content_hash: row.try_get("content_hash")?,
        source_uri: row.try_get("source_uri")?,
    })
}

async fn capability_integrity_issue_count(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<i64, AppError> {
    let count = sqlx::query_scalar(
        r#"
        WITH issues AS (
            SELECT sv.id
            FROM agent_version_skill_bindings b
            LEFT JOIN skill_versions sv
              ON sv.id = b.skill_version_id
             AND sv.tenant_id = $2
             AND sv.status = 'published'
            LEFT JOIN skills s
              ON s.id = sv.skill_id
             AND s.tenant_id = $2
             AND s.status = 'active'
             AND s.deleted_at IS NULL
            WHERE b.agent_version_id = $1
              AND (sv.id IS NULL OR s.id IS NULL)
            UNION ALL
            SELECT tv.id
            FROM agent_version_tool_bindings b
            LEFT JOIN tool_versions tv
              ON tv.id = b.tool_version_id
             AND tv.tenant_id = $2
             AND tv.status = 'published'
            LEFT JOIN tools t
              ON t.id = tv.tool_id
             AND t.tenant_id = $2
             AND t.status = 'active'
             AND t.deleted_at IS NULL
            WHERE b.agent_version_id = $1
              AND (tv.id IS NULL OR t.id IS NULL)
            UNION ALL
            SELECT mt.id
            FROM agent_version_mcp_bindings b
            LEFT JOIN mcp_tools mt
              ON mt.id = b.mcp_tool_id
             AND mt.tenant_id = $2
             AND mt.status = 'active'
            LEFT JOIN mcp_servers ms
              ON ms.id = mt.mcp_server_id
             AND ms.tenant_id = $2
             AND ms.status = 'active'
             AND ms.deleted_at IS NULL
            WHERE b.agent_version_id = $1
              AND (mt.id IS NULL OR ms.id IS NULL)
        )
        SELECT COUNT(*) FROM issues
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

async fn ensure_agent_version_bindings_mutable(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    agent_version_id: Uuid,
) -> Result<(), AppError> {
    let version_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT id
        FROM agent_versions
        WHERE id = $1
          AND tenant_id = $2
          AND status = 'published'
        FOR UPDATE
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?;
    if version_id.is_none() {
        return Err(AppError::NotFound("agent version not found".to_string()));
    }

    let run_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM runs
        WHERE tenant_id = $1
          AND agent_version_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(agent_version_id)
    .fetch_one(&mut **tx)
    .await?;
    if run_count > 0 {
        return Err(AppError::Conflict(
            "agent version bindings are frozen after the version has been used by a run; publish a new agent version".to_string(),
        ));
    }

    Ok(())
}

async fn ensure_skill_version_bindable(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    skill_version_id: Uuid,
) -> Result<(), AppError> {
    ensure_bindable_resource(
        tx,
        tenant_id,
        skill_version_id,
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM skill_versions sv
            JOIN skills s ON s.id = sv.skill_id
            WHERE sv.id = $1
              AND sv.tenant_id = $2
              AND s.tenant_id = $2
              AND sv.status = 'published'
              AND s.status = 'active'
              AND s.deleted_at IS NULL
        )
        "#,
        "skill version is not published in tenant",
    )
    .await
}

async fn ensure_tool_version_bindable(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    tool_version_id: Uuid,
) -> Result<(), AppError> {
    ensure_bindable_resource(
        tx,
        tenant_id,
        tool_version_id,
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM tool_versions tv
            JOIN tools t ON t.id = tv.tool_id
            WHERE tv.id = $1
              AND tv.tenant_id = $2
              AND t.tenant_id = $2
              AND tv.status = 'published'
              AND t.status = 'active'
              AND t.deleted_at IS NULL
        )
        "#,
        "tool version is not published in tenant",
    )
    .await
}

async fn ensure_mcp_tool_bindable(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    mcp_tool_id: Uuid,
) -> Result<(), AppError> {
    ensure_bindable_resource(
        tx,
        tenant_id,
        mcp_tool_id,
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM mcp_tools mt
            JOIN mcp_servers ms ON ms.id = mt.mcp_server_id
            WHERE mt.id = $1
              AND mt.tenant_id = $2
              AND ms.tenant_id = $2
              AND mt.status = 'active'
              AND ms.status = 'active'
              AND ms.deleted_at IS NULL
        )
        "#,
        "mcp tool is not active in tenant",
    )
    .await
}

async fn ensure_bindable_resource(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    resource_id: Uuid,
    sql: &'static str,
    error: &str,
) -> Result<(), AppError> {
    let exists = sqlx::query_scalar::<_, bool>(sql)
        .bind(resource_id)
        .bind(tenant_id)
        .fetch_one(&mut **tx)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(error.to_string()))
    }
}
