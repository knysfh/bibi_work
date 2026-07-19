use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{ferriskey_oidc::PlatformRequestContext, file_store, models::*},
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{file_service, support::*, user_context_service};

const WORKBENCH_BOOTSTRAP_LIMIT: i64 = 50;
const WORKBENCH_DETAIL_EVENT_LIMIT: i64 = 100;
const WORKBENCH_DIFF_MAX_CHARS: usize = 120 * 1024;

pub async fn get_workbench_bootstrap(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkbenchBootstrapQuery>,
) -> Result<Json<WorkbenchBootstrapResponse>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let me = user_context_service::load_current_me(&state, &ctx).await?;
    let is_admin = is_workbench_admin(&me);

    let workspaces =
        load_workspace_summaries(&state, tenant_id, ctx.platform_user_id, is_admin, false).await?;
    let pinned_workspaces =
        load_workspace_summaries(&state, tenant_id, ctx.platform_user_id, is_admin, true).await?;
    let recent_conversations = load_conversation_summaries(
        &state,
        tenant_id,
        ctx.platform_user_id,
        is_admin,
        None,
        WORKBENCH_BOOTSTRAP_LIMIT,
    )
    .await?;
    let teams =
        load_agent_team_summaries(&state, tenant_id, ctx.platform_user_id, is_admin).await?;
    let pending_approvals_count =
        count_pending_approvals(&state, tenant_id, ctx.platform_user_id, is_admin).await?;
    let running_runs_count =
        count_running_runs(&state, tenant_id, ctx.platform_user_id, is_admin).await?;
    let navigation = WorkbenchNavigation {
        primary: workbench_navigation_primary(&me),
        capabilities: me.capabilities.clone(),
    };
    let ui_policy = WorkbenchUiPolicy {
        can_create_workspace: has_any_capability(&me, &["tenant:manage", "project:read"]),
        can_mount_local_folder: false,
        can_manage_catalog: has_any_capability(&me, &["catalog:manage"]),
        risk_auto_approval: vec!["low".to_string()],
    };

    Ok(Json(WorkbenchBootstrapResponse {
        navigation,
        workspaces,
        pinned_workspaces,
        recent_conversations,
        teams,
        pending_approvals_count,
        running_runs_count,
        device: me.device.clone(),
        session: me.session.clone(),
        feature_flags: WorkbenchFeatureFlags::biwork_enterprise_default(),
        ui_policy,
        me,
    }))
}

pub async fn get_workbench_workspace_detail(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<WorkbenchWorkspaceDetailQuery>,
) -> Result<Json<WorkbenchWorkspaceDetailResponse>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let me = user_context_service::load_current_me(&state, &ctx).await?;
    let is_admin = is_workbench_admin(&me);
    let workspace = load_workspace_summary(
        &state,
        tenant_id,
        workspace_id,
        ctx.platform_user_id,
        is_admin,
    )
    .await?;
    let local_mounts = load_local_mount_summaries(&state, tenant_id, workspace_id, &ctx).await?;
    let project = match workspace.remote_project_id {
        Some(project_id) => Some(load_project_summary(&state, tenant_id, project_id).await?),
        None => None,
    };
    let conversations = load_conversation_summaries(
        &state,
        tenant_id,
        ctx.platform_user_id,
        is_admin,
        Some(workspace_id),
        query.conversation_limit.unwrap_or(50).clamp(1, 200),
    )
    .await?;

    Ok(Json(WorkbenchWorkspaceDetailResponse {
        workspace,
        local_mounts,
        project,
        conversations,
        available_actions: vec!["conversation:create".to_string()],
    }))
}

pub async fn get_workbench_conversation_detail(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Query(query): Query<WorkbenchConversationDetailQuery>,
) -> Result<Json<WorkbenchConversationDetailResponse>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let me = user_context_service::load_current_me(&state, &ctx).await?;
    let is_admin = is_workbench_admin(&me);
    let conversation = load_conversation_summary(
        &state,
        tenant_id,
        conversation_id,
        ctx.platform_user_id,
        is_admin,
    )
    .await?;
    let workspace = match conversation.workspace_id {
        Some(workspace_id) => Some(
            load_workspace_summary(
                &state,
                tenant_id,
                workspace_id,
                ctx.platform_user_id,
                is_admin,
            )
            .await?,
        ),
        None => None,
    };
    let project = match conversation.project_id {
        Some(project_id) => Some(load_project_summary(&state, tenant_id, project_id).await?),
        None => None,
    };
    let latest_run = load_latest_run(&state, tenant_id, conversation_id).await?;
    let after_seq = query.events_after_seq.unwrap_or(0).max(0);
    let events_limit = query
        .events_limit
        .unwrap_or(WORKBENCH_DETAIL_EVENT_LIMIT)
        .clamp(1, 500);
    let events =
        load_conversation_events(&state, tenant_id, conversation_id, after_seq, events_limit)
            .await?;
    let last_seq = events.last().map(|event| event.seq).unwrap_or(after_seq);
    let has_more_before =
        after_seq > 0 || has_events_before(&state, tenant_id, conversation_id, after_seq).await?;
    let pending_approvals =
        load_conversation_pending_approvals(&state, tenant_id, conversation_id).await?;
    let latest_run_id = latest_run.as_ref().map(|run| run.id);
    let artifacts =
        load_artifact_summaries(&state, tenant_id, latest_run_id, conversation_id).await?;
    let file_changes = file_changes_from_events(&events);
    let tasks = task_summaries_from_events(&events);
    let subagents = subagent_summaries_from_events(&events);
    let memory_candidates =
        load_memory_candidate_summaries(&state, tenant_id, latest_run_id, conversation.project_id)
            .await?;

    Ok(Json(WorkbenchConversationDetailResponse {
        conversation,
        workspace,
        project,
        latest_run,
        events,
        events_page: WorkbenchEventsPage {
            after_seq,
            last_seq,
            has_more_before,
        },
        pending_approvals,
        artifacts,
        file_changes,
        tasks,
        subagents,
        memory_candidates,
        available_actions: vec!["run:create".to_string(), "approval:decide".to_string()],
    }))
}

