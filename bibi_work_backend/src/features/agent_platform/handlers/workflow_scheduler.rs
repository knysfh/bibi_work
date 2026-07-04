use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::*,
            run_lifecycle,
            run_snapshot::{self, WorkflowNodeRunSnapshotRequest},
            runtime::{CancelRunRequest, DispatchRunRequest},
            secret_resolver, workflow_compile, workflow_mapping, workflow_plan, workflow_runtime,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::support::*;
use super::{capability_authz, memory_injection};

struct PendingWorkflowDispatch {
    agent_id: Uuid,
    project_id: Option<Uuid>,
    dispatch: DispatchRunRequest,
}

pub async fn list_workflow_designs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkflowDesignListQuery>,
) -> Result<Json<Vec<WorkflowDesignResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, owner_user_id, name, description, design,
               status, created_at, updated_at
        FROM workflow_designs
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND ($2::text IS NULL OR status = $2)
        ORDER BY updated_at DESC, created_at DESC
        LIMIT $3
        "#,
    )
    .bind(query.tenant_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;
    let designs = rows
        .into_iter()
        .map(workflow_design_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(designs))
}

pub async fn get_workflow_design(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_design_id): Path<Uuid>,
    Query(query): Query<WorkflowDesignDetailQuery>,
) -> Result<Json<WorkflowDesignResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    load_workflow_design(&state, query.tenant_id, workflow_design_id)
        .await
        .map(Json)
}

pub async fn create_workflow_design(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateWorkflowDesignRequest>,
) -> Result<Json<ResourceResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, payload.tenant_id, "create", "workflow").await?;
    let row = sqlx::query(
        r#"
        INSERT INTO workflow_designs (tenant_id, owner_user_id, name, description, design)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, name, description, status,
                  design AS metadata, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.design.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(resource_from_row(row)?))
}

pub async fn update_workflow_design(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_design_id): Path<Uuid>,
    Json(payload): Json<UpdateWorkflowDesignRequest>,
) -> Result<Json<WorkflowDesignResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "workflow",
        workflow_design_id.to_string(),
        None,
    )
    .await?;
    if let Some(status) = payload.status.as_deref()
        && !matches!(status, "draft" | "active" | "disabled")
    {
        return Err(AppError::InvalidInput(
            "workflow design status must be draft, active, or disabled".to_string(),
        ));
    }
    let row = sqlx::query(
        r#"
        UPDATE workflow_designs
        SET name = COALESCE($3, name),
            description = COALESCE($4, description),
            design = COALESCE($5, design),
            status = COALESCE($6, status),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id, tenant_id, owner_user_id, name, description, design,
                  status, created_at, updated_at
        "#,
    )
    .bind(workflow_design_id)
    .bind(payload.tenant_id)
    .bind(payload.name)
    .bind(payload.description)
    .bind(payload.design)
    .bind(payload.status)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow design not found".to_string()))?;

    Ok(Json(workflow_design_from_row(row)?))
}

pub async fn list_workflow_versions(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_design_id): Path<Uuid>,
    Query(query): Query<WorkflowVersionListQuery>,
) -> Result<Json<Vec<VersionResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, workflow_design_id AS parent_id, version_label,
               compiled_plan AS snapshot, policy_version, status, created_at
        FROM workflow_versions
        WHERE tenant_id = $1
          AND workflow_design_id = $2
          AND ($3::text IS NULL OR status = $3)
        ORDER BY created_at DESC
        LIMIT $4
        "#,
    )
    .bind(query.tenant_id)
    .bind(workflow_design_id)
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

pub async fn publish_workflow_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_design_id): Path<Uuid>,
    Json(payload): Json<PublishWorkflowVersionRequest>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "manage",
        "workflow",
        workflow_design_id.to_string(),
        None,
    )
    .await?;
    let compiled_plan = workflow_compile::compile_plan(
        &state.connect_pool,
        payload.tenant_id,
        &payload.compiled_plan,
    )
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO workflow_versions (
            tenant_id, workflow_design_id, version_label, compiled_plan, policy_version
        )
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, tenant_id, workflow_design_id AS parent_id, version_label,
                  compiled_plan AS snapshot, policy_version, status, created_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(workflow_design_id)
    .bind(payload.version_label)
    .bind(compiled_plan)
    .bind(
        payload
            .policy_version
            .unwrap_or_else(|| LOCAL_POLICY_VERSION.to_string()),
    )
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(Json(version_from_row(row)?))
}

pub async fn get_workflow_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_version_id): Path<Uuid>,
    Query(query): Query<WorkflowVersionDetailQuery>,
) -> Result<Json<VersionResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    load_workflow_version(&state, query.tenant_id, workflow_version_id)
        .await
        .map(Json)
}

pub async fn validate_workflow_version(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_version_id): Path<Uuid>,
    Json(payload): Json<DisableCatalogResourceRequest>,
) -> Result<Json<ValidationResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let row = sqlx::query(
        r#"
        SELECT compiled_plan, status
        FROM workflow_versions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(workflow_version_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow version not found".to_string()))?;

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let status: String = row.try_get("status")?;
    if status != "published" {
        errors.push(format!(
            "workflow version status is {status}, expected published"
        ));
    }
    let compiled_plan: Value = row.try_get("compiled_plan")?;
    match workflow_compile::compile_plan(&state.connect_pool, payload.tenant_id, &compiled_plan)
        .await
    {
        Ok(plan) => {
            if let Err(error) = workflow_plan::validate(&plan) {
                errors.push(error.to_string());
            }
            if workflow_plan::nodes(&plan)?.is_empty() {
                warnings.push("workflow version has no nodes".to_string());
            }
        }
        Err(error) => errors.push(error.to_string()),
    }

    Ok(Json(ValidationResponse {
        valid: errors.is_empty(),
        errors,
        warnings,
    }))
}

pub async fn list_workflow_runs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<WorkflowRunListQuery>,
) -> Result<Json<Vec<WorkflowRunResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, workflow_version_id, conversation_id, project_id,
               status, trace_id, input, created_at, updated_at
        FROM workflow_runs
        WHERE tenant_id = $1
          AND ($2::uuid IS NULL OR workflow_version_id = $2)
          AND ($3::uuid IS NULL OR conversation_id = $3)
          AND ($4::uuid IS NULL OR project_id = $4)
          AND ($5::text IS NULL OR status = $5)
        ORDER BY created_at DESC
        LIMIT $6
        "#,
    )
    .bind(query.tenant_id)
    .bind(query.workflow_version_id)
    .bind(query.conversation_id)
    .bind(query.project_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;
    let runs = rows
        .into_iter()
        .map(workflow_run_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(runs))
}

