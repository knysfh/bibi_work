use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            audit::{self, NewAuditLog},
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            local_runtime_queue,
            models::*,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{run_service, support::*};

const LOCAL_EXEC_PROTOCOL: &str = "local_executor.v1";
const LOCAL_EXEC_KIND_FILE_IO: &str = "file_io";
const LOCAL_EXEC_DEFAULT_TIMEOUT_MS: i32 = 30_000;
const LOCAL_EXEC_MAX_TIMEOUT_MS: i32 = 300_000;
const LOCAL_EXEC_DEFAULT_MAX_OUTPUT_BYTES: i32 = 1_048_576;
const LOCAL_EXEC_MAX_OUTPUT_BYTES: i32 = 8 * 1_048_576;
const BROWSER_PROTOCOL: &str = "biwork_browser.v1";
const BROWSER_KIND: &str = "browser";
const BROWSER_MAX_SESSION_ID_BYTES: usize = 128;
const BROWSER_MAX_ACTION_TEXT_BYTES: usize = 20_000;

#[derive(Debug, Deserialize)]
pub struct InternalLocalExecWaitQuery {
    tenant_id: Uuid,
    timeout_ms: Option<i32>,
}

pub async fn request_local_exec(
    State(state): State<AppState>,
    Json(payload): Json<LocalExecRequest>,
) -> Result<Json<LocalExecResponse>, AppError> {
    let timeout_ms = bounded_timeout_ms(payload.timeout_ms);
    let max_output_bytes = bounded_max_output_bytes(payload.max_output_bytes);

    if payload.operation.is_none() && payload.command.is_some() {
        return queue_legacy_local_exec(state, payload, timeout_ms, max_output_bytes)
            .await
            .map(Json);
    }

    let operation = payload
        .operation
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("local exec operation is required".to_string()))?;
    validate_local_operation(operation)?;
    let local_mount_id = payload
        .local_mount_id
        .ok_or_else(|| AppError::InvalidInput("local_mount_id is required".to_string()))?;
    let device_id = payload
        .device_id
        .or(payload.actor_device_id)
        .ok_or_else(|| AppError::InvalidInput("device_id is required".to_string()))?;
    let virtual_path = payload
        .virtual_path
        .as_deref()
        .ok_or_else(|| AppError::InvalidInput("virtual_path is required".to_string()))?;
    validate_virtual_local_path(virtual_path)?;

    let mount =
        load_authorized_local_mount(&state.connect_pool, &payload, local_mount_id, device_id)
            .await?;
    if !virtual_path.starts_with(&mount.virtual_path) {
        return Err(AppError::InvalidInput(
            "virtual_path is outside the local mount".to_string(),
        ));
    }
    ensure_local_mount_capability(&mount.capabilities, capability_for_operation(operation))?;

    require_ferriskey_allow_for_actor(
        &state,
        payload.tenant_id,
        ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        },
        capability_for_operation(operation),
        "local_mount",
        format!("{local_mount_id}:{virtual_path}"),
        Some(AuthzContext {
            project_id: payload.project_id,
            run_id: payload.run_id,
            risk_level: Some("medium".to_string()),
            ..Default::default()
        }),
    )
    .await?;

    let command = json!({
        "protocol": LOCAL_EXEC_PROTOCOL,
        "kind": LOCAL_EXEC_KIND_FILE_IO,
        "local_mount_id": local_mount_id,
        "operation": operation,
        "virtual_path": virtual_path,
        "content": payload.content,
        "query": payload.query,
        "expected_revision": payload.expected_revision,
        "reason": payload.reason.as_deref(),
        "max_output_bytes": max_output_bytes,
        "tool_context": {
            "tool_call_id": payload.tool_call_id.as_deref(),
            "tool_name": payload.tool_name.as_deref(),
            "args_hash": payload.args_hash.as_deref(),
            "parent_tool_call_id": payload.parent_tool_call_id.as_deref()
        }
    });
    let row = local_runtime_queue::enqueue(
        &state.connect_pool,
        local_runtime_queue::EnqueueLocalRuntimeRequest {
            tenant_id: payload.tenant_id,
            device_id: Some(device_id),
            project_id: payload.project_id,
            run_id: payload.run_id,
            command,
            timeout_ms,
            max_output_bytes,
        },
    )
    .await?;

    let result = local_runtime_queue::wait_for_result(
        &state.connect_pool,
        row.id,
        payload.tenant_id,
        timeout_ms,
    )
    .await?;
    Ok(Json(LocalExecResponse {
        id: row.id,
        tenant_id: row.tenant_id,
        device_id: row.device_id,
        local_mount_id: Some(local_mount_id),
        project_id: row.project_id,
        run_id: row.run_id,
        status: result.status,
        command: row.command,
        result: result.result,
        error: result.error,
        timeout_ms: row.timeout_ms,
        max_output_bytes: row.max_output_bytes,
        created_at: row.created_at,
    }))
}