pub async fn search_workbench(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkbenchSearchQuery>,
) -> Result<Json<WorkbenchSearchResponse>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let me = user_context_service::load_current_me(&state, &ctx).await?;
    let is_admin = is_workbench_admin(&me);
    let trimmed_query = query.query.unwrap_or_default().trim().to_string();
    let limit = query.limit.unwrap_or(20).clamp(1, 50);
    let scope = query.scope.unwrap_or_default();
    let mut items = Vec::new();

    if trimmed_query.len() < 2 {
        items.extend(
            load_recent_search_items(&state, tenant_id, ctx.platform_user_id, is_admin, limit)
                .await?,
        );
        return Ok(Json(WorkbenchSearchResponse { items }));
    }

    if scope.is_empty() || scope == "workspace" {
        items.extend(
            search_workspaces(
                &state,
                tenant_id,
                ctx.platform_user_id,
                is_admin,
                &trimmed_query,
                limit,
            )
            .await?,
        );
    }
    if items.len() < limit as usize && (scope.is_empty() || scope == "conversation") {
        items.extend(
            search_conversations(
                &state,
                tenant_id,
                ctx.platform_user_id,
                is_admin,
                &trimmed_query,
                limit - items.len() as i64,
            )
            .await?,
        );
    }
    if items.len() < limit as usize && (scope.is_empty() || scope == "team") {
        items.extend(
            search_agent_teams(
                &state,
                tenant_id,
                ctx.platform_user_id,
                is_admin,
                &trimmed_query,
                limit - items.len() as i64,
            )
            .await?,
        );
    }
    if items.len() < limit as usize && (scope.is_empty() || scope == "approval") {
        items.extend(
            search_approvals(
                &state,
                tenant_id,
                ctx.platform_user_id,
                is_admin,
                &trimmed_query,
                limit - items.len() as i64,
            )
            .await?,
        );
    }

    Ok(Json(WorkbenchSearchResponse { items }))
}

pub async fn get_workbench_files_tree(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkbenchFileTreeQuery>,
) -> Result<Json<WorkbenchFileTreeResponse>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    file_service::authorize_current_project_file_read(
        &state,
        &ctx,
        tenant_id,
        query.project_id,
        query.run_id,
    )
    .await?;
    let prefix = query.prefix.unwrap_or_default();
    let files = if let Some(pattern) = query.pattern.as_deref() {
        file_store::glob_latest_revisions(
            &state.connect_pool,
            tenant_id,
            query.project_id,
            &prefix,
            pattern,
        )
        .await?
    } else {
        file_store::list_latest_revisions(&state.connect_pool, tenant_id, query.project_id, &prefix)
            .await?
    };
    let entries = file_store::directory_entries(&files, &prefix)?;

    Ok(Json(WorkbenchFileTreeResponse {
        project_id: query.project_id,
        prefix,
        files,
        entries,
    }))
}

pub async fn get_workbench_file_preview(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkbenchFilePreviewQuery>,
) -> Result<Json<PreviewDocument>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;

    if query.artifact_id.is_some() || query.object_reference_id.is_some() {
        let artifact = load_artifact_preview_row(
            &state,
            tenant_id,
            query.artifact_id,
            query.object_reference_id,
        )
        .await?;
        return preview_artifact_row(&state, &ctx, tenant_id, artifact)
            .await
            .map(Json);
    }

    let project_id = query
        .project_id
        .ok_or_else(|| AppError::InvalidInput("project_id is required".to_string()))?;
    let path = query
        .path
        .clone()
        .ok_or_else(|| AppError::InvalidInput("path is required".to_string()))?;
    file_service::authorize_current_file_access(
        &state,
        &ctx,
        tenant_id,
        project_id,
        &path,
        query.run_id,
    )
    .await?;
    let revision = file_store::read_revision(
        &state,
        FileReadRequest {
            tenant_id,
            actor_user_id: ctx.platform_user_id,
            actor_device_id: Some(ctx.device_id),
            actor_session_id: Some(ctx.session_id),
            project_id,
            path,
            revision: query.revision,
            version_id: None,
            run_id: query.run_id,
            include_content: Some(true),
            allow_binary: Some(false),
            offset_bytes: None,
            limit_bytes: None,
        },
    )
    .await?;

    Ok(Json(preview_document_from_revision(
        &revision,
        None,
        None,
        file_preview_kind(&revision.path, &revision.content_type, revision.is_binary),
    )))
}

pub async fn get_workbench_file_diff(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkbenchFileDiffQuery>,
) -> Result<Json<PreviewDocument>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    if query.from_revision <= 0 || query.to_revision <= 0 {
        return Err(AppError::InvalidInput(
            "from_revision and to_revision must be positive".to_string(),
        ));
    }
    file_service::authorize_current_file_access(
        &state,
        &ctx,
        tenant_id,
        query.project_id,
        &query.path,
        query.run_id,
    )
    .await?;
    let from = read_workbench_revision(
        &state,
        &ctx,
        tenant_id,
        query.project_id,
        &query.path,
        query.from_revision,
        query.run_id,
    )
    .await?;
    let to = read_workbench_revision(
        &state,
        &ctx,
        tenant_id,
        query.project_id,
        &query.path,
        query.to_revision,
        query.run_id,
    )
    .await?;
    let diff = if from.is_binary || to.is_binary {
        json!({
            "kind": "binary_metadata",
            "from": file_revision_metadata(&from),
            "to": file_revision_metadata(&to)
        })
    } else {
        let from_text = from.inline_content.as_deref().unwrap_or_default();
        let to_text = to.inline_content.as_deref().unwrap_or_default();
        let (patch, truncated) = simple_unified_diff(
            &query.path,
            query.from_revision,
            from_text,
            query.to_revision,
            to_text,
        );
        json!({
            "kind": "unified_diff",
            "path": query.path,
            "from_revision": query.from_revision,
            "to_revision": query.to_revision,
            "patch": patch,
            "truncated": truncated
        })
    };

    Ok(Json(PreviewDocument {
        id: format!(
            "file-diff:{}:{}:{}",
            query.project_id, query.path, query.to_revision
        ),
        title: file_title(&query.path),
        kind: "diff".to_string(),
        content: diff,
        source: PreviewDocumentSource {
            project_id: Some(query.project_id),
            path: Some(query.path),
            revision: Some(query.to_revision),
            artifact_id: None,
            object_reference_id: None,
        },
        actions: vec!["copy_path".to_string(), "ask_agent_to_edit".to_string()],
    }))
}

