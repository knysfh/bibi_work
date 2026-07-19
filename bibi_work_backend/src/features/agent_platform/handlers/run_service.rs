use std::{collections::HashSet, convert::Infallible};

use axum::{
    Extension, Json,
    extract::{Path, Query, State, WebSocketUpgrade},
    http::HeaderMap,
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
};
use futures_util::Stream;
use serde_json::{Value, json};
use sqlx::Row;
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::{self, ArchivedAuditEvidence, NewAuditLog, ToolCallEvidenceInput},
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            local_runtime_queue, memory_context,
            models::*,
            run_lifecycle,
            run_snapshot::{self, ConversationRunSnapshotRequest},
            runtime::{CancelRunRequest, DispatchRunRequest},
            rustfs::RustFsClient,
            secret_resolver,
        },
        core::{errors::AppError, models::GenericResponse},
    },
    startup::AppState,
};

use super::{
    agent_team_service, capability_authz, memory_injection, memory_service, support::*,
    workflow_scheduler,
};

pub async fn run_stream(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventStreamQuery>,
    Json(payload): Json<CreateRunRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let after_seq = event_store::resolve_after_seq(&headers, query.after_seq);
    let _run = create_and_dispatch_conversation_run(&state, &ctx, conversation_id, payload).await?;
    let events = event_store::fetch_events(&state.connect_pool, conversation_id, after_seq).await?;
    Ok(event_store::events_to_sse(events))
}

pub(super) async fn create_and_dispatch_conversation_run(
    state: &AppState,
    ctx: &PlatformRequestContext,
    conversation_id: Uuid,
    payload: CreateRunRequest,
) -> Result<RunResponse, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let trace_id = Uuid::new_v4().to_string();
    let run_id = Uuid::new_v4();
    let input = payload.input.unwrap_or_else(|| json!({}));
    let conversation_scope =
        load_conversation_run_scope(state, payload.tenant_id, conversation_id).await?;
    let workspace_id = conversation_scope.workspace_id;
    let project_id = resolve_run_project_id(
        payload.project_id,
        conversation_scope.project_id,
        conversation_scope.remote_project_id,
    )?;
    let requested_agent_id = payload
        .agent_id
        .or(conversation_scope.agent_id)
        .or(conversation_scope.default_agent_id);
    let requested_agent_version_id = payload
        .agent_version_id
        .or(conversation_scope.agent_version_id)
        .or(conversation_scope.default_agent_version_id);
    let compiled_snapshot = run_snapshot::compile_conversation_run_snapshot(
        &state.connect_pool,
        ConversationRunSnapshotRequest {
            tenant_id: payload.tenant_id,
            conversation_id,
            run_id,
            workspace_id,
            requested_agent_id,
            agent_version_id: requested_agent_version_id,
            project_id,
            selected_mcp_server_ids: conversation_scope.selected_mcp_server_ids.clone(),
            thread_id: payload.thread_id.clone(),
            client_snapshot: payload.run_config_snapshot,
            ctx,
        },
    )
    .await?;
    let resolved_agent_id = compiled_snapshot.agent_id;
    let resolved_agent_version_id = compiled_snapshot.agent_version_id;
    let mut snapshot = compiled_snapshot.snapshot;
    let runtime_kind = run_snapshot::execution_runtime_kind(&snapshot)?.to_string();
    if runtime_kind != run_snapshot::PYTHON_RUNTIME_KIND
        && runtime_kind != run_snapshot::DESKTOP_ACP_RUNTIME_KIND
    {
        run_snapshot::ensure_python_dispatch_runtime(&snapshot)?;
    }

    require_ferriskey_allow(
        state,
        ctx,
        payload.tenant_id,
        "run",
        "conversation",
        conversation_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(conversation_id),
            agent_id: resolved_agent_id,
            project_id,
            ..Default::default()
        }),
    )
    .await?;
    if let Some(agent_id) = resolved_agent_id {
        require_ferriskey_allow(
            state,
            ctx,
            payload.tenant_id,
            "run",
            "agent",
            agent_id.to_string(),
            Some(AuthzContext {
                conversation_id: Some(conversation_id),
                agent_id: Some(agent_id),
                project_id,
                ..Default::default()
            }),
        )
        .await?;
    }
    if let Some(project_id) = project_id {
        require_ferriskey_allow(
            state,
            ctx,
            payload.tenant_id,
            "use",
            "project",
            project_id.to_string(),
            Some(AuthzContext {
                conversation_id: Some(conversation_id),
                agent_id: resolved_agent_id,
                project_id: Some(project_id),
                ..Default::default()
            }),
        )
        .await?;
    }
    if let Some(agent_version_id) = resolved_agent_version_id {
        capability_authz::require_agent_version_capabilities(
            state,
            ctx,
            payload.tenant_id,
            agent_version_id,
            AuthzContext {
                conversation_id: Some(conversation_id),
                agent_id: resolved_agent_id,
                project_id,
                ..Default::default()
            },
        )
        .await?;
    }

    if let Some(idempotency_key) = payload.idempotency_key.as_deref()
        && let Some(existing) =
            find_run_by_idempotency(&state.connect_pool, payload.tenant_id, idempotency_key).await?
    {
        return Ok(existing);
    }

    let mut tx = state.connect_pool.begin().await?;

    let run_row = sqlx::query(
        r#"
        INSERT INTO runs (
            id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
            project_id, created_by_user_id, status, idempotency_key, input,
            run_config_snapshot, run_scope_snapshot, policy_version, risk_policy_version,
            trace_id, thread_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', $9, $10, $11, $12, $13, $14, $15, $16)
        RETURNING id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
                  project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
                  queued_at, updated_at
        "#,
    )
    .bind(run_id)
    .bind(payload.tenant_id)
    .bind(conversation_id)
    .bind(workspace_id)
    .bind(resolved_agent_id)
    .bind(resolved_agent_version_id)
    .bind(project_id)
    .bind(ctx.platform_user_id)
    .bind(payload.idempotency_key.clone())
    .bind(&input)
    .bind(&snapshot)
    .bind(&compiled_snapshot.scope_snapshot)
    .bind(LOCAL_POLICY_VERSION)
    .bind(LOCAL_RISK_POLICY_VERSION)
    .bind(trace_id.clone())
    .bind(payload.thread_id.clone())
    .fetch_one(&mut *tx)
    .await?;

    let run = run_from_row(run_row)?;
    let initial_events = initial_run_events(&run, conversation_id, ctx.platform_user_id, &input);
    let mut persisted_events = Vec::with_capacity(initial_events.len());
    for initial_event in initial_events {
        let event = event_store::insert_event_tx(
            &mut tx,
            payload.tenant_id,
            conversation_id,
            Some(run.id),
            initial_event,
        )
        .await?;
        persisted_events.push(event);
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    for event in &persisted_events {
        event_store::publish_single_event(state, event).await;
    }

    if runtime_kind == run_snapshot::DESKTOP_ACP_RUNTIME_KIND {
        let runtime = snapshot.get("runtime").cloned().ok_or_else(|| {
            AppError::InvalidInput("desktop ACP runtime config is required".to_string())
        })?;
        let queue_result = local_runtime_queue::enqueue(
            &state.connect_pool,
            local_runtime_queue::EnqueueLocalRuntimeRequest {
                tenant_id: payload.tenant_id,
                device_id: Some(ctx.device_id),
                project_id,
                run_id: Some(run.id),
                command: json!({
                    "protocol": "biwork_acp.v1",
                    "kind": run_snapshot::DESKTOP_ACP_RUNTIME_KIND,
                    "conversation_id": conversation_id,
                    "run_id": run.id,
                    "trace_id": run.trace_id.clone(),
                    "trace_context": crate::telemetry::current_trace_headers(),
                    "input": input.clone(),
                    "runtime": runtime
                }),
                timeout_ms: 300_000,
                max_output_bytes: 4 * 1_048_576,
            },
        )
        .await;
        if let Err(err) = queue_result {
            run_lifecycle::mark_dispatch_failed(
                state,
                payload.tenant_id,
                conversation_id,
                run.id,
                Some(run.trace_id.clone()),
                &err.to_string(),
            )
            .await?;
            return Err(err);
        }
        return Ok(run);
    }

    memory_injection::inject_memory_context_for_run(
        state,
        memory_injection::MemoryInjectionRequest {
            actor: ActorRef {
                user_id: ctx.platform_user_id,
                device_id: Some(ctx.device_id),
                session_id: Some(ctx.session_id),
                roles: ctx.roles.clone(),
            },
            tenant_id: payload.tenant_id,
            run_id: run.id,
            agent_id: resolved_agent_id,
            project_id,
        },
        &input,
        &mut snapshot,
    )
    .await?;

    if let Err(err) = secret_resolver::attach_llm_runtime_credential(
        state,
        payload.tenant_id,
        run.id,
        &mut snapshot,
    )
    .await
    {
        run_lifecycle::mark_dispatch_failed(
            state,
            payload.tenant_id,
            conversation_id,
            run.id,
            Some(run.trace_id.clone()),
            &err.to_string(),
        )
        .await?;
        return Err(err);
    }

    if let Err(err) = state
        .agent_runtime_client
        .dispatch_run(&DispatchRunRequest {
            tenant_id: payload.tenant_id,
            conversation_id,
            run_id: run.id,
            trace_id: run.trace_id.clone(),
            input,
            run_config_snapshot: snapshot,
        })
        .await
    {
        run_lifecycle::mark_dispatch_failed(
            state,
            payload.tenant_id,
            conversation_id,
            run.id,
            Some(run.trace_id.clone()),
            &err.to_string(),
        )
        .await?;
        return Err(err);
    }

    Ok(run)
}

struct ConversationRunScope {
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    agent_id: Option<Uuid>,
    agent_version_id: Option<Uuid>,
    remote_project_id: Option<Uuid>,
    default_agent_id: Option<Uuid>,
    default_agent_version_id: Option<Uuid>,
    selected_mcp_server_ids: Vec<Uuid>,
}

async fn load_conversation_run_scope(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
) -> Result<ConversationRunScope, AppError> {
    let row = sqlx::query(
        r#"
        SELECT c.workspace_id, c.project_id, c.agent_id, c.metadata,
               w.id AS loaded_workspace_id, w.remote_project_id, w.default_agent_id,
               w.default_agent_version_id,
               (
                   SELECT av.id
                   FROM agent_versions av
                   WHERE av.tenant_id = c.tenant_id
                     AND av.agent_id = c.agent_id
                     AND av.status = 'published'
                   ORDER BY av.created_at DESC, av.id DESC
                   LIMIT 1
               ) AS latest_agent_version_id
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
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;
    let workspace_id: Option<Uuid> = row.try_get("workspace_id")?;
    let loaded_workspace_id: Option<Uuid> = row.try_get("loaded_workspace_id")?;
    if workspace_id.is_some() && loaded_workspace_id.is_none() {
        return Err(AppError::NotFound("workspace not found".to_string()));
    }
    let metadata: Value = row.try_get("metadata")?;
    let pinned_agent_version_id = conversation_pinned_agent_version_id(&metadata)?;
    let latest_agent_version_id: Option<Uuid> = row.try_get("latest_agent_version_id")?;
    let agent_version_id = resolve_conversation_agent_version(
        state,
        tenant_id,
        conversation_id,
        pinned_agent_version_id,
        latest_agent_version_id,
    )
    .await?;
    let selected_mcp_server_ids = conversation_selected_mcp_server_ids(&metadata)?;
    Ok(ConversationRunScope {
        workspace_id,
        project_id: row.try_get("project_id")?,
        agent_id: row.try_get("agent_id")?,
        agent_version_id,
        remote_project_id: row.try_get("remote_project_id")?,
        default_agent_id: row.try_get("default_agent_id")?,
        default_agent_version_id: row.try_get("default_agent_version_id")?,
        selected_mcp_server_ids,
    })
}

async fn resolve_conversation_agent_version(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    pinned_agent_version_id: Option<Uuid>,
    latest_agent_version_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    let Some(pinned_agent_version_id) = pinned_agent_version_id else {
        return Ok(latest_agent_version_id);
    };
    let pinned_is_published: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_versions
            WHERE id = $1
              AND tenant_id = $2
              AND status = 'published'
        )
        "#,
    )
    .bind(pinned_agent_version_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if pinned_is_published {
        return Ok(Some(pinned_agent_version_id));
    }

    let Some(latest_agent_version_id) = latest_agent_version_id else {
        return Ok(Some(pinned_agent_version_id));
    };
    sqlx::query(
        r#"
        UPDATE conversations
        SET metadata = jsonb_set(
                metadata,
                '{biwork,agent_version_id}',
                to_jsonb($3::text),
                true
            ),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(latest_agent_version_id)
    .execute(&state.connect_pool)
    .await?;
    Ok(Some(latest_agent_version_id))
}

fn conversation_selected_mcp_server_ids(metadata: &Value) -> Result<Vec<Uuid>, AppError> {
    let selected = metadata
        .pointer("/extra/selected_mcp_server_ids")
        .or_else(|| metadata.pointer("/extra/mcp_server_ids"));
    let Some(values) = selected else {
        return Ok(Vec::new());
    };
    let values = values.as_array().ok_or_else(|| {
        AppError::InvalidInput("conversation MCP server ids must be an array".to_string())
    })?;
    let mut ids = Vec::with_capacity(values.len());
    for value in values {
        let raw = value.as_str().ok_or_else(|| {
            AppError::InvalidInput("conversation MCP server id must be a UUID".to_string())
        })?;
        let id = Uuid::parse_str(raw).map_err(|_| {
            AppError::InvalidInput("conversation MCP server id must be a UUID".to_string())
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn conversation_pinned_agent_version_id(metadata: &Value) -> Result<Option<Uuid>, AppError> {
    let Some(value) = metadata.pointer("/biwork/agent_version_id") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value.as_str().ok_or_else(|| {
        AppError::InvalidInput("conversation agent_version_id must be a UUID".to_string())
    })?;
    Uuid::parse_str(value).map(Some).map_err(|_| {
        AppError::InvalidInput("conversation agent_version_id must be a UUID".to_string())
    })
}

fn resolve_run_project_id(
    requested_project_id: Option<Uuid>,
    conversation_project_id: Option<Uuid>,
    workspace_project_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    let inherited_project_id = conversation_project_id.or(workspace_project_id);
    if let (Some(requested), Some(inherited)) = (requested_project_id, inherited_project_id)
        && requested != inherited
    {
        return Err(AppError::InvalidInput(
            "run project_id cannot expand conversation workspace scope".to_string(),
        ));
    }
    Ok(requested_project_id.or(inherited_project_id))
}

fn initial_run_events(
    run: &RunResponse,
    conversation_id: Uuid,
    author_user_id: Uuid,
    input: &Value,
) -> Vec<RunEventInput> {
    let user_message_id = format!("user.{}", run.id);
    vec![
        RunEventInput {
            event_id: Some(format!("message.completed.{}", user_message_id)),
            event_type: "message.completed".to_string(),
            payload: Some(json!({
                "message_id": user_message_id,
                "role": "user",
                "content": submitted_user_content(input),
                "run_id": run.id,
                "author_user_id": author_user_id
            })),
            trace_id: Some(run.trace_id.clone()),
        },
        RunEventInput {
            event_id: Some(format!("run.queued.{}", run.id)),
            event_type: "run.queued".to_string(),
            payload: Some(json!({
                "run_id": run.id,
                "conversation_id": conversation_id,
                "status": run.status,
                "trace_id": run.trace_id
            })),
            trace_id: Some(run.trace_id.clone()),
        },
    ]
}

fn submitted_user_content(input: &Value) -> String {
    if let Some(messages) = input.get("messages").and_then(Value::as_array)
        && let Some(content) = messages
            .iter()
            .rev()
            .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
            .and_then(|message| message.get("content"))
            .and_then(message_content_to_text)
    {
        return content;
    }

    if let Some(content) = input.get("content").and_then(message_content_to_text) {
        return content;
    }

    message_content_to_text(input).unwrap_or_else(|| input.to_string())
}

fn message_content_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(content) => non_empty(content.clone()),
        Value::Array(parts) => {
            let content = parts
                .iter()
                .filter_map(message_content_to_text)
                .collect::<Vec<_>>()
                .join("\n");
            non_empty(content)
        }
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                return non_empty(text.to_string());
            }
            object.get("content").and_then(message_content_to_text)
        }
        _ => None,
    }
}

fn non_empty(content: String) -> Option<String> {
    if content.trim().is_empty() {
        None
    } else {
        Some(content)
    }
}

#[cfg(test)]
mod initial_run_event_tests {
    use super::*;

    fn test_run() -> RunResponse {
        RunResponse {
            id: Uuid::parse_str("30000000-0000-0000-0000-000000000001").unwrap(),
            tenant_id: Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap(),
            conversation_id: Uuid::parse_str("20000000-0000-0000-0000-000000000001").unwrap(),
            workspace_id: None,
            agent_id: None,
            agent_version_id: None,
            project_id: None,
            status: "queued".to_string(),
            trace_id: "trace".to_string(),
            thread_id: None,
            policy_version: LOCAL_POLICY_VERSION.to_string(),
            run_scope_snapshot: json!({}),
            queued_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn initial_run_events_put_user_message_before_queued() {
        let run = test_run();
        let conversation_id = run.conversation_id;
        let author_user_id = Uuid::parse_str("40000000-0000-0000-0000-000000000001").unwrap();
        let events = initial_run_events(
            &run,
            conversation_id,
            author_user_id,
            &json!({ "messages": [{ "role": "user", "content": "你好" }] }),
        );

        assert_eq!(events[0].event_type, "message.completed");
        assert_eq!(events[1].event_type, "run.queued");
        assert_eq!(events[0].payload.as_ref().unwrap()["role"], "user");
        assert_eq!(events[0].payload.as_ref().unwrap()["content"], "你好");
    }

    #[test]
    fn submitted_user_content_supports_text_parts() {
        let content = submitted_user_content(&json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "第一段" },
                        { "type": "text", "text": "第二段" }
                    ]
                }
            ]
        }));

        assert_eq!(content, "第一段\n第二段");
    }
}