pub async fn internal_wait_local_exec_request(
    State(state): State<AppState>,
    Path(request_id): Path<Uuid>,
    Query(query): Query<InternalLocalExecWaitQuery>,
) -> Result<Json<Value>, AppError> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM local_exec_requests
            WHERE id = $1 AND tenant_id = $2
        )
        "#,
    )
    .bind(request_id)
    .bind(query.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if !exists {
        return Err(AppError::NotFound(
            "local exec request not found".to_string(),
        ));
    }

    let result = local_runtime_queue::wait_for_result(
        &state.connect_pool,
        request_id,
        query.tenant_id,
        bounded_timeout_ms(query.timeout_ms),
    )
    .await?;
    Ok(Json(json!({
        "id": request_id,
        "status": result.status,
        "result": result.result,
        "error": result.error,
    })))
}

pub async fn next_local_exec_request(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<LocalExecNextQuery>,
) -> Result<Json<Option<LocalExecWorkItemResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    local_runtime_queue::mark_expired(&state.connect_pool, query.tenant_id, ctx.device_id).await?;
    let row = sqlx::query(
        r#"
        WITH next_request AS (
            SELECT id
            FROM local_exec_requests
            WHERE tenant_id = $1
              AND device_id = $2
              AND status = 'queued'
              AND ($3::text IS NULL OR command->>'kind' = $3)
            ORDER BY created_at ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE local_exec_requests
        SET status = 'dispatching', updated_at = CURRENT_TIMESTAMP
        WHERE id IN (SELECT id FROM next_request)
        RETURNING id, tenant_id, device_id, command, timeout_ms, max_output_bytes, created_at
        "#,
    )
    .bind(query.tenant_id)
    .bind(ctx.device_id)
    .bind(query.kind.as_deref())
    .fetch_optional(&state.connect_pool)
    .await?;

    let Some(row) = row else {
        return Ok(Json(None));
    };
    Ok(Json(Some(LocalExecWorkItemResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        command: row.try_get("command")?,
        timeout_ms: row.try_get("timeout_ms")?,
        max_output_bytes: row.try_get("max_output_bytes")?,
        created_at: row.try_get("created_at")?,
    })))
}

pub async fn get_local_exec_request_status(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(request_id): Path<Uuid>,
    Query(query): Query<LocalExecNextQuery>,
) -> Result<Json<LocalExecStatusResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT id, status, run_id
        FROM local_exec_requests
        WHERE id = $1
          AND tenant_id = $2
          AND device_id = $3
        "#,
    )
    .bind(request_id)
    .bind(query.tenant_id)
    .bind(ctx.device_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("local exec request not found".to_string()))?;
    Ok(Json(LocalExecStatusResponse {
        id: row.try_get("id")?,
        status: row.try_get("status")?,
        run_id: row.try_get("run_id")?,
    }))
}