pub async fn get_workbench_artifact_preview(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(artifact_id): Path<Uuid>,
    Query(query): Query<WorkbenchArtifactPreviewQuery>,
) -> Result<Json<PreviewDocument>, AppError> {
    let tenant_id = resolve_workbench_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let artifact = load_artifact_preview_row(&state, tenant_id, Some(artifact_id), None).await?;
    preview_artifact_row(&state, &ctx, tenant_id, artifact)
        .await
        .map(Json)
}

struct WorkbenchArtifactPreviewRow {
    id: Uuid,
    run_id: Option<Uuid>,
    tool_call_id: Option<Uuid>,
    view_kind: String,
    project_id: Uuid,
    path: String,
    revision: i64,
    object_reference_id: Uuid,
}

async fn load_artifact_preview_row(
    state: &AppState,
    tenant_id: Uuid,
    artifact_id: Option<Uuid>,
    object_reference_id: Option<Uuid>,
) -> Result<WorkbenchArtifactPreviewRow, AppError> {
    match (artifact_id, object_reference_id) {
        (Some(artifact_id), _) => {
            let row = sqlx::query(
                r#"
                SELECT id, run_id, tool_call_id, view_kind, project_id, path, revision,
                       object_reference_id
                FROM tool_result_artifacts
                WHERE tenant_id = $1 AND id = $2
                "#,
            )
            .bind(tenant_id)
            .bind(artifact_id)
            .fetch_optional(&state.connect_pool)
            .await?
            .ok_or_else(|| AppError::NotFound("artifact not found".to_string()))?;
            artifact_preview_row_from_pg(row)
        }
        (None, Some(object_reference_id)) => {
            let row = sqlx::query(
                r#"
                SELECT id, run_id, tool_call_id, view_kind, project_id, path, revision,
                       object_reference_id
                FROM tool_result_artifacts
                WHERE tenant_id = $1 AND object_reference_id = $2
                "#,
            )
            .bind(tenant_id)
            .bind(object_reference_id)
            .fetch_optional(&state.connect_pool)
            .await?
            .ok_or_else(|| AppError::NotFound("artifact not found".to_string()))?;
            artifact_preview_row_from_pg(row)
        }
        (None, None) => Err(AppError::InvalidInput(
            "artifact_id or object_reference_id is required".to_string(),
        )),
    }
}

fn artifact_preview_row_from_pg(row: PgRow) -> Result<WorkbenchArtifactPreviewRow, AppError> {
    Ok(WorkbenchArtifactPreviewRow {
        id: row.try_get("id")?,
        run_id: row.try_get("run_id")?,
        tool_call_id: row.try_get("tool_call_id")?,
        view_kind: row.try_get("view_kind")?,
        project_id: row.try_get("project_id")?,
        path: row.try_get("path")?,
        revision: row.try_get("revision")?,
        object_reference_id: row.try_get("object_reference_id")?,
    })
}

async fn preview_artifact_row(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: Uuid,
    artifact: WorkbenchArtifactPreviewRow,
) -> Result<PreviewDocument, AppError> {
    file_service::authorize_current_file_access(
        state,
        ctx,
        tenant_id,
        artifact.project_id,
        &artifact.path,
        artifact.run_id,
    )
    .await?;
    let revision = read_workbench_revision(
        state,
        ctx,
        tenant_id,
        artifact.project_id,
        &artifact.path,
        artifact.revision,
        artifact.run_id,
    )
    .await?;
    let mut document = preview_document_from_revision(
        &revision,
        Some(artifact.id),
        Some(artifact.object_reference_id),
        artifact_preview_kind(&artifact.view_kind, &revision),
    );
    document.id = format!("artifact:{}", artifact.id);
    if artifact.tool_call_id.is_some() {
        document
            .actions
            .retain(|action| action != "ask_agent_to_edit");
    }
    Ok(document)
}

async fn read_workbench_revision(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: Uuid,
    project_id: Uuid,
    path: &str,
    revision: i64,
    run_id: Option<Uuid>,
) -> Result<FileRevisionResponse, AppError> {
    file_store::read_revision(
        state,
        FileReadRequest {
            tenant_id,
            actor_user_id: ctx.platform_user_id,
            actor_device_id: Some(ctx.device_id),
            actor_session_id: Some(ctx.session_id),
            project_id,
            path: path.to_string(),
            revision: Some(revision),
            version_id: None,
            run_id,
            include_content: Some(true),
            allow_binary: Some(false),
            offset_bytes: None,
            limit_bytes: None,
        },
    )
    .await
}

fn preview_document_from_revision(
    revision: &FileRevisionResponse,
    artifact_id: Option<Uuid>,
    object_reference_id: Option<Uuid>,
    kind: String,
) -> PreviewDocument {
    let text = revision.inline_content.as_deref().unwrap_or_default();
    let content = if revision.is_binary {
        json!({
            "kind": "binary_metadata",
            "metadata": file_revision_metadata(revision)
        })
    } else {
        json!({
            "kind": "text",
            "text": text,
            "content_type": revision.content_type,
            "size_bytes": revision.size_bytes,
            "offset_bytes": revision.content_offset_bytes.unwrap_or(0),
            "limit_bytes": revision.content_limit_bytes,
            "truncated": revision.content_truncated.unwrap_or(false),
            "metadata": file_revision_metadata(revision)
        })
    };
    PreviewDocument {
        id: if let Some(artifact_id) = artifact_id {
            format!("artifact:{artifact_id}")
        } else {
            format!(
                "file:{}:{}:{}",
                revision.project_id, revision.path, revision.revision
            )
        },
        title: file_title(&revision.path),
        kind,
        content,
        source: PreviewDocumentSource {
            project_id: Some(revision.project_id),
            path: Some(revision.path.clone()),
            revision: Some(revision.revision),
            artifact_id,
            object_reference_id,
        },
        actions: vec![
            "copy_path".to_string(),
            "download".to_string(),
            "ask_agent_to_edit".to_string(),
        ],
    }
}

