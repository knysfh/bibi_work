use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, file_store, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

pub async fn create_project(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateProjectRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "project").await?;
    let row = sqlx::query(
        r#"
        INSERT INTO projects (tenant_id, owner_user_id, name, description, metadata)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, name, description, status, metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn list_projects(
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
        FROM projects
        WHERE tenant_id = $1 AND deleted_at IS NULL
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let projects = rows
        .into_iter()
        .map(resource_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(projects))
}

pub async fn create_project_mount(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<CreateProjectMountRequest>,
) -> Result<Json<ProjectMountResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "project",
        project_id.to_string(),
        Some(AuthzContext {
            project_id: Some(project_id),
            ..Default::default()
        }),
    )
    .await?;
    file_store::validate_virtual_path(&payload.virtual_path)?;
    let row = sqlx::query(
        r#"
        INSERT INTO project_mounts (
            tenant_id, project_id, virtual_path, backend_type, backend_ref, mount_config
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (project_id, virtual_path)
        DO UPDATE SET
            backend_type = EXCLUDED.backend_type,
            backend_ref = EXCLUDED.backend_ref,
            mount_config = EXCLUDED.mount_config
        RETURNING id, tenant_id, project_id, virtual_path, backend_type,
                  backend_ref, mount_config, created_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(project_id)
    .bind(payload.virtual_path)
    .bind(payload.backend_type)
    .bind(payload.backend_ref)
    .bind(payload.mount_config.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(ProjectMountResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        project_id: row.try_get("project_id")?,
        virtual_path: row.try_get("virtual_path")?,
        backend_type: row.try_get("backend_type")?,
        backend_ref: row.try_get("backend_ref")?,
        mount_config: row.try_get("mount_config")?,
        created_at: row.try_get("created_at")?,
    }))
}

pub async fn create_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateConversationRequest>,
) -> Result<Json<ConversationResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "conversation").await?;
    let workspace_scope =
        load_workspace_conversation_defaults(&state, payload.tenant_id, payload.workspace_id)
            .await?;
    let project_id = resolve_workspace_project(
        payload.project_id,
        workspace_scope
            .as_ref()
            .and_then(|scope| scope.remote_project_id),
    )?;
    let agent_id = payload.agent_id.or_else(|| {
        workspace_scope
            .as_ref()
            .and_then(|scope| scope.default_agent_id)
    });
    let row = sqlx::query(
        r#"
        INSERT INTO conversations (
            tenant_id, created_by_user_id, workspace_id, project_id, agent_id, title, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, tenant_id, workspace_id, project_id, agent_id, title, status,
                  metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(payload.workspace_id)
    .bind(project_id)
    .bind(agent_id)
    .bind(
        payload
            .title
            .unwrap_or_else(|| "Untitled conversation".to_string()),
    )
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(conversation_from_row(row)?))
}

pub async fn list_conversations(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<ConversationResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, workspace_id, project_id, agent_id, title, status,
               metadata, created_at, updated_at
        FROM conversations
        WHERE tenant_id = $1 AND deleted_at IS NULL
        ORDER BY updated_at DESC
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let conversations = rows
        .into_iter()
        .map(conversation_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(conversations))
}

pub async fn update_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Json(payload): Json<UpdateConversationRequest>,
) -> Result<Json<ConversationResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let current = sqlx::query(
        r#"
        SELECT workspace_id, project_id
        FROM conversations
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;
    let workspace_id: Option<Uuid> = current.try_get("workspace_id")?;
    let current_project_id: Option<Uuid> = current.try_get("project_id")?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "conversation",
        conversation_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(conversation_id),
            project_id: current_project_id,
            ..Default::default()
        }),
    )
    .await?;

    let workspace_scope =
        load_workspace_conversation_defaults(&state, payload.tenant_id, workspace_id).await?;
    let requested_project_id = match payload.project_id {
        NullableUuidPatch::Missing => {
            return Err(AppError::InvalidInput("project_id is required".to_string()));
        }
        NullableUuidPatch::Clear => None,
        NullableUuidPatch::Set(project_id) => {
            ensure_project_in_tenant(&state, payload.tenant_id, project_id).await?;
            Some(project_id)
        }
    };
    let project_id = resolve_workspace_project(
        requested_project_id,
        workspace_scope
            .as_ref()
            .and_then(|scope| scope.remote_project_id),
    )?;

    let row = sqlx::query(
        r#"
        UPDATE conversations
        SET project_id = $3, updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, workspace_id, project_id, agent_id, title, status,
                  metadata, created_at, updated_at
        "#,
    )
    .bind(conversation_id)
    .bind(payload.tenant_id)
    .bind(project_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(conversation_from_row(row)?))
}

struct WorkspaceConversationDefaults {
    remote_project_id: Option<Uuid>,
    default_agent_id: Option<Uuid>,
}

async fn load_workspace_conversation_defaults(
    state: &AppState,
    tenant_id: Uuid,
    workspace_id: Option<Uuid>,
) -> Result<Option<WorkspaceConversationDefaults>, AppError> {
    let Some(workspace_id) = workspace_id else {
        return Ok(None);
    };
    let row = sqlx::query(
        r#"
        SELECT remote_project_id, default_agent_id
        FROM workspaces
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workspace not found".to_string()))?;
    Ok(Some(WorkspaceConversationDefaults {
        remote_project_id: row.try_get("remote_project_id")?,
        default_agent_id: row.try_get("default_agent_id")?,
    }))
}

async fn ensure_project_in_tenant(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM projects
            WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        ) AS exists
        "#,
    )
    .bind(project_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "project is not in tenant".to_string(),
        ))
    }
}

fn resolve_workspace_project(
    requested_project_id: Option<Uuid>,
    workspace_project_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    if let (Some(requested), Some(workspace_project)) = (requested_project_id, workspace_project_id)
        && requested != workspace_project
    {
        return Err(AppError::InvalidInput(
            "conversation project_id cannot expand workspace remote project".to_string(),
        ));
    }
    Ok(requested_project_id.or(workspace_project_id))
}