pub async fn create_local_exec_permission(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(request_id): Path<Uuid>,
    Json(payload): Json<LocalExecPermissionRequest>,
) -> Result<Json<LocalExecPermissionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let permission_id = bounded_required_text(&payload.permission_id, "permission_id", 256)?;
    let title = bounded_required_text(&payload.title, "title", 500)?;
    let options = sanitize_acp_permission_options(payload.options)?;
    let tool_call = sanitize_bounded_json(payload.tool_call.unwrap_or_else(|| json!({})), 32_768)?;
    let mut tx = state.connect_pool.begin().await?;
    let request = sqlx::query(
        r#"
        SELECT run_id, command
        FROM local_exec_requests
        WHERE id = $1
          AND tenant_id = $2
          AND device_id = $3
          AND status = 'dispatching'
        FOR UPDATE
        "#,
    )
    .bind(request_id)
    .bind(payload.tenant_id)
    .bind(ctx.device_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("desktop ACP request not found".to_string()))?;
    let command: Value = request.try_get("command")?;
    if command.get("kind").and_then(Value::as_str) != Some("biwork_cli") {
        return Err(AppError::InvalidInput(
            "local request is not a desktop ACP execution".to_string(),
        ));
    }
    let run_id: Uuid = request
        .try_get::<Option<Uuid>, _>("run_id")?
        .ok_or_else(|| {
            AppError::InvalidInput("desktop ACP request is missing run_id".to_string())
        })?;
    let conversation_id = command
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            AppError::InvalidInput("desktop ACP request is missing conversation_id".to_string())
        })?;
    if let Some(existing) = load_local_permission_tx(
        &mut tx,
        payload.tenant_id,
        request_id,
        run_id,
        &permission_id,
    )
    .await?
    {
        tx.commit()
            .await
            .map_err(|_| AppError::DatabaseTransaction)?;
        return Ok(Json(existing));
    }

    let tool_call_id = Uuid::new_v4();
    let approval_id = Uuid::new_v4();
    let interrupt_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO tool_calls (
            id, tenant_id, conversation_id, run_id, tool_name,
            resource_type, resource_id, risk_level, status, decision,
            policy_version, input_summary
        )
        VALUES ($1, $2, $3, $4, $5, 'acp_permission', $6, 'high',
                'waiting_approval', 'review', 'desktop-acp-permission-v1', $7)
        "#,
    )
    .bind(tool_call_id)
    .bind(payload.tenant_id)
    .bind(conversation_id)
    .bind(run_id)
    .bind(&title)
    .bind(&permission_id)
    .bind(&title)
    .execute(&mut *tx)
    .await?;
    let request_payload = json!({
        "source": "desktop_acp",
        "local_exec_request_id": request_id,
        "acp_permission_id": permission_id,
        "title": title,
        "options": options,
        "tool_call": tool_call,
    });
    sqlx::query(
        r#"
        INSERT INTO approvals (
            id, tenant_id, conversation_id, run_id, tool_call_id, request_payload
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(approval_id)
    .bind(payload.tenant_id)
    .bind(conversation_id)
    .bind(run_id)
    .bind(tool_call_id)
    .bind(&request_payload)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO interrupts (
            id, tenant_id, conversation_id, run_id, approval_id, type, payload
        )
        VALUES ($1, $2, $3, $4, $5, 'approval', $6)
        "#,
    )
    .bind(interrupt_id)
    .bind(payload.tenant_id)
    .bind(conversation_id)
    .bind(run_id)
    .bind(approval_id)
    .bind(json!({ "approval_id": approval_id, "tool_call_id": tool_call_id }))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE runs SET status = 'waiting_approval', updated_at = CURRENT_TIMESTAMP WHERE id = $1 AND status IN ('queued', 'running')",
    )
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        payload.tenant_id,
        conversation_id,
        Some(run_id),
        RunEventInput {
            event_id: Some(format!("approval.requested.desktop-acp.{approval_id}")),
            event_type: "approval.requested".to_string(),
            payload: Some(json!({
                "approval_id": approval_id,
                "tool_call_id": tool_call_id,
                "tool_name": title,
                "risk_level": "high",
                "input_summary": title,
                "run_id": run_id,
                "source": "desktop_acp"
            })),
            trace_id: command
                .get("trace_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
    )
    .await?;
    let approval_resource_id = approval_id.to_string();
    audit::insert_audit_log_tx(
        &mut tx,
        NewAuditLog {
            tenant_id: payload.tenant_id,
            actor_user_id: Some(ctx.platform_user_id),
            actor_device_id: Some(ctx.device_id),
            session_id: Some(ctx.session_id),
            resource_type: "approval",
            resource_id: &approval_resource_id,
            action: "approval.requested",
            decision: "review",
            policy_version: "desktop-acp-permission-v1",
            reason_code: Some("acp_agent_permission_request"),
            run_id: Some(run_id),
            conversation_id: Some(conversation_id),
            workflow_run_id: None,
            tool_call_id: Some(tool_call_id),
            approval_id: Some(approval_id),
            args_hash: None,
            input_summary: Some(&title),
            output_summary: None,
            risk_level: Some("high"),
            ip: None,
            user_agent: None,
            trace_id: command.get("trace_id").and_then(Value::as_str),
        },
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(&state, &event).await;
    Ok(Json(LocalExecPermissionResponse {
        approval_id,
        status: "pending".to_string(),
        selected_option_id: None,
    }))
}

pub async fn get_local_exec_permission(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((request_id, approval_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<LocalExecNextQuery>,
) -> Result<Json<LocalExecPermissionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT a.status, a.request_payload, a.decision_payload
        FROM approvals a
        JOIN local_exec_requests ler
          ON ler.run_id = a.run_id
         AND ler.tenant_id = a.tenant_id
        WHERE a.id = $1
          AND a.tenant_id = $2
          AND ler.id = $3
          AND ler.device_id = $4
          AND a.request_payload->>'local_exec_request_id' = $3::text
        "#,
    )
    .bind(approval_id)
    .bind(query.tenant_id)
    .bind(request_id)
    .bind(ctx.device_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("desktop ACP permission not found".to_string()))?;
    let status: String = row.try_get("status")?;
    let request_payload: Value = row.try_get("request_payload")?;
    let decision_payload: Option<Value> = row.try_get("decision_payload")?;
    Ok(Json(LocalExecPermissionResponse {
        approval_id,
        selected_option_id: selected_acp_permission_option(
            &status,
            &request_payload,
            decision_payload.as_ref(),
        ),
        status,
    }))
}

pub async fn complete_local_exec_request(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(request_id): Path<Uuid>,
    Json(payload): Json<LocalExecCompleteRequest>,
) -> Result<Json<LocalExecResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let status = match payload.status.as_str() {
        "completed" => "completed",
        "failed" => "failed",
        _ => {
            return Err(AppError::InvalidInput(
                "local exec completion status must be completed or failed".to_string(),
            ));
        }
    };
    let mut tx = state.connect_pool.begin().await?;
    let row = sqlx::query(
        r#"
        UPDATE local_exec_requests
        SET status = $3, updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND device_id = $4
          AND status IN ('queued', 'dispatching')
        RETURNING id, tenant_id, device_id, project_id, run_id, command,
                  timeout_ms, max_output_bytes, created_at
        "#,
    )
    .bind(request_id)
    .bind(payload.tenant_id)
    .bind(status)
    .bind(ctx.device_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("local exec request not found".to_string()))?;

    let event_type = if status == "completed" {
        "local_exec.completed"
    } else {
        "local_exec.failed"
    };
    let bounded_error = payload
        .error
        .as_deref()
        .map(|value| value.chars().take(2_000).collect::<String>());
    let result_bytes = payload
        .result
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| AppError::InvalidInput("local runtime result is not valid JSON".to_string()))?
        .map_or(0, |bytes| bytes.len());
    let max_output_bytes: i32 = row.try_get("max_output_bytes")?;
    if result_bytes > usize::try_from(max_output_bytes).unwrap_or_default() {
        return Err(AppError::InvalidInput(
            "local runtime result exceeds max_output_bytes".to_string(),
        ));
    }
    let command: Value = row.try_get("command")?;
    if command.get("kind").and_then(Value::as_str) == Some("biwork_cli") {
        let run_id: Uuid = row.try_get::<Option<Uuid>, _>("run_id")?.ok_or_else(|| {
            AppError::InvalidInput("desktop ACP request is missing run_id".to_string())
        })?;
        let run_status: String =
            sqlx::query_scalar("SELECT status FROM runs WHERE id = $1 AND tenant_id = $2")
                .bind(run_id)
                .bind(payload.tenant_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| AppError::NotFound("desktop ACP run not found".to_string()))?;
        let expected = if status == "completed" {
            "completed"
        } else {
            "failed"
        };
        if run_status != expected && !(status == "failed" && run_status == "cancelled") {
            return Err(AppError::Conflict(format!(
                "desktop ACP run must be terminal before local completion (expected {expected}, got {run_status})"
            )));
        }
    }
    let event_payload = json!({
        "status": status,
        "result": payload.result,
        "error": bounded_error
    });
    sqlx::query(
        r#"
        INSERT INTO local_exec_events (tenant_id, local_exec_request_id, type, payload)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(payload.tenant_id)
    .bind(request_id)
    .bind(event_type)
    .bind(&event_payload)
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(Json(LocalExecResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        device_id: row.try_get("device_id")?,
        local_mount_id: command_local_mount_id(command.clone()),
        project_id: row.try_get("project_id")?,
        run_id: row.try_get("run_id")?,
        status: status.to_string(),
        command,
        result: payload.result,
        error: bounded_error,
        timeout_ms: row.try_get("timeout_ms")?,
        max_output_bytes,
        created_at: row.try_get("created_at")?,
    }))
}