fn file_revision_metadata(revision: &FileRevisionResponse) -> Value {
    json!({
        "id": revision.id,
        "project_id": revision.project_id,
        "path": revision.path,
        "revision": revision.revision,
        "etag": revision.etag,
        "content_hash": revision.content_hash,
        "object_reference_id": revision.object_reference_id,
        "bucket": revision.bucket,
        "version_id": revision.version_id,
        "size_bytes": revision.size_bytes,
        "content_type": revision.content_type,
        "is_binary": revision.is_binary,
        "is_large": revision.is_large,
        "reason": revision.reason,
        "run_id": revision.run_id,
        "created_at": revision.created_at,
    })
}

fn artifact_preview_kind(view_kind: &str, revision: &FileRevisionResponse) -> String {
    match view_kind {
        "table" | "chart" | "map" | "json" | "markdown" | "html" => view_kind.to_string(),
        _ => file_preview_kind(&revision.path, &revision.content_type, revision.is_binary),
    }
}

fn file_preview_kind(path: &str, content_type: &str, is_binary: bool) -> String {
    let lower_path = path.to_ascii_lowercase();
    let lower_type = content_type.to_ascii_lowercase();
    if lower_type.contains("markdown")
        || lower_path.ends_with(".md")
        || lower_path.ends_with(".mdx")
    {
        "markdown".to_string()
    } else if lower_type.contains("html")
        || lower_path.ends_with(".html")
        || lower_path.ends_with(".htm")
    {
        "html".to_string()
    } else if lower_type.contains("json") || lower_path.ends_with(".json") {
        "json".to_string()
    } else if lower_type.starts_with("image/") {
        "image".to_string()
    } else if lower_type == "application/pdf" || lower_path.ends_with(".pdf") {
        "pdf".to_string()
    } else if is_office_document(&lower_type, &lower_path) {
        "office".to_string()
    } else if !is_binary && is_code_path(&lower_path) {
        "code".to_string()
    } else if is_binary {
        "binary".to_string()
    } else {
        "text".to_string()
    }
}