pub async fn list_runs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TenantListQuery>,
) -> Result<Json<Vec<RunResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
               project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
               queued_at, updated_at
        FROM runs
        WHERE tenant_id = $1 AND ($2::text IS NULL OR status = $2)
        ORDER BY queued_at DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).min(500))
    .fetch_all(&state.connect_pool)
    .await?;

    let runs = rows
        .into_iter()
        .map(run_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(runs))
}

pub async fn get_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunResponse>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
               project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
               queued_at, updated_at
        FROM runs
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("run not found".to_string()))?;

    let run = run_from_row(row)?;
    require_ferriskey_allow(
        &state,
        &ctx,
        run.tenant_id,
        "read",
        "run",
        run.id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(run.conversation_id),
            run_id: Some(run.id),
            agent_id: run.agent_id,
            project_id: run.project_id,
            ..Default::default()
        }),
    )
    .await?;

    Ok(Json(run))
}

pub async fn cancel_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunResponse>, AppError> {
    let run = load_run(&state.connect_pool, run_id).await?;
    ensure_tenant_member(&state.connect_pool, run.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        run.tenant_id,
        "cancel",
        "run",
        run.id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(run.conversation_id),
            run_id: Some(run.id),
            agent_id: run.agent_id,
            project_id: run.project_id,
            ..Default::default()
        }),
    )
    .await?;

    if run.status == "cancelled" {
        return Ok(Json(run));
    }
    if matches!(run.status.as_str(), "completed" | "failed") {
        return Err(AppError::Conflict(
            "terminal run cannot be cancelled".to_string(),
        ));
    }

    let mut tx = state.connect_pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE runs
        SET status = 'cancelled', completed_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND status NOT IN ('completed', 'failed', 'cancelled')
        RETURNING id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
                  project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
                  queued_at, updated_at
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        return Err(AppError::Conflict(
            "terminal run cannot be cancelled".to_string(),
        ));
    };

    let updated = run_from_row(row)?;
    event_store::update_scheduled_job_run_status_from_event(
        &mut tx,
        Some(updated.id),
        "run.cancelled",
    )
    .await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        updated.tenant_id,
        updated.conversation_id,
        Some(updated.id),
        RunEventInput {
            event_id: Some(format!("run.cancelled.{}", updated.id)),
            event_type: "run.cancelled".to_string(),
            payload: Some(json!({ "run_id": updated.id, "status": updated.status })),
            trace_id: Some(updated.trace_id.clone()),
        },
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE local_exec_requests
        SET status = 'cancelled', updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND run_id = $2
          AND status IN ('queued', 'dispatching')
        "#,
    )
    .bind(updated.tenant_id)
    .bind(updated.id)
    .execute(&mut *tx)
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(&state, &event).await;
    if let Err(err) = state
        .agent_runtime_client
        .cancel_run(
            updated.id,
            &CancelRunRequest {
                tenant_id: updated.tenant_id,
                conversation_id: updated.conversation_id,
                trace_id: Some(updated.trace_id.clone()),
                reason: "user_cancelled".to_string(),
            },
        )
        .await
    {
        warn!("failed to propagate cancel for run {}: {}", updated.id, err);
    }

    Ok(Json(updated))
}