pub async fn ingest_local_exec_run_events(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(request_id): Path<Uuid>,
    Json(payload): Json<LocalExecEventsRequest>,
) -> Result<Json<IngestRunEventsResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    if payload.events.is_empty() || payload.events.len() > 100 {
        return Err(AppError::InvalidInput(
            "desktop ACP event batch must contain 1 to 100 events".to_string(),
        ));
    }
    for event in &payload.events {
        if !matches!(
            event.event_type.as_str(),
            "run.started"
                | "run.completed"
                | "run.failed"
                | "run.cancelled"
                | "message.started"
                | "message.delta"
                | "message.completed"
                | "thinking.started"
                | "thinking.delta"
                | "thinking.completed"
                | "tool.call.requested"
                | "tool.call.started"
                | "tool.call.delta"
                | "tool.call.completed"
                | "tool.call.failed"
                | "activity.raw"
        ) {
            return Err(AppError::InvalidInput(format!(
                "desktop ACP event type is not allowed: {}",
                event.event_type
            )));
        }
    }
    let row = sqlx::query(
        r#"
        SELECT run_id, command
        FROM local_exec_requests
        WHERE id = $1
          AND tenant_id = $2
          AND device_id = $3
          AND status = 'dispatching'
        "#,
    )
    .bind(request_id)
    .bind(payload.tenant_id)
    .bind(ctx.device_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("desktop ACP request not found".to_string()))?;
    let command: Value = row.try_get("command")?;
    if command.get("kind").and_then(Value::as_str) != Some("biwork_cli")
        || command.get("protocol").and_then(Value::as_str) != Some("biwork_acp.v1")
    {
        return Err(AppError::InvalidInput(
            "local request is not a desktop ACP execution".to_string(),
        ));
    }
    let run_id: Uuid = row.try_get::<Option<Uuid>, _>("run_id")?.ok_or_else(|| {
        AppError::InvalidInput("desktop ACP request is missing run_id".to_string())
    })?;
    let conversation_id = command
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            AppError::InvalidInput("desktop ACP request is missing conversation_id".to_string())
        })?;
    let trace_id = command
        .get("trace_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let events = payload
        .events
        .into_iter()
        .map(|mut event| {
            event.trace_id = trace_id.clone();
            event
        })
        .collect();
    run_service::ingest_run_events(
        State(state),
        Json(IngestRunEventsRequest {
            tenant_id: payload.tenant_id,
            conversation_id,
            run_id: Some(run_id),
            events,
        }),
    )
    .await
}

fn bounded_required_text(value: &str, field: &str, max_chars: usize) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::InvalidInput(format!("{field} is required")));
    }
    if value.chars().count() > max_chars {
        return Err(AppError::InvalidInput(format!(
            "{field} exceeds {max_chars} characters"
        )));
    }
    Ok(value.to_string())
}

