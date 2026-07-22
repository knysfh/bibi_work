use std::convert::Infallible;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::sse::{Event, Sse},
};
use futures_util::Stream;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction, postgres::PgRow};
use tracing::warn;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::*,
            run_lifecycle,
            run_snapshot::{self, ConversationRunSnapshotRequest},
            runtime::{CancelRunRequest, DispatchRunRequest},
            secret_resolver,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{capability_authz, memory_injection, support::*};

pub async fn list_agent_teams(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<AgentTeamListQuery>,
) -> Result<Json<Vec<AgentTeamSummary>>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
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
          AND ($2::uuid IS NULL OR t.workspace_id = $2)
          AND ($3::text IS NULL OR t.status = $3)
        ORDER BY t.updated_at DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(query.workspace_id)
    .bind(query.status)
    .bind(query.limit.unwrap_or(100).clamp(1, 500))
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(agent_team_summary_from_row)
        .collect::<Result<Vec<_>, AppError>>()
        .map(Json)
}

pub async fn create_agent_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateAgentTeamRequest>,
) -> Result<Json<AgentTeamResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, Some(payload.tenant_id))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    require_tenant_action(&state, &ctx, tenant_id, "create", "agent_team").await?;
    let name = non_empty_string(payload.name, "name")?;
    if let Some(workspace_id) = payload.workspace_id {
        ensure_workspace_exists(&state, tenant_id, workspace_id).await?;
    }

    let row = sqlx::query(
        r#"
        INSERT INTO agent_teams (
            tenant_id, owner_user_id, workspace_id, name, description, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, tenant_id, owner_user_id, workspace_id, name, description, status,
                  metadata, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(ctx.platform_user_id)
    .bind(payload.workspace_id)
    .bind(name)
    .bind(payload.description)
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .fetch_one(&state.connect_pool)
    .await?;

    let team = agent_team_row_from_row(row)?;
    load_agent_team_detail(&state, tenant_id, team.id)
        .await
        .map(Json)
}

pub async fn get_agent_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Query(query): Query<AgentTeamListQuery>,
) -> Result<Json<AgentTeamResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    load_agent_team_detail(&state, tenant_id, team_id)
        .await
        .map(Json)
}

pub async fn update_agent_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Json(payload): Json<UpdateAgentTeamRequest>,
) -> Result<Json<AgentTeamResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, Some(payload.tenant_id))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;

    let current = load_agent_team_row(&state, tenant_id, team_id).await?;
    let workspace_id = match payload.workspace_id {
        NullableUuidPatch::Missing => current.workspace_id,
        NullableUuidPatch::Clear => None,
        NullableUuidPatch::Set(workspace_id) => {
            ensure_workspace_exists(&state, tenant_id, workspace_id).await?;
            Some(workspace_id)
        }
    };
    if let Some(status) = payload.status.as_deref() {
        validate_team_status(status)?;
    }
    let name = payload
        .name
        .map(|name| non_empty_string(name, "name"))
        .transpose()?;

    sqlx::query(
        r#"
        UPDATE agent_teams
        SET workspace_id = $3,
            name = COALESCE($4, name),
            description = COALESCE($5, description),
            status = COALESCE($6, status),
            metadata = COALESCE($7, metadata),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .bind(workspace_id)
    .bind(name)
    .bind(payload.description)
    .bind(payload.status)
    .bind(payload.metadata)
    .execute(&state.connect_pool)
    .await?;

    load_agent_team_detail(&state, tenant_id, team_id)
        .await
        .map(Json)
}

pub async fn create_agent_team_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Json(payload): Json<CreateAgentTeamMemberRequest>,
) -> Result<Json<AgentTeamResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, Some(payload.tenant_id))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    load_agent_team_row(&state, tenant_id, team_id).await?;
    let role = normalize_member_role(payload.role.as_deref().unwrap_or("member"))?;
    validate_slot_order(payload.slot_order)?;
    validate_agent_binding(
        &state,
        tenant_id,
        payload.agent_id,
        payload.agent_version_id,
    )
    .await?;
    ensure_member_slot_available(&state, tenant_id, team_id, payload.slot_order, None).await?;
    if role == "leader" {
        ensure_leader_slot_available(&state, tenant_id, team_id, None).await?;
    }
    let display_name = match payload.display_name {
        Some(display_name) => non_empty_string(display_name, "display_name")?,
        None => load_agent_name(&state, tenant_id, payload.agent_id).await?,
    };

    sqlx::query(
        r#"
        INSERT INTO agent_team_members (
            tenant_id, team_id, agent_id, agent_version_id, role, display_name,
            slot_order, policy_snapshot, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .bind(payload.agent_id)
    .bind(payload.agent_version_id)
    .bind(role)
    .bind(display_name)
    .bind(payload.slot_order)
    .bind(payload.policy_snapshot.unwrap_or_else(|| json!({})))
    .bind(payload.metadata.unwrap_or_else(|| json!({})))
    .execute(&state.connect_pool)
    .await?;

    load_agent_team_detail(&state, tenant_id, team_id)
        .await
        .map(Json)
}

pub async fn update_agent_team_member(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, member_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<UpdateAgentTeamMemberRequest>,
) -> Result<Json<AgentTeamResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, Some(payload.tenant_id))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    let current = load_agent_team_member_row(&state, tenant_id, team_id, member_id).await?;
    let agent_version_id = match payload.agent_version_id {
        NullableUuidPatch::Missing => current.agent_version_id,
        NullableUuidPatch::Clear => None,
        NullableUuidPatch::Set(agent_version_id) => Some(agent_version_id),
    };
    validate_agent_binding(&state, tenant_id, current.agent_id, agent_version_id).await?;
    let role = payload
        .role
        .as_deref()
        .map(normalize_member_role)
        .transpose()?;
    let slot_order = payload.slot_order.unwrap_or(current.slot_order);
    validate_slot_order(slot_order)?;
    ensure_member_slot_available(&state, tenant_id, team_id, slot_order, Some(member_id)).await?;
    if role.as_deref() == Some("leader") || (role.is_none() && current.role == "leader") {
        ensure_leader_slot_available(&state, tenant_id, team_id, Some(member_id)).await?;
    }
    if let Some(status) = payload.status.as_deref() {
        validate_member_status(status)?;
    }
    let display_name = payload
        .display_name
        .map(|name| non_empty_string(name, "display_name"))
        .transpose()?;

    sqlx::query(
        r#"
        UPDATE agent_team_members
        SET agent_version_id = $4,
            role = COALESCE($5, role),
            display_name = COALESCE($6, display_name),
            slot_order = $7,
            status = COALESCE($8, status),
            policy_snapshot = COALESCE($9, policy_snapshot),
            metadata = COALESCE($10, metadata),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND team_id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(member_id)
    .bind(tenant_id)
    .bind(team_id)
    .bind(agent_version_id)
    .bind(role)
    .bind(display_name)
    .bind(slot_order)
    .bind(payload.status)
    .bind(payload.policy_snapshot)
    .bind(payload.metadata)
    .execute(&state.connect_pool)
    .await?;

    load_agent_team_detail(&state, tenant_id, team_id)
        .await
        .map(Json)
}

pub async fn start_agent_team_run_stream(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventStreamQuery>,
    Json(payload): Json<StartAgentTeamRunRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let conversation_id = payload.conversation_id;
    let after_seq = event_store::resolve_after_seq(&headers, query.after_seq);
    create_and_dispatch_agent_team_run(&state, &ctx, team_id, payload).await?;
    let events = event_store::fetch_events(&state.connect_pool, conversation_id, after_seq).await?;
    Ok(event_store::events_to_sse(events))
}