pub async fn internal_agent_run_resume(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<Json<RunResponse>, AppError> {
    let mut tx = state.connect_pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE runs
        SET status = 'running', started_at = COALESCE(started_at, CURRENT_TIMESTAMP), updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        RETURNING id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
                  project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
                  queued_at, updated_at
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("run not found".to_string()))?;

    let run = run_from_row(row)?;
    let event = event_store::insert_event_tx(
        &mut tx,
        run.tenant_id,
        run.conversation_id,
        Some(run.id),
        RunEventInput {
            event_id: Some(format!("run.resumed.{}", run.id)),
            event_type: "run.started".to_string(),
            payload: Some(json!({ "run_id": run.id, "status": run.status })),
            trace_id: Some(run.trace_id.clone()),
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(&state, &event).await;

    Ok(Json(run))
}

pub async fn ingest_run_events(
    State(state): State<AppState>,
    Json(payload): Json<IngestRunEventsRequest>,
) -> Result<Json<IngestRunEventsResponse>, AppError> {
    if payload.events.is_empty() {
        return Err(AppError::InvalidInput(
            "events must not be empty".to_string(),
        ));
    }

    let mut tx = state.connect_pool.begin().await?;
    let mut inserted = Vec::with_capacity(payload.events.len());
    let mut workflow_runs_to_tick = HashSet::new();
    let mut completed_events = Vec::new();
    let mut archived_tool_evidence = Vec::new();
    let process_result = async {
        for event in payload.events {
            let mut event = event;
            let event_type = event.event_type.clone();
            let event_payload = sanitize_tool_result_payload(
                &event_type,
                event.payload.take().unwrap_or_else(|| json!({})),
            );
            let event_payload = validate_tool_result_payload_refs_tx(
                &mut tx,
                payload.tenant_id,
                payload.run_id,
                event_payload,
            )
            .await?;
            event.payload = Some(event_payload.clone());
            let inserted_event = event_store::insert_event_tx(
                &mut tx,
                payload.tenant_id,
                payload.conversation_id,
                payload.run_id,
                event,
            )
            .await?;

            if event_store::is_run_state_event(&event_type) {
                event_store::update_run_status_from_event(&mut tx, payload.run_id, &event_type)
                    .await?;
                inserted.push(inserted_event.clone());
                inserted.extend(
                    agent_team_service::apply_team_member_run_state_event_tx(
                        &mut tx,
                        payload.tenant_id,
                        payload.conversation_id,
                        payload.run_id,
                        &event_type,
                        &event_payload,
                        inserted_event.trace_id.clone(),
                    )
                    .await?,
                );
                if let Some(workflow_run_id) = update_workflow_node_status_from_run_event(
                    &mut tx,
                    payload.run_id,
                    &event_type,
                    &event_payload,
                )
                .await?
                {
                    workflow_runs_to_tick.insert(workflow_run_id);
                }
            }
            if let Some(tool_event) = tool_call_event_update(&event_type, &event_payload) {
                apply_tool_call_event_update(
                    &mut tx,
                    &state.rustfs_client,
                    payload.tenant_id,
                    payload.run_id,
                    tool_event,
                )
                .await?
                .into_iter()
                .for_each(|evidence| archived_tool_evidence.push(evidence));
            }
            if event_type == "run.completed" {
                completed_events.push((payload.run_id, inserted_event.id, event_payload));
            }

            if !event_store::is_run_state_event(&event_type) {
                inserted.push(inserted_event);
            }
        }

        Ok::<_, AppError>(())
    }
    .await;

    if let Err(err) = process_result {
        cleanup_archived_audit_evidence(&state, archived_tool_evidence).await;
        return Err(err);
    }

    if tx.commit().await.is_err() {
        cleanup_archived_audit_evidence(&state, archived_tool_evidence).await;
        return Err(AppError::DatabaseTransaction);
    }

    for event in &inserted {
        event_store::publish_single_event(&state, event).await;
    }
    for workflow_run_id in workflow_runs_to_tick {
        workflow_scheduler::tick_workflow_run(&state, workflow_run_id).await?;
    }
    for (run_id, event_id, event_payload) in completed_events {
        if let Err(err) = memory_service::create_candidates_from_run_completed_event(
            &state,
            run_id,
            event_id,
            &event_payload,
        )
        .await
        {
            warn!(
                "failed to create memory candidates from run.completed event {}: {}",
                event_id, err
            );
        }
    }

    Ok(Json(IngestRunEventsResponse { events: inserted }))
}

pub async fn get_conversation_events(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    Query(query): Query<EventStreamQuery>,
) -> Result<Json<Vec<StreamEventResponse>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "subscribe",
        "conversation",
        conversation_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(conversation_id),
            ..Default::default()
        }),
    )
    .await?;
    let events = event_store::fetch_events(
        &state.connect_pool,
        conversation_id,
        query.after_seq.unwrap_or(0),
    )
    .await?;
    Ok(Json(events))
}

pub async fn get_conversation_event_stream(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventStreamQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "subscribe",
        "conversation",
        conversation_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(conversation_id),
            ..Default::default()
        }),
    )
    .await?;

    let after_seq = event_store::resolve_after_seq(&headers, query.after_seq);
    Ok(event_store::live_events_to_sse(
        state.connect_pool.clone(),
        state.redis_client.clone(),
        tenant_id,
        conversation_id,
        after_seq,
        ctx,
    ))
}

pub async fn get_conversation_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(conversation_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventStreamQuery>,
) -> Result<impl IntoResponse, AppError> {
    let tenant_id = query
        .tenant_id
        .ok_or_else(|| AppError::InvalidInput("tenant_id is required".to_string()))?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "subscribe",
        "conversation",
        conversation_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(conversation_id),
            ..Default::default()
        }),
    )
    .await?;
    let after_seq = event_store::resolve_after_seq(&headers, query.after_seq);
    Ok(ws.on_upgrade(move |socket| async move {
        event_store::handle_conversation_socket(
            socket,
            state.connect_pool.clone(),
            state.redis_client.clone(),
            tenant_id,
            conversation_id,
            after_seq,
            ctx,
        )
        .await;
    }))
}

const MAX_TOOL_RESULT_VIEWS: usize = 8;
const MAX_TOOL_RESULT_COLUMNS: usize = 48;
const MAX_TOOL_RESULT_ROWS: usize = 50;
const MAX_TOOL_RESULT_FILES: usize = 50;
const MAX_TOOL_RESULT_PREVIEW_BYTES: usize = 32 * 1024;
const MAX_TOOL_RESULT_MARKDOWN_CHARS: usize = 4000;
const MAX_TOOL_RESULT_TITLE_CHARS: usize = 160;
const MAX_TOOL_SUMMARY_CHARS: usize = 4000;
const MAX_TOOL_ARGUMENT_DELTA_CHARS: usize = 4096;
const MAX_TOOL_ARGUMENT_TEXT_CHARS: usize = 16 * 1024;
const MAX_TOOL_INPUT_SUMMARY_CHARS: usize = 1000;
const MAX_ARTIFACT_DRAFT_PATH_CHARS: usize = 1024;
const MAX_ARTIFACT_DRAFT_DELTA_CHARS: usize = 4096;
const MAX_ARTIFACT_DRAFT_ERROR_CHARS: usize = 1000;
const MAX_ARTIFACT_DRAFT_PREVIOUS_CHARS: usize = 80 * 1024;

fn sanitize_tool_result_payload(event_type: &str, payload: Value) -> Value {
    if event_type.starts_with("artifact.draft.") {
        return sanitize_artifact_draft_payload(event_type, payload);
    }
    if event_type == "tool.call.delta" {
        return sanitize_tool_call_delta_payload(payload);
    }

    if !matches!(event_type, "tool.call.completed" | "tool.call.failed") {
        return payload;
    }

    let Value::Object(mut payload_object) = payload else {
        return json!({});
    };
    sanitize_tool_terminal_summary(
        &mut payload_object,
        if event_type == "tool.call.completed" {
            "output_summary"
        } else {
            "error_summary"
        },
    );
    sanitize_tool_terminal_summary(&mut payload_object, "input_summary");
    sanitize_tool_terminal_summary(&mut payload_object, "error_type");

    if event_type == "tool.call.failed" {
        return Value::Object(payload_object);
    }

    let Some(raw_views) = payload_object.remove("views") else {
        return Value::Object(payload_object);
    };
    let Value::Array(views) = raw_views else {
        return Value::Object(payload_object);
    };

    let sanitized_views: Vec<Value> = views
        .into_iter()
        .take(MAX_TOOL_RESULT_VIEWS)
        .filter_map(sanitize_tool_result_view)
        .collect();

    if !sanitized_views.is_empty() {
        payload_object.insert("views".to_string(), Value::Array(sanitized_views));
    }

    Value::Object(payload_object)
}

fn sanitize_tool_terminal_summary(payload_object: &mut serde_json::Map<String, Value>, key: &str) {
    let Some(value) = payload_object.get(key).and_then(Value::as_str) else {
        return;
    };
    let redacted = memory_context::redact_sensitive_text(value);
    payload_object.insert(
        key.to_string(),
        Value::String(truncate_chars(&redacted, MAX_TOOL_SUMMARY_CHARS)),
    );
}

fn sanitize_tool_call_delta_payload(payload: Value) -> Value {
    let Value::Object(payload_object) = payload else {
        return json!({});
    };
    let mut sanitized = serde_json::Map::new();

    for key in [
        "run_id",
        "tool_call_id",
        "tool_name",
        "name",
        "status",
        "subagent_id",
        "subagent_name",
        "parent_tool_call_id",
    ] {
        if let Some(value) = payload_object.get(key).and_then(Value::as_str) {
            sanitized.insert(key.to_string(), Value::String(truncate_chars(value, 160)));
        }
    }
    if let Some(value) = payload_object.get("input_summary").and_then(Value::as_str) {
        sanitized.insert(
            "input_summary".to_string(),
            Value::String(truncate_chars(value, MAX_TOOL_INPUT_SUMMARY_CHARS)),
        );
    }
    if let Some(value) = payload_object
        .get("arguments_delta")
        .and_then(Value::as_str)
    {
        sanitized.insert(
            "arguments_delta".to_string(),
            Value::String(truncate_chars(value, MAX_TOOL_ARGUMENT_DELTA_CHARS)),
        );
    }
    if let Some(value) = payload_object.get("arguments_text").and_then(Value::as_str) {
        sanitized.insert(
            "arguments_text".to_string(),
            Value::String(truncate_chars(value, MAX_TOOL_ARGUMENT_TEXT_CHARS)),
        );
    }
    if let Some(value) = payload_object.get("truncated").and_then(Value::as_bool) {
        sanitized.insert("truncated".to_string(), json!(value));
    }
    if let Some(Value::Object(target)) = payload_object.get("target") {
        let kind = target
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let path = target
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_artifact_draft_target_kind(kind) && !path.is_empty() {
            sanitized.insert(
                "target".to_string(),
                json!({
                    "kind": kind,
                    "path": truncate_chars(path, MAX_ARTIFACT_DRAFT_PATH_CHARS)
                }),
            );
        }
    }

    Value::Object(sanitized)
}