fn sanitize_acp_permission_options(value: Value) -> Result<Value, AppError> {
    let values = value.as_array().ok_or_else(|| {
        AppError::InvalidInput("ACP permission options must be an array".to_string())
    })?;
    if values.is_empty() || values.len() > 16 {
        return Err(AppError::InvalidInput(
            "ACP permission options must contain 1 to 16 items".to_string(),
        ));
    }
    let mut sanitized = Vec::with_capacity(values.len());
    for value in values {
        let option_id = bounded_required_text(
            value
                .get("optionId")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "optionId",
            128,
        )?;
        let name = bounded_required_text(
            value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "permission option name",
            256,
        )?;
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .filter(|kind| {
                matches!(
                    *kind,
                    "allow_once" | "allow_always" | "reject_once" | "reject_always"
                )
            })
            .ok_or_else(|| {
                AppError::InvalidInput("ACP permission option kind is invalid".to_string())
            })?;
        sanitized.push(json!({ "optionId": option_id, "name": name, "kind": kind }));
    }
    Ok(Value::Array(sanitized))
}

fn sanitize_bounded_json(value: Value, max_bytes: usize) -> Result<Value, AppError> {
    let value = redact_acp_permission_json(value);
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| AppError::InvalidInput("ACP permission payload is invalid".to_string()))?;
    if bytes.len() > max_bytes {
        return Err(AppError::InvalidInput(
            "ACP permission payload is too large".to_string(),
        ));
    }
    Ok(value)
}

fn redact_acp_permission_json(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .filter_map(|(key, value)| {
                    let normalized = key
                        .chars()
                        .filter(|ch| ch.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    if matches!(
                        normalized.as_str(),
                        "apikey"
                            | "authorization"
                            | "credential"
                            | "credentials"
                            | "password"
                            | "secret"
                            | "secretref"
                            | "token"
                    ) {
                        None
                    } else {
                        Some((key, redact_acp_permission_json(value)))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(redact_acp_permission_json).collect())
        }
        other => other,
    }
}

async fn load_local_permission_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    request_id: Uuid,
    run_id: Uuid,
    permission_id: &str,
) -> Result<Option<LocalExecPermissionResponse>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, status, request_payload, decision_payload
        FROM approvals
        WHERE tenant_id = $1
          AND run_id = $2
          AND request_payload->>'local_exec_request_id' = $3::text
          AND request_payload->>'acp_permission_id' = $4
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(request_id)
    .bind(permission_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let approval_id: Uuid = row.try_get("id")?;
    let status: String = row.try_get("status")?;
    let request_payload: Value = row.try_get("request_payload")?;
    let decision_payload: Option<Value> = row.try_get("decision_payload")?;
    Ok(Some(LocalExecPermissionResponse {
        approval_id,
        selected_option_id: selected_acp_permission_option(
            &status,
            &request_payload,
            decision_payload.as_ref(),
        ),
        status,
    }))
}

fn selected_acp_permission_option(
    status: &str,
    request_payload: &Value,
    decision_payload: Option<&Value>,
) -> Option<String> {
    if let Some(option_id) = decision_payload
        .and_then(|value| value.pointer("/payload/option_id"))
        .and_then(Value::as_str)
    {
        return Some(option_id.to_string());
    }
    let allowed_kinds: &[&str] = match status {
        "approved" => &["allow_once", "allow_always"],
        "rejected" => &["reject_once", "reject_always"],
        _ => return None,
    };
    request_payload
        .get("options")
        .and_then(Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| allowed_kinds.contains(&kind))
            })
        })
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

async fn queue_legacy_local_exec(
    state: AppState,
    payload: LocalExecRequest,
    timeout_ms: i32,
    max_output_bytes: i32,
) -> Result<LocalExecResponse, AppError> {
    let command = payload
        .command
        .as_ref()
        .ok_or_else(|| AppError::InvalidInput("local exec command is required".to_string()))?;
    let is_browser_command = validate_browser_command(command)?;
    if is_browser_command {
        validate_browser_device_binding(&payload)?;
    } else {
        let actor = ActorRef {
            user_id: payload.actor_user_id,
            device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            roles: Vec::new(),
        };
        require_ferriskey_allow_for_actor(
            &state,
            payload.tenant_id,
            actor,
            "execute",
            "local_exec",
            payload
                .device_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "unbound-device".to_string()),
            Some(AuthzContext {
                project_id: payload.project_id,
                run_id: payload.run_id,
                risk_level: Some("critical".to_string()),
                ..Default::default()
            }),
        )
        .await?;
    }
    let row = local_runtime_queue::enqueue(
        &state.connect_pool,
        local_runtime_queue::EnqueueLocalRuntimeRequest {
            tenant_id: payload.tenant_id,
            device_id: payload.device_id,
            project_id: payload.project_id,
            run_id: payload.run_id,
            command: payload.command.expect("validated local exec command"),
            timeout_ms,
            max_output_bytes,
        },
    )
    .await?;
    Ok(LocalExecResponse {
        id: row.id,
        tenant_id: row.tenant_id,
        device_id: row.device_id,
        local_mount_id: None,
        project_id: row.project_id,
        run_id: row.run_id,
        status: "queued".to_string(),
        command: row.command,
        result: None,
        error: None,
        timeout_ms: row.timeout_ms,
        max_output_bytes: row.max_output_bytes,
        created_at: row.created_at,
    })
}

