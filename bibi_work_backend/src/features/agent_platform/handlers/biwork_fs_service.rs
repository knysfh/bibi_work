use axum::{Extension, Json, extract::State};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext,
            file_store,
            models::{FileEntryResponse, FileReadRequest, FileRevisionResponse, FileWriteRequest},
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_compat_service::{epoch_ms, ok},
    support::require_ferriskey_allow,
};

#[derive(Debug, Deserialize)]
pub struct BiWorkFsPayload {
    path: Option<String>,
    dir: Option<String>,
    root: Option<String>,
    workspace: Option<String>,
    data: Option<String>,
    expected_revision: Option<Value>,
}

pub async fn biwork_fs_dir(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_biwork_project_id(&state, &ctx, &payload).await?;
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
    let dir = payload
        .dir
        .as_deref()
        .or(payload.path.as_deref())
        .unwrap_or("/");
    let prefix = normalize_biwork_virtual_path(
        dir,
        payload.root.as_deref().or(payload.workspace.as_deref()),
    )?;
    let directory_prefix = normalize_directory_prefix_for_biwork(&prefix);
    let files = file_store::list_latest_revisions(
        &state.connect_pool,
        ctx.tenant_id,
        project_id,
        &directory_prefix,
    )
    .await?;
    let entries = file_store::directory_entries(&files, &directory_prefix)?;
    let items = entries
        .iter()
        .filter(|entry| entry.path != directory_prefix)
        .filter(|entry| is_immediate_child(&directory_prefix, &entry.path))
        .map(|entry| {
            fs_entry_json(
                entry,
                payload.root.as_deref().or(payload.workspace.as_deref()),
            )
        })
        .collect::<Vec<_>>();
    Ok(ok(Value::Array(items)))
}

pub async fn biwork_fs_list(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let project_id = resolve_biwork_project_id(&state, &ctx, &payload).await?;
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
    let prefix = payload
        .root
        .as_deref()
        .or(payload.workspace.as_deref())
        .map(|root| normalize_biwork_virtual_path(root, Some(root)))
        .transpose()?
        .unwrap_or_else(|| "/".to_string());
    let directory_prefix = normalize_directory_prefix_for_biwork(&prefix);
    let files = file_store::list_latest_revisions(
        &state.connect_pool,
        ctx.tenant_id,
        project_id,
        &directory_prefix,
    )
    .await?;
    let root = payload
        .root
        .as_deref()
        .or(payload.workspace.as_deref())
        .unwrap_or("");
    let items = files
        .iter()
        .map(|file| {
            let relative = relative_path_for_biwork(&directory_prefix, &file.path);
            json!({
                "name": file.path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(""),
                "full_path": full_path_for_biwork(root, &relative),
                "relative_path": relative,
            })
        })
        .collect::<Vec<_>>();
    Ok(ok(Value::Array(items)))
}

pub async fn biwork_fs_read(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let (project_id, path) = resolve_fs_project_path(&state, &ctx, &payload).await?;
    authorize_biwork_file(&state, &ctx, project_id, &path, "read").await?;
    match read_biwork_revision(&state, &ctx, project_id, path, false).await {
        Ok(revision) => Ok(ok(json!(revision.inline_content))),
        Err(AppError::NotFound(_)) => Ok(ok(Value::Null)),
        Err(err) => Err(err),
    }
}

pub async fn biwork_fs_read_buffer(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let (project_id, path) = resolve_fs_project_path(&state, &ctx, &payload).await?;
    authorize_biwork_file(&state, &ctx, project_id, &path, "read").await?;
    match read_biwork_revision(&state, &ctx, project_id, path, true).await {
        Ok(revision) => Ok(ok(json!(
            revision.content_base64.or(revision.inline_content)
        ))),
        Err(AppError::NotFound(_)) => Ok(ok(Value::Null)),
        Err(err) => Err(err),
    }
}

pub async fn biwork_fs_image_base64(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let (project_id, path) = resolve_fs_project_path(&state, &ctx, &payload).await?;
    authorize_biwork_file(&state, &ctx, project_id, &path, "read").await?;
    match read_biwork_revision(&state, &ctx, project_id, path, true).await {
        Ok(revision) => Ok(ok(json!(biwork_image_data_url(
            &revision.content_type,
            revision.content_base64.as_deref(),
            revision.inline_content.as_deref(),
        )))),
        Err(AppError::NotFound(_)) => Ok(ok(Value::Null)),
        Err(err) => Err(err),
    }
}