pub async fn create_workflow_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateWorkflowRunRequest>,
) -> Result<Json<WorkflowRunResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "run",
        "workflow",
        payload.workflow_version_id.to_string(),
        Some(AuthzContext {
            conversation_id: payload.conversation_id,
            project_id: payload.project_id,
            ..Default::default()
        }),
    )
    .await?;
    let version_row = sqlx::query(
        r#"
        SELECT compiled_plan
        FROM workflow_versions
        WHERE id = $1 AND tenant_id = $2 AND status = 'published'
        "#,
    )
    .bind(payload.workflow_version_id)
    .bind(payload.tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow version not found".to_string()))?;
    let compiled_plan: Value = version_row.try_get("compiled_plan")?;
    let compiled_plan =
        workflow_compile::compile_plan(&state.connect_pool, payload.tenant_id, &compiled_plan)
            .await?;
    let node_permissions = workflow_compile::node_permissions(&compiled_plan)?;
    for node_permission in &node_permissions {
        require_ferriskey_allow(
            &state,
            &ctx,
            payload.tenant_id,
            "run",
            "agent",
            node_permission.agent_id.to_string(),
            Some(AuthzContext {
                conversation_id: payload.conversation_id,
                project_id: payload.project_id,
                agent_id: Some(node_permission.agent_id),
                ..Default::default()
            }),
        )
        .await?;
    }
    for node_permission in &node_permissions {
        capability_authz::require_agent_version_capabilities(
            &state,
            &ctx,
            payload.tenant_id,
            node_permission.agent_version_id,
            AuthzContext {
                conversation_id: payload.conversation_id,
                project_id: payload.project_id,
                agent_id: Some(node_permission.agent_id),
                ..Default::default()
            },
        )
        .await?;
    }

    let trace_id = Uuid::new_v4().to_string();
    let input = payload.input.unwrap_or_else(|| json!({}));
    let mut tx = state.connect_pool.begin().await?;
    let conversation_id = match payload.conversation_id {
        Some(conversation_id) => Some(conversation_id),
        None => {
            let row = sqlx::query(
                r#"
                INSERT INTO conversations (
                    tenant_id, created_by_user_id, project_id, title, metadata
                )
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id
                "#,
            )
            .bind(payload.tenant_id)
            .bind(ctx.platform_user_id)
            .bind(payload.project_id)
            .bind("Workflow run")
            .bind(json!({ "workflow_version_id": payload.workflow_version_id }))
            .fetch_one(&mut *tx)
            .await?;
            Some(row.try_get("id")?)
        }
    };

    let row = sqlx::query(
        r#"
        INSERT INTO workflow_runs (
            tenant_id, workflow_version_id, conversation_id, project_id,
            created_by_user_id, status, input, trace_id
        )
        VALUES ($1, $2, $3, $4, $5, 'queued', $6, $7)
        RETURNING id, tenant_id, workflow_version_id, conversation_id, project_id,
                  status, trace_id, input, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.workflow_version_id)
    .bind(conversation_id)
    .bind(payload.project_id)
    .bind(ctx.platform_user_id)
    .bind(&input)
    .bind(trace_id)
    .fetch_one(&mut *tx)
    .await?;

    let workflow_run = workflow_run_from_row(row)?;
    for (node_key, node) in workflow_plan::nodes(&compiled_plan)? {
        let execution_policy = workflow_plan::node_execution_policy(&node)?;
        sqlx::query(
            r#"
            INSERT INTO workflow_node_runs (
                tenant_id, workflow_run_id, node_key, status, input,
                max_attempts, backoff_sec, timeout_sec
            )
            VALUES ($1, $2, $3, 'pending', $4, $5, $6, $7)
            "#,
        )
        .bind(payload.tenant_id)
        .bind(workflow_run.id)
        .bind(&node_key)
        .bind(json!({
            "workflow_input": input,
            "node": node
        }))
        .bind(execution_policy.max_attempts)
        .bind(execution_policy.backoff_sec)
        .bind(execution_policy.timeout_sec)
        .execute(&mut *tx)
        .await?;
    }

    for (from, to) in workflow_plan::edges(&compiled_plan)? {
        sqlx::query(
            r#"
            INSERT INTO workflow_run_dependencies (
                workflow_run_id, from_node_key, to_node_key
            )
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(workflow_run.id)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
    }

    let event = if let Some(conversation_id) = workflow_run.conversation_id {
        Some(
            event_store::insert_event_tx(
                &mut tx,
                workflow_run.tenant_id,
                conversation_id,
                None,
                RunEventInput {
                    event_id: Some(format!("workflow.run.queued.{}", workflow_run.id)),
                    event_type: "workflow.run.queued".to_string(),
                    payload: Some(json!({
                        "workflow_run_id": workflow_run.id,
                        "workflow_version_id": workflow_run.workflow_version_id,
                        "status": workflow_run.status
                    })),
                    trace_id: Some(workflow_run.trace_id.clone()),
                },
            )
            .await?,
        )
    } else {
        None
    };

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    if let Some(event) = event {
        event_store::publish_single_event(&state, &event).await;
    }

    let ticked = tick_workflow_run(&state, workflow_run.id).await?;
    Ok(Json(ticked.workflow_run))
}

pub async fn get_workflow_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_run_id): Path<Uuid>,
    Query(query): Query<WorkflowRunDetailQuery>,
) -> Result<Json<WorkflowRunDetailResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    load_workflow_run_detail(&state, query.tenant_id, workflow_run_id)
        .await
        .map(Json)
}

pub async fn list_workflow_node_runs(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_run_id): Path<Uuid>,
    Query(query): Query<WorkflowRunDetailQuery>,
) -> Result<Json<Vec<WorkflowNodeRunResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM workflow_runs
            WHERE id = $1 AND tenant_id = $2
        ) AS exists
        "#,
    )
    .bind(workflow_run_id)
    .bind(query.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if !exists {
        return Err(AppError::NotFound("workflow run not found".to_string()));
    }
    load_workflow_node_runs(&state, workflow_run_id)
        .await
        .map(Json)
}

