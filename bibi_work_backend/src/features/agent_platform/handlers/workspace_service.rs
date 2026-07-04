use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};
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

const LOCAL_MOUNT_CAPABILITIES: &[&str] = &["read", "write", "execute"];
const LOCAL_MAIN_VIRTUAL_PATH: &str = "/local/main/";

pub async fn create_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateWorkspaceRequest>,
) -> Result<Json<WorkspaceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "workspace").await?;
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            "workspace name is required".to_string(),
        ));
    }
    ensure_optional_project(&state, payload.tenant_id, payload.remote_project_id).await?;
    ensure_optional_agent(&state, payload.tenant_id, payload.default_agent_id).await?;
    ensure_optional_agent_version(&state, payload.tenant_id, payload.default_agent_version_id)
        .await?;
    ensure_optional_model_profile(&state, payload.tenant_id, payload.default_model_profile_id)
        .await?;

    let row = sqlx::query(
        r#"
        INSERT INTO workspaces (
            tenant_id, owner_user_id, name, remote_project_id, default_agent_id,
            default_agent_version_id, default_model_profile_id, tool_policy, file_policy,
            include_globs, exclude_globs, trust_state, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        RETURNING id, tenant_id, owner_user_id, name, remote_project_id, default_agent_id,
                  default_agent_version_id, default_model_profile_id, tool_policy, file_policy,
                  include_globs, exclude_globs, trust_state, metadata, status, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(name)
    .bind(payload.remote_project_id)
    .bind(payload.default_agent_id)
    .bind(payload.default_agent_version_id)
    .bind(payload.default_model_profile_id)
    .bind(payload.tool_policy.unwrap_or_else(|| json!({})))
    .bind(payload.file_policy.unwrap_or_else(|| json!({})))
    .bind(array_value_or_empty(payload.include_globs, "include_globs")?)
    .bind(array_value_or_empty(payload.exclude_globs, "exclude_globs")?)
    .bind(payload.trust_state.unwrap_or_else(|| "untrusted".to_string()))
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(workspace_from_row(row)?))
}

pub async fn list_workspaces(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<WorkspaceResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, owner_user_id, name, remote_project_id, default_agent_id,
               default_agent_version_id, default_model_profile_id, tool_policy, file_policy,
               include_globs, exclude_globs, trust_state, metadata, status, created_at, updated_at
        FROM workspaces
        WHERE tenant_id = $1 AND deleted_at IS NULL
        ORDER BY updated_at DESC
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let workspaces = rows
        .into_iter()
        .map(workspace_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(workspaces))
}

pub async fn create_local_mount(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workspace_id): Path<Uuid>,
    Json(payload): Json<CreateLocalMountRequest>,
) -> Result<Json<LocalMountResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    ensure_workspace(&state, payload.tenant_id, workspace_id).await?;
    let virtual_path = normalize_local_virtual_path(&payload.virtual_path)?;
    let display_name = payload.display_name.trim().to_string();
    if display_name.is_empty() {
        return Err(AppError::InvalidInput(
            "local mount display_name is required".to_string(),
        ));
    }
    let capabilities = validate_capabilities(payload.capabilities)?;
    let row = sqlx::query(
        r#"
        INSERT INTO local_mounts (
            tenant_id, user_id, device_id, workspace_id, display_name, virtual_path,
            capabilities, include_globs, exclude_globs, trust_state, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT (user_id, device_id, workspace_id, virtual_path)
        DO UPDATE SET
            display_name = EXCLUDED.display_name,
            capabilities = EXCLUDED.capabilities,
            include_globs = EXCLUDED.include_globs,
            exclude_globs = EXCLUDED.exclude_globs,
            trust_state = EXCLUDED.trust_state,
            metadata = EXCLUDED.metadata,
            status = 'active',
            updated_at = CURRENT_TIMESTAMP
        RETURNING id, tenant_id, user_id, device_id, workspace_id, display_name, virtual_path,
                  capabilities, include_globs, exclude_globs, trust_state, metadata, status,
                  created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(ctx.device_id)
    .bind(workspace_id)
    .bind(display_name)
    .bind(virtual_path)
    .bind(capabilities)
    .bind(array_value_or_empty(
        payload.include_globs,
        "include_globs",
    )?)
    .bind(array_value_or_empty(
        payload.exclude_globs,
        "exclude_globs",
    )?)
    .bind(
        payload
            .trust_state
            .unwrap_or_else(|| "untrusted".to_string()),
    )
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(local_mount_from_row(row)?))
}