pub async fn biwork_fs_write(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let data = payload
        .data
        .clone()
        .ok_or_else(|| AppError::InvalidInput("data is required".to_string()))?;
    let (project_id, path) = resolve_fs_project_path(&state, &ctx, &payload).await?;
    authorize_biwork_file(&state, &ctx, project_id, &path, "write").await?;
    let expected_revision = match parse_expected_revision(payload.expected_revision.as_ref())? {
        Some(revision) => revision,
        None => latest_revision_number(&state, ctx.tenant_id, project_id, &path).await?,
    };
    match file_store::write_revision(
        &state,
        FileWriteRequest {
            tenant_id: ctx.tenant_id,
            actor_user_id: ctx.platform_user_id,
            actor_device_id: Some(ctx.device_id),
            actor_session_id: Some(ctx.session_id),
            project_id,
            path,
            content_ref: None,
            inline_content: Some(data),
            content_base64: None,
            content_type: Some("text/plain; charset=utf-8".to_string()),
            expected_revision,
            reason: "biwork.fs.write".to_string(),
            run_id: None,
            lock_token: None,
            tool_call_id: None,
            tool_name: None,
            args_hash: None,
            parent_tool_call_id: None,
            operation: Some("biwork.fs.write".to_string()),
        },
    )
    .await
    {
        Ok(_) => {}
        Err(AppError::Conflict(message)) if message.starts_with("file revision conflict:") => {
            return Err(AppError::WorkspaceRevisionConflict(message));
        }
        Err(err) => return Err(err),
    }
    Ok(ok(json!(true)))
}

pub async fn biwork_fs_metadata(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<BiWorkFsPayload>,
) -> Result<Json<Value>, AppError> {
    let (project_id, path) = resolve_fs_project_path(&state, &ctx, &payload).await?;
    authorize_biwork_file(&state, &ctx, project_id, &path, "read").await?;
    match read_biwork_revision(&state, &ctx, project_id, path.clone(), false).await {
        Ok(revision) => {
            let name = path.rsplit('/').find(|part| !part.is_empty()).unwrap_or("");
            Ok(ok(json!({
                "name": name,
                "path": path,
                "size": revision.size_bytes,
                "type": revision.content_type,
                "lastModified": epoch_ms(revision.created_at),
                "isDirectory": false,
                "revision": revision.revision,
                "etag": revision.etag,
            })))
        }
        Err(AppError::NotFound(_)) => Ok(ok(json!({
            "name": "",
            "path": path,
            "size": 0,
            "type": "missing",
            "lastModified": 0,
            "isDirectory": false,
        }))),
        Err(err) => Err(err),
    }
}

async fn resolve_fs_project_path(
    state: &AppState,
    ctx: &PlatformRequestContext,
    payload: &BiWorkFsPayload,
) -> Result<(Uuid, String), AppError> {
    let project_id = resolve_biwork_project_id(state, ctx, payload).await?;
    let raw_path = payload
        .path
        .as_deref()
        .or(payload.dir.as_deref())
        .ok_or_else(|| AppError::InvalidInput("path is required".to_string()))?;
    let path = normalize_biwork_virtual_path(
        raw_path,
        payload.root.as_deref().or(payload.workspace.as_deref()),
    )?;
    Ok((project_id, path))
}