pub async fn cancel_workflow_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(workflow_run_id): Path<Uuid>,
) -> Result<Json<WorkflowRunResponse>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, workflow_version_id, conversation_id, project_id,
               status, trace_id, input, created_at, updated_at
        FROM workflow_runs
        WHERE id = $1
        "#,
    )
    .bind(workflow_run_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow run not found".to_string()))?;
    let workflow_run = workflow_run_from_row(row)?;

    ensure_tenant_member(
        &state.connect_pool,
        workflow_run.tenant_id,
        ctx.platform_user_id,
    )
    .await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        workflow_run.tenant_id,
        "cancel",
        "workflow",
        workflow_run
            .workflow_version_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| workflow_run.id.to_string()),
        Some(AuthzContext {
            conversation_id: workflow_run.conversation_id,
            workflow_run_id: Some(workflow_run.id),
            project_id: workflow_run.project_id,
            ..Default::default()
        }),
    )
    .await?;

    if workflow_run.status == "cancelled" {
        return Ok(Json(workflow_run));
    }
    if matches!(workflow_run.status.as_str(), "completed" | "failed") {
        return Err(AppError::Conflict(
            "terminal workflow run cannot be cancelled".to_string(),
        ));
    }

    let mut tx = state.connect_pool.begin().await?;
    let child_run_rows = sqlx::query(
        r#"
        SELECT DISTINCT n.agent_run_id
        FROM workflow_node_runs n
        JOIN runs r ON r.id = n.agent_run_id
        WHERE n.workflow_run_id = $1
          AND n.agent_run_id IS NOT NULL
          AND n.status NOT IN ('completed', 'failed', 'cancelled', 'blocked', 'skipped')
          AND r.status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(workflow_run.id)
    .fetch_all(&mut *tx)
    .await?;
    let child_run_ids = child_run_rows
        .into_iter()
        .map(|row| row.try_get("agent_run_id"))
        .collect::<Result<Vec<Uuid>, sqlx::Error>>()?;

    sqlx::query(
        r#"
        UPDATE workflow_node_runs
        SET status = 'cancelled',
            completed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE workflow_run_id = $1
          AND status NOT IN ('completed', 'failed', 'cancelled', 'blocked', 'skipped')
        "#,
    )
    .bind(workflow_run.id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'cancelled',
            completed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ANY($1)
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(&child_run_ids)
    .execute(&mut *tx)
    .await?;

    let updated_row = sqlx::query(
        r#"
        UPDATE workflow_runs
        SET status = 'cancelled', updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
        RETURNING id, tenant_id, workflow_version_id, conversation_id, project_id,
                  status, trace_id, input, created_at, updated_at
        "#,
    )
    .bind(workflow_run.id)
    .fetch_one(&mut *tx)
    .await?;
    let updated = workflow_run_from_row(updated_row)?;

    let mut events_to_publish = Vec::new();
    if let Some(conversation_id) = updated.conversation_id {
        events_to_publish.push(
            event_store::insert_event_tx(
                &mut tx,
                updated.tenant_id,
                conversation_id,
                None,
                RunEventInput {
                    event_id: Some(format!("workflow.run.cancelled.{}", updated.id)),
                    event_type: "workflow.run.cancelled".to_string(),
                    payload: Some(json!({
                        "workflow_run_id": updated.id,
                        "status": updated.status
                    })),
                    trace_id: Some(updated.trace_id.clone()),
                },
            )
            .await?,
        );

        for child_run_id in &child_run_ids {
            events_to_publish.push(
                event_store::insert_event_tx(
                    &mut tx,
                    updated.tenant_id,
                    conversation_id,
                    Some(*child_run_id),
                    RunEventInput {
                        event_id: Some(format!("run.cancelled.workflow.{}", child_run_id)),
                        event_type: "run.cancelled".to_string(),
                        payload: Some(json!({
                            "run_id": child_run_id,
                            "workflow_run_id": updated.id,
                            "reason": "workflow_cancelled"
                        })),
                        trace_id: Some(updated.trace_id.clone()),
                    },
                )
                .await?,
            );
        }
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    for event in &events_to_publish {
        event_store::publish_single_event(&state, event).await;
    }
    if let Some(conversation_id) = updated.conversation_id {
        for child_run_id in &child_run_ids {
            if let Err(err) = state
                .agent_runtime_client
                .cancel_run(
                    *child_run_id,
                    &CancelRunRequest {
                        tenant_id: updated.tenant_id,
                        conversation_id,
                        trace_id: Some(updated.trace_id.clone()),
                        reason: "workflow_cancelled".to_string(),
                    },
                )
                .await
            {
                warn!(
                    "failed to propagate workflow cancel {} to child run {}: {}",
                    updated.id, child_run_id, err
                );
            }
        }
    }

    Ok(Json(updated))
}

pub async fn internal_workflow_run_tick(
    State(state): State<AppState>,
    Path(workflow_run_id): Path<Uuid>,
) -> Result<Json<WorkflowTickResponse>, AppError> {
    tick_workflow_run(&state, workflow_run_id).await.map(Json)
}

pub(crate) async fn tick_workflow_run(
    state: &AppState,
    workflow_run_id: Uuid,
) -> Result<WorkflowTickResponse, AppError> {
    let mut tx = state.connect_pool.begin().await?;
    let workflow_row = sqlx::query(
        r#"
        SELECT wr.id, wr.tenant_id, wr.workflow_version_id, wr.conversation_id,
               wr.project_id, wr.created_by_user_id, wr.status, wr.input, wr.trace_id,
               wr.created_at, wr.updated_at, wv.compiled_plan
        FROM workflow_runs wr
        JOIN workflow_versions wv ON wv.id = wr.workflow_version_id
        WHERE wr.id = $1
        FOR UPDATE OF wr
        "#,
    )
    .bind(workflow_run_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow run not found".to_string()))?;

    let actor_user_id: Option<Uuid> = workflow_row.try_get("created_by_user_id")?;
    let actor_user_id = actor_user_id.ok_or_else(|| {
        AppError::InvalidInput("workflow run is missing created_by_user_id".to_string())
    })?;
    let stored_compiled_plan: Value = workflow_row.try_get("compiled_plan")?;
    let workflow_run = workflow_run_from_row(workflow_row)?;
    if matches!(
        workflow_run.status.as_str(),
        "completed" | "failed" | "cancelled"
    ) {
        tx.commit()
            .await
            .map_err(|_| AppError::DatabaseTransaction)?;
        return Ok(WorkflowTickResponse {
            workflow_run,
            dispatched_runs: Vec::new(),
        });
    }

    let compiled_plan = workflow_compile::compile_plan(
        &state.connect_pool,
        workflow_run.tenant_id,
        &stored_compiled_plan,
    )
    .await?;
    workflow_plan::validate(&compiled_plan)?;
    let node_by_key = workflow_plan::node_map(&compiled_plan)?;

    sqlx::query(
        r#"
        WITH timed_out_nodes AS (
            UPDATE workflow_node_runs
            SET status = CASE
                    WHEN attempts < max_attempts THEN 'pending'
                    ELSE 'failed'
                END,
                agent_run_id = CASE
                    WHEN attempts < max_attempts THEN NULL
                    ELSE agent_run_id
                END,
                not_before = CASE
                    WHEN attempts < max_attempts THEN CURRENT_TIMESTAMP + (backoff_sec * INTERVAL '1 second')
                    ELSE not_before
                END,
                completed_at = CASE
                    WHEN attempts < max_attempts THEN NULL
                    ELSE CURRENT_TIMESTAMP
                END,
                last_error = 'node execution timed out',
                updated_at = CURRENT_TIMESTAMP
            WHERE workflow_run_id = $1
              AND status IN ('running', 'waiting_approval', 'waiting_user_input')
              AND timeout_sec IS NOT NULL
              AND started_at IS NOT NULL
              AND started_at + (timeout_sec * INTERVAL '1 second') <= CURRENT_TIMESTAMP
            RETURNING agent_run_id
        )
        UPDATE runs
        SET status = 'failed',
            completed_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id IN (
            SELECT agent_run_id
            FROM timed_out_nodes
            WHERE agent_run_id IS NOT NULL
        )
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(workflow_run_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE workflow_node_runs node
        SET status = 'blocked', updated_at = CURRENT_TIMESTAMP
        WHERE node.workflow_run_id = $1
          AND node.status IN ('pending', 'ready')
          AND EXISTS (
              SELECT 1
              FROM workflow_run_dependencies dep
              JOIN workflow_node_runs upstream
                ON upstream.workflow_run_id = dep.workflow_run_id
               AND upstream.node_key = dep.from_node_key
              WHERE dep.workflow_run_id = node.workflow_run_id
                AND dep.to_node_key = node.node_key
                AND upstream.status IN ('failed', 'cancelled', 'blocked')
          )
        "#,
    )
    .bind(workflow_run_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE workflow_node_runs node
        SET status = 'ready', updated_at = CURRENT_TIMESTAMP
        WHERE node.workflow_run_id = $1
          AND node.status = 'pending'
          AND COALESCE(node.not_before, CURRENT_TIMESTAMP) <= CURRENT_TIMESTAMP
          AND NOT EXISTS (
              SELECT 1
              FROM workflow_run_dependencies dep
              JOIN workflow_node_runs upstream
                ON upstream.workflow_run_id = dep.workflow_run_id
               AND upstream.node_key = dep.from_node_key
              WHERE dep.workflow_run_id = node.workflow_run_id
                AND dep.to_node_key = node.node_key
                AND upstream.status <> 'completed'
          )
        "#,
    )
    .bind(workflow_run_id)
    .execute(&mut *tx)
    .await?;

    let mut ready_rows = sqlx::query(
        r#"
        SELECT id, node_key, attempts
        FROM workflow_node_runs
        WHERE workflow_run_id = $1 AND status = 'ready'
        ORDER BY created_at ASC
        "#,
    )
    .bind(workflow_run_id)
    .fetch_all(&mut *tx)
    .await?;

    if let Some(limit) = workflow_plan::concurrency_limit(&compiled_plan)? {
        let active_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM workflow_node_runs
            WHERE workflow_run_id = $1
              AND status IN ('queued', 'running', 'waiting_approval', 'waiting_user_input')
            "#,
        )
        .bind(workflow_run_id)
        .fetch_one(&mut *tx)
        .await?;
        let remaining_capacity = limit.saturating_sub(active_count);
        ready_rows.truncate(usize::try_from(remaining_capacity).unwrap_or(usize::MAX));
    }

    let mut dispatches = Vec::new();
    let mut dispatched_runs = Vec::new();
    let mut events_to_publish = Vec::new();
    for ready_row in ready_rows {
        let node_run_id: Uuid = ready_row.try_get("id")?;
        let node_key: String = ready_row.try_get("node_key")?;
        let attempts: i32 = ready_row.try_get("attempts")?;
        let node = node_by_key.get(&node_key).ok_or_else(|| {
            AppError::InvalidInput(format!("compiled_plan node missing for key {node_key}"))
        })?;
        let agent_version_id = workflow_plan::node_agent_version_id(node)?;
        let agent_row = sqlx::query(
            r#"
            SELECT agent_id
            FROM agent_versions
            WHERE id = $1 AND tenant_id = $2 AND status = 'published'
            "#,
        )
        .bind(agent_version_id)
        .bind(workflow_run.tenant_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            AppError::InvalidInput(format!(
                "node {node_key} references an unpublished agent_version_id"
            ))
        })?;
        let agent_id: Uuid = agent_row.try_get("agent_id")?;
        let node_permission_snapshot =
            workflow_compile::node_permission_snapshot(&compiled_plan, &node_key)?;
        let run_id = Uuid::new_v4();
        let thread_id = format!(
            "tenant:{}:workflow:{}:node:{}:attempt:{}",
            workflow_run.tenant_id,
            workflow_run.id,
            node_key,
            attempts + 1
        );
        let input = build_workflow_node_input(&mut tx, &workflow_run, &node_key, node).await?;
        let snapshot = run_snapshot::compile_workflow_node_run_snapshot(
            &state.connect_pool,
            WorkflowNodeRunSnapshotRequest {
                tenant_id: workflow_run.tenant_id,
                run_id,
                workflow_run_id: workflow_run.id,
                workflow_version_id: workflow_run.workflow_version_id,
                conversation_id: workflow_run.conversation_id,
                workspace_id: None,
                node_run_id,
                node_key: &node_key,
                node,
                node_permission_snapshot,
                actor_user_id,
                agent_id,
                agent_version_id,
                project_id: workflow_run.project_id,
                thread_id: thread_id.clone(),
            },
        )
        .await?;
        let run_scope_snapshot = snapshot
            .get("workspace")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let row = sqlx::query(
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
        .bind(workflow_run.tenant_id)
        .bind(workflow_run.conversation_id.ok_or_else(|| {
            AppError::InvalidInput("workflow run has no conversation_id".to_string())
        })?)
        .bind(Option::<Uuid>::None)
        .bind(agent_id)
        .bind(agent_version_id)
        .bind(workflow_run.project_id)
        .bind(actor_user_id)
        .bind(format!(
            "workflow:{}:node:{}:attempt:{}",
            workflow_run.id,
            node_key,
            attempts + 1
        ))
        .bind(&input)
        .bind(&snapshot)
        .bind(run_scope_snapshot)
        .bind(LOCAL_POLICY_VERSION)
        .bind(LOCAL_RISK_POLICY_VERSION)
        .bind(workflow_run.trace_id.clone())
        .bind(&thread_id)
        .fetch_one(&mut *tx)
        .await?;
        let run = run_from_row(row)?;

        sqlx::query(
            r#"
            UPDATE workflow_node_runs
            SET status = 'queued',
                attempts = attempts + 1,
                agent_run_id = $1,
                input = $2,
                not_before = NULL,
                started_at = NULL,
                completed_at = NULL,
                last_error = NULL,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $3
            "#,
        )
        .bind(run.id)
        .bind(&input)
        .bind(node_run_id)
        .execute(&mut *tx)
        .await?;

        if let Some(conversation_id) = workflow_run.conversation_id {
            let event = event_store::insert_event_tx(
                &mut tx,
                workflow_run.tenant_id,
                conversation_id,
                Some(run.id),
                RunEventInput {
                    event_id: Some(format!(
                        "workflow.node.queued.{}.{}",
                        workflow_run.id, node_key
                    )),
                    event_type: "workflow.node.queued".to_string(),
                    payload: Some(json!({
                        "workflow_run_id": workflow_run.id,
                        "node_key": node_key,
                        "run_id": run.id
                    })),
                    trace_id: Some(workflow_run.trace_id.clone()),
                },
            )
            .await?;
            events_to_publish.push(event);
        }

        dispatched_runs.push(run.id);
        dispatches.push(PendingWorkflowDispatch {
            agent_id,
            project_id: workflow_run.project_id,
            dispatch: DispatchRunRequest {
                tenant_id: workflow_run.tenant_id,
                conversation_id: run.conversation_id,
                run_id: run.id,
                trace_id: run.trace_id,
                input,
                run_config_snapshot: snapshot,
            },
        });
    }

    let new_status = workflow_status_in_tx(&mut tx, workflow_run_id).await?;
    let updated_row = sqlx::query(
        r#"
        UPDATE workflow_runs
        SET status = $1, updated_at = CURRENT_TIMESTAMP
        WHERE id = $2
        RETURNING id, tenant_id, workflow_version_id, conversation_id, project_id,
                  status, trace_id, input, created_at, updated_at
        "#,
    )
    .bind(new_status)
    .bind(workflow_run_id)
    .fetch_one(&mut *tx)
    .await?;
    let updated_workflow_run = workflow_run_from_row(updated_row)?;

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    for event in &events_to_publish {
        event_store::publish_single_event(state, event).await;
    }
    for pending in &mut dispatches {
        memory_injection::inject_memory_context_for_run(
            state,
            memory_injection::MemoryInjectionRequest {
                actor: ActorRef {
                    user_id: actor_user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                tenant_id: pending.dispatch.tenant_id,
                run_id: pending.dispatch.run_id,
                agent_id: Some(pending.agent_id),
                project_id: pending.project_id,
            },
            &pending.dispatch.input,
            &mut pending.dispatch.run_config_snapshot,
        )
        .await?;
        if let Err(err) = secret_resolver::attach_llm_runtime_credential(
            state,
            pending.dispatch.tenant_id,
            pending.dispatch.run_id,
            &mut pending.dispatch.run_config_snapshot,
        )
        .await
        {
            let _workflow_run_id = run_lifecycle::mark_dispatch_failed(
                state,
                pending.dispatch.tenant_id,
                pending.dispatch.conversation_id,
                pending.dispatch.run_id,
                Some(pending.dispatch.trace_id.clone()),
                &err.to_string(),
            )
            .await?;
            return Err(err);
        }
    }
    for pending in &dispatches {
        let dispatch = &pending.dispatch;
        if let Err(err) = state.agent_runtime_client.dispatch_run(dispatch).await {
            let _workflow_run_id = run_lifecycle::mark_dispatch_failed(
                state,
                dispatch.tenant_id,
                dispatch.conversation_id,
                dispatch.run_id,
                Some(dispatch.trace_id.clone()),
                &err.to_string(),
            )
            .await?;
            return Err(err);
        }
    }

    Ok(WorkflowTickResponse {
        workflow_run: updated_workflow_run,
        dispatched_runs,
    })
}

async fn build_workflow_node_input(
    tx: &mut Transaction<'_, Postgres>,
    workflow_run: &WorkflowRunResponse,
    node_key: &str,
    node: &Value,
) -> Result<Value, AppError> {
    let upstream_rows = sqlx::query(
        r#"
        SELECT dep.from_node_key, upstream.output
        FROM workflow_run_dependencies dep
        JOIN workflow_node_runs upstream
          ON upstream.workflow_run_id = dep.workflow_run_id
         AND upstream.node_key = dep.from_node_key
        WHERE dep.workflow_run_id = $1 AND dep.to_node_key = $2
        ORDER BY dep.from_node_key ASC
        "#,
    )
    .bind(workflow_run.id)
    .bind(node_key)
    .fetch_all(&mut **tx)
    .await?;

    let mut upstream_outputs = serde_json::Map::new();
    for row in upstream_rows {
        let key: String = row.try_get("from_node_key")?;
        let output: Option<Value> = row.try_get("output")?;
        upstream_outputs.insert(key, output.unwrap_or_else(|| json!({})));
    }

    workflow_mapping::build_node_input_envelope(
        workflow_run.id,
        workflow_run.workflow_version_id,
        &workflow_run.input,
        node_key,
        node,
        upstream_outputs,
    )
}

async fn workflow_status_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workflow_run_id: Uuid,
) -> Result<String, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT status, COUNT(*) AS count
        FROM workflow_node_runs
        WHERE workflow_run_id = $1
        GROUP BY status
        "#,
    )
    .bind(workflow_run_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut counts = HashMap::<String, i64>::new();
    let mut total = 0_i64;
    for row in rows {
        let status: String = row.try_get("status")?;
        let count: i64 = row.try_get("count")?;
        total += count;
        counts.insert(status, count);
    }

    Ok(workflow_runtime::workflow_status_from_counts(
        &counts, total,
    ))
}

async fn load_workflow_design(
    state: &AppState,
    tenant_id: Uuid,
    workflow_design_id: Uuid,
) -> Result<WorkflowDesignResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, owner_user_id, name, description, design,
               status, created_at, updated_at
        FROM workflow_designs
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workflow_design_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow design not found".to_string()))?;
    workflow_design_from_row(row)
}

async fn load_workflow_version(
    state: &AppState,
    tenant_id: Uuid,
    workflow_version_id: Uuid,
) -> Result<VersionResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, workflow_design_id AS parent_id, version_label,
               compiled_plan AS snapshot, policy_version, status, created_at
        FROM workflow_versions
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(workflow_version_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow version not found".to_string()))?;
    version_from_row(row)
}

async fn load_workflow_run_detail(
    state: &AppState,
    tenant_id: Uuid,
    workflow_run_id: Uuid,
) -> Result<WorkflowRunDetailResponse, AppError> {
    let run_row = sqlx::query(
        r#"
        SELECT id, tenant_id, workflow_version_id, conversation_id, project_id,
               status, trace_id, input, created_at, updated_at
        FROM workflow_runs
        WHERE id = $1 AND tenant_id = $2
        "#,
    )
    .bind(workflow_run_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("workflow run not found".to_string()))?;
    let run = workflow_run_from_row(run_row)?;

    let version = if let Some(workflow_version_id) = run.workflow_version_id {
        Some(load_workflow_version(state, tenant_id, workflow_version_id).await?)
    } else {
        None
    };
    let design = if let Some(version) = &version {
        Some(load_workflow_design(state, tenant_id, version.parent_id).await?)
    } else {
        None
    };
    let node_runs = load_workflow_node_runs(state, workflow_run_id).await?;
    let dependencies = load_workflow_run_dependencies(state, workflow_run_id).await?;

    Ok(WorkflowRunDetailResponse {
        run,
        version,
        design,
        node_runs,
        dependencies,
    })
}

async fn load_workflow_node_runs(
    state: &AppState,
    workflow_run_id: Uuid,
) -> Result<Vec<WorkflowNodeRunResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, workflow_run_id, node_key, agent_run_id, status,
               attempts, max_attempts, backoff_sec, timeout_sec, not_before,
               started_at, completed_at, input, output, last_error, created_at, updated_at
        FROM workflow_node_runs
        WHERE workflow_run_id = $1
        ORDER BY created_at ASC, node_key ASC
        "#,
    )
    .bind(workflow_run_id)
    .fetch_all(&state.connect_pool)
    .await?;
    rows.into_iter()
        .map(workflow_node_run_from_row)
        .collect::<Result<Vec<_>, AppError>>()
}