fn sanitize_artifact_draft_payload(event_type: &str, payload: Value) -> Value {
    let Value::Object(payload_object) = payload else {
        return json!({});
    };
    let mut sanitized = serde_json::Map::new();

    for key in [
        "draft_id",
        "run_id",
        "project_id",
        "tool_call_id",
        "tool_name",
        "args_hash",
        "operation",
        "content_type",
        "format",
        "mime_type",
        "content_hash",
        "object_reference_id",
        "subagent_id",
        "subagent_name",
        "parent_tool_call_id",
    ] {
        if let Some(value) = payload_object.get(key).and_then(Value::as_str) {
            sanitized.insert(key.to_string(), Value::String(truncate_chars(value, 160)));
        }
    }

    if let Some(path) = payload_object.get("path").and_then(Value::as_str) {
        sanitized.insert(
            "path".to_string(),
            Value::String(truncate_chars(path, MAX_ARTIFACT_DRAFT_PATH_CHARS)),
        );
    }
    if let Some(renderer) = payload_object.get("renderer").and_then(Value::as_str)
        && matches!(
            renderer,
            "markdown"
                | "html"
                | "svg"
                | "mermaid"
                | "drawio"
                | "json"
                | "text"
                | "py"
                | "rs"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "sql"
                | "yaml"
                | "yml"
        )
    {
        sanitized.insert("renderer".to_string(), Value::String(renderer.to_string()));
    }
    if let Some(status) = payload_object.get("status").and_then(Value::as_str)
        && matches!(status, "running" | "completed" | "failed")
    {
        sanitized.insert("status".to_string(), Value::String(status.to_string()));
    }
    if event_type == "artifact.draft.delta"
        && let Some(delta) = payload_object.get("delta").and_then(Value::as_str)
    {
        sanitized.insert(
            "delta".to_string(),
            Value::String(truncate_chars(delta, MAX_ARTIFACT_DRAFT_DELTA_CHARS)),
        );
    }
    if let Some(error_summary) = payload_object.get("error_summary").and_then(Value::as_str) {
        sanitized.insert(
            "error_summary".to_string(),
            Value::String(truncate_chars(
                error_summary,
                MAX_ARTIFACT_DRAFT_ERROR_CHARS,
            )),
        );
    }
    if let Some(previous_preview) = payload_object
        .get("previous_preview")
        .and_then(Value::as_str)
    {
        sanitized.insert(
            "previous_preview".to_string(),
            Value::String(truncate_chars(
                previous_preview,
                MAX_ARTIFACT_DRAFT_PREVIOUS_CHARS,
            )),
        );
    }
    for key in [
        "chunk_index",
        "offset",
        "offset_bytes",
        "size_bytes",
        "previous_size_bytes",
        "preview_size_bytes",
        "revision",
    ] {
        if let Some(value) = payload_object.get(key).and_then(Value::as_i64) {
            sanitized.insert(key.to_string(), json!(value.max(0)));
        }
    }
    if let Some(value) = payload_object.get("truncated").and_then(Value::as_bool) {
        sanitized.insert("truncated".to_string(), json!(value));
    }
    if let Some(Value::Object(target)) = payload_object.get("target") {
        let kind = target
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let path = target
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_artifact_draft_target_kind(kind) && !path.is_empty() {
            sanitized.insert(
                "target".to_string(),
                json!({
                    "kind": kind,
                    "path": truncate_chars(path, MAX_ARTIFACT_DRAFT_PATH_CHARS)
                }),
            );
        }
    }
    Value::Object(sanitized)
}

fn is_artifact_draft_target_kind(kind: &str) -> bool {
    matches!(
        kind,
        "artifact" | "workspace_file" | "local_file" | "scratch_file"
    )
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

async fn validate_tool_result_payload_refs_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    payload: Value,
) -> Result<Value, AppError> {
    let Value::Object(mut payload_object) = payload else {
        return Ok(json!({}));
    };
    let Some(Value::Array(views)) = payload_object.remove("views") else {
        return Ok(Value::Object(payload_object));
    };
    let tool_call_id = payload_object
        .get("tool_call_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());

    let mut valid_views = Vec::new();
    for view in views {
        if let Some(view) =
            validate_tool_result_view_refs_tx(tx, tenant_id, run_id, tool_call_id, view).await?
        {
            valid_views.push(view);
        }
    }
    if !valid_views.is_empty() {
        payload_object.insert("views".to_string(), Value::Array(valid_views));
    }
    Ok(Value::Object(payload_object))
}

async fn validate_tool_result_view_refs_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    tool_call_id: Option<Uuid>,
    view: Value,
) -> Result<Option<Value>, AppError> {
    let Value::Object(mut view_object) = view else {
        return Ok(None);
    };
    let kind = view_object
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    for key in ["data_ref", "artifact_ref"] {
        let Some(reference) = view_object.get(key) else {
            continue;
        };
        let Some(artifact) = tool_result_artifact_info_tx(tx, tenant_id, run_id, reference).await?
        else {
            view_object.remove(key);
            continue;
        };
        register_tool_result_artifact_tx(
            tx,
            ToolResultArtifactRegistration {
                tenant_id,
                run_id: run_id.or(artifact.run_id),
                tool_call_id,
                view_kind: &kind,
                ref_kind: key,
                artifact: &artifact,
                reference,
            },
        )
        .await?;
    }

    let has_ref =
        view_object.get("data_ref").is_some() || view_object.get("artifact_ref").is_some();
    if matches!(kind.as_str(), "map" | "artifact") && !has_ref {
        return Ok(None);
    }

    Ok(Some(Value::Object(view_object)))
}

struct ToolResultArtifactInfo {
    file_revision_id: Uuid,
    project_id: Uuid,
    path: String,
    revision: i64,
    run_id: Option<Uuid>,
    object_reference_id: Uuid,
    content_hash: String,
    content_type: String,
    size_bytes: i64,
}

async fn tool_result_artifact_info_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    reference: &Value,
) -> Result<Option<ToolResultArtifactInfo>, AppError> {
    let Some(object_reference_id) = reference
        .get("object_reference_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return Ok(None);
    };
    let Some(content_type) = reference.get("content_type").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(content_hash) = reference.get("content_hash").and_then(Value::as_str) else {
        return Ok(None);
    };
    let normalized_hash = content_hash.strip_prefix("sha256:").unwrap_or(content_hash);

    let row = sqlx::query(
        r#"
        SELECT fr.id AS file_revision_id, fr.project_id, fr.path, fr.revision,
               fr.run_id, obj.id AS object_reference_id, obj.content_hash,
               obj.content_type, obj.size_bytes
        FROM object_references obj
        JOIN file_revisions fr ON fr.object_reference_id = obj.id
        WHERE obj.id = $1
          AND obj.tenant_id = $2
          AND fr.tenant_id = $2
          AND ($3::uuid IS NULL OR fr.run_id = $3)
          AND fr.path LIKE '/artifacts/%'
          AND obj.content_hash = $4
          AND obj.content_type = $5
        LIMIT 1
        "#,
    )
    .bind(object_reference_id)
    .bind(tenant_id)
    .bind(run_id)
    .bind(normalized_hash)
    .bind(content_type)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(ToolResultArtifactInfo {
        file_revision_id: row.try_get("file_revision_id")?,
        project_id: row.try_get("project_id")?,
        path: row.try_get("path")?,
        revision: row.try_get("revision")?,
        run_id: row.try_get("run_id")?,
        object_reference_id: row.try_get("object_reference_id")?,
        content_hash: row.try_get("content_hash")?,
        content_type: row.try_get("content_type")?,
        size_bytes: row.try_get("size_bytes")?,
    }))
}

struct ToolResultArtifactRegistration<'a> {
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    tool_call_id: Option<Uuid>,
    view_kind: &'a str,
    ref_kind: &'a str,
    artifact: &'a ToolResultArtifactInfo,
    reference: &'a Value,
}

async fn register_tool_result_artifact_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    registration: ToolResultArtifactRegistration<'_>,
) -> Result<(), AppError> {
    let persisted_tool_call_id = existing_tool_call_id_tx(
        tx,
        registration.tenant_id,
        registration.run_id,
        registration.tool_call_id,
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO tool_result_artifacts (
            tenant_id, run_id, tool_call_id, view_kind, ref_kind, project_id,
            path, revision, file_revision_id, object_reference_id, content_hash,
            content_type, size_bytes, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (object_reference_id) DO UPDATE
        SET run_id = COALESCE(tool_result_artifacts.run_id, EXCLUDED.run_id),
            tool_call_id = COALESCE(tool_result_artifacts.tool_call_id, EXCLUDED.tool_call_id),
            view_kind = EXCLUDED.view_kind,
            ref_kind = EXCLUDED.ref_kind,
            metadata = EXCLUDED.metadata
        "#,
    )
    .bind(registration.tenant_id)
    .bind(registration.run_id)
    .bind(persisted_tool_call_id)
    .bind(registration.view_kind)
    .bind(registration.ref_kind)
    .bind(registration.artifact.project_id)
    .bind(&registration.artifact.path)
    .bind(registration.artifact.revision)
    .bind(registration.artifact.file_revision_id)
    .bind(registration.artifact.object_reference_id)
    .bind(&registration.artifact.content_hash)
    .bind(&registration.artifact.content_type)
    .bind(registration.artifact.size_bytes)
    .bind(json!({
        "artifact_id": registration.reference.get("artifact_id").cloned().unwrap_or(Value::Null)
    }))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn existing_tool_call_id_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    tool_call_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    let Some(tool_call_id) = tool_call_id else {
        return Ok(None);
    };
    sqlx::query_scalar(
        r#"
        SELECT id
        FROM tool_calls
        WHERE id = $1
          AND tenant_id = $2
          AND ($3::uuid IS NULL OR run_id = $3)
        "#,
    )
    .bind(tool_call_id)
    .bind(tenant_id)
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)
}