async fn resolve_biwork_project_id(
    state: &AppState,
    ctx: &PlatformRequestContext,
    payload: &BiWorkFsPayload,
) -> Result<Uuid, AppError> {
    let workspace = payload
        .workspace
        .as_deref()
        .or(payload.root.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(workspace) = workspace
        && let Some(project_id) = lookup_biwork_project(state, ctx, workspace).await?
    {
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

async fn lookup_biwork_project(
    state: &AppState,
    ctx: &PlatformRequestContext,
    workspace: &str,
) -> Result<Option<Uuid>, AppError> {
    let workspace_uuid = Uuid::parse_str(workspace).ok();
    sqlx::query_scalar(
        r#"
        SELECT p.id
        FROM projects p
        LEFT JOIN workspaces w
          ON w.remote_project_id = p.id
         AND w.tenant_id = p.tenant_id
         AND w.deleted_at IS NULL
        WHERE p.tenant_id = $1
          AND p.deleted_at IS NULL
          AND p.status = 'active'
          AND (p.owner_user_id = $2 OR p.owner_user_id IS NULL)
          AND (
              p.id = $3
              OR w.id = $3
              OR p.name = $4
              OR w.name = $4
              OR p.metadata->>'workspace' = $4
              OR p.metadata->>'root' = $4
              OR w.metadata->>'workspace' = $4
              OR w.metadata->>'root' = $4
          )
        ORDER BY w.updated_at DESC NULLS LAST, p.updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(workspace_uuid)
    .bind(workspace)
    .fetch_optional(&state.connect_pool)
    .await
    .map_err(AppError::from)
}

pub(super) fn normalize_biwork_virtual_path(
    raw_path: &str,
    root: Option<&str>,
) -> Result<String, AppError> {
    let mut path = raw_path.replace('\\', "/");
    let root = root
        .map(|value| value.replace('\\', "/"))
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty() && value != "/");
    if let Some(root) = root.as_deref() {
        if path == root {
            path = "/".to_string();
        } else if let Some(stripped) = path.strip_prefix(&format!("{root}/")) {
            path = stripped.to_string();
        }
    }
    if path == "." || path.is_empty() {
        path = "/".to_string();
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    while path.contains("//") {
        path = path.replace("//", "/");
    }
    if path.len() > 1 {
        path = path.trim_end_matches('/').to_string();
    }
    file_store::validate_virtual_path(&path)?;
    Ok(path)
}

pub(super) fn normalize_directory_prefix_for_biwork(path: &str) -> String {
    if path == "/" {
        "/".to_string()
    } else {
        format!("{}/", path.trim_end_matches('/'))
    }
}

pub(super) fn is_immediate_child(prefix: &str, path: &str) -> bool {
    let remainder = path.strip_prefix(prefix).unwrap_or(path);
    let remainder = remainder.trim_end_matches('/');
    !remainder.is_empty() && !remainder.contains('/')
}

fn relative_path_for_biwork(prefix: &str, path: &str) -> String {
    let relative = path
        .strip_prefix(prefix)
        .unwrap_or(path)
        .trim_start_matches('/')
        .trim_end_matches('/');
    if relative.is_empty() {
        ".".to_string()
    } else {
        relative.to_string()
    }
}

fn full_path_for_biwork(root: &str, relative: &str) -> String {
    let root = root.trim_end_matches('/');
    if relative == "." || relative.is_empty() {
        root.to_string()
    } else if root.is_empty() {
        format!("/{relative}")
    } else {
        format!("{root}/{relative}")
    }
}

pub(super) fn biwork_workspace_search_entry_json(directory_prefix: &str, path: &str) -> Value {
    let relative = relative_path_for_biwork(directory_prefix, path);
    json!({
        "name": path.rsplit('/').find(|part| !part.is_empty()).unwrap_or(""),
        "type": "file",
        "full_path": path,
        "relative_path": relative,
    })
}

fn fs_entry_json(entry: &FileEntryResponse, root: Option<&str>) -> Value {
    let root = root.unwrap_or("");
    let relative = entry
        .path
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string();
    let name = entry
        .path
        .trim_end_matches('/')
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("");
    let is_dir = entry.entry_type == "directory";
    json!({
        "name": name,
        "fullPath": full_path_for_biwork(root, &relative),
        "relativePath": relative,
        "isDir": is_dir,
        "isFile": !is_dir,
    })
}

async fn authorize_biwork_file(
    state: &AppState,
    ctx: &PlatformRequestContext,
    project_id: Uuid,
    path: &str,
    action: &str,
) -> Result<(), AppError> {
    let path_hash = file_store::path_hash(path)?;
    require_ferriskey_allow(
        state,
        ctx,
        ctx.tenant_id,
        action,
        "file",
        format!("{project_id}:{path_hash}"),
        None,
    )
    .await
    .map(|_| ())
}

async fn read_biwork_revision(
    state: &AppState,
    ctx: &PlatformRequestContext,
    project_id: Uuid,
    path: String,
    allow_binary: bool,
) -> Result<FileRevisionResponse, AppError> {
    file_store::read_revision(
        state,
        FileReadRequest {
            tenant_id: ctx.tenant_id,
            actor_user_id: ctx.platform_user_id,
            actor_device_id: Some(ctx.device_id),
            actor_session_id: Some(ctx.session_id),
            project_id,
            path,
            revision: None,
            version_id: None,
            run_id: None,
            include_content: Some(true),
            allow_binary: Some(allow_binary),
            offset_bytes: None,
            limit_bytes: None,
        },
    )
    .await
}

fn parse_expected_revision(value: Option<&Value>) -> Result<Option<i64>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .filter(|revision| *revision >= 0)
            .map(Some)
            .ok_or_else(|| AppError::InvalidInput("expected_revision is invalid".to_string())),
        Some(Value::String(raw)) => {
            let normalized = raw.trim().trim_start_matches("rev_");
            normalized
                .parse::<i64>()
                .ok()
                .filter(|revision| *revision >= 0)
                .map(Some)
                .ok_or_else(|| AppError::InvalidInput("expected_revision is invalid".to_string()))
        }
        Some(_) => Err(AppError::InvalidInput(
            "expected_revision is invalid".to_string(),
        )),
    }
}

fn biwork_image_data_url(
    content_type: &str,
    content_base64: Option<&str>,
    inline_content: Option<&str>,
) -> Option<String> {
    let content_type = content_type.trim();
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    if !media_type.starts_with("image/") {
        return None;
    }

    if let Some(content_base64) = content_base64
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if content_base64.starts_with("data:") {
            return Some(content_base64.to_string());
        }
        return Some(format!("data:{content_type};base64,{content_base64}"));
    }

    inline_content
        .map(|content| BASE64_STANDARD.encode(content.as_bytes()))
        .map(|content_base64| format!("data:{content_type};base64,{content_base64}"))
}

async fn latest_revision_number(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
    path: &str,
) -> Result<i64, AppError> {
    let path_hash = file_store::path_hash(path)?;
    let revision: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(revision), 0)
        FROM file_revisions
        WHERE tenant_id = $1 AND project_id = $2 AND path_hash = $3
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .fetch_one(&state.connect_pool)
    .await?;
    Ok(revision)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_search_entry_preserves_nested_virtual_paths() {
        let entry =
            biwork_workspace_search_entry_json("/workspace/", "/workspace/docs/nested/report.md");

        assert_eq!(entry["name"], "report.md");
        assert_eq!(entry["type"], "file");
        assert_eq!(entry["full_path"], "/workspace/docs/nested/report.md");
        assert_eq!(entry["relative_path"], "docs/nested/report.md");
    }

    #[test]
    fn image_data_url_preserves_frontend_contract() {
        assert_eq!(
            biwork_image_data_url("image/png", Some("iVBORw0KGgo="), None).as_deref(),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        assert_eq!(
            biwork_image_data_url("image/svg+xml; charset=utf-8", None, Some("<svg/>")).as_deref(),
            Some("data:image/svg+xml; charset=utf-8;base64,PHN2Zy8+")
        );
        assert_eq!(
            biwork_image_data_url("text/plain", Some("aGVsbG8="), None),
            None
        );
    }

    #[test]
    fn expected_revision_accepts_biwork_revision_tokens() {
        assert_eq!(parse_expected_revision(None).unwrap(), None);
        assert_eq!(parse_expected_revision(Some(&Value::Null)).unwrap(), None);
        assert_eq!(parse_expected_revision(Some(&json!(7))).unwrap(), Some(7));
        assert_eq!(
            parse_expected_revision(Some(&json!("rev_123"))).unwrap(),
            Some(123)
        );
        assert_eq!(
            parse_expected_revision(Some(&json!(" 42 "))).unwrap(),
            Some(42)
        );

        for invalid in [
            json!(-1),
            json!("rev_"),
            json!("rev_-1"),
            json!("abc"),
            json!({}),
        ] {
            let err = parse_expected_revision(Some(&invalid)).unwrap_err();
            assert!(err.to_string().contains("expected_revision"));
        }
    }
}