pub(super) async fn create_and_dispatch_agent_team_run(
    state: &AppState,
    ctx: &PlatformRequestContext,
    team_id: Uuid,
    payload: StartAgentTeamRunRequest,
) -> Result<AgentTeamRunDetailResponse, AppError> {
    let tenant_id = resolve_api_tenant(ctx, Some(payload.tenant_id))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let team = load_agent_team_row(state, tenant_id, team_id).await?;
    if team.status != "active" {
        return Err(AppError::Conflict(
            "agent team must be active to run".to_string(),
        ));
    }
    let members = load_active_team_members(state, tenant_id, team_id).await?;
    if members.is_empty() {
        return Err(AppError::InvalidInput(
            "agent team must have at least one active member".to_string(),
        ));
    }

    let trace_id = Uuid::new_v4().to_string();
    let team_run_id = Uuid::new_v4();
    let input = payload.input.unwrap_or_else(|| json!({}));
    let scope = load_team_run_conversation_scope(
        state,
        tenant_id,
        payload.conversation_id,
        team.workspace_id,
    )
    .await?;
    if let (Some(team_workspace_id), Some(conversation_workspace_id)) =
        (team.workspace_id, scope.conversation_workspace_id)
        && team_workspace_id != conversation_workspace_id
    {
        return Err(AppError::InvalidInput(
            "team workspace does not match conversation workspace".to_string(),
        ));
    }
    let workspace_id = scope.effective_workspace_id;
    let project_id = resolve_team_run_project_id(
        payload.project_id,
        scope.conversation_project_id,
        scope.workspace_project_id,
    )?;
    let metadata = payload.metadata.clone().unwrap_or_else(|| json!({}));

    require_ferriskey_allow(
        state,
        ctx,
        tenant_id,
        "run",
        "conversation",
        payload.conversation_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(payload.conversation_id),
            project_id,
            ..Default::default()
        }),
    )
    .await?;

    let mut prepared = Vec::with_capacity(members.len());
    for member in members {
        let member_conversation_id = team_member_conversation_id(&member, payload.conversation_id);
        let member_thread_id = if member_conversation_id == payload.conversation_id {
            payload.thread_id.clone()
        } else {
            Some(member_conversation_id.to_string())
        };
        require_ferriskey_allow(
            state,
            ctx,
            tenant_id,
            "run",
            "agent",
            member.agent_id.to_string(),
            Some(AuthzContext {
                conversation_id: Some(payload.conversation_id),
                agent_id: Some(member.agent_id),
                project_id,
                ..Default::default()
            }),
        )
        .await?;
        if let Some(agent_version_id) = member.agent_version_id {
            capability_authz::require_agent_version_capabilities(
                state,
                ctx,
                tenant_id,
                agent_version_id,
                AuthzContext {
                    conversation_id: Some(payload.conversation_id),
                    agent_id: Some(member.agent_id),
                    project_id,
                    ..Default::default()
                },
            )
            .await?;
        }
        let run_id = Uuid::new_v4();
        let member_trace_id = format!("{trace_id}:{}", member.slot_order);
        let client_snapshot = team_member_client_snapshot(
            payload.run_config_snapshot.clone(),
            &team,
            &member,
            team_run_id,
        );
        let compiled_snapshot = run_snapshot::compile_conversation_run_snapshot(
            &state.connect_pool,
            ConversationRunSnapshotRequest {
                tenant_id,
                conversation_id: member_conversation_id,
                run_id,
                workspace_id,
                requested_agent_id: Some(member.agent_id),
                agent_version_id: member.agent_version_id,
                project_id,
                selected_model_profile_id: None,
                selected_mcp_server_ids: Vec::new(),
                thread_id: member_thread_id.clone(),
                client_snapshot: Some(client_snapshot),
                ctx,
            },
        )
        .await?;
        run_snapshot::ensure_python_dispatch_runtime(&compiled_snapshot.snapshot)?;
        prepared.push(PreparedTeamMemberRun {
            member,
            conversation_id: member_conversation_id,
            thread_id: member_thread_id,
            run_id,
            trace_id: member_trace_id,
            snapshot: compiled_snapshot.snapshot,
            scope_snapshot: compiled_snapshot.scope_snapshot,
        });
    }
    let run_config_snapshot = team_run_config_snapshot(&team, team_run_id, &prepared);

    let mut tx = state.connect_pool.begin().await?;
    insert_team_run_tx(
        &mut tx,
        NewTeamRun {
            tenant_id,
            team_run_id,
            team_id,
            conversation_id: payload.conversation_id,
            workspace_id,
            project_id,
            created_by_user_id: ctx.platform_user_id,
            trace_id: &trace_id,
            thread_id: payload.thread_id.as_deref(),
            input: &input,
            run_config_snapshot: &run_config_snapshot,
            metadata: &metadata,
        },
    )
    .await?;

    let mut persisted_events = Vec::new();
    let user_event = event_store::insert_event_tx(
        &mut tx,
        tenant_id,
        payload.conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!("team.message.completed.{team_run_id}")),
            event_type: "message.completed".to_string(),
            payload: Some(json!({
                "message_id": format!("team.user.{team_run_id}"),
                "role": "user",
                "content": submitted_user_content(&input),
                "team_id": team_id,
                "team_run_id": team_run_id,
                "author_user_id": ctx.platform_user_id
            })),
            trace_id: Some(trace_id.clone()),
        },
    )
    .await?;
    link_team_event_tx(&mut tx, tenant_id, user_event.id, team_run_id, None, None).await?;
    persisted_events.push(user_event);

    let team_event = event_store::insert_event_tx(
        &mut tx,
        tenant_id,
        payload.conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!("team.run.started.{team_run_id}")),
            event_type: "team.run.started".to_string(),
            payload: Some(json!({
                "team_run_id": team_run_id,
                "team_id": team_id,
                "status": "running",
                "member_count": prepared.len()
            })),
            trace_id: Some(trace_id.clone()),
        },
    )
    .await?;
    link_team_event_tx(&mut tx, tenant_id, team_event.id, team_run_id, None, None).await?;
    persisted_events.push(team_event);

    for prepared_member in &prepared {
        let run = insert_member_run_tx(
            &mut tx,
            NewMemberRun {
                tenant_id,
                conversation_id: prepared_member.conversation_id,
                workspace_id,
                project_id,
                created_by_user_id: ctx.platform_user_id,
                thread_id: prepared_member.thread_id.as_deref(),
                input: &input,
                prepared: prepared_member,
            },
        )
        .await?;
        let run_member_id =
            insert_team_run_member_tx(&mut tx, tenant_id, team_run_id, prepared_member, run.id)
                .await?;
        let team_member_event = event_store::insert_event_tx(
            &mut tx,
            tenant_id,
            prepared_member.conversation_id,
            Some(run.id),
            RunEventInput {
                event_id: Some(format!(
                    "team.member.queued.{team_run_id}.{}",
                    prepared_member.member.id
                )),
                event_type: "team.member.queued".to_string(),
                payload: Some(json!({
                    "team_id": team_id,
                    "team_run_id": team_run_id,
                    "team_member_id": prepared_member.member.id,
                    "team_run_member_id": run_member_id,
                    "run_id": run.id,
                    "slot_order": prepared_member.member.slot_order,
                    "role": prepared_member.member.role,
                    "status": "queued"
                })),
                trace_id: Some(prepared_member.trace_id.clone()),
            },
        )
        .await?;
        link_team_event_tx(
            &mut tx,
            tenant_id,
            team_member_event.id,
            team_run_id,
            Some(prepared_member.member.id),
            Some(run_member_id),
        )
        .await?;
        persisted_events.push(team_member_event);

        let run_event = event_store::insert_event_tx(
            &mut tx,
            tenant_id,
            prepared_member.conversation_id,
            Some(run.id),
            RunEventInput {
                event_id: Some(format!("run.queued.{}", run.id)),
                event_type: "run.queued".to_string(),
                payload: Some(json!({
                    "run_id": run.id,
                    "conversation_id": prepared_member.conversation_id,
                    "status": run.status,
                    "trace_id": run.trace_id,
                    "team_run_id": team_run_id,
                    "team_member_id": prepared_member.member.id,
                    "team_run_member_id": run_member_id
                })),
                trace_id: Some(run.trace_id.clone()),
            },
        )
        .await?;
        link_team_event_tx(
            &mut tx,
            tenant_id,
            run_event.id,
            team_run_id,
            Some(prepared_member.member.id),
            Some(run_member_id),
        )
        .await?;
        persisted_events.push(run_event);
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    for event in &persisted_events {
        event_store::publish_single_event(state, event).await;
    }

    let mut dispatch_failures = 0usize;
    for prepared_member in prepared {
        if let Err(err) =
            dispatch_prepared_member_run(state, ctx, tenant_id, project_id, &input, prepared_member)
                .await
        {
            dispatch_failures += 1;
            warn!("failed to dispatch team member run: {}", err);
        }
    }
    if dispatch_failures > 0 {
        refresh_team_run_status_after_dispatch(state, tenant_id, team_run_id).await?;
    }

    load_team_run_detail(state, tenant_id, Some(team_id), team_run_id).await
}