fn is_office_document(content_type: &str, path: &str) -> bool {
    content_type.contains("officedocument")
        || [".doc", ".docx", ".ppt", ".pptx", ".xls", ".xlsx"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
}

fn is_code_path(path: &str) -> bool {
    [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".sql", ".css", ".html", ".json", ".yaml",
        ".yml", ".toml", ".sh", ".md",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

fn file_title(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_string()
}

fn simple_unified_diff(
    path: &str,
    from_revision: i64,
    from_text: &str,
    to_revision: i64,
    to_text: &str,
) -> (String, bool) {
    let from_lines: Vec<&str> = from_text.lines().collect();
    let to_lines: Vec<&str> = to_text.lines().collect();
    let max_len = from_lines.len().max(to_lines.len());
    let mut patch = format!("--- {path}@{from_revision}\n+++ {path}@{to_revision}\n");
    let mut truncated = false;
    for index in 0..max_len {
        match (from_lines.get(index), to_lines.get(index)) {
            (Some(left), Some(right)) if left == right => push_diff_line(&mut patch, ' ', left),
            (Some(left), Some(right)) => {
                push_diff_line(&mut patch, '-', left);
                push_diff_line(&mut patch, '+', right);
            }
            (Some(left), None) => push_diff_line(&mut patch, '-', left),
            (None, Some(right)) => push_diff_line(&mut patch, '+', right),
            (None, None) => {}
        }
        if patch.len() >= WORKBENCH_DIFF_MAX_CHARS {
            truncated = true;
            patch.truncate(WORKBENCH_DIFF_MAX_CHARS);
            patch.push_str("\n... diff truncated ...\n");
            break;
        }
    }
    (patch, truncated)
}

fn push_diff_line(patch: &mut String, prefix: char, line: &str) {
    patch.push(prefix);
    patch.push_str(line);
    patch.push('\n');
}

fn resolve_workbench_tenant(
    ctx: &PlatformRequestContext,
    requested_tenant_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if let Some(tenant_id) = requested_tenant_id
        && tenant_id != ctx.tenant_id
    {
        return Err(AppError::PermissionDenied(
            "requested tenant does not match current session tenant".to_string(),
        ));
    }
    Ok(ctx.tenant_id)
}

fn is_workbench_admin(me: &MeResponse) -> bool {
    has_any_role(me, &["platform_admin", "tenant_admin"])
        || has_any_capability(me, &["tenant:manage", "audit:read"])
}

fn has_any_role(me: &MeResponse, roles: &[&str]) -> bool {
    roles
        .iter()
        .any(|role| me.roles.iter().any(|candidate| candidate == role))
}

fn has_any_capability(me: &MeResponse, capabilities: &[&str]) -> bool {
    capabilities.iter().any(|capability| {
        me.capabilities
            .iter()
            .any(|candidate| candidate == capability)
    })
}

fn workbench_navigation_primary(me: &MeResponse) -> Vec<String> {
    let mut items = vec!["workbench".to_string()];
    items.push("teams".to_string());
    if has_any_capability(
        me,
        &[
            "catalog:manage",
            "conversation:create",
            "conversation:read",
            "run:read",
        ],
    ) {
        items.push("assistants".to_string());
    }
    if has_any_capability(me, &["workflow:manage", "workflow:run"]) {
        items.push("workflows".to_string());
    }
    if has_any_capability(me, &["memory:govern", "audit:read", "approval:decide"]) {
        items.push("governance".to_string());
    }
    items.push("settings".to_string());
    items
}

async fn load_agent_team_summaries(
    state: &AppState,
    tenant_id: Uuid,
    _user_id: Uuid,
    _is_admin: bool,
) -> Result<Vec<AgentTeamSummary>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT t.id, t.name, t.status, t.updated_at,
               COALESCE(m.member_count, 0)::BIGINT AS member_count,
               COALESCE(p.pending_approvals_count, 0)::BIGINT AS pending_approvals_count
        FROM agent_teams t
        LEFT JOIN LATERAL (
            SELECT COUNT(*)::BIGINT AS member_count
            FROM agent_team_members tm
            WHERE tm.tenant_id = t.tenant_id
              AND tm.team_id = t.id
              AND tm.deleted_at IS NULL
        ) m ON TRUE
        LEFT JOIN LATERAL (
            SELECT COUNT(DISTINCT a.id)::BIGINT AS pending_approvals_count
            FROM agent_team_runs tr
            JOIN agent_team_run_members trm
              ON trm.tenant_id = tr.tenant_id
             AND trm.team_run_id = tr.id
            JOIN approvals a
              ON a.tenant_id = tr.tenant_id
             AND a.run_id = trm.run_id
             AND a.status = 'pending'
            WHERE tr.tenant_id = t.tenant_id
              AND tr.team_id = t.id
        ) p ON TRUE
        WHERE t.tenant_id = $1
          AND t.deleted_at IS NULL
          AND t.status <> 'archived'
        ORDER BY t.updated_at DESC
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(WORKBENCH_BOOTSTRAP_LIMIT)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(agent_team_summary_from_row)
        .collect::<Result<Vec<_>, AppError>>()
}

fn agent_team_summary_from_row(row: PgRow) -> Result<AgentTeamSummary, AppError> {
    Ok(AgentTeamSummary {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        status: row.try_get("status")?,
        member_count: row.try_get("member_count")?,
        pending_approvals_count: row.try_get("pending_approvals_count")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_workspace_summaries(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    pinned_only: bool,
) -> Result<Vec<WorkspaceSummary>, AppError> {
    let rows = if pinned_only {
        sqlx::query(
            r#"
            SELECT w.id, w.tenant_id, w.name, w.status, w.trust_state, w.remote_project_id,
                   w.updated_at
            FROM workspace_pins p
            JOIN workspaces w
              ON w.id = p.target_id
             AND w.tenant_id = p.tenant_id
             AND p.target_type = 'workspace'
            WHERE p.tenant_id = $1
              AND p.user_id = $2
              AND w.deleted_at IS NULL
              AND ($3::bool OR w.owner_user_id = $2 OR w.owner_user_id IS NULL)
            ORDER BY p.sort_order ASC, p.created_at DESC
            LIMIT $4
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(is_admin)
        .bind(WORKBENCH_BOOTSTRAP_LIMIT)
        .fetch_all(&state.connect_pool)
        .await?
    } else {
        sqlx::query(
            r#"
            SELECT id, tenant_id, name, status, trust_state, remote_project_id, updated_at
            FROM workspaces
            WHERE tenant_id = $1
              AND deleted_at IS NULL
              AND ($2::bool OR owner_user_id = $3 OR owner_user_id IS NULL)
            ORDER BY updated_at DESC
            LIMIT $4
            "#,
        )
        .bind(tenant_id)
        .bind(is_admin)
        .bind(user_id)
        .bind(WORKBENCH_BOOTSTRAP_LIMIT)
        .fetch_all(&state.connect_pool)
        .await?
    };

    rows.into_iter().map(workspace_summary_from_row).collect()
}

async fn load_workspace_summary(
    state: &AppState,
    tenant_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
) -> Result<WorkspaceSummary, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, status, trust_state, remote_project_id, updated_at
        FROM workspaces
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
          AND ($3::bool OR owner_user_id = $4 OR owner_user_id IS NULL)
        "#,
    )
    .bind(workspace_id)
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workspace not found".to_string()))?;

    workspace_summary_from_row(row)
}

fn workspace_summary_from_row(row: PgRow) -> Result<WorkspaceSummary, AppError> {
    Ok(WorkspaceSummary {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        name: row.try_get("name")?,
        status: row.try_get("status")?,
        trust_state: row.try_get("trust_state")?,
        remote_project_id: row.try_get("remote_project_id")?,
        updated_at: row.try_get("updated_at")?,
        available_actions: vec![
            "conversation:create".to_string(),
            "workspace:read".to_string(),
        ],
    })
}

async fn load_local_mount_summaries(
    state: &AppState,
    tenant_id: Uuid,
    workspace_id: Uuid,
    ctx: &PlatformRequestContext,
) -> Result<Vec<LocalMountSummary>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, workspace_id, display_name, virtual_path, capabilities,
               trust_state, status, updated_at
        FROM local_mounts
        WHERE tenant_id = $1
          AND workspace_id = $2
          AND user_id = $3
          AND device_id = $4
          AND status = 'active'
        ORDER BY virtual_path ASC
        LIMIT 100
        "#,
    )
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(ctx.platform_user_id)
    .bind(ctx.device_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(LocalMountSummary {
                id: row.try_get("id")?,
                workspace_id: row.try_get("workspace_id")?,
                display_name: row.try_get("display_name")?,
                virtual_path: row.try_get("virtual_path")?,
                capabilities: row.try_get("capabilities")?,
                trust_state: row.try_get("trust_state")?,
                status: row.try_get("status")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .collect()
}

async fn load_project_summary(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
) -> Result<ResourceResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, name, description, status, metadata, created_at, updated_at
        FROM projects
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(project_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("project not found".to_string()))?;

    resource_from_row(row)
}

async fn load_conversation_summaries(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    workspace_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<ConversationSummary>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT c.id, c.tenant_id, c.workspace_id, c.project_id, c.title, c.status,
               c.updated_at,
               latest_run.status AS latest_run_status,
               COALESCE(activity.unread_activity_count, 0)::BIGINT AS unread_activity_count
        FROM conversations c
        LEFT JOIN LATERAL (
            SELECT status
            FROM runs r
            WHERE r.tenant_id = c.tenant_id
              AND r.conversation_id = c.id
            ORDER BY r.queued_at DESC
            LIMIT 1
        ) latest_run ON TRUE
        LEFT JOIN LATERAL (
            SELECT COUNT(*)::BIGINT AS unread_activity_count
            FROM run_events e
            WHERE e.tenant_id = c.tenant_id
              AND e.conversation_id = c.id
              AND e.type IN ('approval.requested', 'tool.call.failed', 'run.failed')
        ) activity ON TRUE
        WHERE c.tenant_id = $1
          AND c.deleted_at IS NULL
          AND ($2::bool OR c.created_by_user_id = $3)
          AND ($4::uuid IS NULL OR c.workspace_id = $4)
        ORDER BY c.updated_at DESC
        LIMIT $5
        "#,
    )
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .bind(workspace_id)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(conversation_summary_from_row)
        .collect()
}

async fn load_conversation_summary(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
) -> Result<ConversationSummary, AppError> {
    let row = sqlx::query(
        r#"
        SELECT c.id, c.tenant_id, c.workspace_id, c.project_id, c.title, c.status,
               c.updated_at,
               latest_run.status AS latest_run_status,
               COALESCE(activity.unread_activity_count, 0)::BIGINT AS unread_activity_count
        FROM conversations c
        LEFT JOIN LATERAL (
            SELECT status
            FROM runs r
            WHERE r.tenant_id = c.tenant_id
              AND r.conversation_id = c.id
            ORDER BY r.queued_at DESC
            LIMIT 1
        ) latest_run ON TRUE
        LEFT JOIN LATERAL (
            SELECT COUNT(*)::BIGINT AS unread_activity_count
            FROM run_events e
            WHERE e.tenant_id = c.tenant_id
              AND e.conversation_id = c.id
              AND e.type IN ('approval.requested', 'tool.call.failed', 'run.failed')
        ) activity ON TRUE
        WHERE c.id = $1
          AND c.tenant_id = $2
          AND c.deleted_at IS NULL
          AND ($3::bool OR c.created_by_user_id = $4)
        "#,
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;

    conversation_summary_from_row(row)
}

fn conversation_summary_from_row(row: PgRow) -> Result<ConversationSummary, AppError> {
    Ok(ConversationSummary {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        workspace_id: row.try_get("workspace_id")?,
        project_id: row.try_get("project_id")?,
        title: row.try_get("title")?,
        status: row.try_get("status")?,
        latest_run_status: row.try_get("latest_run_status")?,
        updated_at: row.try_get("updated_at")?,
        unread_activity_count: row.try_get("unread_activity_count")?,
    })
}

async fn count_pending_approvals(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
) -> Result<i64, AppError> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM approvals a
        LEFT JOIN conversations c ON c.id = a.conversation_id AND c.tenant_id = a.tenant_id
        LEFT JOIN runs r ON r.id = a.run_id AND r.tenant_id = a.tenant_id
        WHERE a.tenant_id = $1
          AND a.status = 'pending'
          AND ($2::bool OR c.created_by_user_id = $3 OR r.created_by_user_id = $3)
        "#,
    )
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(row.try_get("count")?)
}

async fn count_running_runs(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
) -> Result<i64, AppError> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS count
        FROM runs r
        LEFT JOIN conversations c ON c.id = r.conversation_id AND c.tenant_id = r.tenant_id
        WHERE r.tenant_id = $1
          AND r.status IN ('queued', 'running', 'waiting_approval')
          AND ($2::bool OR r.created_by_user_id = $3 OR c.created_by_user_id = $3)
        "#,
    )
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(row.try_get("count")?)
}