fn sanitize_tool_result_view(view: Value) -> Option<Value> {
    let Value::Object(view_object) = view else {
        return None;
    };
    let kind = view_object.get("kind")?.as_str()?;
    let mut sanitized = serde_json::Map::new();
    sanitized.insert("kind".to_string(), Value::String(kind.to_string()));
    insert_optional_short_string(
        &mut sanitized,
        &view_object,
        "title",
        MAX_TOOL_RESULT_TITLE_CHARS,
    );

    match kind {
        "table" => {
            let columns = sanitize_table_columns(view_object.get("columns")?)?;
            let rows_preview = sanitize_array_preview(
                view_object.get("rows_preview")?,
                MAX_TOOL_RESULT_ROWS,
                MAX_TOOL_RESULT_PREVIEW_BYTES,
            )?;
            sanitized.insert("columns".to_string(), columns);
            sanitized.insert("rows_preview".to_string(), rows_preview);
            insert_optional_ref(&mut sanitized, &view_object, "data_ref");
        }
        "chart" => {
            if view_object.get("spec_kind").and_then(Value::as_str) != Some("vega_lite") {
                return None;
            }
            let spec =
                clone_if_small_object(view_object.get("spec")?, MAX_TOOL_RESULT_PREVIEW_BYTES)?;
            sanitized.insert(
                "spec_kind".to_string(),
                Value::String("vega_lite".to_string()),
            );
            sanitized.insert("spec".to_string(), spec);
            insert_optional_ref(&mut sanitized, &view_object, "data_ref");
        }
        "map" => {
            if view_object.get("format").and_then(Value::as_str) != Some("geojson") {
                return None;
            }
            let data_ref = sanitize_artifact_ref(view_object.get("data_ref")?)?;
            sanitized.insert("format".to_string(), Value::String("geojson".to_string()));
            sanitized.insert("data_ref".to_string(), data_ref);
            if let Some(data_preview) = view_object
                .get("data_preview")
                .and_then(|value| clone_if_small_object(value, MAX_TOOL_RESULT_PREVIEW_BYTES))
            {
                sanitized.insert("data_preview".to_string(), data_preview);
            }
            insert_optional_short_string(
                &mut sanitized,
                &view_object,
                "style_ref",
                MAX_TOOL_RESULT_TITLE_CHARS,
            );
        }
        "json" => {
            let value_preview = clone_if_small_value(
                view_object.get("value_preview")?,
                MAX_TOOL_RESULT_PREVIEW_BYTES,
            )?;
            sanitized.insert("value_preview".to_string(), value_preview);
            insert_optional_ref(&mut sanitized, &view_object, "data_ref");
        }
        "file_diff" => {
            let files = sanitize_array_preview(
                view_object.get("files")?,
                MAX_TOOL_RESULT_FILES,
                MAX_TOOL_RESULT_PREVIEW_BYTES,
            )?;
            sanitized.insert("files".to_string(), files);
        }
        "markdown" => {
            let text = view_object.get("text")?.as_str()?;
            sanitized.insert(
                "text".to_string(),
                Value::String(text.chars().take(MAX_TOOL_RESULT_MARKDOWN_CHARS).collect()),
            );
        }
        "artifact" => {
            let artifact_ref = sanitize_artifact_ref(view_object.get("artifact_ref")?)?;
            sanitized.insert("artifact_ref".to_string(), artifact_ref);
        }
        "source_list" => {
            let sources = sanitize_source_list(view_object.get("sources")?)?;
            sanitized.insert("sources".to_string(), sources);
            insert_optional_ref(&mut sanitized, &view_object, "data_ref");
        }
        "document" => {
            let text_preview = view_object.get("text_preview")?.as_str()?;
            sanitized.insert(
                "text_preview".to_string(),
                Value::String(
                    text_preview
                        .chars()
                        .take(MAX_TOOL_RESULT_MARKDOWN_CHARS)
                        .collect(),
                ),
            );
            insert_optional_short_string(&mut sanitized, &view_object, "url", 2048);
            insert_optional_ref(&mut sanitized, &view_object, "data_ref");
        }
        _ => return None,
    }

    Some(Value::Object(sanitized))
}

fn sanitize_source_list(sources: &Value) -> Option<Value> {
    let Value::Array(items) = sources else {
        return None;
    };
    let mut sanitized_items = Vec::new();
    for item in items.iter().take(MAX_TOOL_RESULT_FILES) {
        let Value::Object(source) = item else {
            continue;
        };
        let url = source.get("url")?.as_str()?.trim();
        if url.is_empty() || url.chars().count() > 2048 {
            continue;
        }
        let mut sanitized = serde_json::Map::new();
        sanitized.insert("url".to_string(), Value::String(url.to_string()));
        insert_optional_short_string(&mut sanitized, source, "title", 240);
        insert_optional_short_string(&mut sanitized, source, "snippet", 1000);
        insert_optional_ref(&mut sanitized, source, "text_ref");
        sanitized_items.push(Value::Object(sanitized));
    }
    if sanitized_items.is_empty() {
        return None;
    }
    let value = Value::Array(sanitized_items);
    clone_if_small_value(&value, MAX_TOOL_RESULT_PREVIEW_BYTES)
}

fn sanitize_table_columns(columns: &Value) -> Option<Value> {
    let Value::Array(items) = columns else {
        return None;
    };
    if items.is_empty() {
        return None;
    }

    let columns: Vec<Value> = items
        .iter()
        .take(MAX_TOOL_RESULT_COLUMNS)
        .filter_map(|item| {
            let Value::Object(column) = item else {
                return None;
            };
            let key = column.get("key")?.as_str()?.trim();
            if key.is_empty() || key.chars().count() > 128 {
                return None;
            }
            let mut sanitized = serde_json::Map::new();
            sanitized.insert("key".to_string(), Value::String(key.to_string()));
            insert_optional_short_string(
                &mut sanitized,
                column,
                "label",
                MAX_TOOL_RESULT_TITLE_CHARS,
            );
            if let Some(column_type) = column.get("type").and_then(Value::as_str)
                && matches!(
                    column_type,
                    "string" | "number" | "boolean" | "datetime" | "currency"
                )
            {
                sanitized.insert("type".to_string(), Value::String(column_type.to_string()));
            }
            Some(Value::Object(sanitized))
        })
        .collect();

    if columns.is_empty() {
        None
    } else {
        Some(Value::Array(columns))
    }
}

fn sanitize_array_preview(value: &Value, max_items: usize, max_bytes: usize) -> Option<Value> {
    let Value::Array(items) = value else {
        return None;
    };
    let preview = Value::Array(items.iter().take(max_items).cloned().collect());
    clone_if_small_value(&preview, max_bytes)
}

fn clone_if_small_object(value: &Value, max_bytes: usize) -> Option<Value> {
    if !value.is_object() {
        return None;
    }
    clone_if_small_value(value, max_bytes)
}

fn clone_if_small_value(value: &Value, max_bytes: usize) -> Option<Value> {
    let size = serde_json::to_vec(value).ok()?.len();
    if size > max_bytes {
        None
    } else {
        Some(value.clone())
    }
}

fn insert_optional_short_string(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
    key: &str,
    max_chars: usize,
) {
    if let Some(value) = source.get(key).and_then(Value::as_str) {
        target.insert(
            key.to_string(),
            Value::String(value.chars().take(max_chars).collect()),
        );
    }
}

fn insert_optional_ref(
    target: &mut serde_json::Map<String, Value>,
    source: &serde_json::Map<String, Value>,
    key: &str,
) {
    if let Some(value) = source.get(key).and_then(sanitize_artifact_ref) {
        target.insert(key.to_string(), value);
    }
}

fn sanitize_artifact_ref(value: &Value) -> Option<Value> {
    let Value::Object(reference) = value else {
        return None;
    };
    let artifact_id = reference.get("artifact_id")?.as_str()?;
    if artifact_id.is_empty() || artifact_id.chars().count() > 128 {
        return None;
    }
    if let Some(object_reference_id) = reference.get("object_reference_id").and_then(Value::as_str)
    {
        Uuid::parse_str(object_reference_id).ok()?;
    }
    let content_type = reference.get("content_type")?.as_str()?;
    if !content_type.contains('/') || content_type.chars().count() > 128 {
        return None;
    }
    let content_hash = reference.get("content_hash")?.as_str()?;
    if !content_hash.starts_with("sha256:") || content_hash.chars().count() > 96 {
        return None;
    }
    let size_bytes = reference.get("size_bytes")?.as_i64()?;
    if size_bytes < 0 {
        return None;
    }

    let mut sanitized = serde_json::Map::new();
    sanitized.insert(
        "artifact_id".to_string(),
        Value::String(artifact_id.to_string()),
    );
    if let Some(object_reference_id) = reference.get("object_reference_id").and_then(Value::as_str)
    {
        sanitized.insert(
            "object_reference_id".to_string(),
            Value::String(object_reference_id.to_string()),
        );
    }
    sanitized.insert(
        "content_type".to_string(),
        Value::String(content_type.to_string()),
    );
    sanitized.insert(
        "content_hash".to_string(),
        Value::String(content_hash.to_string()),
    );
    sanitized.insert(
        "size_bytes".to_string(),
        Value::Number(serde_json::Number::from(size_bytes)),
    );
    Some(Value::Object(sanitized))
}

struct ToolCallEventUpdate {
    tool_call_id: Uuid,
    status: &'static str,
    output_summary: Option<String>,
    error_summary: Option<String>,
}

fn tool_call_event_update(event_type: &str, payload: &Value) -> Option<ToolCallEventUpdate> {
    let status = match event_type {
        "tool.call.completed" => "completed",
        "tool.call.failed" => "failed",
        _ => return None,
    };
    let tool_call_id = payload
        .get("tool_call_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())?;
    let summary = if status == "completed" {
        payload
            .get("output_summary")
            .and_then(Value::as_str)
            .map(|value| value.chars().take(4000).collect())
    } else {
        payload
            .get("error_summary")
            .and_then(Value::as_str)
            .map(|value| value.chars().take(4000).collect())
    };

    let (output_summary, error_summary) = match status {
        "completed" => (summary, None),
        "failed" => (None, summary),
        _ => unreachable!("tool call event status is constrained above"),
    };

    Some(ToolCallEventUpdate {
        tool_call_id,
        status,
        output_summary,
        error_summary,
    })
}

async fn apply_tool_call_event_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rustfs_client: &RustFsClient,
    tenant_id: Uuid,
    run_id: Option<Uuid>,
    update: ToolCallEventUpdate,
) -> Result<Option<ArchivedAuditEvidence>, AppError> {
    let row = sqlx::query(
        r#"
        UPDATE tool_calls
        SET status = $1,
            output_summary = COALESCE($2, output_summary),
            error_summary = COALESCE($3, error_summary),
            completed_at = CURRENT_TIMESTAMP
        WHERE id = $4
          AND tenant_id = $5
          AND ($6::uuid IS NULL OR run_id = $6)
        RETURNING id, tenant_id, conversation_id, run_id, tool_name, resource_type,
                  resource_id, args_hash, risk_level, policy_version, input_summary,
                  output_summary, error_summary, status, decision, completed_at,
                  evidence_object_reference_id
        "#,
    )
    .bind(update.status)
    .bind(update.output_summary.clone())
    .bind(update.error_summary.clone())
    .bind(update.tool_call_id)
    .bind(tenant_id)
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some(row) = row {
        insert_tool_call_event_audit_tx(tx, &row, &update).await?;
        return archive_tool_call_evidence_if_needed_tx(tx, rustfs_client, &row).await;
    }
    Ok(None)
}

async fn archive_tool_call_evidence_if_needed_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rustfs_client: &RustFsClient,
    row: &sqlx::postgres::PgRow,
) -> Result<Option<ArchivedAuditEvidence>, AppError> {
    let risk_level: Option<String> = row.try_get("risk_level")?;
    if !audit::should_archive_tool_call_evidence(risk_level.as_deref()) {
        return Ok(None);
    }

    let existing_evidence_object_reference_id: Option<Uuid> =
        row.try_get("evidence_object_reference_id")?;
    if existing_evidence_object_reference_id.is_some() {
        return Ok(None);
    }

    let run_id: Option<Uuid> = row.try_get("run_id")?;
    let (actor_user_id, trace_id) = load_run_actor_trace_tx(tx, run_id).await?;
    let tool_call_id: Uuid = row.try_get("id")?;
    let tool_name: String = row.try_get("tool_name")?;
    let resource_type: String = row
        .try_get::<Option<String>, _>("resource_type")?
        .unwrap_or_else(|| "tool".to_string());
    let resource_id: String = row
        .try_get::<Option<String>, _>("resource_id")?
        .unwrap_or_else(|| tool_name.clone());

    audit::archive_tool_call_evidence_tx(
        tx,
        rustfs_client,
        ToolCallEvidenceInput {
            tenant_id: row.try_get("tenant_id")?,
            tool_call_id,
            actor_user_id,
            conversation_id: row.try_get("conversation_id")?,
            run_id,
            tool_name,
            resource_type,
            resource_id,
            status: row.try_get("status")?,
            decision: row.try_get("decision")?,
            policy_version: row.try_get("policy_version")?,
            args_hash: row.try_get("args_hash")?,
            input_summary: row.try_get("input_summary")?,
            output_summary: row.try_get("output_summary")?,
            error_summary: row.try_get("error_summary")?,
            risk_level,
            trace_id,
            completed_at: row.try_get("completed_at")?,
        },
    )
    .await
    .map(Some)
}