fn validate_browser_command(command: &Value) -> Result<bool, AppError> {
    if command.get("protocol").and_then(Value::as_str) != Some(BROWSER_PROTOCOL)
        || command.get("kind").and_then(Value::as_str) != Some(BROWSER_KIND)
    {
        return Ok(false);
    }
    let object = command
        .as_object()
        .ok_or_else(|| AppError::InvalidInput("browser command must be an object".to_string()))?;
    reject_unknown_fields(
        object.keys().map(String::as_str),
        &["protocol", "kind", "session_id", "profile", "action"],
        "browser command",
    )?;
    let session_id = required_bounded_string(
        command.get("session_id"),
        "browser session_id",
        BROWSER_MAX_SESSION_ID_BYTES,
        false,
    )?;
    if !session_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(AppError::InvalidInput(
            "browser session_id contains unsupported characters".to_string(),
        ));
    }
    if let Some(profile) = command.get("profile") {
        let profile = required_bounded_string(Some(profile), "browser profile", 64, false)?;
        if !profile
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        {
            return Err(AppError::InvalidInput(
                "browser profile contains unsupported characters".to_string(),
            ));
        }
    }
    let action = command
        .get("action")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::InvalidInput("browser action must be an object".to_string()))?;
    let action_name =
        required_bounded_string(action.get("name"), "browser action.name", 32, false)?;
    match action_name {
        "open" | "goto" | "tab_open" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "url"],
                "browser action",
            )?;
            validate_browser_url(required_bounded_string(
                action.get("url"),
                "browser action.url",
                4_096,
                false,
            )?)?;
        }
        "snapshot" | "close" | "tab_list" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name"],
                "browser action",
            )?;
        }
        "tab_select" | "tab_close" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "tab_id"],
                "browser action",
            )?;
            required_bounded_string(action.get("tab_id"), "browser action.tab_id", 128, false)?;
        }
        "click" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "ref"],
                "browser action",
            )?;
            required_bounded_string(action.get("ref"), "browser action.ref", 128, false)?;
        }
        "fill" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "ref", "text"],
                "browser action",
            )?;
            required_bounded_string(action.get("ref"), "browser action.ref", 128, false)?;
            required_bounded_string(
                action.get("text"),
                "browser action.text",
                BROWSER_MAX_ACTION_TEXT_BYTES,
                true,
            )?;
        }
        "press" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "key"],
                "browser action",
            )?;
            required_bounded_string(action.get("key"), "browser action.key", 128, false)?;
        }
        "scroll" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "ref", "delta_x", "delta_y"],
                "browser action",
            )?;
            optional_bounded_string(action.get("ref"), "browser action.ref", 128)?;
            optional_bounded_i64(
                action.get("delta_x"),
                "browser action.delta_x",
                -5_000,
                5_000,
            )?;
            optional_bounded_i64(
                action.get("delta_y"),
                "browser action.delta_y",
                -5_000,
                5_000,
            )?;
        }
        "wait_for_change" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "timeout_ms"],
                "browser action",
            )?;
            optional_bounded_i64(
                action.get("timeout_ms"),
                "browser action.timeout_ms",
                1_000,
                30_000,
            )?;
        }
        "extract_text" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "ref", "selector"],
                "browser action",
            )?;
            optional_bounded_string(action.get("ref"), "browser action.ref", 128)?;
            optional_bounded_string(action.get("selector"), "browser action.selector", 2_048)?;
            if action.get("ref").is_some_and(|value| !value.is_null())
                && action.get("selector").is_some_and(|value| !value.is_null())
            {
                return Err(AppError::InvalidInput(
                    "browser extract_text accepts ref or selector, not both".to_string(),
                ));
            }
        }
        "wait_for_user" => {
            reject_unknown_fields(
                action.keys().map(String::as_str),
                &["name", "reason", "expected_url"],
                "browser action",
            )?;
            required_bounded_string(action.get("reason"), "browser action.reason", 1_024, false)?;
            optional_bounded_string(
                action.get("expected_url"),
                "browser action.expected_url",
                4_096,
            )?;
        }
        _ => {
            return Err(AppError::InvalidInput(format!(
                "unsupported browser action: {action_name}"
            )));
        }
    }
    Ok(true)
}

fn validate_browser_device_binding(payload: &LocalExecRequest) -> Result<(), AppError> {
    let device_id = payload
        .device_id
        .ok_or_else(|| AppError::InvalidInput("browser device_id is required".to_string()))?;
    if payload.actor_device_id != Some(device_id) {
        return Err(AppError::PermissionDenied(
            "browser work must target the authenticated actor device".to_string(),
        ));
    }
    Ok(())
}

fn validate_browser_url(value: &str) -> Result<(), AppError> {
    let url = Url::parse(value)
        .map_err(|_| AppError::InvalidInput("browser URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::InvalidInput(
            "only credential-free HTTP(S) browser URLs are allowed; use browser_snapshot instead of view-source URLs"
                .to_string(),
        ));
    }
    Ok(())
}