async fn load_latest_run(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
) -> Result<Option<RunResponse>, AppError> {
    let maybe_row = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
               project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
               queued_at, updated_at
        FROM runs
        WHERE tenant_id = $1 AND conversation_id = $2
        ORDER BY queued_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    maybe_row.map(run_from_row).transpose()
}

async fn load_conversation_events(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<StreamEventResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, run_id, seq, event_id, type, payload, trace_id,
               created_at
        FROM run_events
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND seq > $3
        ORDER BY seq ASC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(after_seq)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter().map(stream_event_from_row).collect()
}

async fn has_events_before(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    after_seq: i64,
) -> Result<bool, AppError> {
    let row = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM run_events
            WHERE tenant_id = $1
              AND conversation_id = $2
              AND seq <= $3
        ) AS exists
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .bind(after_seq)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(row.try_get("exists")?)
}

fn stream_event_from_row(row: PgRow) -> Result<StreamEventResponse, AppError> {
    Ok(StreamEventResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        conversation_id: row.try_get("conversation_id")?,
        run_id: row.try_get("run_id")?,
        seq: row.try_get("seq")?,
        event_id: row.try_get("event_id")?,
        event_type: row.try_get("type")?,
        payload: row.try_get("payload")?,
        trace_id: row.try_get("trace_id")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_conversation_pending_approvals(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
) -> Result<Vec<ApprovalResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, run_id, tool_call_id, status,
               approval_policy_id, request_payload, decision_payload,
               evidence_object_reference_id, created_at, decided_at
        FROM approvals
        WHERE tenant_id = $1
          AND conversation_id = $2
          AND status = 'pending'
        ORDER BY created_at ASC
        LIMIT 20
        "#,
    )
    .bind(tenant_id)
    .bind(conversation_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter().map(approval_from_row).collect()
}

async fn load_artifact_summaries(
    state: &AppState,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    conversation_id: Uuid,
) -> Result<Vec<WorkbenchArtifactSummary>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.run_id, a.tool_call_id, a.view_kind, a.project_id, a.path, a.revision,
               a.object_reference_id, a.content_type, a.size_bytes, a.created_at
        FROM tool_result_artifacts a
        LEFT JOIN runs r ON r.id = a.run_id AND r.tenant_id = a.tenant_id
        WHERE a.tenant_id = $1
          AND ($2::uuid IS NULL OR a.run_id = $2)
          AND ($2::uuid IS NOT NULL OR r.conversation_id = $3)
        ORDER BY a.created_at DESC
        LIMIT 20
        "#,
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(conversation_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let path: String = row.try_get("path")?;
            Ok(WorkbenchArtifactSummary {
                id: row.try_get("id")?,
                run_id: row.try_get("run_id")?,
                tool_call_id: row.try_get("tool_call_id")?,
                kind: row.try_get("view_kind")?,
                title: path.rsplit('/').next().unwrap_or(&path).to_string(),
                project_id: row.try_get("project_id")?,
                path,
                revision: row.try_get("revision")?,
                object_reference_id: row.try_get("object_reference_id")?,
                content_type: row.try_get("content_type")?,
                size_bytes: row.try_get("size_bytes")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

fn file_changes_from_events(events: &[StreamEventResponse]) -> Vec<WorkbenchFileChangeSummary> {
    events
        .iter()
        .filter(|event| event.event_type == "file.changed")
        .rev()
        .take(20)
        .map(|event| WorkbenchFileChangeSummary {
            event_id: event.event_id.clone(),
            seq: event.seq,
            project_id: payload_uuid(&event.payload, "project_id"),
            path: payload_string(&event.payload, "path"),
            operation: payload_string(&event.payload, "operation"),
            revision: payload_i64(&event.payload, "revision"),
            reason: payload_string(&event.payload, "reason"),
            created_at: event.created_at,
        })
        .collect()
}

fn task_summaries_from_events(events: &[StreamEventResponse]) -> Vec<WorkbenchTaskSummary> {
    events
        .iter()
        .filter(|event| event.event_type.starts_with("task."))
        .rev()
        .take(20)
        .filter_map(|event| {
            let task_id = payload_string(&event.payload, "task_id")?;
            Some(WorkbenchTaskSummary {
                task_id,
                title: payload_string(&event.payload, "title")
                    .unwrap_or_else(|| "Task".to_string()),
                status: payload_string(&event.payload, "status")
                    .unwrap_or_else(|| event.event_type.replace("task.", "")),
                summary: payload_string(&event.payload, "summary"),
                updated_at: event.created_at,
            })
        })
        .collect()
}

fn subagent_summaries_from_events(events: &[StreamEventResponse]) -> Vec<WorkbenchSubagentSummary> {
    events
        .iter()
        .filter(|event| event.event_type.starts_with("subagent."))
        .rev()
        .take(20)
        .filter_map(|event| {
            let subagent_id = payload_string(&event.payload, "subagent_id")?;
            Some(WorkbenchSubagentSummary {
                subagent_id,
                name: payload_string(&event.payload, "name")
                    .or_else(|| payload_string(&event.payload, "subagent_name"))
                    .unwrap_or_else(|| "Subagent".to_string()),
                status: payload_string(&event.payload, "status")
                    .unwrap_or_else(|| event.event_type.replace("subagent.", "")),
                parent_tool_call_id: payload_string(&event.payload, "parent_tool_call_id"),
                summary: payload_string(&event.payload, "summary"),
                updated_at: event.created_at,
            })
        })
        .collect()
}

async fn load_memory_candidate_summaries(
    state: &AppState,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    project_id: Option<Uuid>,
) -> Result<Vec<WorkbenchMemoryCandidateSummary>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, layer, content, confidence, status, updated_at
        FROM memory_items
        WHERE tenant_id = $1
          AND status = 'candidate'
          AND ($2::uuid IS NULL OR source_run_id = $2)
          AND ($3::uuid IS NULL OR project_id = $3 OR project_id IS NULL)
        ORDER BY updated_at DESC
        LIMIT 20
        "#,
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(project_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(WorkbenchMemoryCandidateSummary {
                id: row.try_get("id")?,
                layer: row.try_get("layer")?,
                content: row.try_get("content")?,
                confidence: row.try_get("confidence")?,
                status: row.try_get("status")?,
                updated_at: row.try_get("updated_at")?,
            })
        })
        .collect()
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).map(str::to_string)
}

fn payload_uuid(payload: &Value, key: &str) -> Option<Uuid> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn payload_i64(payload: &Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
            .or_else(|| value.as_str().and_then(|text| text.parse::<i64>().ok()))
    })
}