async fn load_workflow_run_dependencies(
    state: &AppState,
    workflow_run_id: Uuid,
) -> Result<Vec<WorkflowRunDependencyResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT from_node_key, to_node_key, created_at
        FROM workflow_run_dependencies
        WHERE workflow_run_id = $1
        ORDER BY from_node_key ASC, to_node_key ASC
        "#,
    )
    .bind(workflow_run_id)
    .fetch_all(&state.connect_pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(WorkflowRunDependencyResponse {
                from_node_key: row.try_get("from_node_key")?,
                to_node_key: row.try_get("to_node_key")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .collect()
}

fn workflow_design_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<WorkflowDesignResponse, AppError> {
    Ok(WorkflowDesignResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        design: row.try_get("design")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn workflow_node_run_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<WorkflowNodeRunResponse, AppError> {
    Ok(WorkflowNodeRunResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        workflow_run_id: row.try_get("workflow_run_id")?,
        node_key: row.try_get("node_key")?,
        agent_run_id: row.try_get("agent_run_id")?,
        status: row.try_get("status")?,
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        backoff_sec: row.try_get("backoff_sec")?,
        timeout_sec: row.try_get("timeout_sec")?,
        not_before: row.try_get("not_before")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        input: row.try_get("input")?,
        output: row.try_get("output")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::json;
    use sqlx::{PgPool, Row, postgres::PgPoolOptions};

    use crate::{
        configuration::{AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings},
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
    };

    use super::*;

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn tick_dispatches_three_node_dag_in_dependency_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, conversation_id) = seed_context(&state.connect_pool).await?;
        let first_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "root").await?;
        let second_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "middle").await?;
        let third_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "leaf").await?;
        let workflow_run_id = seed_workflow_run(
            &state.connect_pool,
            tenant_id,
            user_id,
            conversation_id,
            json!({
                "concurrency_limit": 4,
                "nodes": [
                    agent_node("a", first_agent_version_id),
                    agent_node("b", second_agent_version_id),
                    agent_node("c", third_agent_version_id)
                ],
                "edges": [
                    {"from": "a", "to": "b"},
                    {"from": "b", "to": "c"}
                ]
            }),
        )
        .await?;

        let first_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(first_tick.dispatched_runs.len(), 1);
        let first_snapshot =
            run_snapshot(&state.connect_pool, first_tick.dispatched_runs[0]).await?;
        let first_node_permission = first_snapshot
            .get("workflow")
            .and_then(|workflow| workflow.get("node_permission_snapshot"))
            .expect("node permission snapshot");
        assert_eq!(
            first_node_permission
                .get("agent_version_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            Some(first_agent_version_id.to_string())
        );
        assert_eq!(
            first_node_permission
                .get("required_permissions")
                .and_then(Value::as_array)
                .and_then(|permissions| permissions.first())
                .and_then(|permission| permission.get("resource_type"))
                .and_then(Value::as_str),
            Some("agent")
        );
        assert_eq!(
            first_snapshot
                .pointer("/model/model_name")
                .and_then(Value::as_str),
            Some("fake-model")
        );
        assert!(
            first_snapshot
                .get("tools")
                .and_then(Value::as_array)
                .is_some()
        );
        assert_node_statuses(
            &state.connect_pool,
            workflow_run_id,
            &[("a", "queued"), ("b", "pending"), ("c", "pending")],
        )
        .await?;

        set_node_terminal(
            &state.connect_pool,
            workflow_run_id,
            "a",
            "completed",
            json!({"value": "a"}),
        )
        .await?;
        let second_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(second_tick.dispatched_runs.len(), 1);
        assert_node_statuses(
            &state.connect_pool,
            workflow_run_id,
            &[("a", "completed"), ("b", "queued"), ("c", "pending")],
        )
        .await?;

        set_node_terminal(
            &state.connect_pool,
            workflow_run_id,
            "b",
            "completed",
            json!({"value": "b"}),
        )
        .await?;
        let third_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(third_tick.dispatched_runs.len(), 1);
        assert_node_statuses(
            &state.connect_pool,
            workflow_run_id,
            &[("a", "completed"), ("b", "completed"), ("c", "queued")],
        )
        .await?;

        set_node_terminal(
            &state.connect_pool,
            workflow_run_id,
            "c",
            "completed",
            json!({"value": "c"}),
        )
        .await?;
        let final_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert!(final_tick.dispatched_runs.is_empty());
        assert_eq!(final_tick.workflow_run.status, "completed");

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn compile_plan_rejects_unpublished_node_agent_version()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, _, _) = seed_context(&state.connect_pool).await?;
        let plan = json!({
            "nodes": [agent_node("missing", Uuid::new_v4())],
            "edges": []
        });

        let err = workflow_compile::compile_plan(&state.connect_pool, tenant_id, &plan)
            .await
            .expect_err("missing agent version should be rejected");

        assert!(err.to_string().contains("unpublished agent_version_id"));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn tick_respects_workflow_concurrency_limit() -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, conversation_id) = seed_context(&state.connect_pool).await?;
        let first_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "parallel-a").await?;
        let second_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "parallel-b").await?;
        let workflow_run_id = seed_workflow_run(
            &state.connect_pool,
            tenant_id,
            user_id,
            conversation_id,
            json!({
                "concurrency_limit": 1,
                "nodes": [
                    agent_node("a", first_agent_version_id),
                    agent_node("b", second_agent_version_id)
                ],
                "edges": []
            }),
        )
        .await?;

        let first_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(first_tick.dispatched_runs.len(), 1);
        let statuses = node_statuses(&state.connect_pool, workflow_run_id).await?;
        assert_eq!(
            statuses
                .values()
                .filter(|status| status.as_str() == "queued")
                .count(),
            1
        );
        assert_eq!(
            statuses
                .values()
                .filter(|status| status.as_str() == "ready")
                .count(),
            1
        );

        let second_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert!(second_tick.dispatched_runs.is_empty());

        let queued_node = statuses
            .iter()
            .find_map(|(node_key, status)| (status == "queued").then_some(node_key.clone()))
            .expect("one queued node");
        set_node_terminal(
            &state.connect_pool,
            workflow_run_id,
            &queued_node,
            "completed",
            json!({"value": queued_node}),
        )
        .await?;
        let third_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(third_tick.dispatched_runs.len(), 1);

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn tick_blocks_downstream_nodes_after_upstream_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, conversation_id) = seed_context(&state.connect_pool).await?;
        let upstream_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "upstream").await?;
        let downstream_agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "downstream").await?;
        let workflow_run_id = seed_workflow_run(
            &state.connect_pool,
            tenant_id,
            user_id,
            conversation_id,
            json!({
                "nodes": [
                    agent_node("a", upstream_agent_version_id),
                    agent_node("b", downstream_agent_version_id)
                ],
                "edges": [{"from": "a", "to": "b"}]
            }),
        )
        .await?;

        let first_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(first_tick.dispatched_runs.len(), 1);
        set_node_terminal(
            &state.connect_pool,
            workflow_run_id,
            "a",
            "failed",
            json!({"error": "boom"}),
        )
        .await?;

        let second_tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert!(second_tick.dispatched_runs.is_empty());
        assert_eq!(second_tick.workflow_run.status, "failed");
        assert_node_statuses(
            &state.connect_pool,
            workflow_run_id,
            &[("a", "failed"), ("b", "blocked")],
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn tick_injects_memory_context_for_workflow_node_run()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, conversation_id) = seed_context(&state.connect_pool).await?;
        let agent_version_id =
            seed_agent_version(&state.connect_pool, tenant_id, user_id, "memory-node").await?;
        let agent_id = agent_id_for_version(&state.connect_pool, agent_version_id).await?;
        let memory_id = seed_approved_memory(
            &state.connect_pool,
            tenant_id,
            user_id,
            agent_id,
            "sales revenue data should be reused by workflow nodes",
        )
        .await?;
        let workflow_run_id = seed_workflow_run(
            &state.connect_pool,
            tenant_id,
            user_id,
            conversation_id,
            json!({
                "nodes": [
                    agent_node_with_memory_query("a", agent_version_id, "sales revenue")
                ],
                "edges": []
            }),
        )
        .await?;

        let tick = tick_workflow_run(&state, workflow_run_id).await?;
        assert_eq!(tick.dispatched_runs.len(), 1);
        let child_run_id = tick.dispatched_runs[0];
        let snapshot = run_snapshot(&state.connect_pool, child_run_id).await?;
        let memories = snapshot
            .get("memory_context")
            .and_then(Value::as_array)
            .expect("memory_context array");
        assert_eq!(memories.len(), 1);
        assert_eq!(
            memories[0]
                .get("memory_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            Some(memory_id.to_string())
        );
        assert_eq!(
            snapshot
                .pointer("/memory_context_meta/source")
                .and_then(Value::as_str),
            Some("memory_keyword_search")
        );

        let access_logged: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM memory_access_logs
                WHERE tenant_id = $1
                  AND memory_id = $2
                  AND user_id = $3
                  AND run_id = $4
                  AND action = 'memory_keyword_search'
            )
            "#,
        )
        .bind(tenant_id)
        .bind(memory_id)
        .bind(user_id)
        .bind(child_run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert!(access_logged);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;

        Ok(())
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
        })
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }

    fn agent_node(node_key: &str, agent_version_id: Uuid) -> serde_json::Value {
        json!({
            "node_key": node_key,
            "node_type": "agent_task",
            "agent_version_id": agent_version_id
        })
    }

    fn agent_node_with_memory_query(
        node_key: &str,
        agent_version_id: Uuid,
        query: &str,
    ) -> serde_json::Value {
        json!({
            "node_key": node_key,
            "node_type": "agent_task",
            "agent_version_id": agent_version_id,
            "memory_retrieval": {
                "query": query,
                "limit": 3
            }
        })
    }

    async fn seed_context(pool: &PgPool) -> Result<(Uuid, Uuid, Uuid), sqlx::Error> {
        let suffix = Uuid::new_v4();
        let tenant_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tenants (name, slug, metadata)
            VALUES ($1, $2, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(format!("Workflow Test {suffix}"))
        .bind(format!("workflow-test-{suffix}"))
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
        .bind(format!("test-subject-{suffix}"))
        .bind(format!("test-user-{suffix}"))
        .fetch_one(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO user_tenant_memberships (tenant_id, user_id, role)
            VALUES ($1, $2, 'admin')
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(pool)
        .await?;

        let conversation_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO conversations (tenant_id, created_by_user_id, title, metadata)
            VALUES ($1, $2, 'Workflow test', '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_one(pool)
        .await?;

        Ok((tenant_id, user_id, conversation_id))
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn seed_agent_version(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        name: &str,
    ) -> Result<Uuid, sqlx::Error> {
        let suffix = Uuid::new_v4();
        let model_profile_id = seed_model_profile(pool, tenant_id, &suffix).await?;
        let agent_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO agents (tenant_id, owner_user_id, name, status)
            VALUES ($1, $2, $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("{name}-{suffix}"))
        .fetch_one(pool)
        .await?;

        sqlx::query_scalar(
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
        .bind(format!("v-{suffix}"))
        .bind(json!({
            "model_profile_id": model_profile_id,
            "agent": {
                "system_prompt": format!("workflow test agent {name}")
            }
        }))
        .fetch_one(pool)
        .await
    }

    async fn agent_id_for_version(
        pool: &PgPool,
        agent_version_id: Uuid,
    ) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT agent_id
            FROM agent_versions
            WHERE id = $1
            "#,
        )
        .bind(agent_version_id)
        .fetch_one(pool)
        .await
    }

    async fn seed_approved_memory(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        agent_id: Uuid,
        content: &str,
    ) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            INSERT INTO memory_items (
                tenant_id, user_id, agent_id, layer, content, content_hash,
                confidence, status, visibility, retention_policy, sensitivity
            )
            VALUES ($1, $2, $3, 'semantic', $4, $5, 0.9, 'approved', 'private', 'default', 'normal')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(agent_id)
        .bind(content)
        .bind(format!("test-hash-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
    }

    async fn seed_model_profile(
        pool: &PgPool,
        tenant_id: Uuid,
        suffix: &Uuid,
    ) -> Result<Uuid, sqlx::Error> {
        let provider_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO llm_providers (tenant_id, provider_key, display_name, base_url)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind("test")
        .bind(format!("Test Provider {suffix}"))
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
        .bind(format!("test-profile-{suffix}"))
        .bind("fake-model")
        .bind(1024_i64)
        .bind(0.0_f64)
        .fetch_one(pool)
        .await
    }

    async fn seed_workflow_run(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        conversation_id: Uuid,
        compiled_plan: serde_json::Value,
    ) -> Result<Uuid, Box<dyn std::error::Error>> {
        let compiled_plan = workflow_compile::compile_plan(pool, tenant_id, &compiled_plan).await?;
        let workflow_design_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO workflow_designs (tenant_id, owner_user_id, name, design)
            VALUES ($1, $2, $3, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(format!("workflow-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;

        let workflow_version_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO workflow_versions (
                tenant_id, workflow_design_id, version_label, compiled_plan, status
            )
            VALUES ($1, $2, $3, $4, 'published')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_design_id)
        .bind(format!("v-{}", Uuid::new_v4()))
        .bind(&compiled_plan)
        .fetch_one(pool)
        .await?;

        let workflow_run_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO workflow_runs (
                tenant_id, workflow_version_id, conversation_id, created_by_user_id,
                status, input, trace_id
            )
            VALUES ($1, $2, $3, $4, 'queued', $5, $6)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(workflow_version_id)
        .bind(conversation_id)
        .bind(user_id)
        .bind(json!({"seed": true}))
        .bind(format!("trace-{}", Uuid::new_v4()))
        .fetch_one(pool)
        .await?;

        for (node_key, node) in workflow_plan::nodes(&compiled_plan)? {
            let execution_policy = workflow_plan::node_execution_policy(&node)?;
            sqlx::query(
                r#"
                INSERT INTO workflow_node_runs (
                    tenant_id, workflow_run_id, node_key, status, input,
                    max_attempts, backoff_sec, timeout_sec
                )
                VALUES ($1, $2, $3, 'pending', $4, $5, $6, $7)
                "#,
            )
            .bind(tenant_id)
            .bind(workflow_run_id)
            .bind(node_key)
            .bind(json!({"node": node}))
            .bind(execution_policy.max_attempts)
            .bind(execution_policy.backoff_sec)
            .bind(execution_policy.timeout_sec)
            .execute(pool)
            .await?;
        }

        for (from, to) in workflow_plan::edges(&compiled_plan)? {
            sqlx::query(
                r#"
                INSERT INTO workflow_run_dependencies (
                    workflow_run_id, from_node_key, to_node_key
                )
                VALUES ($1, $2, $3)
                "#,
            )
            .bind(workflow_run_id)
            .bind(from)
            .bind(to)
            .execute(pool)
            .await?;
        }

        Ok(workflow_run_id)
    }

    async fn run_snapshot(pool: &PgPool, run_id: Uuid) -> Result<serde_json::Value, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT run_config_snapshot
            FROM runs
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .fetch_one(pool)
        .await
    }

    async fn set_node_terminal(
        pool: &PgPool,
        workflow_run_id: Uuid,
        node_key: &str,
        status: &str,
        output: serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE workflow_node_runs
            SET status = $1,
                output = $2,
                completed_at = CURRENT_TIMESTAMP,
                updated_at = CURRENT_TIMESTAMP
            WHERE workflow_run_id = $3 AND node_key = $4
            "#,
        )
        .bind(status)
        .bind(output)
        .bind(workflow_run_id)
        .bind(node_key)
        .execute(pool)
        .await?;
        Ok(())
    }

    async fn node_statuses(
        pool: &PgPool,
        workflow_run_id: Uuid,
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT node_key, status
            FROM workflow_node_runs
            WHERE workflow_run_id = $1
            "#,
        )
        .bind(workflow_run_id)
        .fetch_all(pool)
        .await?;

        rows.into_iter()
            .map(|row| Ok((row.try_get("node_key")?, row.try_get("status")?)))
            .collect()
    }

    async fn assert_node_statuses(
        pool: &PgPool,
        workflow_run_id: Uuid,
        expected: &[(&str, &str)],
    ) -> Result<(), sqlx::Error> {
        let statuses = node_statuses(pool, workflow_run_id).await?;
        for (node_key, status) in expected {
            assert_eq!(statuses.get(*node_key).map(String::as_str), Some(*status));
        }
        Ok(())
    }
}