async fn insert_tool_call_event_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &sqlx::postgres::PgRow,
    update: &ToolCallEventUpdate,
) -> Result<(), AppError> {
    let tenant_id: Uuid = row.try_get("tenant_id")?;
    let tool_call_id: Uuid = row.try_get("id")?;
    let conversation_id: Option<Uuid> = row.try_get("conversation_id")?;
    let run_id: Option<Uuid> = row.try_get("run_id")?;
    let tool_name: String = row.try_get("tool_name")?;
    let resource_type: String = row
        .try_get::<Option<String>, _>("resource_type")?
        .unwrap_or_else(|| "tool".to_string());
    let resource_id: String = row
        .try_get::<Option<String>, _>("resource_id")?
        .unwrap_or_else(|| tool_name.clone());
    let args_hash: Option<String> = row.try_get("args_hash")?;
    let risk_level: Option<String> = row.try_get("risk_level")?;
    let policy_version: String = row.try_get("policy_version")?;
    let input_summary: Option<String> = row.try_get("input_summary")?;
    let output_summary: Option<String> = match update.status {
        "completed" => row.try_get("output_summary")?,
        "failed" => row.try_get("error_summary")?,
        _ => None,
    };
    let (actor_user_id, trace_id) = load_run_actor_trace_tx(tx, run_id).await?;

    let action = match update.status {
        "completed" => "tool.call.completed",
        "failed" => "tool.call.failed",
        _ => "tool.call.unknown",
    };

    audit::insert_audit_log_tx(
        tx,
        NewAuditLog {
            tenant_id,
            actor_user_id,
            actor_device_id: None,
            session_id: None,
            resource_type: &resource_type,
            resource_id: &resource_id,
            action,
            decision: "allow",
            policy_version: &policy_version,
            reason_code: update.error_summary.as_deref(),
            run_id,
            conversation_id,
            workflow_run_id: None,
            tool_call_id: Some(tool_call_id),
            approval_id: None,
            args_hash: args_hash.as_deref(),
            input_summary: input_summary.as_deref(),
            output_summary: output_summary.as_deref(),
            risk_level: risk_level.as_deref(),
            ip: None,
            user_agent: None,
            trace_id: trace_id.as_deref(),
        },
    )
    .await?;
    Ok(())
}