pub async fn list_local_mounts(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<LocalMountResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    ensure_workspace(&state, tenant_id, workspace_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, device_id, workspace_id, display_name, virtual_path,
               capabilities, include_globs, exclude_globs, trust_state, metadata, status,
               created_at, updated_at
        FROM local_mounts
        WHERE tenant_id = $1
          AND workspace_id = $2
          AND user_id = $3
          AND device_id = $4
          AND status = 'active'
        ORDER BY virtual_path ASC
        LIMIT $5
        "#,
    )
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(ctx.platform_user_id)
    .bind(ctx.device_id)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let mounts = rows
        .into_iter()
        .map(local_mount_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(mounts))
}

async fn ensure_workspace(
    state: &AppState,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM workspaces
            WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        ) AS exists
        "#,
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Ok(())
    } else {
        Err(AppError::NotFound("workspace not found".to_string()))
    }
}

async fn ensure_optional_project(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM projects WHERE id = $1 AND tenant_id = $2
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

async fn ensure_optional_agent(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(agent_id) = agent_id else {
        return Ok(());
    };
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agents WHERE id = $1 AND tenant_id = $2
        ) AS exists
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput("agent is not in tenant".to_string()))
    }
}

async fn ensure_optional_agent_version(
    state: &AppState,
    tenant_id: Uuid,
    agent_version_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(agent_version_id) = agent_version_id else {
        return Ok(());
    };
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_versions WHERE id = $1 AND tenant_id = $2
        ) AS exists
        "#,
    )
    .bind(agent_version_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "agent version is not in tenant".to_string(),
        ))
    }
}

async fn ensure_optional_model_profile(
    state: &AppState,
    tenant_id: Uuid,
    model_profile_id: Option<Uuid>,
) -> Result<(), AppError> {
    let Some(model_profile_id) = model_profile_id else {
        return Ok(());
    };
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM llm_model_profiles WHERE id = $1 AND tenant_id = $2
        ) AS exists
        "#,
    )
    .bind(model_profile_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "model profile is not in tenant".to_string(),
        ))
    }
}

fn normalize_local_virtual_path(path: &str) -> Result<&'static str, AppError> {
    file_store::validate_virtual_path(path)?;
    let path = path.trim();
    if path == LOCAL_MAIN_VIRTUAL_PATH || path == "/local/" {
        return Ok(LOCAL_MAIN_VIRTUAL_PATH);
    }
    Err(AppError::InvalidInput(format!(
        "local mount virtual_path must be {LOCAL_MAIN_VIRTUAL_PATH}"
    )))
}

fn validate_capabilities(value: Option<Value>) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| json!(["read"]));
    let Some(items) = value.as_array() else {
        return Err(AppError::InvalidInput(
            "capabilities must be an array".to_string(),
        ));
    };
    if items.is_empty() {
        return Err(AppError::InvalidInput(
            "capabilities must not be empty".to_string(),
        ));
    }
    for item in items {
        let Some(capability) = item.as_str() else {
            return Err(AppError::InvalidInput(
                "capabilities must contain strings".to_string(),
            ));
        };
        if !LOCAL_MOUNT_CAPABILITIES.contains(&capability) {
            return Err(AppError::InvalidInput(format!(
                "unsupported local mount capability: {capability}"
            )));
        }
    }
    Ok(value)
}

fn array_value_or_empty(value: Option<Value>, field_name: &str) -> Result<Value, AppError> {
    let value = value.unwrap_or_else(|| json!([]));
    if value.is_array() {
        Ok(value)
    } else {
        Err(AppError::InvalidInput(format!(
            "{field_name} must be an array"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_local_virtual_path_accepts_runtime_root() {
        assert_eq!(
            normalize_local_virtual_path("/local/main/").unwrap(),
            "/local/main/"
        );
    }

    #[test]
    fn normalize_local_virtual_path_maps_legacy_local_root() {
        assert_eq!(
            normalize_local_virtual_path("/local/").unwrap(),
            "/local/main/"
        );
    }

    #[test]
    fn normalize_local_virtual_path_rejects_unsupported_roots() {
        let err = normalize_local_virtual_path("/local/other/").unwrap_err();

        assert!(err.to_string().contains("/local/main/"));
    }
}
