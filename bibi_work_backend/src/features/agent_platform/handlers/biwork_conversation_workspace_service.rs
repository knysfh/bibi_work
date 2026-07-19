use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, file_store},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_compat_service::ok,
    biwork_fs_service::{
        biwork_workspace_search_entry_json, is_immediate_child, normalize_biwork_virtual_path,
        normalize_directory_prefix_for_biwork,
    },
    support::require_ferriskey_allow,
};

#[derive(Debug, Deserialize)]
pub struct ConversationWorkspaceQuery {
    path: Option<String>,
    search: Option<String>,
}

pub async fn biwork_conversation_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Query(query): Query<ConversationWorkspaceQuery>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_conversation_project_id(&state, &ctx, conversation_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "read",
        "project",
        project_id.to_string(),
        None,
    )
    .await?;
    let path = normalize_biwork_virtual_path(query.path.as_deref().unwrap_or("."), None)?;
    let directory_prefix = normalize_directory_prefix_for_biwork(&path);
    let items = if let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let files = file_store::search_latest_revisions(
            &state,
            ctx.tenant_id,
            project_id,
            &directory_prefix,
            search,
            200,
        )
        .await?;
        files
            .iter()
            .map(|file| biwork_workspace_search_entry_json(&directory_prefix, &file.path))
            .collect::<Vec<_>>()
    } else {
        let files = file_store::list_latest_revisions(
            &state.connect_pool,
            ctx.tenant_id,
            project_id,
            &directory_prefix,
        )
        .await?;
        let entries = file_store::directory_entries(&files, &directory_prefix)?;
        entries
            .iter()
            .filter(|entry| entry.path != directory_prefix)
            .filter(|entry| is_immediate_child(&directory_prefix, &entry.path))
            .map(|entry| {
                let name = entry
                    .path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .find(|part| !part.is_empty())
                    .unwrap_or("");
                json!({
                    "name": name,
                    "type": entry.entry_type,
                })
            })
            .collect::<Vec<_>>()
    };
    Ok(ok(Value::Array(items)))
}

async fn resolve_conversation_project_id(
    state: &AppState,
    ctx: &PlatformRequestContext,
    conversation_id: Uuid,
) -> Result<Uuid, AppError> {
    let row = sqlx::query(
        r#"
        SELECT c.project_id,
               w.remote_project_id AS workspace_project_id
        FROM conversations c
        LEFT JOIN workspaces w
          ON w.id = c.workspace_id
         AND w.tenant_id = c.tenant_id
         AND w.deleted_at IS NULL
        WHERE c.id = $1
          AND c.tenant_id = $2
          AND c.deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;

    if let Some(project_id) = row.try_get::<Option<Uuid>, _>("project_id")? {
        return Ok(project_id);
    }
    if let Some(project_id) = row.try_get::<Option<Uuid>, _>("workspace_project_id")? {
        return Ok(project_id);
    }
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM projects
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND status = 'active'
          AND (owner_user_id = $2 OR owner_user_id IS NULL)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workspace project not found".to_string()))
}