pub async fn get_agent_team_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, team_run_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<AgentTeamListQuery>,
) -> Result<Json<AgentTeamRunDetailResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, query.tenant_id)?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    load_team_run_detail(&state, tenant_id, Some(team_id), team_run_id)
        .await
        .map(Json)
}

pub async fn cancel_agent_team_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_run_id): Path<Uuid>,
    Json(payload): Json<CancelAgentTeamRunRequest>,
) -> Result<Json<AgentTeamRunDetailResponse>, AppError> {
    let tenant_id = resolve_api_tenant(&ctx, Some(payload.tenant_id))?;
    ensure_tenant_member(&state.connect_pool, tenant_id, ctx.platform_user_id).await?;
    let detail = load_team_run_detail(&state, tenant_id, None, team_run_id).await?;
    require_ferriskey_allow(
        &state,
        &ctx,
        tenant_id,
        "cancel",
        "run",
        team_run_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(detail.team_run.conversation_id),
            project_id: detail.team_run.project_id,
            ..Default::default()
        }),
    )
    .await?;
    if detail.team_run.status == "cancelled" {
        return Ok(Json(detail));
    }
    if detail.team_run.status == "cancelling" {
        return Ok(Json(detail));
    }
    if matches!(detail.team_run.status.as_str(), "completed" | "failed") {
        return Err(AppError::Conflict(
            "terminal team run cannot be cancelled".to_string(),
        ));
    }

    let reason = payload
        .reason
        .unwrap_or_else(|| "user_cancelled".to_string());
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE agent_team_runs
        SET status = 'cancelling',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND tenant_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled', 'cancelling')
        "#,
    )
    .bind(team_run_id)
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;

    let mut persisted_events = Vec::new();
    for member in &detail.members {
        if matches!(member.status.as_str(), "completed" | "failed" | "cancelled") {
            continue;
        }
        sqlx::query(
            r#"
            UPDATE agent_team_run_members
            SET status = 'cancelling',
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND tenant_id = $2
              AND status NOT IN ('completed', 'failed', 'cancelled', 'cancelling')
            "#,
        )
        .bind(member.id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;

        if let Some(run_id) = member.run_id {
            sqlx::query(
                r#"
                UPDATE runs
                SET status = 'cancelling',
                    updated_at = CURRENT_TIMESTAMP
                WHERE id = $1
                  AND tenant_id = $2
                  AND status NOT IN ('completed', 'failed', 'cancelled', 'cancelling')
                "#,
            )
            .bind(run_id)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await?;
        }

        let team_member_event = event_store::insert_event_tx(
            &mut tx,
            tenant_id,
            detail.team_run.conversation_id,
            member.run_id,
            RunEventInput {
                event_id: Some(format!(
                    "team.member.cancelling.{team_run_id}.{}",
                    member.id
                )),
                event_type: "team.member.updated".to_string(),
                payload: Some(json!({
                    "team_id": detail.team_run.team_id,
                    "team_run_id": team_run_id,
                    "team_member_id": member.team_member_id.unwrap_or(member.id),
                    "team_run_member_id": member.id,
                    "run_id": member.run_id,
                    "slot_order": member.slot_order,
                    "role": member.role,
                    "status": "cancelling",
                    "reason": reason
                })),
                trace_id: Some(detail.team_run.trace_id.clone()),
            },
        )
        .await?;
        link_team_event_tx(
            &mut tx,
            tenant_id,
            team_member_event.id,
            team_run_id,
            member.team_member_id,
            Some(member.id),
        )
        .await?;
        persisted_events.push(team_member_event);
    }

    let team_event = event_store::insert_event_tx(
        &mut tx,
        tenant_id,
        detail.team_run.conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!("team.run.cancelling.{team_run_id}")),
            event_type: "team.run.updated".to_string(),
            payload: Some(json!({
                "team_run_id": team_run_id,
                "team_id": detail.team_run.team_id,
                "status": "cancelling",
                "reason": reason
            })),
            trace_id: Some(detail.team_run.trace_id.clone()),
        },
    )
    .await?;
    link_team_event_tx(&mut tx, tenant_id, team_event.id, team_run_id, None, None).await?;
    persisted_events.push(team_event);

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    for event in &persisted_events {
        event_store::publish_single_event(&state, event).await;
    }
    for member in detail.members {
        if let Some(run_id) = member.run_id
            && let Err(err) = state
                .agent_runtime_client
                .cancel_run(
                    run_id,
                    &CancelRunRequest {
                        tenant_id,
                        conversation_id: detail.team_run.conversation_id,
                        trace_id: Some(detail.team_run.trace_id.clone()),
                        reason: reason.clone(),
                    },
                )
                .await
        {
            warn!(
                "failed to propagate cancel for team member run {}: {}",
                run_id, err
            );
        }
    }

    load_team_run_detail(&state, tenant_id, None, team_run_id)
        .await
        .map(Json)
}

struct AgentTeamRow {
    id: Uuid,
    tenant_id: Uuid,
    owner_user_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    name: String,
    description: Option<String>,
    status: String,
    metadata: Value,
    created_at: time::OffsetDateTime,
    updated_at: time::OffsetDateTime,
}

struct PreparedTeamMemberRun {
    member: AgentTeamMemberResponse,
    conversation_id: Uuid,
    thread_id: Option<String>,
    run_id: Uuid,
    trace_id: String,
    snapshot: Value,
    scope_snapshot: Value,
}