fn required_bounded_string<'a>(
    value: Option<&'a Value>,
    field: &str,
    max_bytes: usize,
    allow_empty: bool,
) -> Result<&'a str, AppError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput(format!("{field} must be a string")))?;
    if (!allow_empty && value.trim().is_empty()) || value.len() > max_bytes {
        return Err(AppError::InvalidInput(format!(
            "{field} must contain at most {max_bytes} bytes"
        )));
    }
    Ok(value)
}

fn optional_bounded_string(
    value: Option<&Value>,
    field: &str,
    max_bytes: usize,
) -> Result<(), AppError> {
    if value.is_none_or(Value::is_null) {
        return Ok(());
    }
    required_bounded_string(value, field, max_bytes, true).map(|_| ())
}

fn optional_bounded_i64(
    value: Option<&Value>,
    field: &str,
    minimum: i64,
    maximum: i64,
) -> Result<(), AppError> {
    if value.is_none_or(Value::is_null) {
        return Ok(());
    }
    let value = value
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::InvalidInput(format!("{field} must be an integer")))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(AppError::InvalidInput(format!(
            "{field} must be between {minimum} and {maximum}"
        )));
    }
    Ok(())
}

fn reject_unknown_fields<'a>(
    fields: impl Iterator<Item = &'a str>,
    allowed: &[&str],
    object_name: &str,
) -> Result<(), AppError> {
    if let Some(field) = fields.into_iter().find(|field| !allowed.contains(field)) {
        return Err(AppError::InvalidInput(format!(
            "{object_name} contains unsupported field: {field}"
        )));
    }
    Ok(())
}

async fn load_authorized_local_mount(
    pool: &PgPool,
    payload: &LocalExecRequest,
    local_mount_id: Uuid,
    device_id: Uuid,
) -> Result<LocalMountAccess, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, virtual_path, capabilities
        FROM local_mounts
        WHERE id = $1
          AND tenant_id = $2
          AND user_id = $3
          AND device_id = $4
          AND status = 'active'
        "#,
    )
    .bind(local_mount_id)
    .bind(payload.tenant_id)
    .bind(payload.actor_user_id)
    .bind(device_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("active local mount not found".to_string()))?;
    Ok(LocalMountAccess {
        virtual_path: row.try_get("virtual_path")?,
        capabilities: row.try_get("capabilities")?,
    })
}

fn validate_local_operation(operation: &str) -> Result<(), AppError> {
    if matches!(operation, "read_text" | "write_text" | "list" | "search") {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "unsupported local exec operation: {operation}"
        )))
    }
}

fn capability_for_operation(operation: &str) -> &'static str {
    if operation == "write_text" {
        "write"
    } else {
        "read"
    }
}

fn validate_virtual_local_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(AppError::InvalidInput(
            "virtual_path is required".to_string(),
        ));
    }
    if path.contains('\0') {
        return Err(AppError::InvalidInput(
            "virtual_path contains null byte".to_string(),
        ));
    }
    if path.starts_with("//") || !path.starts_with("/local/") {
        return Err(AppError::InvalidInput(
            "virtual_path must start with /local/".to_string(),
        ));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(AppError::InvalidInput(
            "virtual_path may not contain ..".to_string(),
        ));
    }
    Ok(())
}

fn ensure_local_mount_capability(capabilities: &Value, required: &str) -> Result<(), AppError> {
    let Some(items) = capabilities.as_array() else {
        return Err(AppError::PermissionDenied(
            "local mount capabilities are invalid".to_string(),
        ));
    };
    if items.iter().any(|item| item.as_str() == Some(required)) {
        Ok(())
    } else {
        Err(AppError::PermissionDenied(format!(
            "local mount lacks {required} capability"
        )))
    }
}

fn bounded_timeout_ms(timeout_ms: Option<i32>) -> i32 {
    timeout_ms
        .unwrap_or(LOCAL_EXEC_DEFAULT_TIMEOUT_MS)
        .clamp(1_000, LOCAL_EXEC_MAX_TIMEOUT_MS)
}

fn bounded_max_output_bytes(max_output_bytes: Option<i32>) -> i32 {
    max_output_bytes
        .unwrap_or(LOCAL_EXEC_DEFAULT_MAX_OUTPUT_BYTES)
        .clamp(1_024, LOCAL_EXEC_MAX_OUTPUT_BYTES)
}