async fn load_run_actor_trace_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    run_id: Option<Uuid>,
) -> Result<(Option<Uuid>, Option<String>), AppError> {
    let Some(run_id) = run_id else {
        return Ok((None, None));
    };

    let run = sqlx::query(
        r#"
        SELECT created_by_user_id, trace_id
        FROM runs
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some(run) = run {
        Ok((
            run.try_get::<Option<Uuid>, _>("created_by_user_id")?,
            run.try_get::<Option<String>, _>("trace_id")?,
        ))
    } else {
        Ok((None, None))
    }
}

async fn cleanup_archived_audit_evidence(state: &AppState, archived: Vec<ArchivedAuditEvidence>) {
    for evidence in archived {
        if let Some(object_key) = evidence.object_key {
            let _ = state.rustfs_client.delete_audit_object(&object_key).await;
        }
    }
}

pub async fn publish_outbox(
    State(state): State<AppState>,
) -> Result<Json<GenericResponse>, AppError> {
    let published = event_store::publish_pending_outbox(&state).await?;

    Ok(Json(GenericResponse {
        code: "OUTBOX_PUBLISHED".to_string(),
        message: format!("Published {} pending event(s)", published),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::middleware;
    use redis::Client as RedisClient;
    use reqwest::StatusCode;
    use secrecy::SecretBox;
    use serde_json::Value;
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};
    use tokio::{net::TcpListener, task::JoinHandle};

    use crate::{
        configuration::{AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings},
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            internal_auth::internal_token_middleware, memory_vector::MemoryVectorClient,
            runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
        startup::AppState,
    };

    #[test]
    fn conversation_mcp_selection_accepts_alias_and_deduplicates_ids() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let metadata = json!({
            "extra": {
                "selected_mcp_server_ids": [first, first, second]
            }
        });

        assert_eq!(
            conversation_selected_mcp_server_ids(&metadata).unwrap(),
            vec![first, second]
        );
    }

    #[test]
    fn conversation_mcp_selection_rejects_non_uuid_values() {
        let metadata = json!({
            "extra": {
                "selected_mcp_server_ids": ["builtin-local-server"]
            }
        });

        assert!(conversation_selected_mcp_server_ids(&metadata).is_err());
    }

    #[test]
    fn conversation_agent_version_pin_accepts_uuid_and_rejects_corruption() {
        let version_id = Uuid::new_v4();
        assert_eq!(
            conversation_pinned_agent_version_id(&json!({
                "biwork": { "agent_version_id": version_id.to_string() }
            }))
            .unwrap(),
            Some(version_id)
        );
        assert_eq!(
            conversation_pinned_agent_version_id(&json!({ "biwork": {} })).unwrap(),
            None
        );
        assert!(
            conversation_pinned_agent_version_id(&json!({
                "biwork": { "agent_version_id": "not-a-uuid" }
            }))
            .is_err()
        );
    }

    #[test]
    fn tool_call_event_update_extracts_completed_summary() {
        let tool_call_id = Uuid::new_v4();
        let update = tool_call_event_update(
            "tool.call.completed",
            &json!({
                "tool_call_id": tool_call_id,
                "output_summary": "ok"
            }),
        )
        .expect("tool event update");

        assert_eq!(update.tool_call_id, tool_call_id);
        assert_eq!(update.status, "completed");
        assert_eq!(update.output_summary.as_deref(), Some("ok"));
        assert_eq!(update.error_summary, None);
    }

    #[test]
    fn tool_call_event_update_extracts_failed_summary() {
        let tool_call_id = Uuid::new_v4();
        let update = tool_call_event_update(
            "tool.call.failed",
            &json!({
                "tool_call_id": tool_call_id,
                "error_summary": "boom"
            }),
        )
        .expect("tool event update");

        assert_eq!(update.tool_call_id, tool_call_id);
        assert_eq!(update.status, "failed");
        assert_eq!(update.output_summary, None);
        assert_eq!(update.error_summary.as_deref(), Some("boom"));
    }

    #[test]
    fn tool_call_event_update_ignores_unrelated_or_invalid_events() {
        assert!(tool_call_event_update("message.completed", &json!({})).is_none());
        assert!(
            tool_call_event_update("tool.call.completed", &json!({"tool_call_id": "bad"}))
                .is_none()
        );
    }

    #[test]
    fn sanitize_tool_result_payload_keeps_valid_views_and_drops_invalid_views() {
        let sanitized = sanitize_tool_result_payload(
            "tool.call.completed",
            json!({
                "tool_call_id": Uuid::new_v4(),
                "output_summary": "ok",
                "views": [
                    {
                        "kind": "table",
                        "title": "Rows",
                        "columns": [{"key": "name", "label": "Name", "type": "string"}],
                        "rows_preview": [{"name": "alice"}],
                        "unexpected": "dropped"
                    },
                    {
                        "kind": "file_diff",
                        "title": "Patch preview",
                        "files": [{
                            "file_name": "report.md",
                            "file_diff": "--- a/report.md\n+++ b/report.md\n@@\n-old\n+new\n",
                            "path": "/workspace/report.md"
                        }]
                    },
                    {
                        "kind": "markdown",
                        "title": "Summary",
                        "text": "renderable summary"
                    },
                    {
                        "kind": "chart",
                        "spec_kind": "unsupported",
                        "spec": {}
                    },
                    {
                        "kind": "artifact",
                        "title": "Full result",
                        "artifact_ref": {
                            "artifact_id": "artifact-1",
                            "object_reference_id": Uuid::new_v4(),
                            "content_type": "application/json",
                            "content_hash": "sha256:abc",
                            "size_bytes": 12
                        }
                    },
                    {
                        "kind": "source_list",
                        "title": "Web sources",
                        "sources": [
                            {
                                "title": "Example",
                                "url": "https://example.com",
                                "snippet": "source snippet",
                                "text_ref": {
                                    "artifact_id": "source-text",
                                    "object_reference_id": Uuid::new_v4(),
                                    "content_type": "text/plain; charset=utf-8",
                                    "content_hash": "sha256:def",
                                    "size_bytes": 128
                                }
                            }
                        ]
                    },
                    {
                        "kind": "document",
                        "title": "Fetched page",
                        "url": "https://example.com",
                        "text_preview": "page text",
                        "data_ref": {
                            "artifact_id": "source-text",
                            "object_reference_id": Uuid::new_v4(),
                            "content_type": "text/plain; charset=utf-8",
                            "content_hash": "sha256:def",
                            "size_bytes": 128
                        }
                    }
                ]
            }),
        );

        let views = sanitized
            .get("views")
            .and_then(Value::as_array)
            .expect("views");
        assert_eq!(views.len(), 6);
        assert_eq!(views[0].get("kind").and_then(Value::as_str), Some("table"));
        assert_eq!(views[0].get("title").and_then(Value::as_str), Some("Rows"));
        assert!(views[0].get("unexpected").is_none());
        assert_eq!(
            views[1].get("kind").and_then(Value::as_str),
            Some("file_diff")
        );
        assert_eq!(
            views[1].get("title").and_then(Value::as_str),
            Some("Patch preview")
        );
        assert_eq!(
            views[1]
                .pointer("/files/0/file_diff")
                .and_then(Value::as_str),
            Some("--- a/report.md\n+++ b/report.md\n@@\n-old\n+new\n")
        );
        assert_eq!(
            views[2].get("kind").and_then(Value::as_str),
            Some("markdown")
        );
        assert_eq!(
            views[2].get("title").and_then(Value::as_str),
            Some("Summary")
        );
        assert_eq!(
            views[3].get("kind").and_then(Value::as_str),
            Some("artifact")
        );
        assert_eq!(
            views[3].get("title").and_then(Value::as_str),
            Some("Full result")
        );
        assert_eq!(
            views[4]
                .pointer("/sources/0/text_ref/content_hash")
                .and_then(Value::as_str),
            Some("sha256:def")
        );
        assert_eq!(
            views[5]
                .pointer("/data_ref/content_hash")
                .and_then(Value::as_str),
            Some("sha256:def")
        );
    }

    #[test]
    fn sanitize_tool_result_payload_removes_invalid_views_array() {
        let sanitized = sanitize_tool_result_payload(
            "tool.call.completed",
            json!({
                "tool_call_id": Uuid::new_v4(),
                "output_summary": "ok",
                "views": [{"kind": "map", "format": "geojson"}]
            }),
        );

        assert!(sanitized.get("views").is_none());
        assert_eq!(
            sanitized.get("output_summary").and_then(Value::as_str),
            Some("ok")
        );
    }

    #[test]
    fn sanitize_tool_result_payload_redacts_terminal_summaries() {
        let failed = sanitize_tool_result_payload(
            "tool.call.failed",
            json!({
                "tool_call_id": Uuid::new_v4(),
                "tool_name": "call_provider",
                "error_summary": "provider failed api_key=sk-test authorization: Bearer raw-secret Bearer standalone-secret"
            }),
        );
        let failed_text = failed.to_string();

        assert!(failed_text.contains("api_key=[REDACTED]"));
        assert!(failed_text.contains("Bearer [REDACTED]"));
        assert!(!failed_text.contains("sk-test"));
        assert!(!failed_text.contains("raw-secret"));
        assert!(!failed_text.contains("standalone-secret"));

        let completed = sanitize_tool_result_payload(
            "tool.call.completed",
            json!({
                "tool_call_id": Uuid::new_v4(),
                "output_summary": "ok token=plain-secret",
                "views": []
            }),
        );
        let completed_text = completed.to_string();

        assert!(completed_text.contains("token=[REDACTED]"));
        assert!(!completed_text.contains("plain-secret"));
    }

    #[test]
    fn sanitize_tool_result_payload_caps_artifact_draft_delta() {
        let sanitized = sanitize_tool_result_payload(
            "artifact.draft.delta",
            json!({
                "draft_id": "draft-1",
                "run_id": Uuid::new_v4().to_string(),
                "path": "/local/main/report.md",
                "format": "markdown",
                "mime_type": "text/markdown",
                "renderer": "markdown",
                "status": "running",
                "subagent_id": "sub-1",
                "subagent_name": "writer",
                "parent_tool_call_id": "call-task",
                "chunk_index": 2,
                "offset": 20,
                "offset_bytes": 20,
                "preview_size_bytes": 4096,
                "previous_size_bytes": 8192,
                "previous_preview": "p".repeat(MAX_ARTIFACT_DRAFT_PREVIOUS_CHARS + 10),
                "truncated": true,
                "delta": "x".repeat(MAX_ARTIFACT_DRAFT_DELTA_CHARS + 10),
                "unsafe": "dropped"
            }),
        );

        assert_eq!(
            sanitized.get("draft_id").and_then(Value::as_str),
            Some("draft-1")
        );
        assert_eq!(
            sanitized.get("renderer").and_then(Value::as_str),
            Some("markdown")
        );
        assert_eq!(
            sanitized.get("mime_type").and_then(Value::as_str),
            Some("text/markdown")
        );
        assert_eq!(
            sanitized.get("subagent_id").and_then(Value::as_str),
            Some("sub-1")
        );
        assert_eq!(
            sanitized.get("subagent_name").and_then(Value::as_str),
            Some("writer")
        );
        assert_eq!(
            sanitized.get("parent_tool_call_id").and_then(Value::as_str),
            Some("call-task")
        );
        assert_eq!(sanitized.get("offset").and_then(Value::as_i64), Some(20));
        assert_eq!(
            sanitized.get("preview_size_bytes").and_then(Value::as_i64),
            Some(4096)
        );
        assert_eq!(
            sanitized.get("previous_size_bytes").and_then(Value::as_i64),
            Some(8192)
        );
        assert_eq!(
            sanitized
                .get("previous_preview")
                .and_then(Value::as_str)
                .unwrap()
                .len(),
            MAX_ARTIFACT_DRAFT_PREVIOUS_CHARS
        );
        assert_eq!(
            sanitized.get("truncated").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            sanitized
                .get("delta")
                .and_then(Value::as_str)
                .unwrap()
                .len(),
            MAX_ARTIFACT_DRAFT_DELTA_CHARS
        );
        assert!(sanitized.get("unsafe").is_none());
    }

    #[test]
    fn sanitize_artifact_draft_payload_keeps_artifact_and_scratch_targets() {
        let artifact = sanitize_tool_result_payload(
            "artifact.draft.completed",
            json!({
                "draft_id": "draft-artifact",
                "path": "/artifacts/report.md",
                "target": {"kind": "artifact", "path": "/artifacts/report.md"},
                "status": "completed"
            }),
        );

        assert_eq!(
            artifact.pointer("/target/kind").and_then(Value::as_str),
            Some("artifact")
        );
        assert_eq!(
            artifact.pointer("/target/path").and_then(Value::as_str),
            Some("/artifacts/report.md")
        );

        let scratch = sanitize_tool_result_payload(
            "artifact.draft.started",
            json!({
                "draft_id": "draft-scratch",
                "path": "/scratch/notes.md",
                "target": {"kind": "scratch_file", "path": "/scratch/notes.md"},
                "status": "running"
            }),
        );

        assert_eq!(
            scratch.pointer("/target/kind").and_then(Value::as_str),
            Some("scratch_file")
        );
        assert_eq!(
            scratch.pointer("/target/path").and_then(Value::as_str),
            Some("/scratch/notes.md")
        );
    }

    #[test]
    fn sanitize_tool_result_payload_caps_tool_call_delta() {
        let sanitized = sanitize_tool_result_payload(
            "tool.call.delta",
            json!({
                "run_id": Uuid::new_v4().to_string(),
                "tool_call_id": "call-1",
                "tool_name": "write_file",
                "status": "running",
                "subagent_id": "sub-1",
                "subagent_name": "writer",
                "parent_tool_call_id": "call-task",
                "input_summary": "s".repeat(MAX_TOOL_INPUT_SUMMARY_CHARS + 10),
                "arguments_delta": "d".repeat(MAX_TOOL_ARGUMENT_DELTA_CHARS + 10),
                "arguments_text": "t".repeat(MAX_TOOL_ARGUMENT_TEXT_CHARS + 10),
                "target": {"kind": "local_file", "path": "/local/main/report.md"},
                "unsafe": "dropped"
            }),
        );

        assert_eq!(
            sanitized.get("tool_call_id").and_then(Value::as_str),
            Some("call-1")
        );
        assert_eq!(
            sanitized.get("subagent_id").and_then(Value::as_str),
            Some("sub-1")
        );
        assert_eq!(
            sanitized.get("subagent_name").and_then(Value::as_str),
            Some("writer")
        );
        assert_eq!(
            sanitized.get("parent_tool_call_id").and_then(Value::as_str),
            Some("call-task")
        );
        assert_eq!(
            sanitized.pointer("/target/path").and_then(Value::as_str),
            Some("/local/main/report.md")
        );
        assert_eq!(
            sanitized
                .get("input_summary")
                .and_then(Value::as_str)
                .unwrap()
                .len(),
            MAX_TOOL_INPUT_SUMMARY_CHARS
        );
        assert_eq!(
            sanitized
                .get("arguments_delta")
                .and_then(Value::as_str)
                .unwrap()
                .len(),
            MAX_TOOL_ARGUMENT_DELTA_CHARS
        );
        assert_eq!(
            sanitized
                .get("arguments_text")
                .and_then(Value::as_str)
                .unwrap()
                .len(),
            MAX_TOOL_ARGUMENT_TEXT_CHARS
        );
        assert!(sanitized.get("unsafe").is_none());
    }

    #[test]
    fn sanitize_tool_call_delta_keeps_artifact_target() {
        let sanitized = sanitize_tool_result_payload(
            "tool.call.delta",
            json!({
                "tool_call_id": "call-artifact",
                "tool_name": "write_file",
                "target": {"kind": "artifact", "path": "/artifacts/report.md"}
            }),
        );

        assert_eq!(
            sanitized.pointer("/target/kind").and_then(Value::as_str),
            Some("artifact")
        );
        assert_eq!(
            sanitized.pointer("/target/path").and_then(Value::as_str),
            Some("/artifacts/report.md")
        );
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn create_conversation_run_denies_agent_version_capability_before_writing_run()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_conversation_run_capability_context(&state.connect_pool).await?;

        let result = create_and_dispatch_conversation_run(
            &state,
            &test_platform_context(context.tenant_id, context.user_id),
            context.conversation_id,
            CreateRunRequest {
                tenant_id: context.tenant_id,
                agent_id: Some(context.agent_id),
                agent_version_id: Some(context.agent_version_id),
                project_id: None,
                idempotency_key: Some(format!("deny-{}", Uuid::new_v4())),
                input: Some(json!({"prompt": "must not dispatch"})),
                run_config_snapshot: None,
                thread_id: Some(format!("thread-{}", Uuid::new_v4())),
            },
        )
        .await;

        match result {
            Err(AppError::PermissionDenied(message)) => {
                assert!(message.contains("resource=skill:"));
                assert!(message.contains(&context.skill_id.to_string()));
                assert!(message.contains("policy_explicit_deny"));
            }
            other => panic!("expected skill capability denial, got {other:?}"),
        }

        let run_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM runs
            WHERE tenant_id = $1 AND conversation_id = $2
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.conversation_id)
        .fetch_one(&state.connect_pool)
        .await?;
        let run_event_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM run_events
            WHERE tenant_id = $1 AND conversation_id = $2
            "#,
        )
        .bind(context.tenant_id)
        .bind(context.conversation_id)
        .fetch_one(&state.connect_pool)
        .await?;

        assert_eq!(run_count, 0);
        assert_eq!(run_event_count, 0);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn failed_tool_event_persists_error_summary_without_overwriting_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let context = seed_tool_call_context(&pool, Some("previous output")).await?;

        let update = tool_call_event_update(
            "tool.call.failed",
            &json!({
                "tool_call_id": context.tool_call_id,
                "error_summary": "permission denied"
            }),
        )
        .expect("tool call update");
        let mut tx = pool.begin().await?;
        apply_tool_call_event_update(
            &mut tx,
            &RustFsClient::disabled_for_tests(),
            context.tenant_id,
            Some(context.run_id),
            update,
        )
        .await?;
        tx.commit().await?;

        let row = load_tool_call_summary(&pool, context.tool_call_id).await?;

        assert_eq!(row.try_get::<String, _>("status")?, "failed");
        assert_eq!(
            row.try_get::<Option<String>, _>("output_summary")?
                .as_deref(),
            Some("previous output")
        );
        assert_eq!(
            row.try_get::<Option<String>, _>("error_summary")?
                .as_deref(),
            Some("permission denied")
        );

        cleanup_tenant(&pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn ingest_run_events_handler_persists_failed_tool_event()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_tool_call_context(&state.connect_pool, Some("previous output")).await?;

        let Json(response) = ingest_run_events(
            State(state.clone()),
            Json(IngestRunEventsRequest {
                tenant_id: context.tenant_id,
                conversation_id: context.conversation_id,
                run_id: Some(context.run_id),
                events: vec![RunEventInput {
                    event_id: Some(format!("tool.call.failed.{}", context.tool_call_id)),
                    event_type: "tool.call.failed".to_string(),
                    payload: Some(json!({
                        "tool_call_id": context.tool_call_id,
                        "error_summary": "permission denied"
                    })),
                    trace_id: Some("trace".to_string()),
                }],
            }),
        )
        .await?;

        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].event_type, "tool.call.failed");

        let row = load_tool_call_summary(&state.connect_pool, context.tool_call_id).await?;
        assert_eq!(row.try_get::<String, _>("status")?, "failed");
        assert_eq!(
            row.try_get::<Option<String>, _>("output_summary")?
                .as_deref(),
            Some("previous output")
        );
        assert_eq!(
            row.try_get::<Option<String>, _>("error_summary")?
                .as_deref(),
            Some("permission denied")
        );

        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM run_events WHERE run_id = $1 AND type = 'tool.call.failed'",
        )
        .bind(context.run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(event_count, 1);

        let outbox_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM event_outbox outbox
            JOIN run_events event ON event.id = outbox.event_row_id
            WHERE event.run_id = $1
              AND event.type = 'tool.call.failed'
            "#,
        )
        .bind(context.run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(outbox_count, 1);

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, Redis, and the bibi_work schema"]
    async fn tool_event_round_trips_through_internal_http_service()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let context = seed_tool_call_context(&state.connect_pool, None).await?;
        let (base_url, server) = spawn_internal_app(state.clone()).await?;
        let http = reqwest::Client::new();

        let result = async {
            let authorize = post_json(
                &http,
                &format!("{base_url}/tool-calls:authorize"),
                json!({
                    "tenant_id": context.tenant_id,
                    "actor": {
                        "user_id": context.user_id,
                        "device_id": null,
                        "session_id": null,
                        "roles": []
                    },
                    "conversation_id": context.conversation_id,
                    "run_id": context.run_id,
                    "tool_id": null,
                    "tool_name": "file_write",
                    "resource": null,
                    "args_hash": "args-hash-http-e2e",
                    "risk_level": "low",
                    "input_summary": "{\"path\":\"/workspace/http-e2e.txt\"}"
                }),
            )
            .await?;

            assert_eq!(authorize["decision"]["decision"].as_str(), Some("allow"));
            let tool_call_id = authorize["tool_call_id"]
                .as_str()
                .ok_or("missing tool_call_id")?;

            let ingested = post_json(
                &http,
                &format!("{base_url}/run-events"),
                json!({
                    "tenant_id": context.tenant_id,
                    "conversation_id": context.conversation_id,
                    "run_id": context.run_id,
                    "events": [
                        {
                            "event_id": format!("tool.call.completed.{tool_call_id}"),
                            "type": "tool.call.completed",
                            "payload": {
                                "run_id": context.run_id,
                                "tool_call_id": tool_call_id,
                                "tool_name": "file_write",
                                "args_hash": "args-hash-http-e2e",
                                "status": "completed",
                                "output_summary": "wrote /workspace/http-e2e.txt"
                            },
                            "trace_id": "trace"
                        }
                    ]
                }),
            )
            .await?;

            assert_eq!(ingested["events"].as_array().map(Vec::len), Some(1));

            let row =
                load_tool_call_summary(&state.connect_pool, Uuid::parse_str(tool_call_id)?).await?;
            assert_eq!(row.try_get::<String, _>("status")?, "completed");
            assert_eq!(
                row.try_get::<Option<String>, _>("output_summary")?
                    .as_deref(),
                Some("wrote /workspace/http-e2e.txt")
            );

            let audit_count: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)
                FROM audit_logs
                WHERE tenant_id = $1
                  AND tool_call_id = $2
                  AND action = 'tool.call.completed'
                  AND row_hash IS NOT NULL
                "#,
            )
            .bind(context.tenant_id)
            .bind(Uuid::parse_str(tool_call_id)?)
            .fetch_one(&state.connect_pool)
            .await?;
            assert_eq!(audit_count, 1);

            Ok::<(), Box<dyn std::error::Error>>(())
        }
        .await;

        cleanup_tenant(&state.connect_pool, context.tenant_id).await?;
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
            .bearer_auth("test-internal-token")
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

    async fn test_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(pool)
    }

    async fn test_state() -> Result<AppState, Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6380".to_string());

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
            internal_shared_token: "test-internal-token".to_string(),
            audit_partition_cleanup_enabled: false,
            secret_resolver:
                crate::features::agent_platform::secret_resolver::SecretResolver::env_only_for_tests(
                ),
            credential_rotation_worker_enabled: false,
        })
    }

    struct ToolCallTestContext {
        tenant_id: Uuid,
        user_id: Uuid,
        conversation_id: Uuid,
        run_id: Uuid,
        tool_call_id: Uuid,
    }

    struct ConversationRunCapabilityContext {
        tenant_id: Uuid,
        user_id: Uuid,
        conversation_id: Uuid,
        agent_id: Uuid,
        agent_version_id: Uuid,
        skill_id: Uuid,
    }

    async fn seed_conversation_run_capability_context(
        pool: &PgPool,
    ) -> Result<ConversationRunCapabilityContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Conversation run capability test")
            .bind(format!("conversation-run-capability-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("conversation-run-capability-subject-{user_id}"))
        .bind(format!("conversation-run-capability-user-{user_id}"))
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

        let model_profile_id = seed_run_model_profile(pool, tenant_id).await?;
        let agent_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agents (tenant_id, owner_user_id, name, status)
            VALUES ($1, $2, $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("conversation-run-agent-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        let agent_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agent_versions (
                tenant_id, agent_id, version_label, config_snapshot, status
            )
            VALUES ($1, $2, $3, $4, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(agent_id)
        .bind(format!("v-{}", Uuid::new_v4()))
        .bind(json!({
            "model_profile_id": model_profile_id,
            "agent": {"system_prompt": "capability gated conversation run"}
        }))
        .fetch_one(pool)
        .await?;
        let skill_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO skills (tenant_id, name, status)
            VALUES ($1, $2, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("conversation-run-skill-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        let skill_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO skill_versions (tenant_id, skill_id, version_label, status)
            VALUES ($1, $2, $3, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(skill_id)
        .bind(format!("v-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO agent_version_skill_bindings (agent_version_id, skill_version_id)
            VALUES ($1, $2)
            "#,
        )
        .bind(agent_version_id)
        .bind(skill_version_id)
        .execute(pool)
        .await?;

        seed_run_policy_binding(
            pool,
            tenant_id,
            user_id,
            "agent",
            &agent_id.to_string(),
            "run",
            "allow",
        )
        .await?;
        seed_run_policy_binding(
            pool,
            tenant_id,
            user_id,
            "skill",
            &skill_id.to_string(),
            "use",
            "deny",
        )
        .await?;

        sqlx::query(
            r#"
            INSERT INTO conversations (id, tenant_id, created_by_user_id, agent_id, title)
            VALUES ($1, $2, $3, $4, 'Conversation run capability test')
            "#,
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(agent_id)
        .execute(pool)
        .await?;

        Ok(ConversationRunCapabilityContext {
            tenant_id,
            user_id,
            conversation_id,
            agent_id,
            agent_version_id,
            skill_id,
        })
    }

    async fn seed_run_model_profile(pool: &PgPool, tenant_id: Uuid) -> Result<Uuid, sqlx::Error> {
        let suffix = Uuid::new_v4();
        let provider_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO llm_providers (tenant_id, provider_key, display_name, base_url)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind("test")
        .bind(format!("Conversation Run Test Provider {suffix}"))
        .bind("http://localhost:1/v1")
        .fetch_one(pool)
        .await?;

        sqlx::query_scalar(
            r#"
            INSERT INTO llm_model_profiles (
                tenant_id, provider_id, profile_name, model_name,
                max_output_tokens, temperature
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(provider_id)
        .bind(format!("conversation-run-profile-{suffix}"))
        .bind("fake-model")
        .bind(1024_i64)
        .bind(0.0_f64)
        .fetch_one(pool)
        .await
    }

    async fn seed_run_policy_binding(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        resource_type: &str,
        resource_id: &str,
        action: &str,
        effect: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO resource_policy_bindings (
                tenant_id, resource_type, resource_id, action,
                subject_type, subject_id, effect, created_by_user_id
            )
            VALUES ($1, $2, $3, $4, 'user', $5, $6, $7)
            "#,
        )
        .bind(tenant_id)
        .bind(resource_type)
        .bind(resource_id)
        .bind(action)
        .bind(user_id.to_string())
        .bind(effect)
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    fn test_platform_context(tenant_id: Uuid, user_id: Uuid) -> PlatformRequestContext {
        PlatformRequestContext {
            tenant_id,
            platform_user_id: user_id,
            ferriskey_subject: format!("test-subject-{user_id}"),
            preferred_username: Some(format!("test-user-{user_id}")),
            email: None,
            roles: vec!["tenant_member".to_string()],
            session_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            trace_id: format!("trace-{}", Uuid::new_v4()),
            token_jti: None,
            token_exp: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        }
    }

    async fn seed_tool_call_context(
        pool: &PgPool,
        output_summary: Option<&str>,
    ) -> Result<ToolCallTestContext, sqlx::Error> {
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let conversation_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let tool_call_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Tool call event test")
            .bind(format!("tool-call-event-test-{tenant_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("tool-call-event-subject-{user_id}"))
        .bind(format!("tool-call-event-user-{user_id}"))
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
        sqlx::query(
            r#"
            INSERT INTO resource_relations (
                tenant_id, resource_type, resource_id, relation,
                subject_type, subject_id, created_by_user_id
            )
            VALUES ($1, 'tool', 'file_write', 'user', 'user', $2, $3)
            "#,
        )
        .bind(tenant_id)
        .bind(user_id.to_string())
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversations (id, tenant_id, created_by_user_id, title)
            VALUES ($1, $2, $3, 'Tool call event conversation')
            "#,
        )
        .bind(conversation_id)
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO runs (
                id, tenant_id, conversation_id, created_by_user_id, status,
                input, run_config_snapshot, trace_id
            )
            VALUES ($1, $2, $3, $4, 'running', '{}'::jsonb, '{}'::jsonb, 'trace')
            "#,
        )
        .bind(run_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO tool_calls (
                id, tenant_id, conversation_id, run_id, tool_name, status,
                decision, output_summary
            )
            VALUES ($1, $2, $3, $4, 'file_write', 'authorized', 'allow', $5)
            "#,
        )
        .bind(tool_call_id)
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(run_id)
        .bind(output_summary)
        .execute(pool)
        .await?;

        Ok(ToolCallTestContext {
            tenant_id,
            user_id,
            conversation_id,
            run_id,
            tool_call_id,
        })
    }

    async fn load_tool_call_summary(
        pool: &PgPool,
        tool_call_id: Uuid,
    ) -> Result<sqlx::postgres::PgRow, sqlx::Error> {
        sqlx::query(
            r#"
            SELECT status, output_summary, error_summary
            FROM tool_calls
            WHERE id = $1
            "#,
        )
        .bind(tool_call_id)
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

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