fn team_member_conversation_id(member: &AgentTeamMemberResponse, fallback: Uuid) -> Uuid {
    member
        .metadata
        .pointer("/biwork/conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or(fallback)
}

struct TeamRunConversationScope {
    conversation_workspace_id: Option<Uuid>,
    effective_workspace_id: Option<Uuid>,
    conversation_project_id: Option<Uuid>,
    workspace_project_id: Option<Uuid>,
}

fn resolve_api_tenant(
    ctx: &PlatformRequestContext,
    requested_tenant_id: Option<Uuid>,
) -> Result<Uuid, AppError> {
    if let Some(tenant_id) = requested_tenant_id {
        if tenant_id != ctx.tenant_id {
            return Err(AppError::PermissionDenied(
                "requested tenant does not match current session tenant".to_string(),
            ));
        }
        return Ok(tenant_id);
    }
    Ok(ctx.tenant_id)
}

async fn load_agent_team_detail(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<AgentTeamResponse, AppError> {
    let team = load_agent_team_row(state, tenant_id, team_id).await?;
    let members = load_team_members(state, tenant_id, team_id).await?;
    Ok(AgentTeamResponse {
        id: team.id,
        tenant_id: team.tenant_id,
        owner_user_id: team.owner_user_id,
        workspace_id: team.workspace_id,
        name: team.name,
        description: team.description,
        status: team.status,
        metadata: team.metadata,
        members,
        created_at: team.created_at,
        updated_at: team.updated_at,
        available_actions: vec![
            "team:update".to_string(),
            "team:member:update".to_string(),
            "team:run".to_string(),
        ],
    })
}

async fn load_agent_team_row(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<AgentTeamRow, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, owner_user_id, workspace_id, name, description, status,
               metadata, created_at, updated_at
        FROM agent_teams
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent team not found".to_string()))?;

    agent_team_row_from_row(row)
}

fn agent_team_row_from_row(row: PgRow) -> Result<AgentTeamRow, AppError> {
    Ok(AgentTeamRow {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        owner_user_id: row.try_get("owner_user_id")?,
        workspace_id: row.try_get("workspace_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        status: row.try_get("status")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_team_members(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<Vec<AgentTeamMemberResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, team_id, agent_id, agent_version_id, role, display_name,
               slot_order, policy_snapshot, status, metadata, created_at, updated_at
        FROM agent_team_members
        WHERE tenant_id = $1
          AND team_id = $2
          AND deleted_at IS NULL
        ORDER BY slot_order ASC, created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter().map(agent_team_member_from_row).collect()
}

async fn load_active_team_members(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<Vec<AgentTeamMemberResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, team_id, agent_id, agent_version_id, role, display_name,
               slot_order, policy_snapshot, status, metadata, created_at, updated_at
        FROM agent_team_members
        WHERE tenant_id = $1
          AND team_id = $2
          AND status = 'active'
          AND deleted_at IS NULL
        ORDER BY slot_order ASC, created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter().map(agent_team_member_from_row).collect()
}

async fn load_agent_team_member_row(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    member_id: Uuid,
) -> Result<AgentTeamMemberResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, team_id, agent_id, agent_version_id, role, display_name,
               slot_order, policy_snapshot, status, metadata, created_at, updated_at
        FROM agent_team_members
        WHERE id = $1
          AND tenant_id = $2
          AND team_id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(member_id)
    .bind(tenant_id)
    .bind(team_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent team member not found".to_string()))?;

    agent_team_member_from_row(row)
}

fn agent_team_member_from_row(row: PgRow) -> Result<AgentTeamMemberResponse, AppError> {
    Ok(AgentTeamMemberResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        team_id: row.try_get("team_id")?,
        agent_id: row.try_get("agent_id")?,
        agent_version_id: row.try_get("agent_version_id")?,
        role: row.try_get("role")?,
        display_name: row.try_get("display_name")?,
        slot_order: row.try_get("slot_order")?,
        policy_snapshot: row.try_get("policy_snapshot")?,
        status: row.try_get("status")?,
        metadata: row.try_get("metadata")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
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

async fn load_team_summary_by_id(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<AgentTeamSummary, AppError> {
    let row = sqlx::query(
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
        WHERE t.id = $1
          AND t.tenant_id = $2
          AND t.deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent team not found".to_string()))?;

    agent_team_summary_from_row(row)
}

async fn ensure_workspace_exists(
    state: &AppState,
    tenant_id: Uuid,
    workspace_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM workspaces
            WHERE id = $1
              AND tenant_id = $2
              AND deleted_at IS NULL
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

async fn validate_agent_binding(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Uuid,
    agent_version_id: Option<Uuid>,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agents
            WHERE id = $1
              AND tenant_id = $2
              AND deleted_at IS NULL
        ) AS exists
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if !exists {
        return Err(AppError::NotFound("agent not found".to_string()));
    }

    if let Some(agent_version_id) = agent_version_id {
        let valid: bool = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM agent_versions
                WHERE id = $1
                  AND tenant_id = $2
                  AND agent_id = $3
                  AND status = 'published'
            ) AS exists
            "#,
        )
        .bind(agent_version_id)
        .bind(tenant_id)
        .bind(agent_id)
        .fetch_one(&state.connect_pool)
        .await?
        .try_get("exists")?;
        if !valid {
            return Err(AppError::InvalidInput(
                "agent_version_id does not reference a published version for the agent".to_string(),
            ));
        }
    }

    Ok(())
}

async fn load_agent_name(
    state: &AppState,
    tenant_id: Uuid,
    agent_id: Uuid,
) -> Result<String, AppError> {
    let name = sqlx::query_scalar(
        r#"
        SELECT name
        FROM agents
        WHERE id = $1
          AND tenant_id = $2
          AND deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent not found".to_string()))?;
    Ok(name)
}

async fn ensure_member_slot_available(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    slot_order: i32,
    current_member_id: Option<Uuid>,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_team_members
            WHERE tenant_id = $1
              AND team_id = $2
              AND slot_order = $3
              AND deleted_at IS NULL
              AND ($4::uuid IS NULL OR id <> $4)
        ) AS exists
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .bind(slot_order)
    .bind(current_member_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Err(AppError::Conflict(
            "agent team slot_order is already occupied".to_string(),
        ))
    } else {
        Ok(())
    }
}

async fn ensure_leader_slot_available(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    current_member_id: Option<Uuid>,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_team_members
            WHERE tenant_id = $1
              AND team_id = $2
              AND role = 'leader'
              AND deleted_at IS NULL
              AND ($3::uuid IS NULL OR id <> $3)
        ) AS exists
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .bind(current_member_id)
    .fetch_one(&state.connect_pool)
    .await?
    .try_get("exists")?;
    if exists {
        Err(AppError::Conflict(
            "agent team can only have one active leader".to_string(),
        ))
    } else {
        Ok(())
    }
}

async fn load_team_run_conversation_scope(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    fallback_workspace_id: Option<Uuid>,
) -> Result<TeamRunConversationScope, AppError> {
    let row = sqlx::query(
        r#"
        SELECT c.workspace_id AS conversation_workspace_id,
               c.project_id AS conversation_project_id,
               w.id AS loaded_workspace_id,
               w.remote_project_id AS workspace_project_id
        FROM conversations c
        LEFT JOIN workspaces w
          ON w.id = COALESCE(c.workspace_id, $3)
         AND w.tenant_id = c.tenant_id
         AND w.deleted_at IS NULL
        WHERE c.id = $1
          AND c.tenant_id = $2
          AND c.deleted_at IS NULL
        "#,
    )
    .bind(conversation_id)
    .bind(tenant_id)
    .bind(fallback_workspace_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("conversation not found".to_string()))?;

    let conversation_workspace_id: Option<Uuid> = row.try_get("conversation_workspace_id")?;
    let loaded_workspace_id: Option<Uuid> = row.try_get("loaded_workspace_id")?;
    if conversation_workspace_id
        .or(fallback_workspace_id)
        .is_some()
        && loaded_workspace_id.is_none()
    {
        return Err(AppError::NotFound("workspace not found".to_string()));
    }
    Ok(TeamRunConversationScope {
        conversation_workspace_id,
        effective_workspace_id: conversation_workspace_id.or(fallback_workspace_id),
        conversation_project_id: row.try_get("conversation_project_id")?,
        workspace_project_id: row.try_get("workspace_project_id")?,
    })
}

fn resolve_team_run_project_id(
    requested_project_id: Option<Uuid>,
    conversation_project_id: Option<Uuid>,
    workspace_project_id: Option<Uuid>,
) -> Result<Option<Uuid>, AppError> {
    let inherited_project_id = conversation_project_id.or(workspace_project_id);
    if let (Some(requested), Some(inherited)) = (requested_project_id, inherited_project_id)
        && requested != inherited
    {
        return Err(AppError::InvalidInput(
            "team run project_id cannot expand conversation workspace scope".to_string(),
        ));
    }
    Ok(requested_project_id.or(inherited_project_id))
}

fn team_member_client_snapshot(
    client_snapshot: Option<Value>,
    team: &AgentTeamRow,
    member: &AgentTeamMemberResponse,
    team_run_id: Uuid,
) -> Value {
    let mut base = json!({
        "runtime": {
            "kind": run_snapshot::PYTHON_RUNTIME_KIND
        }
    });
    if let Some(object) = base.as_object_mut() {
        if let Some(Value::Object(client_object)) = client_snapshot {
            for key in [
                "memory_retrieval",
                "memory_query",
                "permissions",
                "interrupt_on",
                "file_mounts",
                "ui",
                "cron",
                "channel",
            ] {
                if let Some(value) = client_object.get(key) {
                    object.insert(key.to_string(), value.clone());
                }
            }
        }
        object.insert(
            "runtime".to_string(),
            json!({
                "kind": run_snapshot::PYTHON_RUNTIME_KIND
            }),
        );
        object.insert(
            "team".to_string(),
            json!({
                "team_id": team.id,
                "team_run_id": team_run_id,
                "name": team.name,
                "workspace_id": team.workspace_id,
                "member": {
                    "team_member_id": member.id,
                    "agent_id": member.agent_id,
                    "agent_version_id": member.agent_version_id,
                    "role": member.role,
                    "display_name": member.display_name,
                    "slot_order": member.slot_order,
                    "policy_snapshot": member.policy_snapshot
                }
            }),
        );
    }
    base
}

fn team_run_config_snapshot(
    team: &AgentTeamRow,
    team_run_id: Uuid,
    prepared: &[PreparedTeamMemberRun],
) -> Value {
    let members = prepared
        .iter()
        .map(|prepared| {
            let member = &prepared.member;
            json!({
                "team_member_id": member.id,
                "run_id": prepared.run_id,
                "agent_id": member.agent_id,
                "agent_version_id": member.agent_version_id,
                "role": member.role,
                "display_name": member.display_name,
                "slot_order": member.slot_order,
                "trace_id": prepared.trace_id
            })
        })
        .collect::<Vec<_>>();

    json!({
        "runtime": {
            "kind": run_snapshot::PYTHON_RUNTIME_KIND
        },
        "team": {
            "team_id": team.id,
            "team_run_id": team_run_id,
            "tenant_id": team.tenant_id,
            "workspace_id": team.workspace_id,
            "name": team.name,
            "members": members
        }
    })
}

struct NewTeamRun<'a> {
    tenant_id: Uuid,
    team_run_id: Uuid,
    team_id: Uuid,
    conversation_id: Uuid,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    created_by_user_id: Uuid,
    trace_id: &'a str,
    thread_id: Option<&'a str>,
    input: &'a Value,
    run_config_snapshot: &'a Value,
    metadata: &'a Value,
}

async fn insert_team_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    run: NewTeamRun<'_>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO agent_team_runs (
            id, tenant_id, team_id, conversation_id, workspace_id, project_id,
            created_by_user_id, status, input_snapshot, run_config_snapshot,
            trace_id, thread_id, metadata, started_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'running', $8, $9, $10, $11, $12, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(run.team_run_id)
    .bind(run.tenant_id)
    .bind(run.team_id)
    .bind(run.conversation_id)
    .bind(run.workspace_id)
    .bind(run.project_id)
    .bind(run.created_by_user_id)
    .bind(run.input)
    .bind(run.run_config_snapshot)
    .bind(run.trace_id)
    .bind(run.thread_id)
    .bind(run.metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

struct NewMemberRun<'a> {
    tenant_id: Uuid,
    conversation_id: Uuid,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    created_by_user_id: Uuid,
    thread_id: Option<&'a str>,
    input: &'a Value,
    prepared: &'a PreparedTeamMemberRun,
}

async fn insert_member_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    run: NewMemberRun<'_>,
) -> Result<RunResponse, AppError> {
    let run_row = sqlx::query(
        r#"
        INSERT INTO runs (
            id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
            project_id, created_by_user_id, status, input, run_config_snapshot,
            run_scope_snapshot, policy_version, risk_policy_version, trace_id, thread_id
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'queued', $9, $10, $11, $12, $13, $14, $15)
        RETURNING id, tenant_id, conversation_id, workspace_id, agent_id, agent_version_id,
                  project_id, status, trace_id, thread_id, policy_version, run_scope_snapshot,
                  queued_at, updated_at
        "#,
    )
    .bind(run.prepared.run_id)
    .bind(run.tenant_id)
    .bind(run.conversation_id)
    .bind(run.workspace_id)
    .bind(run.prepared.member.agent_id)
    .bind(run.prepared.member.agent_version_id)
    .bind(run.project_id)
    .bind(run.created_by_user_id)
    .bind(run.input)
    .bind(&run.prepared.snapshot)
    .bind(&run.prepared.scope_snapshot)
    .bind(LOCAL_POLICY_VERSION)
    .bind(LOCAL_RISK_POLICY_VERSION)
    .bind(&run.prepared.trace_id)
    .bind(run.thread_id)
    .fetch_one(&mut **tx)
    .await?;

    run_from_row(run_row)
}

async fn insert_team_run_member_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    team_run_id: Uuid,
    prepared: &PreparedTeamMemberRun,
    run_id: Uuid,
) -> Result<Uuid, AppError> {
    let snapshot = json!({
        "team_member_id": prepared.member.id,
        "agent_id": prepared.member.agent_id,
        "agent_version_id": prepared.member.agent_version_id,
        "role": prepared.member.role,
        "display_name": prepared.member.display_name,
        "slot_order": prepared.member.slot_order,
        "policy_snapshot": prepared.member.policy_snapshot
    });
    let row = sqlx::query(
        r#"
        INSERT INTO agent_team_run_members (
            tenant_id, team_run_id, team_member_id, run_id, agent_id, agent_version_id,
            role, display_name, slot_order, status, member_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'queued', $10)
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .bind(team_run_id)
    .bind(prepared.member.id)
    .bind(run_id)
    .bind(prepared.member.agent_id)
    .bind(prepared.member.agent_version_id)
    .bind(&prepared.member.role)
    .bind(&prepared.member.display_name)
    .bind(prepared.member.slot_order)
    .bind(snapshot)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.try_get("id")?)
}

async fn link_team_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    event_id: Uuid,
    team_run_id: Uuid,
    team_member_id: Option<Uuid>,
    team_run_member_id: Option<Uuid>,
) -> Result<(), AppError> {
    insert_run_event_link_tx(
        tx,
        tenant_id,
        event_id,
        "agent_team_run",
        Some(team_run_id),
        None,
    )
    .await?;
    if let Some(team_member_id) = team_member_id {
        insert_run_event_link_tx(
            tx,
            tenant_id,
            event_id,
            "agent_team_member",
            Some(team_member_id),
            None,
        )
        .await?;
    }
    if let Some(team_run_member_id) = team_run_member_id {
        insert_run_event_link_tx(
            tx,
            tenant_id,
            event_id,
            "agent_team_run_member",
            Some(team_run_member_id),
            None,
        )
        .await?;
    }
    Ok(())
}

async fn insert_run_event_link_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    event_id: Uuid,
    link_type: &str,
    link_id: Option<Uuid>,
    link_key: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO run_event_links (tenant_id, run_event_id, link_type, link_id, link_key)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(tenant_id)
    .bind(event_id)
    .bind(link_type)
    .bind(link_id)
    .bind(link_key)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn dispatch_prepared_member_run(
    state: &AppState,
    ctx: &PlatformRequestContext,
    tenant_id: Uuid,
    project_id: Option<Uuid>,
    input: &Value,
    prepared: PreparedTeamMemberRun,
) -> Result<(), AppError> {
    let conversation_id = prepared.conversation_id;
    let mut snapshot = prepared.snapshot;
    if let Err(err) = memory_injection::inject_memory_context_for_run(
        state,
        memory_injection::MemoryInjectionRequest {
            actor: ActorRef {
                user_id: ctx.platform_user_id,
                device_id: Some(ctx.device_id),
                session_id: Some(ctx.session_id),
                roles: ctx.roles.clone(),
            },
            tenant_id,
            run_id: prepared.run_id,
            agent_id: Some(prepared.member.agent_id),
            project_id,
        },
        input,
        &mut snapshot,
    )
    .await
    {
        record_team_member_dispatch_failure(
            state,
            tenant_id,
            conversation_id,
            prepared.run_id,
            prepared.member.id,
            &prepared.trace_id,
            err.to_string(),
        )
        .await?;
        return Err(err);
    }

    if let Err(err) = secret_resolver::attach_llm_runtime_credential(
        state,
        tenant_id,
        prepared.run_id,
        &mut snapshot,
    )
    .await
    {
        record_team_member_dispatch_failure(
            state,
            tenant_id,
            conversation_id,
            prepared.run_id,
            prepared.member.id,
            &prepared.trace_id,
            err.to_string(),
        )
        .await?;
        return Err(err);
    }

    if let Err(err) = state
        .agent_runtime_client
        .dispatch_run(&DispatchRunRequest {
            tenant_id,
            conversation_id,
            run_id: prepared.run_id,
            trace_id: prepared.trace_id.clone(),
            input: input.clone(),
            run_config_snapshot: snapshot,
        })
        .await
    {
        record_team_member_dispatch_failure(
            state,
            tenant_id,
            conversation_id,
            prepared.run_id,
            prepared.member.id,
            &prepared.trace_id,
            err.to_string(),
        )
        .await?;
        return Err(err);
    }

    Ok(())
}

pub(super) async fn apply_team_member_run_state_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    conversation_id: Uuid,
    run_id: Option<Uuid>,
    event_type: &str,
    event_payload: &Value,
    trace_id: Option<String>,
) -> Result<Vec<StreamEventResponse>, AppError> {
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    let Some((member_status, member_event_type, team_event_type)) =
        team_member_status_event_mapping(event_type)
    else {
        return Ok(Vec::new());
    };

    let rows = sqlx::query(
        r#"
        UPDATE agent_team_run_members
        SET status = $1,
            last_error = CASE WHEN $1 = 'failed' THEN $4 ELSE last_error END,
            completed_at = CASE
                WHEN $1 IN ('completed', 'failed', 'cancelled') THEN COALESCE(agent_team_run_members.completed_at, CURRENT_TIMESTAMP)
                ELSE agent_team_run_members.completed_at
            END,
            updated_at = CURRENT_TIMESTAMP
        FROM agent_team_runs tr
        WHERE agent_team_run_members.tenant_id = $2
          AND agent_team_run_members.run_id = $3
          AND agent_team_run_members.status NOT IN ('completed', 'failed', 'cancelled')
          AND tr.id = agent_team_run_members.team_run_id
          AND tr.tenant_id = agent_team_run_members.tenant_id
        RETURNING agent_team_run_members.id AS team_run_member_id,
                  agent_team_run_members.team_run_id,
                  agent_team_run_members.team_member_id,
                  agent_team_run_members.slot_order,
                  agent_team_run_members.role,
                  tr.team_id,
                  tr.status AS team_run_status
        "#,
    )
    .bind(member_status)
    .bind(tenant_id)
    .bind(run_id)
    .bind(team_event_error_summary(event_payload))
    .fetch_all(&mut **tx)
    .await?;

    let mut inserted_events = Vec::new();
    for row in rows {
        let team_id: Uuid = row.try_get("team_id")?;
        let team_run_id: Uuid = row.try_get("team_run_id")?;
        let team_run_member_id: Uuid = row.try_get("team_run_member_id")?;
        let team_member_id: Option<Uuid> = row.try_get("team_member_id")?;
        let slot_order: i32 = row.try_get("slot_order")?;
        let role: String = row.try_get("role")?;
        let member_event = event_store::insert_event_tx(
            tx,
            tenant_id,
            conversation_id,
            Some(run_id),
            RunEventInput {
                event_id: Some(format!(
                    "{member_event_type}.{team_run_id}.{team_run_member_id}"
                )),
                event_type: member_event_type.to_string(),
                payload: Some(json!({
                    "team_id": team_id,
                    "team_run_id": team_run_id,
                    "team_member_id": team_member_id.unwrap_or(team_run_member_id),
                    "team_run_member_id": team_run_member_id,
                    "run_id": run_id,
                    "slot_order": slot_order,
                    "role": role,
                    "status": member_status,
                    "reason": event_payload.get("reason").and_then(Value::as_str),
                    "error": team_event_error_summary(event_payload),
                })),
                trace_id: trace_id.clone(),
            },
        )
        .await?;
        link_team_event_tx(
            tx,
            tenant_id,
            member_event.id,
            team_run_id,
            team_member_id,
            Some(team_run_member_id),
        )
        .await?;
        inserted_events.push(member_event);

        if let Some(team_event) = refresh_team_run_status_from_member_events_tx(
            tx,
            tenant_id,
            conversation_id,
            team_id,
            team_run_id,
            team_event_type,
            trace_id.clone(),
        )
        .await?
        {
            inserted_events.push(team_event);
        }
    }

    Ok(inserted_events)
}

async fn refresh_team_run_status_from_member_events_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    conversation_id: Uuid,
    team_id: Uuid,
    team_run_id: Uuid,
    terminal_team_event_type: &str,
    trace_id: Option<String>,
) -> Result<Option<StreamEventResponse>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT tr.status AS stored_status,
               COUNT(*)::BIGINT AS total,
               COUNT(*) FILTER (WHERE COALESCE(r.status, trm.status) IN ('queued', 'running', 'waiting_approval', 'blocked', 'cancelling'))::BIGINT AS active,
               COUNT(*) FILTER (WHERE COALESCE(r.status, trm.status) IN ('queued', 'pending'))::BIGINT AS pending,
               COUNT(*) FILTER (WHERE COALESCE(r.status, trm.status) = 'failed')::BIGINT AS failed,
               COUNT(*) FILTER (WHERE COALESCE(r.status, trm.status) = 'cancelled')::BIGINT AS cancelled,
               COUNT(*) FILTER (WHERE COALESCE(r.status, trm.status) = 'completed')::BIGINT AS completed
        FROM agent_team_runs tr
        JOIN agent_team_run_members trm
          ON trm.team_run_id = tr.id
         AND trm.tenant_id = tr.tenant_id
        LEFT JOIN runs r
          ON r.id = trm.run_id
         AND r.tenant_id = trm.tenant_id
        WHERE tr.tenant_id = $1
          AND tr.id = $2
        GROUP BY tr.status
        "#,
    )
    .bind(tenant_id)
    .bind(team_run_id)
    .fetch_one(&mut **tx)
    .await?;

    let stored_status: String = row.try_get("stored_status")?;
    let total: i64 = row.try_get("total")?;
    let active: i64 = row.try_get("active")?;
    let pending: i64 = row.try_get("pending")?;
    let failed: i64 = row.try_get("failed")?;
    let cancelled: i64 = row.try_get("cancelled")?;
    let completed: i64 = row.try_get("completed")?;

    let (next_status, event_type) = if active > 0 {
        let status = if stored_status == "cancelling" {
            "cancelling"
        } else {
            "running"
        };
        (status, "team.run.updated")
    } else if failed > 0 {
        ("failed", "team.run.failed")
    } else if cancelled > 0 {
        ("cancelled", "team.run.cancelled")
    } else if total > 0 && completed == total {
        ("completed", "team.run.completed")
    } else {
        (stored_status.as_str(), terminal_team_event_type)
    };

    sqlx::query(
        r#"
        UPDATE agent_team_runs
        SET status = $1,
            completed_at = CASE
                WHEN $1 IN ('completed', 'failed', 'cancelled') THEN COALESCE(completed_at, CURRENT_TIMESTAMP)
                ELSE completed_at
            END,
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $2
          AND id = $3
          AND status NOT IN ('completed', 'failed', 'cancelled')
        "#,
    )
    .bind(next_status)
    .bind(tenant_id)
    .bind(team_run_id)
    .execute(&mut **tx)
    .await?;

    let event = event_store::insert_event_tx(
        tx,
        tenant_id,
        conversation_id,
        None,
        RunEventInput {
            event_id: Some(format!("{event_type}.{team_run_id}")),
            event_type: event_type.to_string(),
            payload: Some(json!({
                "team_id": team_id,
                "team_run_id": team_run_id,
                "status": next_status,
                "active_child_count": active,
                "pending_wake_count": pending,
            })),
            trace_id,
        },
    )
    .await?;
    link_team_event_tx(tx, tenant_id, event.id, team_run_id, None, None).await?;
    Ok(Some(event))
}