fn command_local_mount_id(command: Value) -> Option<Uuid> {
    command
        .get("local_mount_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

struct LocalMountAccess {
    virtual_path: String,
    capabilities: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_local_virtual_paths() {
        assert!(validate_virtual_local_path("/local/main/src/lib.rs").is_ok());
        assert!(validate_virtual_local_path("/local/main/../secret").is_err());
        assert!(validate_virtual_local_path("/workspace/file").is_err());
        assert!(validate_virtual_local_path("//local/main/file").is_err());
    }

    #[test]
    fn maps_file_operations_to_capabilities() {
        assert_eq!(capability_for_operation("read_text"), "read");
        assert_eq!(capability_for_operation("list"), "read");
        assert_eq!(capability_for_operation("search"), "read");
        assert_eq!(capability_for_operation("write_text"), "write");
    }

    #[test]
    fn sanitizes_acp_permission_options_and_rejects_unknown_kinds() {
        let sanitized = sanitize_acp_permission_options(json!([
            {"optionId": "allow", "name": "Allow once", "kind": "allow_once", "secret": "drop"},
            {"optionId": "deny", "name": "Reject", "kind": "reject_once"}
        ]))
        .unwrap();
        assert_eq!(sanitized[0]["optionId"], json!("allow"));
        assert!(sanitized[0].get("secret").is_none());
        assert!(
            sanitize_acp_permission_options(json!([
                {"optionId": "bad", "name": "Bad", "kind": "auto_allow"}
            ]))
            .is_err()
        );
    }

    #[test]
    fn selects_acp_option_matching_the_platform_decision() {
        let request = json!({
            "options": [
                {"optionId": "yes", "kind": "allow_once"},
                {"optionId": "no", "kind": "reject_once"}
            ]
        });
        assert_eq!(
            selected_acp_permission_option("approved", &request, None),
            Some("yes".to_string())
        );
        assert_eq!(
            selected_acp_permission_option("rejected", &request, None),
            Some("no".to_string())
        );
        assert_eq!(
            selected_acp_permission_option(
                "approved",
                &request,
                Some(&json!({"payload": {"option_id": "always"}}))
            ),
            Some("always".to_string())
        );
    }

    #[test]
    fn acp_permission_payload_redacts_nested_secrets_before_persistence() {
        let sanitized = sanitize_bounded_json(
            json!({
                "safe": true,
                "authorization": "Bearer secret",
                "nested": {"api_key": "secret", "path": "/tmp/file"}
            }),
            1024,
        )
        .unwrap();
        assert_eq!(sanitized["safe"], json!(true));
        assert!(sanitized.get("authorization").is_none());
        assert!(sanitized["nested"].get("api_key").is_none());
        assert_eq!(sanitized["nested"]["path"], json!("/tmp/file"));
    }

    #[test]
    fn browser_protocol_accepts_only_bounded_declared_actions() {
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "profile": "research",
                "action": {"name": "open", "url": "https://www.mail.com/"}
            }))
            .unwrap()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "fill", "ref": "e1", "text": "query"}
            }))
            .unwrap()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "extract_text", "ref": "e16"}
            }))
            .unwrap()
        );
        for action in [
            json!({"name": "tab_list"}),
            json!({"name": "tab_open", "url": "https://example.test/report"}),
            json!({"name": "tab_select", "tab_id": "t2"}),
            json!({"name": "tab_close", "tab_id": "t2"}),
            json!({"name": "scroll", "ref": "e7", "delta_x": 0, "delta_y": 900}),
            json!({"name": "wait_for_change", "timeout_ms": 12000}),
        ] {
            assert!(
                validate_browser_command(&json!({
                    "protocol": BROWSER_PROTOCOL,
                    "kind": BROWSER_KIND,
                    "session_id": "session-1",
                    "action": action
                }))
                .unwrap()
            );
        }
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "extract_text", "ref": "e16", "selector": "[ref=\"e16\"] a"}
            }))
            .is_err()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "tab_select"}
            }))
            .is_err()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "scroll", "delta_y": 5001}
            }))
            .is_err()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "wait_for_change", "timeout_ms": 999}
            }))
            .is_err()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "script", "source": "process.exit()"}
            }))
            .is_err()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "open", "url": "file:///etc/passwd"}
            }))
            .is_err()
        );
        assert!(
            validate_browser_command(&json!({
                "protocol": BROWSER_PROTOCOL,
                "kind": BROWSER_KIND,
                "session_id": "session-1",
                "action": {"name": "goto", "url": "view-source:https://oa.example.com"}
            }))
            .is_err()
        );
    }

    #[test]
    fn non_browser_legacy_commands_keep_the_generic_authorization_path() {
        assert!(!validate_browser_command(&json!({"argv": ["git", "status"]})).unwrap());
        assert!(
            !validate_browser_command(&json!({
                "protocol": "other.v1",
                "kind": "browser"
            }))
            .unwrap()
        );
    }

    #[test]
    fn browser_work_must_be_bound_to_the_actor_device() {
        let actor_device_id = Uuid::new_v4();
        let payload = LocalExecRequest {
            tenant_id: Uuid::new_v4(),
            actor_user_id: Uuid::new_v4(),
            actor_device_id: Some(actor_device_id),
            actor_session_id: Some(Uuid::new_v4()),
            device_id: Some(actor_device_id),
            local_mount_id: None,
            project_id: None,
            run_id: None,
            operation: None,
            virtual_path: None,
            content: None,
            query: None,
            expected_revision: None,
            reason: None,
            command: None,
            timeout_ms: None,
            max_output_bytes: None,
            tool_call_id: None,
            tool_name: None,
            args_hash: None,
            parent_tool_call_id: None,
        };
        assert!(validate_browser_device_binding(&payload).is_ok());
        let mismatched = LocalExecRequest {
            device_id: Some(Uuid::new_v4()),
            ..payload
        };
        assert!(matches!(
            validate_browser_device_binding(&mismatched),
            Err(AppError::PermissionDenied(_))
        ));
    }
}
