use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn list_skills(
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
        FROM skills
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

    let skills = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(skills))
}

pub async fn get_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status, metadata, created_at, updated_at
        FROM skills
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(skill_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("skill not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_id): Path<Uuid>,
    Json(payload): Json<UpdateSkillRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "skill",
        skill_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE skills
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            metadata = COALESCE($5, metadata),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(skill_id)
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.metadata)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("skill not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "skill",
        skill_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE skills
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(skill_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("skill not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateSkillRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "skill").await?;
    let row = sqlx::query(
        r#"
        INSERT INTO skills (tenant_id, name, description, metadata)
        VALUES ($1, $2, $3, $4)
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_skill_versions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<Vec<VersionResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, skill_id AS parent_id, version_label,
               manifest AS snapshot, NULL::text AS policy_version, status, created_at
        FROM skill_versions
        WHERE tenant_id = $1
          AND skill_id = $2
          AND ($3::text IS NULL OR status = $3)
        ORDER BY created_at DESC
        LIMIT $4
        "#,
    )
    .bind(query.tenant_id)
    .bind(skill_id)
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

pub async fn get_skill_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_version_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, skill_id AS parent_id, version_label,
               manifest AS snapshot, NULL::text AS policy_version, status, created_at
        FROM skill_versions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(skill_version_id)
    .bind(query.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("skill version not found".to_string()))?;
    Ok(Json(version_from_row(row)?))
}

pub async fn publish_skill_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_id): Path<Uuid>,
    Json(payload): Json<PublishVersionRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "skill",
        skill_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO skill_versions (
            tenant_id, skill_id, version_label, manifest, content_hash, source_uri
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, skill_id AS parent_id, version_label,
                  manifest AS snapshot, NULL::text AS policy_version, status, created_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(skill_id)
    .bind(payload.version_label)
    .bind(payload.snapshot.unwrap_or_else(|| json!({})))
    .bind(payload.content_hash)
    .bind(payload.source_uri)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(version_from_row(row)?))
}

pub async fn disable_skill_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_version_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "skill_version",
        skill_version_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE skill_versions
        SET status = 'disabled'
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, skill_id AS parent_id, version_label,
                  manifest AS snapshot, NULL::text AS policy_version, status, created_at
        "#,
    )
    .bind(skill_version_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("skill version not found".to_string()))?;
    Ok(Json(version_from_row(row)?))
}

pub async fn list_tools(
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
               metadata || jsonb_build_object('tool_type', tool_type, 'schema', schema) AS metadata,
               created_at, updated_at
        FROM tools
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

    let tools = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(tools))
}

pub async fn get_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<ResourceResponse>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status,
               metadata || jsonb_build_object('tool_type', tool_type, 'schema', schema) AS metadata,
               created_at, updated_at
        FROM tools
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(tool_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("tool not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_id): Path<Uuid>,
    Json(payload): Json<UpdateToolRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "tool",
        tool_id.to_string(),
        Some(AuthzContext {
            tool_id: Some(tool_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE tools
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            tool_type = COALESCE($5, tool_type),
            schema = COALESCE($6, schema),
            metadata = COALESCE($7, metadata),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status,
                  metadata || jsonb_build_object('tool_type', tool_type, 'schema', schema) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(tool_id)
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.tool_type)
    .bind(payload.schema)
    .bind(payload.metadata)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("tool not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn disable_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "tool",
        tool_id.to_string(),
        Some(AuthzContext {
            tool_id: Some(tool_id),
            ..Default::default()
        }),
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE tools
        SET status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, name, description, status,
                  metadata || jsonb_build_object('tool_type', tool_type, 'schema', schema) AS metadata,
                  created_at, updated_at
        "#,
    )
    .bind(tool_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("tool not found".to_string()))?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn create_tool(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateToolRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "tool").await?;
    let row = sqlx::query(
        r#"
        INSERT INTO tools (tenant_id, name, description, tool_type, schema, metadata)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.tool_type.unwrap_or_else(|| "custom".to_string()))
    .bind(payload.schema.unwrap_or_else(|| json!({})))
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_tool_versions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<Vec<VersionResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, tool_id AS parent_id, version_label,
               schema_snapshot AS snapshot, NULL::text AS policy_version, status, created_at
        FROM tool_versions
        WHERE tenant_id = $1
          AND tool_id = $2
          AND ($3::text IS NULL OR status = $3)
        ORDER BY created_at DESC
        LIMIT $4
        "#,
    )
    .bind(query.tenant_id)
    .bind(tool_id)
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

pub async fn get_tool_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_version_id): Path<Uuid>,
    Query(query): Query<VersionListQuery>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, tool_id AS parent_id, version_label,
               schema_snapshot AS snapshot, NULL::text AS policy_version, status, created_at
        FROM tool_versions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(tool_version_id)
    .bind(query.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("tool version not found".to_string()))?;
    Ok(Json(version_from_row(row)?))
}

pub async fn publish_tool_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_id): Path<Uuid>,
    Json(payload): Json<PublishVersionRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "tool",
        tool_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO tool_versions (tenant_id, tool_id, version_label, schema_snapshot, schema_hash)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, tool_id AS parent_id, version_label,
                  schema_snapshot AS snapshot, NULL::text AS policy_version, status, created_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(tool_id)
    .bind(payload.version_label)
    .bind(payload.snapshot.unwrap_or_else(|| json!({})))
    .bind(payload.schema_hash)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(version_from_row(row)?))
}

pub async fn disable_tool_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(tool_version_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "disable",
        "tool_version",
        tool_version_id.to_string(),
        None,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE tool_versions
        SET status = 'disabled'
        WHERE id = $1 AND tenant_id = $2
        RETURNING id, tenant_id, tool_id AS parent_id, version_label,
                  schema_snapshot AS snapshot, NULL::text AS policy_version, status, created_at
        "#,
    )
    .bind(tool_version_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("tool version not found".to_string()))?;
    Ok(Json(version_from_row(row)?))
}