fn team_member_status_event_mapping(
    event_type: &str,
) -> Option<(&'static str, &'static str, &'static str)> {
    match event_type {
        "run.completed" => Some(("completed", "team.member.completed", "team.run.completed")),
        "run.failed" => Some(("failed", "team.member.failed", "team.run.failed")),
        "run.cancelled" => Some(("cancelled", "team.member.cancelled", "team.run.cancelled")),
        _ => None,
    }
}

fn team_event_error_summary(payload: &Value) -> Option<String> {
    payload
        .get("error")
        .or_else(|| payload.get("error_summary"))
        .or_else(|| payload.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(1000).collect())
}

async fn record_team_member_dispatch_failure(
    state: &AppState,
    tenant_id: Uuid,
    conversation_id: Uuid,
    run_id: Uuid,
    team_member_id: Uuid,
    trace_id: &str,
    error: String,
) -> Result<(), AppError> {
    run_lifecycle::mark_dispatch_failed(
        state,
        tenant_id,
        conversation_id,
        run_id,
        Some(trace_id.to_string()),
        &error,
    )
    .await?;
    let row = sqlx::query(
        r#"
        UPDATE agent_team_run_members
        SET status = 'failed',
            last_error = $4,
            completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        FROM agent_team_runs tr
        WHERE agent_team_run_members.tenant_id = $1
          AND agent_team_run_members.run_id = $2
          AND agent_team_run_members.team_member_id = $3
          AND tr.id = agent_team_run_members.team_run_id
          AND tr.tenant_id = agent_team_run_members.tenant_id
        RETURNING agent_team_run_members.id, agent_team_run_members.team_run_id,
                  tr.team_id, agent_team_run_members.slot_order, agent_team_run_members.role
        "#,
    )
    .bind(tenant_id)
    .bind(run_id)
    .bind(team_member_id)
    .bind(error.chars().take(512).collect::<String>())
    .fetch_optional(&state.connect_pool)
    .await?;

    let Some(row) = row else {
        return Ok(());
    };
    let team_run_member_id: Uuid = row.try_get("id")?;
    let team_run_id: Uuid = row.try_get("team_run_id")?;
    let team_id: Uuid = row.try_get("team_id")?;
    let slot_order: i32 = row.try_get("slot_order")?;
    let role: String = row.try_get("role")?;
    let mut tx = state.connect_pool.begin().await?;
    let event = event_store::insert_event_tx(
        &mut tx,
        tenant_id,
        conversation_id,
        Some(run_id),
        RunEventInput {
            event_id: Some(format!("team.member.failed.{team_run_id}.{team_member_id}")),
            event_type: "team.member.failed".to_string(),
            payload: Some(json!({
                "team_id": team_id,
                "team_run_id": team_run_id,
                "team_member_id": team_member_id,
                "team_run_member_id": team_run_member_id,
                "run_id": run_id,
                "slot_order": slot_order,
                "role": role,
                "status": "failed",
                "error": error.chars().take(1000).collect::<String>()
            })),
            trace_id: Some(trace_id.to_string()),
        },
    )
    .await?;
    link_team_event_tx(
        &mut tx,
        tenant_id,
        event.id,
        team_run_id,
        Some(team_member_id),
        Some(team_run_member_id),
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    event_store::publish_single_event(state, &event).await;
    Ok(())
}

async fn refresh_team_run_status_after_dispatch(
    state: &AppState,
    tenant_id: Uuid,
    team_run_id: Uuid,
) -> Result<(), AppError> {
    let row = sqlx::query(
        r#"
        SELECT COUNT(*)::BIGINT AS total,
               COUNT(*) FILTER (WHERE status = 'failed')::BIGINT AS failed
        FROM agent_team_run_members
        WHERE tenant_id = $1
          AND team_run_id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(team_run_id)
    .fetch_one(&state.connect_pool)
    .await?;
    let total: i64 = row.try_get("total")?;
    let failed: i64 = row.try_get("failed")?;
    if total > 0 && total == failed {
        sqlx::query(
            r#"
            UPDATE agent_team_runs
            SET status = 'failed',
                completed_at = COALESCE(completed_at, CURRENT_TIMESTAMP),
                updated_at = CURRENT_TIMESTAMP
            WHERE tenant_id = $1
              AND id = $2
              AND status NOT IN ('completed', 'failed', 'cancelled')
            "#,
        )
        .bind(tenant_id)
        .bind(team_run_id)
        .execute(&state.connect_pool)
        .await?;
    }
    Ok(())
}

async fn load_team_run_detail(
    state: &AppState,
    tenant_id: Uuid,
    expected_team_id: Option<Uuid>,
    team_run_id: Uuid,
) -> Result<AgentTeamRunDetailResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, team_id, conversation_id, workspace_id, project_id, status,
               trace_id, thread_id, input_snapshot, metadata, queued_at, started_at,
               completed_at, updated_at
        FROM agent_team_runs
        WHERE id = $1
          AND tenant_id = $2
          AND ($3::uuid IS NULL OR team_id = $3)
        "#,
    )
    .bind(team_run_id)
    .bind(tenant_id)
    .bind(expected_team_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("agent team run not found".to_string()))?;

    let mut team_run = agent_team_run_from_row(row)?;
    let team = load_team_summary_by_id(state, tenant_id, team_run.team_id).await?;
    let members = load_team_run_members(state, tenant_id, team_run_id).await?;
    team_run.status = derive_team_run_status(&team_run.status, &members);
    let available_actions = if matches!(
        team_run.status.as_str(),
        "queued" | "running" | "waiting_approval"
    ) {
        vec!["team_run:cancel".to_string()]
    } else {
        Vec::new()
    };

    Ok(AgentTeamRunDetailResponse {
        team_run,
        team,
        members,
        available_actions,
    })
}