async fn load_recent_search_items(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    limit: i64,
) -> Result<Vec<WorkbenchSearchItem>, AppError> {
    let conversations =
        load_conversation_summaries(state, tenant_id, user_id, is_admin, None, limit).await?;
    Ok(conversations
        .into_iter()
        .map(|conversation| WorkbenchSearchItem {
            id: format!("conversation:{}", conversation.id),
            kind: "conversation".to_string(),
            title: conversation.title,
            subtitle: conversation
                .latest_run_status
                .clone()
                .unwrap_or_else(|| conversation.status.clone()),
            matched_text: None,
            target: WorkbenchSearchTarget {
                route: "workbench".to_string(),
                conversation_id: Some(conversation.id),
                workspace_id: conversation.workspace_id,
                team_id: None,
                artifact_id: None,
            },
            updated_at: conversation.updated_at,
        })
        .collect())
}

async fn search_workspaces(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    query: &str,
    limit: i64,
) -> Result<Vec<WorkbenchSearchItem>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, name, status, trust_state, remote_project_id, updated_at
        FROM workspaces
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND ($2::bool OR owner_user_id = $3 OR owner_user_id IS NULL)
          AND name ILIKE ('%' || $4 || '%')
        ORDER BY updated_at DESC
        LIMIT $5
        "#,
    )
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .bind(query)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(workspace_summary_from_row)
        .map(|result| {
            result.map(|workspace| WorkbenchSearchItem {
                id: format!("workspace:{}", workspace.id),
                kind: "workspace".to_string(),
                title: workspace.name,
                subtitle: workspace.trust_state,
                matched_text: None,
                target: WorkbenchSearchTarget {
                    route: "workbench".to_string(),
                    conversation_id: None,
                    workspace_id: Some(workspace.id),
                    team_id: None,
                    artifact_id: None,
                },
                updated_at: workspace.updated_at,
            })
        })
        .collect()
}

