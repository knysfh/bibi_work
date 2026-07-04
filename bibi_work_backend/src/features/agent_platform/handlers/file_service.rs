use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext, file_lock, file_store, models::*,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;

const TOOL_RESULT_ARTIFACT_PAGE_SIZE: i64 = 50;
const TOOL_RESULT_ARTIFACT_MAX_PAGE_SIZE: i64 = 500;
const TOOL_RESULT_ARTIFACT_TEXT_PREVIEW_CHARS: usize = 64 * 1024;

struct ToolResultArtifactRow {
    id: Uuid,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    tool_call_id: Option<Uuid>,
    view_kind: String,
    ref_kind: String,
    project_id: Uuid,
    path: String,
    revision: i64,
    file_revision_id: Uuid,
    object_reference_id: Uuid,
    content_hash: String,
    content_type: String,
    size_bytes: i64,
    created_at: OffsetDateTime,
}

async fn authorize_file_access(
    state: &AppState,
    payload: &FileReadRequest,
    action: &str,
) -> Result<(), AppError> {
    let path_hash = file_store::path_hash(&payload.path)?;
    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        action,
        "file",
        format!("{}:{}", payload.project_id, path_hash),
        Some(AuthzContext {
            project_id: Some(payload.project_id),
            run_id: payload.run_id,
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_file_write(
    state: &AppState,
    payload: &FileWriteRequest,
) -> Result<(), AppError> {
    let path_hash = file_store::path_hash(&payload.path)?;
    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        "write",
        "file",
        format!("{}:{}", payload.project_id, path_hash),
        Some(AuthzContext {
            project_id: Some(payload.project_id),
            run_id: payload.run_id,
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_file_edit(state: &AppState, payload: &FileEditRequest) -> Result<(), AppError> {
    let path_hash = file_store::path_hash(&payload.path)?;
    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        "write",
        "file",
        format!("{}:{}", payload.project_id, path_hash),
        Some(AuthzContext {
            project_id: Some(payload.project_id),
            run_id: payload.run_id,
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_file_lock(
    state: &AppState,
    tenant_id: uuid::Uuid,
    actor_user_id: uuid::Uuid,
    actor_device_id: Option<uuid::Uuid>,
    actor_session_id: Option<uuid::Uuid>,
    project_id: uuid::Uuid,
    path: &str,
    run_id: Option<uuid::Uuid>,
) -> Result<(), AppError> {
    let path_hash = file_store::path_hash(path)?;
    require_ferriskey_allow_for_actor(
        state,
        tenant_id,
        ActorRef {
            user_id: actor_user_id,
            device_id: actor_device_id,
            session_id: actor_session_id,
            roles: Vec::new(),
        },
        "write",
        "file",
        format!("{}:{}", project_id, path_hash),
        Some(AuthzContext {
            project_id: Some(project_id),
            run_id,
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_project_file_listing(
    state: &AppState,
    payload: &FileListQuery,
) -> Result<(), AppError> {
    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        "read",
        "project",
        payload.project_id.to_string(),
        Some(AuthzContext {
            project_id: Some(payload.project_id),
            run_id: payload.run_id,
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_project_file_search(
    state: &AppState,
    payload: &FileSearchRequest,
) -> Result<(), AppError> {
    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        "read",
        "project",
        payload.project_id.to_string(),
        Some(AuthzContext {
            project_id: Some(payload.project_id),
            run_id: payload.run_id,
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_current_file_access(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: uuid::Uuid,
    project_id: uuid::Uuid,
    path: &str,
    run_id: Option<uuid::Uuid>,
) -> Result<(), AppError> {
    let path_hash = file_store::path_hash(path)?;
    require_ferriskey_allow(
        state,
        ctx,
        tenant_id,
        "read",
        "file",
        format!("{project_id}:{path_hash}"),
        Some(AuthzContext {
            project_id: Some(project_id),
            run_id,
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

async fn authorize_current_project_file_read(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: uuid::Uuid,
    project_id: uuid::Uuid,
    run_id: Option<uuid::Uuid>,
) -> Result<(), AppError> {
    require_ferriskey_allow(
        state,
        ctx,
        tenant_id,
        "read",
        "project",
        project_id.to_string(),
        Some(AuthzContext {
            project_id: Some(project_id),
            run_id,
            ..Default::default()
        }),
    )
    .await?;
    Ok(())
}

pub async fn file_read_query(
    State(state): State<AppState>,
    Query(payload): Query<FileReadRequest>,
) -> Result<Json<FileRevisionResponse>, AppError> {
    authorize_file_access(&state, &payload, "read").await?;
    file_store::read_revision(&state, payload).await.map(Json)
}

pub async fn file_read_body(
    State(state): State<AppState>,
    Json(payload): Json<FileReadRequest>,
) -> Result<Json<FileRevisionResponse>, AppError> {
    authorize_file_access(&state, &payload, "read").await?;
    file_store::read_revision(&state, payload).await.map(Json)
}

pub async fn file_write(
    State(state): State<AppState>,
    Json(payload): Json<FileWriteRequest>,
) -> Result<Json<FileRevisionResponse>, AppError> {
    authorize_file_write(&state, &payload).await?;
    let revision = file_store::write_revision(&state, payload).await?;
    Ok(Json(revision))
}

pub async fn file_lock_acquire(
    State(state): State<AppState>,
    Json(payload): Json<FileLockRequest>,
) -> Result<Json<FileLockResponse>, AppError> {
    authorize_file_lock(
        &state,
        payload.tenant_id,
        payload.actor_user_id,
        payload.actor_device_id,
        payload.actor_session_id,
        payload.project_id,
        &payload.path,
        payload.run_id,
    )
    .await?;
    file_lock::acquire_lock(&state, payload).await.map(Json)
}

pub async fn file_lock_release(
    State(state): State<AppState>,
    Json(payload): Json<FileUnlockRequest>,
) -> Result<Json<FileLockResponse>, AppError> {
    authorize_file_lock(
        &state,
        payload.tenant_id,
        payload.actor_user_id,
        payload.actor_device_id,
        payload.actor_session_id,
        payload.project_id,
        &payload.path,
        payload.run_id,
    )
    .await?;
    file_lock::release_lock(&state, payload).await.map(Json)
}

pub async fn file_edit(
    State(state): State<AppState>,
    Json(payload): Json<FileEditRequest>,
) -> Result<Json<FileRevisionResponse>, AppError> {
    authorize_file_edit(&state, &payload).await?;
    let current = file_store::read_revision(
        &state,
        FileReadRequest {
            tenant_id: payload.tenant_id,
            actor_user_id: payload.actor_user_id,
            actor_device_id: payload.actor_device_id,
            actor_session_id: payload.actor_session_id,
            project_id: payload.project_id,
            path: payload.path.clone(),
            revision: None,
            version_id: None,
            run_id: payload.run_id,
            include_content: None,
            allow_binary: None,
        },
    )
    .await?;

    let content = current
        .inline_content
        .ok_or_else(|| AppError::InvalidInput("file has no inline content to edit".to_string()))?;
    let edited = content.replacen(&payload.find, &payload.replace, 1);
    if edited == content {
        return Err(AppError::InvalidInput(
            "find text was not present".to_string(),
        ));
    }

    let revision = file_store::write_revision(
        &state,
        FileWriteRequest {
            tenant_id: payload.tenant_id,
            actor_user_id: payload.actor_user_id,
            actor_device_id: payload.actor_device_id,
            actor_session_id: payload.actor_session_id,
            project_id: payload.project_id,
            path: payload.path,
            content_ref: None,
            inline_content: Some(edited),
            content_base64: None,
            content_type: None,
            expected_revision: payload.expected_revision,
            reason: payload.reason,
            run_id: payload.run_id,
            lock_token: None,
        },
    )
    .await?;

    Ok(Json(revision))
}

pub async fn file_list(
    State(state): State<AppState>,
    Query(query): Query<FileListQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    authorize_project_file_listing(&state, &query).await?;
    let prefix = query.prefix.unwrap_or_default();
    let files = file_store::list_latest_revisions(
        &state.connect_pool,
        query.tenant_id,
        query.project_id,
        &prefix,
    )
    .await?;
    let entries = file_store::directory_entries(&files, &prefix)?;
    Ok(Json(FileListResponse { files, entries }))
}

pub async fn file_glob(
    State(state): State<AppState>,
    Query(query): Query<FileListQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    authorize_project_file_listing(&state, &query).await?;
    let prefix = query.prefix.unwrap_or_default();
    let Some(pattern) = query.pattern.as_deref() else {
        let files = file_store::list_latest_revisions(
            &state.connect_pool,
            query.tenant_id,
            query.project_id,
            &prefix,
        )
        .await?;
        let entries = file_store::directory_entries(&files, &prefix)?;
        return Ok(Json(FileListResponse { files, entries }));
    };
    let files = file_store::glob_latest_revisions(
        &state.connect_pool,
        query.tenant_id,
        query.project_id,
        &prefix,
        pattern,
    )
    .await?;
    let entries = file_store::directory_entries(&files, &prefix)?;
    Ok(Json(FileListResponse { files, entries }))
}

pub async fn file_search(
    State(state): State<AppState>,
    Json(payload): Json<FileSearchRequest>,
) -> Result<Json<FileListResponse>, AppError> {
    authorize_project_file_search(&state, &payload).await?;
    let prefix = payload.prefix.unwrap_or_default();
    let files = file_store::search_latest_revisions(
        &state,
        payload.tenant_id,
        payload.project_id,
        &prefix,
        &payload.query,
        payload.limit.unwrap_or(50).min(200),
    )
    .await?;
    let entries = file_store::directory_entries(&files, &prefix)?;
    Ok(Json(FileListResponse { files, entries }))
}

pub async fn public_file_read(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(project_id): Path<uuid::Uuid>,
    Query(query): Query<PublicFileReadQuery>,
) -> Result<Json<FileRevisionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    authorize_current_file_access(
        &state,
        &ctx,
        query.tenant_id,
        project_id,
        &query.path,
        query.run_id,
    )
    .await?;
    file_store::read_revision(
        &state,
        FileReadRequest {
            tenant_id: query.tenant_id,
            actor_user_id: ctx.platform_user_id,
            actor_device_id: Some(ctx.device_id),
            actor_session_id: Some(ctx.session_id),
            project_id,
            path: query.path,
            revision: query.revision,
            version_id: query.version_id,
            run_id: query.run_id,
            include_content: query.include_content,
            allow_binary: query.allow_binary,
        },
    )
    .await
    .map(Json)
}

pub async fn public_file_list(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(project_id): Path<uuid::Uuid>,
    Query(query): Query<PublicFileListQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    authorize_current_project_file_read(&state, &ctx, query.tenant_id, project_id, query.run_id)
        .await?;
    let prefix = query.prefix.unwrap_or_default();
    let files = if let Some(pattern) = query.pattern.as_deref() {
        file_store::glob_latest_revisions(
            &state.connect_pool,
            query.tenant_id,
            project_id,
            &prefix,
            pattern,
        )
        .await?
    } else {
        file_store::list_latest_revisions(&state.connect_pool, query.tenant_id, project_id, &prefix)
            .await?
    };
    let entries = file_store::directory_entries(&files, &prefix)?;
    Ok(Json(FileListResponse { files, entries }))
}

pub async fn public_file_search(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(project_id): Path<uuid::Uuid>,
    Json(payload): Json<PublicFileSearchRequest>,
) -> Result<Json<FileListResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    authorize_current_project_file_read(
        &state,
        &ctx,
        payload.tenant_id,
        project_id,
        payload.run_id,
    )
    .await?;
    let prefix = payload.prefix.unwrap_or_default();
    let files = file_store::search_latest_revisions(
        &state,
        payload.tenant_id,
        project_id,
        &prefix,
        &payload.query,
        payload.limit.unwrap_or(50),
    )
    .await?;
    let entries = file_store::directory_entries(&files, &prefix)?;
    Ok(Json(FileListResponse { files, entries }))
}

pub async fn public_file_history(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(project_id): Path<uuid::Uuid>,
    Query(query): Query<PublicFileHistoryQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    authorize_current_file_access(&state, &ctx, query.tenant_id, project_id, &query.path, None)
        .await?;
    let files = file_store::list_revision_history(
        &state.connect_pool,
        query.tenant_id,
        project_id,
        &query.path,
        query.limit.unwrap_or(50),
    )
    .await?;
    Ok(Json(FileListResponse {
        files,
        entries: Vec::new(),
    }))
}

pub async fn public_project_artifacts(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(project_id): Path<uuid::Uuid>,
    Query(query): Query<ProjectArtifactsQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    authorize_current_project_file_read(&state, &ctx, query.tenant_id, project_id, query.run_id)
        .await?;
    let files = file_store::list_artifact_revisions(
        &state.connect_pool,
        query.tenant_id,
        project_id,
        query.run_id,
        query.limit.unwrap_or(100),
    )
    .await?;
    let entries = file_store::directory_entries(&files, "/")?;
    Ok(Json(FileListResponse { files, entries }))
}

pub async fn public_tool_result_artifact_read(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<ToolResultArtifactReadQuery>,
) -> Result<Json<ToolResultArtifactReadResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let artifact =
        load_tool_result_artifact(&state, query.tenant_id, query.object_reference_id).await?;
    authorize_current_file_access(
        &state,
        &ctx,
        query.tenant_id,
        artifact.project_id,
        &artifact.path,
        artifact.run_id,
    )
    .await?;

    let revision = file_store::read_revision(
        &state,
        FileReadRequest {
            tenant_id: query.tenant_id,
            actor_user_id: ctx.platform_user_id,
            actor_device_id: Some(ctx.device_id),
            actor_session_id: Some(ctx.session_id),
            project_id: artifact.project_id,
            path: artifact.path.clone(),
            revision: Some(artifact.revision),
            version_id: None,
            run_id: artifact.run_id,
            include_content: Some(true),
            allow_binary: Some(false),
        },
    )
    .await?;
    let content = tool_result_artifact_content(
        &revision,
        query.offset.unwrap_or(0),
        query.limit.unwrap_or(TOOL_RESULT_ARTIFACT_PAGE_SIZE),
    )?;

    Ok(Json(ToolResultArtifactReadResponse {
        id: artifact.id,
        tenant_id: artifact.tenant_id,
        run_id: artifact.run_id,
        tool_call_id: artifact.tool_call_id,
        view_kind: artifact.view_kind,
        ref_kind: artifact.ref_kind,
        project_id: artifact.project_id,
        path: artifact.path,
        revision: artifact.revision,
        file_revision_id: artifact.file_revision_id,
        object_reference_id: artifact.object_reference_id,
        content_hash: artifact.content_hash,
        content_type: artifact.content_type,
        size_bytes: artifact.size_bytes,
        content,
        created_at: artifact.created_at,
    }))
}

async fn load_tool_result_artifact(
    state: &AppState,
    tenant_id: Uuid,
    object_reference_id: Uuid,
) -> Result<ToolResultArtifactRow, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, run_id, tool_call_id, view_kind, ref_kind, project_id,
               path, revision, file_revision_id, object_reference_id, content_hash,
               content_type, size_bytes, created_at
        FROM tool_result_artifacts
        WHERE tenant_id = $1 AND object_reference_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(object_reference_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("tool result artifact not found".to_string()))?;

    Ok(ToolResultArtifactRow {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        run_id: row.try_get("run_id")?,
        tool_call_id: row.try_get("tool_call_id")?,
        view_kind: row.try_get("view_kind")?,
        ref_kind: row.try_get("ref_kind")?,
        project_id: row.try_get("project_id")?,
        path: row.try_get("path")?,
        revision: row.try_get("revision")?,
        file_revision_id: row.try_get("file_revision_id")?,
        object_reference_id: row.try_get("object_reference_id")?,
        content_hash: row.try_get("content_hash")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
        created_at: row.try_get("created_at")?,
    })
}

fn tool_result_artifact_content(
    revision: &FileRevisionResponse,
    offset: i64,
    limit: i64,
) -> Result<Value, AppError> {
    if revision.is_binary {
        return Ok(json!({
            "kind": "binary_metadata",
            "content_type": revision.content_type,
            "size_bytes": revision.size_bytes
        }));
    }

    let Some(text) = revision.inline_content.as_deref() else {
        return Ok(json!({
            "kind": "text",
            "text": "",
            "truncated": false
        }));
    };
    if is_jsonl_artifact(revision) {
        return jsonl_artifact_content(text, offset, limit);
    }
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        if let Value::Array(rows) = value {
            let offset = offset.max(0) as usize;
            let limit: usize = limit
                .clamp(1, TOOL_RESULT_ARTIFACT_MAX_PAGE_SIZE)
                .try_into()
                .map_err(|_| AppError::InvalidInput("limit is invalid".to_string()))?;
            let total_rows = rows.len();
            let page = rows
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            return Ok(json!({
                "kind": "json_rows",
                "offset": offset,
                "limit": limit,
                "total_rows": total_rows,
                "rows": page
            }));
        }
        return Ok(json!({
            "kind": "json_value",
            "value": value
        }));
    }

    let preview = text
        .chars()
        .take(TOOL_RESULT_ARTIFACT_TEXT_PREVIEW_CHARS)
        .collect::<String>();
    Ok(json!({
        "kind": "text",
        "text": preview,
        "truncated": text.chars().count() > TOOL_RESULT_ARTIFACT_TEXT_PREVIEW_CHARS
    }))
}

fn is_jsonl_artifact(revision: &FileRevisionResponse) -> bool {
    let content_type = revision
        .content_type
        .split(';')
        .next()
        .unwrap_or(&revision.content_type)
        .trim()
        .to_ascii_lowercase();
    matches!(
        content_type.as_str(),
        "application/x-ndjson" | "application/jsonl" | "application/x-jsonlines"
    ) || revision.path.ends_with(".jsonl")
}

fn jsonl_artifact_content(text: &str, offset: i64, limit: i64) -> Result<Value, AppError> {
    let offset = offset.max(0) as usize;
    let limit: usize = limit
        .clamp(1, TOOL_RESULT_ARTIFACT_MAX_PAGE_SIZE)
        .try_into()
        .map_err(|_| AppError::InvalidInput("limit is invalid".to_string()))?;
    let mut total_rows = 0_usize;
    let mut rows = Vec::new();

    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if total_rows >= offset && rows.len() < limit {
            rows.push(serde_json::from_str::<Value>(line).map_err(|_| {
                AppError::InvalidInput("tool result JSONL artifact is invalid".to_string())
            })?);
        }
        total_rows += 1;
    }

    Ok(json!({
        "kind": "json_rows",
        "offset": offset,
        "limit": limit,
        "total_rows": total_rows,
        "rows": rows
    }))
}

#[cfg(test)]
mod tests {
    use axum::middleware;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use redis::Client as RedisClient;
    use reqwest::StatusCode;
    use secrecy::SecretBox;
    use serde_json::{Value, json};
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};
    use time::OffsetDateTime;
    use tokio::{net::TcpListener, task::JoinHandle};
    use uuid::Uuid;

    use crate::{
        configuration::{
            AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings, ObjectStoreSettings,
        },
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier, file_store,
            internal_auth::internal_token_middleware, memory_vector::MemoryVectorClient,
            models::FileRevisionResponse, runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
        startup::AppState,
    };

    const INTERNAL_TOKEN: &str = "test-internal-token";

    #[test]
    fn tool_result_artifact_content_pages_json_rows() {
        let revision = FileRevisionResponse {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            path: "/artifacts/tool-results/table.json".to_string(),
            revision: 1,
            etag: "etag".to_string(),
            content_hash: "hash".to_string(),
            object_key: "object".to_string(),
            object_reference_id: Some(Uuid::new_v4()),
            bucket: None,
            version_id: None,
            inline_content: Some(r#"[{"n":1},{"n":2},{"n":3}]"#.to_string()),
            content_base64: None,
            size_bytes: 25,
            content_type: "application/json".to_string(),
            is_binary: false,
            is_large: false,
            reason: "test".to_string(),
            run_id: None,
            metadata: json!({}),
            created_at: OffsetDateTime::now_utc(),
        };

        let content = super::tool_result_artifact_content(&revision, 1, 1).expect("content");

        assert_eq!(content["kind"], "json_rows");
        assert_eq!(content["offset"], 1);
        assert_eq!(content["total_rows"], 3);
        assert_eq!(content["rows"], json!([{"n": 2}]));
    }

    #[test]
    fn tool_result_artifact_content_pages_jsonl_rows() {
        let revision = FileRevisionResponse {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            path: "/artifacts/tool-results/table.jsonl".to_string(),
            revision: 1,
            etag: "etag".to_string(),
            content_hash: "hash".to_string(),
            object_key: "object".to_string(),
            object_reference_id: Some(Uuid::new_v4()),
            bucket: None,
            version_id: None,
            inline_content: Some("{\"n\":1}\n{\"n\":2}\n{\"n\":3}".to_string()),
            content_base64: None,
            size_bytes: 23,
            content_type: "application/x-ndjson".to_string(),
            is_binary: false,
            is_large: false,
            reason: "test".to_string(),
            run_id: None,
            metadata: json!({}),
            created_at: OffsetDateTime::now_utc(),
        };

        let content = super::tool_result_artifact_content(&revision, 1, 2).expect("content");

        assert_eq!(content["kind"], "json_rows");
        assert_eq!(content["offset"], 1);
        assert_eq!(content["total_rows"], 3);
        assert_eq!(content["rows"], json!([{"n": 2}, {"n": 3}]));
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, Redis, RustFS, and the bibi_work schema"]
    async fn file_service_http_round_trips_rustfs_revisions_and_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state_with_rustfs().await?;
        let path = format!("/workspace/http-e2e-{}.txt", Uuid::new_v4());
        let (tenant_id, user_id, project_id) =
            seed_authorized_file_context(&state.connect_pool, &path).await?;
        let (base_url, server) = spawn_internal_app(state.clone()).await?;
        let http = reqwest::Client::new();
        let mut written_object_keys = Vec::new();

        let result = async {
            let first = post_json(
                &http,
                &format!("{base_url}/files/write"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": path,
                    "content_ref": null,
                    "inline_content": "first revision through http",
                    "expected_revision": 0,
                    "reason": "http e2e initial write",
                    "run_id": null
                }),
            )
            .await?;

            assert_eq!(first["revision"].as_i64(), Some(1));
            assert_eq!(
                first["inline_content"].as_str(),
                Some("first revision through http")
            );
            if let Some(object_key) = first["object_key"].as_str() {
                written_object_keys.push(object_key.to_string());
            }
            assert!(first["object_reference_id"].as_str().is_some());
            assert_eq!(first["bucket"].as_str(), Some("bibi-work-files"));

            let conflict = http
                .post(format!("{base_url}/files/write"))
                .bearer_auth(INTERNAL_TOKEN)
                .json(&json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": path,
                    "content_ref": null,
                    "inline_content": "stale overwrite",
                    "expected_revision": 0,
                    "reason": "stale write",
                    "run_id": null
                }))
                .send()
                .await?;
            assert_eq!(conflict.status(), StatusCode::CONFLICT);

            let second = post_json(
                &http,
                &format!("{base_url}/files/write"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": path,
                    "content_ref": null,
                    "inline_content": "second revision through http",
                    "expected_revision": 1,
                    "reason": "http e2e update",
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(second["revision"].as_i64(), Some(2));
            if let Some(object_key) = second["object_key"].as_str() {
                written_object_keys.push(object_key.to_string());
            }
            assert!(second["object_reference_id"].as_str().is_some());

            let latest = post_json(
                &http,
                &format!("{base_url}/files/read"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": path,
                    "revision": null,
                    "version_id": null,
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(latest["revision"].as_i64(), Some(2));
            assert_eq!(
                latest["inline_content"].as_str(),
                Some("second revision through http")
            );

            let historical = post_json(
                &http,
                &format!("{base_url}/files/read"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": path,
                    "revision": 1,
                    "version_id": null,
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(historical["revision"].as_i64(), Some(1));
            assert_eq!(
                historical["inline_content"].as_str(),
                Some("first revision through http")
            );

            if let Some(version_id) = first["version_id"].as_str() {
                let by_version = post_json(
                    &http,
                    &format!("{base_url}/files/read"),
                    json!({
                        "tenant_id": tenant_id,
                        "actor_user_id": user_id,
                        "actor_device_id": null,
                        "actor_session_id": null,
                        "project_id": project_id,
                        "path": path,
                        "revision": null,
                        "version_id": version_id,
                        "run_id": null
                    }),
                )
                .await?;
                assert_eq!(by_version["revision"].as_i64(), Some(1));
                assert_eq!(
                    by_version["inline_content"].as_str(),
                    Some("first revision through http")
                );
            }

            let lock_path = format!("/workspace/http-lock-e2e-{}.txt", Uuid::new_v4());
            grant_file_writer(
                &state.connect_pool,
                tenant_id,
                user_id,
                project_id,
                &lock_path,
            )
            .await?;
            let other_user_id =
                seed_additional_file_writer(&state.connect_pool, tenant_id, project_id, &lock_path)
                    .await?;
            let lock = post_json(
                &http,
                &format!("{base_url}/files/locks/acquire"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": lock_path,
                    "run_id": null,
                    "ttl_seconds": 60,
                    "reason": "http e2e lock"
                }),
            )
            .await?;
            assert!(lock["lock_token"].as_str().is_some());

            let locked_write = http
                .post(format!("{base_url}/files/write"))
                .bearer_auth(INTERNAL_TOKEN)
                .json(&json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": other_user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": lock_path,
                    "content_ref": null,
                    "inline_content": "blocked by lock",
                    "expected_revision": 0,
                    "reason": "locked write",
                    "run_id": null
                }))
                .send()
                .await?;
            assert_eq!(locked_write.status(), StatusCode::CONFLICT);

            post_json(
                &http,
                &format!("{base_url}/files/locks/release"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": lock_path,
                    "run_id": null,
                    "lock_token": lock["lock_token"],
                    "reason": "http e2e unlock"
                }),
            )
            .await?;

            let lock_released_write = post_json(
                &http,
                &format!("{base_url}/files/write"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": other_user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": lock_path,
                    "content_ref": null,
                    "inline_content": "write after unlock",
                    "expected_revision": 0,
                    "reason": "unlocked write",
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(lock_released_write["revision"].as_i64(), Some(1));
            if let Some(object_key) = lock_released_write["object_key"].as_str() {
                written_object_keys.push(object_key.to_string());
            }

            let binary_path = format!("/workspace/http-binary-e2e-{}.bin", Uuid::new_v4());
            grant_file_writer(
                &state.connect_pool,
                tenant_id,
                user_id,
                project_id,
                &binary_path,
            )
            .await?;
            let binary_payload = BASE64_STANDARD.encode([0_u8, 1, 2, 3, 255]);
            let binary = post_json(
                &http,
                &format!("{base_url}/files/write"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": binary_path,
                    "content_ref": null,
                    "inline_content": null,
                    "content_base64": binary_payload,
                    "content_type": "application/octet-stream",
                    "expected_revision": 0,
                    "reason": "binary write",
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(binary["is_binary"].as_bool(), Some(true));
            assert_eq!(
                binary["content_base64"].as_str(),
                Some(binary_payload.as_str())
            );
            if let Some(object_key) = binary["object_key"].as_str() {
                written_object_keys.push(object_key.to_string());
            }

            let binary_default_read = post_json(
                &http,
                &format!("{base_url}/files/read"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": binary_path,
                    "revision": null,
                    "version_id": null,
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(binary_default_read["is_binary"].as_bool(), Some(true));
            assert!(binary_default_read["content_base64"].is_null());

            let binary_allowed_read = post_json(
                &http,
                &format!("{base_url}/files/read"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": binary_path,
                    "revision": null,
                    "version_id": null,
                    "run_id": null,
                    "allow_binary": true
                }),
            )
            .await?;
            assert_eq!(
                binary_allowed_read["content_base64"].as_str(),
                Some(binary_payload.as_str())
            );

            let large_path = format!("/workspace/http-large-e2e-{}.txt", Uuid::new_v4());
            grant_file_writer(
                &state.connect_pool,
                tenant_id,
                user_id,
                project_id,
                &large_path,
            )
            .await?;
            let large_content = "x".repeat(1024 * 1024 + 1);
            let large = post_json(
                &http,
                &format!("{base_url}/files/write"),
                json!({
                    "tenant_id": tenant_id,
                    "actor_user_id": user_id,
                    "actor_device_id": null,
                    "actor_session_id": null,
                    "project_id": project_id,
                    "path": large_path,
                    "content_ref": null,
                    "inline_content": large_content,
                    "expected_revision": 0,
                    "reason": "large write",
                    "run_id": null
                }),
            )
            .await?;
            assert_eq!(large["is_large"].as_bool(), Some(true));
            assert_eq!(large["is_binary"].as_bool(), Some(false));
            if let Some(object_key) = large["object_key"].as_str() {
                written_object_keys.push(object_key.to_string());
            }

            let object_reference_count: i64 = sqlx::query(
                r#"
                SELECT COUNT(*) AS count
                FROM object_references
                WHERE tenant_id = $1
                  AND owner_resource_type = 'file_revision'
                "#,
            )
            .bind(tenant_id)
            .fetch_one(&state.connect_pool)
            .await?
            .try_get("count")?;
            assert!(object_reference_count >= 5);

            let audit_count: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)
                FROM audit_logs
                WHERE tenant_id = $1
                  AND action IN ('write_object', 'lock.acquire', 'lock.release')
                  AND row_hash IS NOT NULL
                "#,
            )
            .bind(tenant_id)
            .fetch_one(&state.connect_pool)
            .await?;
            assert!(audit_count >= 5);

            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;

        for object_key in written_object_keys {
            state.rustfs_client.delete_file_object(&object_key).await?;
        }
        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        server.abort();

        result
    }

    async fn post_json(
        http: &reqwest::Client,
        url: &str,
        payload: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let response = http
            .post(url)
            .bearer_auth(INTERNAL_TOKEN)
            .json(&payload)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(response.json::<Value>().await?)
    }

    async fn spawn_internal_app(
        state: AppState,
    ) -> Result<(String, JoinHandle<Result<(), std::io::Error>>), Box<dyn std::error::Error>> {
        let router = crate::features::agent_platform::internal_router()
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                internal_token_middleware,
            ))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move { axum::serve(listener, router).await });
        Ok((format!("http://{address}"), server))
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
                shared_token: secret(INTERNAL_TOKEN),
                timeout_milliseconds: 1000,
            })?,
            rustfs_client: RustFsClient::disabled_for_tests(),
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
            internal_shared_token: INTERNAL_TOKEN.to_string(),
        })
    }

    async fn test_state_with_rustfs() -> Result<AppState, Box<dyn std::error::Error>> {
        let mut state = test_state().await?;
        state.rustfs_client = RustFsClient::new(ObjectStoreSettings {
            enabled: true,
            endpoint: std::env::var("RUSTFS_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9004".to_string()),
            access_key: secret(
                &std::env::var("RUSTFS_ACCESS_KEY").unwrap_or_else(|_| "rustfsadmin".to_string()),
            ),
            secret_key: secret(
                &std::env::var("RUSTFS_SECRET_KEY").unwrap_or_else(|_| "rustfsadmin".to_string()),
            ),
            region: std::env::var("RUSTFS_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            files_bucket: std::env::var("RUSTFS_FILES_BUCKET")
                .unwrap_or_else(|_| "bibi-work-files".to_string()),
            audit_bucket: std::env::var("RUSTFS_AUDIT_BUCKET")
                .unwrap_or_else(|_| "bibi-work-audit".to_string()),
            timeout_milliseconds: 5000,
        })?;
        Ok(state)
    }

    async fn seed_authorized_file_context(
        pool: &PgPool,
        path: &str,
    ) -> Result<(Uuid, Uuid, Uuid), Box<dyn std::error::Error>> {
        let suffix = Uuid::new_v4();
        let tenant_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tenants (name, slug, metadata)
            VALUES ($1, $2, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(format!("File service HTTP test {suffix}"))
        .bind(format!("file-service-http-test-{suffix}"))
        .fetch_one(pool)
        .await?;

        let user_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO platform_users (tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("file-service-http-subject-{suffix}"))
        .bind(format!("file-service-http-user-{suffix}"))
        .fetch_one(pool)
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

        let project_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO projects (tenant_id, owner_user_id, name, metadata)
            VALUES ($1, $2, $3, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("File service HTTP project {suffix}"))
        .fetch_one(pool)
        .await?;

        let file_resource_id = format!("{}:{}", project_id, file_store::path_hash(path)?);
        grant_file_writer_by_resource_id(pool, tenant_id, user_id, &file_resource_id).await?;

        Ok((tenant_id, user_id, project_id))
    }

    async fn seed_additional_file_writer(
        pool: &PgPool,
        tenant_id: Uuid,
        project_id: Uuid,
        path: &str,
    ) -> Result<Uuid, Box<dyn std::error::Error>> {
        let suffix = Uuid::new_v4();
        let user_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO platform_users (tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("file-service-http-other-subject-{suffix}"))
        .bind(format!("file-service-http-other-user-{suffix}"))
        .fetch_one(pool)
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

        grant_file_writer(pool, tenant_id, user_id, project_id, path).await?;
        Ok(user_id)
    }

    async fn grant_file_writer(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        project_id: Uuid,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let file_resource_id = format!("{}:{}", project_id, file_store::path_hash(path)?);
        grant_file_writer_by_resource_id(pool, tenant_id, user_id, &file_resource_id).await
    }

    async fn grant_file_writer_by_resource_id(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        file_resource_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        sqlx::query(
            r#"
            INSERT INTO resource_relations (
                tenant_id, resource_type, resource_id, relation,
                subject_type, subject_id, created_by_user_id
            )
            VALUES ($1, 'file', $2, 'writer', 'user', $3, $4)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(tenant_id)
        .bind(file_resource_id)
        .bind(user_id.to_string())
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
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