async fn load_team_run_members(
    state: &AppState,
    tenant_id: Uuid,
    team_run_id: Uuid,
) -> Result<Vec<AgentTeamRunMemberResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT trm.id, trm.tenant_id, trm.team_run_id, trm.team_member_id, trm.run_id,
               COALESCE(r.agent_id, trm.agent_id) AS agent_id,
               COALESCE(r.agent_version_id, trm.agent_version_id) AS agent_version_id,
               trm.role, trm.display_name, trm.slot_order,
               CASE
                   WHEN r.status = 'waiting_approval' THEN 'blocked'
                   WHEN r.status IS NOT NULL THEN r.status
                   ELSE trm.status
               END AS status,
               trm.member_snapshot, trm.last_error, trm.queued_at,
               COALESCE(r.started_at, trm.started_at) AS started_at,
               COALESCE(r.completed_at, trm.completed_at) AS completed_at,
               GREATEST(trm.updated_at, COALESCE(r.updated_at, trm.updated_at)) AS updated_at
        FROM agent_team_run_members trm
        LEFT JOIN runs r
          ON r.id = trm.run_id
         AND r.tenant_id = trm.tenant_id
        WHERE trm.tenant_id = $1
          AND trm.team_run_id = $2
        ORDER BY trm.slot_order ASC, trm.queued_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(team_run_id)
    .fetch_all(&state.connect_pool)
    .await?;

    rows.into_iter()
        .map(agent_team_run_member_from_row)
        .collect()
}