async fn search_conversations(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    query: &str,
    limit: i64,
) -> Result<Vec<WorkbenchSearchItem>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT c.id, c.tenant_id, c.workspace_id, c.project_id, c.title, c.status,
               c.updated_at,
               latest_run.status AS latest_run_status,
               COALESCE(activity.unread_activity_count, 0)::BIGINT AS unread_activity_count
        FROM conversations c
        LEFT JOIN LATERAL (
            SELECT status
            FROM runs r
            WHERE r.tenant_id = c.tenant_id
              AND r.conversation_id = c.id
            ORDER BY r.queued_at DESC
            LIMIT 1
        ) latest_run ON TRUE
        LEFT JOIN LATERAL (
            SELECT COUNT(*)::BIGINT AS unread_activity_count
            FROM run_events e
            WHERE e.tenant_id = c.tenant_id
              AND e.conversation_id = c.id
              AND e.type IN ('approval.requested', 'tool.call.failed', 'run.failed')
        ) activity ON TRUE
        WHERE c.tenant_id = $1
          AND c.deleted_at IS NULL
          AND ($2::bool OR c.created_by_user_id = $3)
          AND c.title ILIKE ('%' || $4 || '%')
        ORDER BY c.updated_at DESC
        LIMIT $5
        "#,
    )
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .bind(query)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(conversation_summary_from_row)
        .map(|result| {
            result.map(|conversation| WorkbenchSearchItem {
                id: format!("conversation:{}", conversation.id),
                kind: "conversation".to_string(),
                title: conversation.title,
                subtitle: conversation
                    .latest_run_status
                    .unwrap_or_else(|| conversation.status.clone()),
                matched_text: None,
                target: WorkbenchSearchTarget {
                    route: "workbench".to_string(),
                    conversation_id: Some(conversation.id),
                    workspace_id: conversation.workspace_id,
                    team_id: None,
                    artifact_id: None,
                },
                updated_at: conversation.updated_at,
            })
        })
        .collect()
}

async fn search_agent_teams(
    state: &AppState,
    tenant_id: Uuid,
    _user_id: Uuid,
    _is_admin: bool,
    query: &str,
    limit: i64,
) -> Result<Vec<WorkbenchSearchItem>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT t.id, t.name, t.status, t.updated_at,
               COALESCE(m.member_count, 0)::BIGINT AS member_count,
               COALESCE(p.pending_approvals_count, 0)::BIGINT AS pending_approvals_count
        FROM agent_teams t
        LEFT JOIN LATERAL (
            SELECT COUNT(*)::BIGINT AS member_count
            FROM agent_team_members tm
            WHERE tm.tenant_id = t.tenant_id
              AND tm.team_id = t.id
              AND tm.deleted_at IS NULL
        ) m ON TRUE
        LEFT JOIN LATERAL (
            SELECT COUNT(DISTINCT a.id)::BIGINT AS pending_approvals_count
            FROM agent_team_runs tr
            JOIN agent_team_run_members trm
              ON trm.tenant_id = tr.tenant_id
             AND trm.team_run_id = tr.id
            JOIN approvals a
              ON a.tenant_id = tr.tenant_id
             AND a.run_id = trm.run_id
             AND a.status = 'pending'
            WHERE tr.tenant_id = t.tenant_id
              AND tr.team_id = t.id
        ) p ON TRUE
        WHERE t.tenant_id = $1
          AND t.deleted_at IS NULL
          AND t.status <> 'archived'
          AND t.name ILIKE ('%' || $2 || '%')
        ORDER BY t.updated_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(query)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(agent_team_summary_from_row)
        .map(|result| {
            result.map(|team| WorkbenchSearchItem {
                id: format!("team:{}", team.id),
                kind: "team".to_string(),
                title: team.name,
                subtitle: format!("{} members · {}", team.member_count, team.status),
                matched_text: None,
                target: WorkbenchSearchTarget {
                    route: "team".to_string(),
                    conversation_id: None,
                    workspace_id: None,
                    team_id: Some(team.id),
                    artifact_id: None,
                },
                updated_at: team.updated_at,
            })
        })
        .collect()
}

async fn search_approvals(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    query: &str,
    limit: i64,
) -> Result<Vec<WorkbenchSearchItem>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.conversation_id, a.request_payload, a.status, a.created_at
        FROM approvals a
        LEFT JOIN conversations c ON c.id = a.conversation_id AND c.tenant_id = a.tenant_id
        LEFT JOIN runs r ON r.id = a.run_id AND r.tenant_id = a.tenant_id
        WHERE a.tenant_id = $1
          AND ($2::bool OR c.created_by_user_id = $3 OR r.created_by_user_id = $3)
          AND a.request_payload::text ILIKE ('%' || $4 || '%')
        ORDER BY a.created_at DESC
        LIMIT $5
        "#,
    )
    .bind(tenant_id)
    .bind(is_admin)
    .bind(user_id)
    .bind(query)
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            let approval_id: Uuid = row.try_get("id")?;
            let conversation_id: Option<Uuid> = row.try_get("conversation_id")?;
            let payload: Value = row.try_get("request_payload")?;
            let status: String = row.try_get("status")?;
            Ok(WorkbenchSearchItem {
                id: format!("approval:{approval_id}"),
                kind: "approval".to_string(),
                title: payload_string(&payload, "summary")
                    .or_else(|| payload_string(&payload, "tool_name"))
                    .unwrap_or_else(|| "Approval".to_string()),
                subtitle: status,
                matched_text: payload_string(&payload, "summary"),
                target: WorkbenchSearchTarget {
                    route: "workbench".to_string(),
                    conversation_id,
                    workspace_id: None,
                    team_id: None,
                    artifact_id: None,
                },
                updated_at: row.try_get("created_at")?,
            })
        })
        .collect()
}