fn agent_team_run_from_row(row: PgRow) -> Result<AgentTeamRunResponse, AppError> {
    Ok(AgentTeamRunResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        team_id: row.try_get("team_id")?,
        conversation_id: row.try_get("conversation_id")?,
        workspace_id: row.try_get("workspace_id")?,
        project_id: row.try_get("project_id")?,
        status: row.try_get("status")?,
        trace_id: row.try_get("trace_id")?,
        thread_id: row.try_get("thread_id")?,
        input_snapshot: row.try_get("input_snapshot")?,
        metadata: row.try_get("metadata")?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn agent_team_run_member_from_row(row: PgRow) -> Result<AgentTeamRunMemberResponse, AppError> {
    Ok(AgentTeamRunMemberResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        team_run_id: row.try_get("team_run_id")?,
        team_member_id: row.try_get("team_member_id")?,
        run_id: row.try_get("run_id")?,
        agent_id: row.try_get("agent_id")?,
        agent_version_id: row.try_get("agent_version_id")?,
        role: row.try_get("role")?,
        display_name: row.try_get("display_name")?,
        slot_order: row.try_get("slot_order")?,
        status: row.try_get("status")?,
        member_snapshot: row.try_get("member_snapshot")?,
        last_error: row.try_get("last_error")?,
        queued_at: row.try_get("queued_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn derive_team_run_status(stored_status: &str, members: &[AgentTeamRunMemberResponse]) -> String {
    if matches!(stored_status, "cancelled" | "failed" | "completed") || members.is_empty() {
        return stored_status.to_string();
    }
    if members.iter().any(|member| member.status == "cancelling") {
        return "cancelling".to_string();
    }
    if members.iter().any(|member| member.status == "blocked") {
        return "waiting_approval".to_string();
    }
    if members.iter().any(|member| {
        matches!(
            member.status.as_str(),
            "queued" | "running" | "waiting_approval"
        )
    }) {
        return "running".to_string();
    }
    if members.iter().all(|member| member.status == "completed") {
        return "completed".to_string();
    }
    if members.iter().any(|member| member.status == "failed") {
        return "failed".to_string();
    }
    if members.iter().any(|member| member.status == "cancelled") {
        return "cancelled".to_string();
    }
    stored_status.to_string()
}

fn non_empty_string(value: String, field: &str) -> Result<String, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(AppError::InvalidInput(format!("{field} must not be empty")))
    } else {
        Ok(trimmed.to_string())
    }
}

fn normalize_member_role(role: &str) -> Result<String, AppError> {
    match role {
        "leader" | "member" => Ok(role.to_string()),
        _ => Err(AppError::InvalidInput(
            "member role must be leader or member".to_string(),
        )),
    }
}

fn validate_team_status(status: &str) -> Result<(), AppError> {
    if matches!(status, "active" | "disabled" | "archived") {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "team status must be active, disabled, or archived".to_string(),
        ))
    }
}

fn validate_member_status(status: &str) -> Result<(), AppError> {
    if matches!(status, "active" | "disabled") {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "team member status must be active or disabled".to_string(),
        ))
    }
}

fn validate_slot_order(slot_order: i32) -> Result<(), AppError> {
    if slot_order < 0 {
        Err(AppError::InvalidInput(
            "slot_order must be greater than or equal to zero".to_string(),
        ))
    } else {
        Ok(())
    }
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
mod tests {
    use super::*;

    fn run_member(status: &str) -> AgentTeamRunMemberResponse {
        AgentTeamRunMemberResponse {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            team_run_id: Uuid::nil(),
            team_member_id: None,
            run_id: None,
            agent_id: None,
            agent_version_id: None,
            role: "member".to_string(),
            display_name: "member".to_string(),
            slot_order: 0,
            status: status.to_string(),
            member_snapshot: json!({}),
            last_error: None,
            queued_at: time::OffsetDateTime::UNIX_EPOCH,
            started_at: None,
            completed_at: None,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn team_row() -> AgentTeamRow {
        AgentTeamRow {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            owner_user_id: Some(Uuid::new_v4()),
            workspace_id: Some(Uuid::new_v4()),
            name: "research team".to_string(),
            description: None,
            status: "active".to_string(),
            metadata: json!({}),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn team_member(team_id: Uuid) -> AgentTeamMemberResponse {
        AgentTeamMemberResponse {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            team_id,
            agent_id: Uuid::new_v4(),
            agent_version_id: Some(Uuid::new_v4()),
            role: "researcher".to_string(),
            display_name: "Researcher".to_string(),
            slot_order: 1,
            policy_snapshot: json!({"tool_policy": "allowlisted"}),
            status: "active".to_string(),
            metadata: json!({}),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn team_member_client_snapshot_uses_server_owned_deepagents_runtime() {
        let team = team_row();
        let mut member = team_member(team.id);
        member.agent_version_id = None;
        let team_run_id = Uuid::new_v4();

        let snapshot = team_member_client_snapshot(
            Some(json!({
                "runtime": { "kind": "biwork_cli" },
                "secret": "raw-client-secret",
                "model": {"provider": "client-provider"},
                "tools": [{"name": "client-tool"}],
                "ui": {"client": "biwork", "conversation_type": "team"},
                "memory_query": "project notes"
            })),
            &team,
            &member,
            team_run_id,
        );

        assert_eq!(
            snapshot.pointer("/runtime/kind"),
            Some(&json!(run_snapshot::PYTHON_RUNTIME_KIND))
        );
        assert_eq!(
            snapshot.pointer("/ui/conversation_type"),
            Some(&json!("team"))
        );
        assert_eq!(
            snapshot.pointer("/memory_query"),
            Some(&json!("project notes"))
        );
        assert_eq!(
            snapshot.pointer("/team/member/agent_version_id"),
            Some(&Value::Null)
        );
        assert_eq!(
            snapshot.pointer("/team/team_run_id"),
            Some(&json!(team_run_id))
        );

        let serialized = snapshot.to_string();
        assert!(!serialized.contains("biwork_cli"));
        assert!(!serialized.contains("raw-client-secret"));
        assert!(!serialized.contains("client-provider"));
        assert!(!serialized.contains("client-tool"));
        assert!(snapshot.get("secret").is_none());
        assert!(snapshot.get("model").is_none());
        assert!(snapshot.get("tools").is_none());
    }

    #[test]
    fn team_member_conversation_uses_biwork_slot_conversation_with_safe_fallback() {
        let team = team_row();
        let mut member = team_member(team.id);
        let fallback = Uuid::new_v4();
        assert_eq!(team_member_conversation_id(&member, fallback), fallback);

        let slot_conversation_id = Uuid::new_v4();
        member.metadata = json!({
            "biwork": { "conversation_id": slot_conversation_id.to_string() }
        });
        assert_eq!(
            team_member_conversation_id(&member, fallback),
            slot_conversation_id
        );
    }

    #[test]
    fn team_run_config_snapshot_uses_server_owned_summary() {
        let team = team_row();
        let member = team_member(team.id);
        let team_run_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let trace_id = "team-trace:1".to_string();
        let client_snapshot = team_member_client_snapshot(
            Some(json!({
                "runtime": { "kind": "biwork_cli" },
                "secret": "raw-client-secret"
            })),
            &team,
            &member,
            team_run_id,
        );
        let prepared = vec![PreparedTeamMemberRun {
            member: member.clone(),
            conversation_id: Uuid::new_v4(),
            thread_id: None,
            run_id,
            trace_id: trace_id.clone(),
            snapshot: client_snapshot,
            scope_snapshot: json!({"scope_secret": "not-for-parent"}),
        }];

        let snapshot = team_run_config_snapshot(&team, team_run_id, &prepared);

        assert_eq!(
            snapshot.pointer("/runtime/kind"),
            Some(&json!(run_snapshot::PYTHON_RUNTIME_KIND))
        );
        assert_eq!(
            snapshot.pointer("/team/team_run_id"),
            Some(&json!(team_run_id))
        );
        assert_eq!(
            snapshot.pointer("/team/members/0/run_id"),
            Some(&json!(run_id))
        );
        assert_eq!(
            snapshot.pointer("/team/members/0/trace_id"),
            Some(&json!(trace_id))
        );

        let serialized = snapshot.to_string();
        assert!(!serialized.contains("raw-client-secret"));
        assert!(!serialized.contains("biwork_cli"));
        assert!(!serialized.contains("scope_secret"));
    }

    #[test]
    fn derives_team_run_status_from_member_runs() {
        assert_eq!(
            derive_team_run_status("running", &[run_member("completed"), run_member("blocked")]),
            "waiting_approval"
        );
        assert_eq!(
            derive_team_run_status("running", &[run_member("completed"), run_member("queued")]),
            "running"
        );
        assert_eq!(
            derive_team_run_status("running", &[run_member("completed"), run_member("failed")]),
            "failed"
        );
        assert_eq!(
            derive_team_run_status("running", &[run_member("cancelling")]),
            "cancelling"
        );
        assert_eq!(
            derive_team_run_status(
                "cancelling",
                &[run_member("completed"), run_member("cancelled")]
            ),
            "cancelled"
        );
    }

    #[test]
    fn submitted_user_content_supports_text_parts() {
        let content = submitted_user_content(&json!({
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "first" },
                    { "type": "text", "text": "second" }
                ]
            }]
        }));

        assert_eq!(content, "first\nsecond");
    }
}
