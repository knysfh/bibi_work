use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            event_store,
            ferriskey_oidc::PlatformRequestContext,
            models::{
                AgentTeamRunDetailResponse, AuthzContext, RunEventInput, StartAgentTeamRunRequest,
            },
            runtime::CancelRunRequest,
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    agent_team_service::create_and_dispatch_agent_team_run,
    biwork_agent_support::{
        BIWORK_ACTIVE_LEASE_SECONDS, biwork_agent_type, biwork_assistant_runtime_disabled_reason,
        runtime_kind,
    },
    biwork_compat_service::{active_lease_payload, biwork_failure, epoch_ms, ok, value_string},
    support::require_ferriskey_allow,
};

#[derive(Debug, Deserialize)]
pub struct TeamListQuery {
    user_id: Option<String>,
}

pub async fn biwork_list_teams(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<TeamListQuery>,
) -> Result<Json<Value>, AppError> {
    if let Some(user_id) = query.user_id.as_deref()
        && let Ok(user_id) = Uuid::parse_str(user_id)
        && user_id != ctx.platform_user_id
    {
        return Ok(ok(json!([])));
    }
    let rows = sqlx::query(
        r#"
        SELECT id
        FROM agent_teams
        WHERE tenant_id = $1
          AND owner_user_id = $2
          AND deleted_at IS NULL
          AND status != 'archived'
        ORDER BY updated_at DESC, created_at DESC
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .fetch_all(&state.connect_pool)
    .await?;
    let mut teams = Vec::with_capacity(rows.len());
    for row in rows {
        teams.push(biwork_load_team(&state, ctx.tenant_id, row.try_get("id")?).await?);
    }
    Ok(ok(json!(teams)))
}

pub async fn biwork_create_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "create",
        "agent_team",
        "new".to_string(),
        None,
    )
    .await?;
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("team name is required".to_string()))?;
    let assistants = payload
        .get("assistants")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("assistants are required".to_string()))?;
    if assistants.is_empty() {
        return Err(AppError::InvalidInput(
            "team requires at least one assistant".to_string(),
        ));
    }
    let metadata = json!({
        "biwork": {
            "workspace": payload.get("workspace").cloned().unwrap_or_else(|| json!("")),
            "workspace_mode": payload.get("workspace_mode").cloned().unwrap_or_else(|| json!("shared")),
            "session_mode": payload.get("session_mode").cloned().unwrap_or_else(|| json!("plan")),
        }
    });

    let mut tx = state.connect_pool.begin().await?;
    let team_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO agent_teams (tenant_id, owner_user_id, name, metadata)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(name)
    .bind(metadata)
    .fetch_one(&mut *tx)
    .await?;

    let mut has_leader = false;
    for (idx, assistant) in assistants.iter().enumerate() {
        let requested_role =
            value_string(assistant, "role").unwrap_or_else(|| "member".to_string());
        let is_leader =
            requested_role == "lead" || requested_role == "leader" || (!has_leader && idx == 0);
        if is_leader {
            has_leader = true;
        }
        insert_biwork_team_member(
            &mut tx,
            ctx.tenant_id,
            team_id,
            assistant,
            idx as i32,
            is_leader,
        )
        .await?;
    }
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(ok(biwork_load_team(&state, ctx.tenant_id, team_id).await?))
}

pub async fn biwork_get_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    Ok(ok(biwork_load_team(&state, ctx.tenant_id, team_id).await?))
}

pub async fn biwork_delete_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "delete",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE agent_teams
        SET status = 'archived',
            deleted_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE agent_team_members
        SET status = 'disabled',
            deleted_at = COALESCE(deleted_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND team_id = $2
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(team_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE conversations
        SET status = 'archived',
            deleted_at = COALESCE(deleted_at, CURRENT_TIMESTAMP),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND metadata #>> '{biwork,team_id}' = $2
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(team_id.to_string())
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(Value::Null))
}

pub async fn biwork_add_team_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    ensure_biwork_team_exists(&state, ctx.tenant_id, team_id).await?;
    let assistant = payload.get("assistant").unwrap_or(&payload);
    let slot_order: i32 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(MAX(slot_order) + 1, 0)::INT
        FROM agent_team_members
        WHERE tenant_id = $1 AND team_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(team_id)
    .fetch_one(&state.connect_pool)
    .await?;
    let requested_role = value_string(assistant, "role").unwrap_or_else(|| "member".to_string());
    let is_leader = requested_role == "lead" || requested_role == "leader";
    let mut tx = state.connect_pool.begin().await?;
    let member_id = insert_biwork_team_member(
        &mut tx,
        ctx.tenant_id,
        team_id,
        assistant,
        slot_order,
        is_leader,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(biwork_load_team_member(
        &state,
        ctx.tenant_id,
        team_id,
        member_id,
    )
    .await?))
}

pub async fn biwork_remove_team_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, slot_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE agent_team_members
        SET deleted_at = CURRENT_TIMESTAMP,
            status = 'disabled',
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND team_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(slot_id)
    .bind(ctx.tenant_id)
    .bind(team_id)
    .execute(&state.connect_pool)
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_rename_team_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, slot_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("name is required".to_string()))?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE agent_team_members
        SET display_name = $4,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND team_id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(slot_id)
    .bind(ctx.tenant_id)
    .bind(team_id)
    .bind(name)
    .execute(&state.connect_pool)
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_rename_team(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("name is required".to_string()))?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "update",
        "agent_team",
        team_id.to_string(),
        None,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE agent_teams
        SET name = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(ctx.tenant_id)
    .bind(name)
    .execute(&state.connect_pool)
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_set_team_session_mode(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let mode = payload
        .get("mode")
        .or_else(|| payload.get("session_mode"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("plan");
    sqlx::query(
        r#"
        UPDATE agent_teams
        SET metadata = jsonb_set(
                jsonb_set(metadata, '{biwork}', COALESCE(metadata->'biwork', '{}'::jsonb), true),
                '{biwork,session_mode}',
                to_jsonb($3::text),
                true
            ),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(ctx.tenant_id)
    .bind(mode)
    .execute(&state.connect_pool)
    .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_ensure_team_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let now = OffsetDateTime::now_utc();
    let session = team_session_payload(
        "active",
        ctx.platform_user_id,
        ctx.session_id,
        ctx.device_id,
        now,
    );
    set_biwork_team_metadata_key(&state, ctx.tenant_id, team_id, "session", &session).await?;
    Ok(ok(team_session_response(session)))
}

pub async fn biwork_stop_team_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let now = OffsetDateTime::now_utc();
    let session = team_session_payload(
        "stopped",
        ctx.platform_user_id,
        ctx.session_id,
        ctx.device_id,
        now,
    );
    set_biwork_team_metadata_key(&state, ctx.tenant_id, team_id, "session", &session).await?;
    Ok(ok(team_session_response(session)))
}

pub async fn biwork_team_active_lease(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let leased_until = OffsetDateTime::now_utc() + Duration::seconds(BIWORK_ACTIVE_LEASE_SECONDS);
    let lease = active_lease_payload(
        ctx.platform_user_id,
        ctx.session_id,
        ctx.device_id,
        leased_until,
    );
    set_biwork_team_metadata_key(&state, ctx.tenant_id, team_id, "active_lease", &lease).await?;
    Ok(ok(json!({
        "leased_until_ms": epoch_ms(leased_until),
        "lease": lease,
    })))
}

pub async fn biwork_team_run_state(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    ensure_biwork_team_exists(&state, ctx.tenant_id, team_id).await?;
    let team_run_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM agent_team_runs
        WHERE tenant_id = $1
          AND team_id = $2
          AND status NOT IN ('completed', 'failed', 'cancelled')
        ORDER BY updated_at DESC, started_at DESC, queued_at DESC
        LIMIT 1
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(team_id)
    .fetch_optional(&state.connect_pool)
    .await?;

    let active_run = match team_run_id {
        Some(team_run_id) => {
            let event = biwork_team_run_event(&state, ctx.tenant_id, team_id, team_run_id).await?;
            match event.get("status").and_then(Value::as_str) {
                Some("completed" | "cancelled" | "failed") => Value::Null,
                _ => event,
            }
        }
        None => Value::Null,
    };
    Ok(ok(json!({ "active_run": active_run })))
}

pub async fn biwork_send_team_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(team_id): Path<Uuid>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let target = resolve_biwork_team_run_target(&state, ctx.tenant_id, team_id, None).await?;
    let detail =
        dispatch_biwork_team_message(&state, &ctx, team_id, &target, payload, None).await?;
    Ok(ok(biwork_team_run_ack(&detail, &target)))
}

pub async fn biwork_send_team_agent_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, slot_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let target =
        resolve_biwork_team_run_target(&state, ctx.tenant_id, team_id, Some(slot_id)).await?;
    let detail =
        dispatch_biwork_team_message(&state, &ctx, team_id, &target, payload, Some(slot_id))
            .await?;
    Ok(ok(biwork_team_run_ack(&detail, &target)))
}

pub async fn biwork_cancel_team_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, team_run_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let reason = payload
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("user_cancelled");
    cancel_biwork_team_run_members(&state, &ctx, team_id, team_run_id, None, reason).await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_cancel_team_agent_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, team_run_id, slot_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let reason = payload
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("user_cancelled");
    cancel_biwork_team_run_members(&state, &ctx, team_id, team_run_id, Some(slot_id), reason)
        .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_pause_team_agent_run(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path((team_id, team_run_id, slot_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let reason = payload
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("user_paused_slot");
    cancel_biwork_team_run_members(&state, &ctx, team_id, team_run_id, Some(slot_id), reason)
        .await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_team_runtime_unavailable() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(biwork_failure(
            "TEAM_RUNTIME_UNAVAILABLE",
            "team runtime capability is not available for this route",
            json!({}),
        )),
    )
}

pub(super) fn team_session_payload(
    state: &str,
    user_id: Uuid,
    session_id: Uuid,
    device_id: Uuid,
    timestamp: OffsetDateTime,
) -> Value {
    let timestamp_ms = epoch_ms(timestamp);
    let mut payload = json!({
        "state": state,
        "holder_user_id": user_id,
        "session_id": session_id,
        "device_id": device_id,
        "updated_at_ms": timestamp_ms,
    });
    if let Some(object) = payload.as_object_mut() {
        let timestamp_key = if state == "stopped" {
            "stopped_at_ms"
        } else {
            "ensured_at_ms"
        };
        object.insert(timestamp_key.to_string(), json!(timestamp_ms));
    }
    payload
}

pub(super) fn team_session_response(session: Value) -> Value {
    json!({
        "state": session.get("state").cloned().unwrap_or(Value::Null),
        "session": session,
    })
}

async fn ensure_biwork_team_exists(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<(), AppError> {
    let exists: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM agent_teams
            WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        )
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::NotFound("team not found".to_string()))
    }
}

async fn set_biwork_team_metadata_key(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    key: &str,
    value: &Value,
) -> Result<(), AppError> {
    let updated: Option<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE agent_teams
        SET metadata = jsonb_set(
                jsonb_set(
                    COALESCE(metadata, '{}'::jsonb),
                    '{biwork}',
                    COALESCE(metadata->'biwork', '{}'::jsonb),
                    true
                ),
                ARRAY['biwork'::text, $3::text],
                $4::jsonb,
                true
            ),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        RETURNING id
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .bind(key)
    .bind(value)
    .fetch_optional(&state.connect_pool)
    .await?;
    if updated.is_some() {
        Ok(())
    } else {
        Err(AppError::NotFound("team not found".to_string()))
    }
}

async fn biwork_load_team(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
) -> Result<Value, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, owner_user_id, name, metadata, created_at, updated_at
        FROM agent_teams
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .bind(tenant_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("team not found".to_string()))?;
    let owner_user_id: Uuid = row
        .try_get::<Option<Uuid>, _>("owner_user_id")?
        .ok_or_else(|| AppError::InvalidInput("team owner is required".to_string()))?;
    let members = biwork_load_team_members(state, tenant_id, owner_user_id, team_id).await?;
    let leader_assistant_id = members
        .iter()
        .find(|member| member.get("role").and_then(Value::as_str) == Some("leader"))
        .and_then(|member| member.get("slot_id").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let metadata: Value = row.try_get("metadata")?;
    let biwork = metadata.get("biwork").unwrap_or(&Value::Null);
    let created_at: OffsetDateTime = row.try_get("created_at")?;
    let updated_at: OffsetDateTime = row.try_get("updated_at")?;
    Ok(json!({
        "id": row.try_get::<Uuid, _>("id")?.to_string(),
        "user_id": owner_user_id.to_string(),
        "name": row.try_get::<String, _>("name")?,
        "workspace": biwork.get("workspace").and_then(Value::as_str).unwrap_or(""),
        "workspace_mode": biwork.get("workspace_mode").and_then(Value::as_str).unwrap_or("shared"),
        "leader_assistant_id": leader_assistant_id,
        "leader_agent_id": leader_assistant_id,
        "assistants": members,
        "agents": members,
        "session_mode": biwork.get("session_mode").and_then(Value::as_str).unwrap_or("plan"),
        "created_at": epoch_ms(created_at),
        "updated_at": epoch_ms(updated_at),
    }))
}

async fn biwork_load_team_members(
    state: &AppState,
    tenant_id: Uuid,
    owner_user_id: Uuid,
    team_id: Uuid,
) -> Result<Vec<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT m.id, m.agent_id, m.role, m.display_name, m.status, m.metadata,
               a.name AS agent_name
        FROM agent_team_members m
        JOIN agents a
          ON a.id = m.agent_id
         AND a.tenant_id = m.tenant_id
        WHERE m.tenant_id = $1
          AND m.team_id = $2
          AND m.deleted_at IS NULL
        ORDER BY m.slot_order ASC, m.created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut members = Vec::with_capacity(rows.len());
    for row in rows {
        members.push(
            biwork_team_member_from_row(state, tenant_id, owner_user_id, team_id, &row).await?,
        );
    }
    Ok(members)
}

async fn biwork_load_team_member(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    member_id: Uuid,
) -> Result<Value, AppError> {
    let row = sqlx::query(
        r#"
        SELECT m.id, m.agent_id, m.role, m.display_name, m.status, m.metadata,
               t.owner_user_id,
               a.name AS agent_name
        FROM agent_team_members m
        JOIN agent_teams t
          ON t.id = m.team_id
         AND t.tenant_id = m.tenant_id
         AND t.deleted_at IS NULL
        JOIN agents a
          ON a.id = m.agent_id
         AND a.tenant_id = m.tenant_id
        WHERE m.id = $1
          AND m.tenant_id = $2
          AND m.team_id = $3
          AND m.deleted_at IS NULL
        "#,
    )
    .bind(member_id)
    .bind(tenant_id)
    .bind(team_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("team agent not found".to_string()))?;
    let owner_user_id: Uuid = row
        .try_get::<Option<Uuid>, _>("owner_user_id")?
        .ok_or_else(|| AppError::InvalidInput("team owner is required".to_string()))?;
    biwork_team_member_from_row(state, tenant_id, owner_user_id, team_id, &row).await
}

async fn biwork_team_member_from_row(
    state: &AppState,
    tenant_id: Uuid,
    owner_user_id: Uuid,
    team_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> Result<Value, AppError> {
    let metadata: Value = row.try_get("metadata")?;
    let biwork = metadata.get("biwork").unwrap_or(&Value::Null);
    let role: String = row.try_get("role")?;
    let status: String = row.try_get("status")?;
    let member_id: Uuid = row.try_get("id")?;
    let agent_id: Uuid = row.try_get("agent_id")?;
    let display_name: String = row.try_get("display_name")?;
    let conversation_id = ensure_biwork_team_member_conversation(
        state,
        BiWorkTeamMemberConversation {
            tenant_id,
            owner_user_id,
            team_id,
            member_id,
            agent_id,
            display_name: &display_name,
            metadata: &metadata,
        },
    )
    .await?;
    Ok(json!({
        "slot_id": member_id.to_string(),
        "conversation_id": conversation_id.to_string(),
        "role": if role == "leader" { "leader" } else { "teammate" },
        "assistant_backend": biwork.get("assistant_backend").and_then(Value::as_str).unwrap_or("deepagents"),
        "backend": biwork.get("assistant_backend").and_then(Value::as_str).unwrap_or("deepagents"),
        "icon": biwork.get("icon").cloned().unwrap_or(Value::Null),
        "assistant_name": display_name,
        "agent_name": row.try_get::<String, _>("agent_name")?,
        "status": if status == "active" { "idle" } else { "error" },
        "assistant_id": agent_id.to_string(),
        "model": biwork.get("model").and_then(Value::as_str).unwrap_or("default"),
        "pending_confirmations": 0,
    }))
}

struct BiWorkTeamRunTarget {
    slot_id: Uuid,
    conversation_id: Uuid,
    role: String,
}

struct BiWorkTeamMemberConversation<'a> {
    tenant_id: Uuid,
    owner_user_id: Uuid,
    team_id: Uuid,
    member_id: Uuid,
    agent_id: Uuid,
    display_name: &'a str,
    metadata: &'a Value,
}

async fn ensure_biwork_team_member_conversation(
    state: &AppState,
    member: BiWorkTeamMemberConversation<'_>,
) -> Result<Uuid, AppError> {
    let BiWorkTeamMemberConversation {
        tenant_id,
        owner_user_id,
        team_id,
        member_id,
        agent_id,
        display_name,
        metadata,
    } = member;
    if let Some(conversation_id) = metadata
        .pointer("/biwork/conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
    {
        return Ok(conversation_id);
    }

    let model = metadata
        .pointer("/biwork/model")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let backend = metadata
        .pointer("/biwork/assistant_backend")
        .and_then(Value::as_str)
        .unwrap_or("deepagents");
    let conversation_metadata = json!({
        "biwork": {
            "type": "acp",
            "assistant": {
                "id": agent_id,
                "name": display_name,
                "backend": backend,
            },
            "model": {
                "model": model,
                "use_model": model,
            },
            "team_id": team_id,
            "team_member_id": member_id,
        },
        "extra": {
            "team_id": team_id,
            "team_member_id": member_id,
            "agent_name": display_name,
            "workspace": "",
        },
    });
    let conversation_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO conversations (
            tenant_id, created_by_user_id, agent_id, title, metadata
        )
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .bind(owner_user_id)
    .bind(agent_id)
    .bind(display_name)
    .bind(conversation_metadata)
    .fetch_one(&state.connect_pool)
    .await?;

    sqlx::query(
        r#"
        UPDATE agent_team_members
        SET metadata = jsonb_set(
                jsonb_set(
                    COALESCE(metadata, '{}'::jsonb),
                    '{biwork}',
                    COALESCE(metadata->'biwork', '{}'::jsonb),
                    true
                ),
                '{biwork,conversation_id}',
                to_jsonb($4::text),
                true
            ),
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND team_id = $2
          AND id = $3
          AND deleted_at IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .bind(member_id)
    .bind(conversation_id.to_string())
    .execute(&state.connect_pool)
    .await?;

    Ok(conversation_id)
}

async fn resolve_biwork_team_run_target(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    target_slot_id: Option<Uuid>,
) -> Result<BiWorkTeamRunTarget, AppError> {
    let team = biwork_load_team(state, tenant_id, team_id).await?;
    let assistants = team
        .get("assistants")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("team assistants are missing".to_string()))?;
    let selected = if let Some(target_slot_id) = target_slot_id {
        assistants
            .iter()
            .find(|assistant| {
                assistant.get("slot_id").and_then(Value::as_str)
                    == Some(target_slot_id.to_string().as_str())
            })
            .ok_or_else(|| AppError::NotFound("team agent not found".to_string()))?
    } else {
        assistants
            .iter()
            .find(|assistant| assistant.get("role").and_then(Value::as_str) == Some("leader"))
            .or_else(|| assistants.first())
            .ok_or_else(|| AppError::InvalidInput("team requires at least one agent".to_string()))?
    };
    let slot_id = selected
        .get("slot_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| AppError::InvalidInput("team agent slot_id is invalid".to_string()))?;
    let conversation_id = selected
        .get("conversation_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| {
            AppError::InvalidInput("team agent conversation_id is invalid".to_string())
        })?;
    let role = selected
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("teammate")
        .to_string();
    Ok(BiWorkTeamRunTarget {
        slot_id,
        conversation_id,
        role,
    })
}

async fn dispatch_biwork_team_message(
    state: &AppState,
    ctx: &PlatformRequestContext,
    team_id: Uuid,
    target: &BiWorkTeamRunTarget,
    payload: Value,
    target_slot_id: Option<Uuid>,
) -> Result<AgentTeamRunDetailResponse, AppError> {
    let content = payload
        .get("content")
        .cloned()
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| json!(""));
    let files = payload.get("files").cloned().unwrap_or_else(|| json!([]));
    let input = json!({
        "messages": [
            {
                "role": "user",
                "content": content,
            }
        ],
        "biwork": {
            "client": "biwork",
            "team_id": team_id,
            "target_slot_id": target_slot_id.unwrap_or(target.slot_id),
            "files": files,
        },
    });
    create_and_dispatch_agent_team_run(
        state,
        ctx,
        team_id,
        StartAgentTeamRunRequest {
            tenant_id: ctx.tenant_id,
            conversation_id: target.conversation_id,
            project_id: None,
            input: Some(input),
            run_config_snapshot: Some(json!({
                "runtime": { "kind": "deepagents" },
                "ui": {
                    "client": "biwork",
                    "conversation_type": "team",
                },
                "team": {
                    "team_id": team_id,
                    "target_slot_id": target_slot_id.unwrap_or(target.slot_id),
                },
            })),
            thread_id: Some(target.conversation_id.to_string()),
            metadata: Some(json!({
                "biwork": {
                    "target_slot_id": target_slot_id.unwrap_or(target.slot_id),
                    "target_role": biwork_team_target_role(&target.role),
                }
            })),
        },
    )
    .await
}

fn biwork_team_run_ack(detail: &AgentTeamRunDetailResponse, target: &BiWorkTeamRunTarget) -> Value {
    json!({
        "team_run_id": detail.team_run.id.to_string(),
        "team_id": detail.team_run.team_id.to_string(),
        "target_slot_id": target.slot_id.to_string(),
        "target_role": biwork_team_target_role(&target.role),
        "accepted_slot_id": target.slot_id.to_string(),
        "accepted_role": biwork_team_target_role(&target.role),
        "status": "accepted",
        "message_id": format!("team.user.{}", detail.team_run.id),
    })
}

async fn biwork_team_run_event(
    state: &AppState,
    tenant_id: Uuid,
    team_id: Uuid,
    team_run_id: Uuid,
) -> Result<Value, AppError> {
    let run_row = sqlx::query(
        r#"
        SELECT id, status, metadata
        FROM agent_team_runs
        WHERE tenant_id = $1
          AND team_id = $2
          AND id = $3
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .bind(team_run_id)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("team run not found".to_string()))?;
    let metadata: Value = run_row.try_get("metadata")?;
    let target_slot_id = metadata
        .pointer("/biwork/target_slot_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let member_rows = sqlx::query(
        r#"
        SELECT trm.team_member_id, trm.run_id, trm.role,
               CASE
                   WHEN r.status = 'waiting_approval' THEN 'running'
                   WHEN r.status IS NOT NULL THEN r.status
                   ELSE trm.status
               END AS status
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

    let mut slot_work = Vec::with_capacity(member_rows.len());
    let mut active_child_count = 0i64;
    let mut pending_wake_count = 0i64;
    let mut fallback_slot_id = None;
    let mut fallback_role = "teammate".to_string();
    let mut selected_role = None;
    let mut member_statuses = Vec::new();
    for row in member_rows {
        let slot_id: Option<Uuid> = row.try_get("team_member_id")?;
        let Some(slot_id) = slot_id else {
            continue;
        };
        let role: String = row.try_get("role")?;
        if fallback_slot_id.is_none() {
            fallback_slot_id = Some(slot_id);
            fallback_role = role.clone();
        }
        if Some(slot_id) == target_slot_id {
            selected_role = Some(role.clone());
        }
        let status: String = row.try_get("status")?;
        member_statuses.push(status.clone());
        let is_pending = matches!(status.as_str(), "queued" | "pending");
        let is_active = matches!(
            status.as_str(),
            "running" | "waiting_approval" | "cancelling" | "canceling"
        );
        if is_pending {
            pending_wake_count += 1;
        }
        if is_active {
            active_child_count += 1;
        }
        let run_id: Option<Uuid> = row.try_get("run_id")?;
        slot_work.push(json!({
            "slot_id": slot_id.to_string(),
            "role": biwork_team_target_role(&role),
            "pending_wake_count": if is_pending { 1 } else { 0 },
            "starting_child_count": 0,
            "active_turn_id": if is_active { run_id.map(|id| id.to_string()) } else { None },
        }));
    }
    let effective_slot_id = target_slot_id
        .or(fallback_slot_id)
        .ok_or_else(|| AppError::InvalidInput("team run has no members".to_string()))?;
    let effective_role = selected_role.unwrap_or(fallback_role);
    let status: String = run_row.try_get("status")?;
    let status = biwork_team_run_state_status(&status, &member_statuses);
    Ok(json!({
        "team_id": team_id.to_string(),
        "team_run_id": team_run_id.to_string(),
        "target_slot_id": effective_slot_id.to_string(),
        "target_role": biwork_team_target_role(&effective_role),
        "status": status,
        "active_child_count": active_child_count,
        "pending_wake_count": pending_wake_count,
        "starting_child_count": 0,
        "slot_work": slot_work,
    }))
}

async fn cancel_biwork_team_run_members(
    state: &AppState,
    ctx: &PlatformRequestContext,
    team_id: Uuid,
    team_run_id: Uuid,
    target_slot_id: Option<Uuid>,
    reason: &str,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        r#"
        SELECT tr.conversation_id, tr.project_id, tr.trace_id, trm.id AS run_member_id,
               trm.team_member_id, trm.run_id, trm.slot_order, trm.role
        FROM agent_team_runs tr
        JOIN agent_team_run_members trm
          ON trm.team_run_id = tr.id
         AND trm.tenant_id = tr.tenant_id
        WHERE tr.tenant_id = $1
          AND tr.team_id = $2
          AND tr.id = $3
          AND ($4::uuid IS NULL OR trm.team_member_id = $4)
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(team_id)
    .bind(team_run_id)
    .bind(target_slot_id)
    .fetch_all(&state.connect_pool)
    .await?;
    if rows.is_empty() {
        return Err(AppError::NotFound("team run not found".to_string()));
    }
    let conversation_id: Uuid = rows[0].try_get("conversation_id")?;
    let project_id: Option<Uuid> = rows[0].try_get("project_id")?;
    require_ferriskey_allow(
        state,
        ctx,
        ctx.tenant_id,
        "cancel",
        "run",
        team_run_id.to_string(),
        Some(AuthzContext {
            conversation_id: Some(conversation_id),
            project_id,
            ..Default::default()
        }),
    )
    .await?;

    let run_member_ids = rows
        .iter()
        .map(|row| row.try_get::<Uuid, _>("run_member_id"))
        .collect::<Result<Vec<_>, _>>()?;
    let run_ids = rows
        .iter()
        .filter_map(|row| row.try_get::<Option<Uuid>, _>("run_id").ok().flatten())
        .collect::<Vec<_>>();
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE agent_team_run_members
        SET status = 'cancelling',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND id = ANY($2)
          AND status NOT IN ('completed', 'failed', 'cancelled', 'cancelling')
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&run_member_ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE runs
        SET status = 'cancelling',
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND id = ANY($2)
          AND status NOT IN ('completed', 'failed', 'cancelled', 'cancelling')
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&run_ids)
    .execute(&mut *tx)
    .await?;

    let mut persisted_events = Vec::new();
    for row in &rows {
        let run_member_id: Uuid = row.try_get("run_member_id")?;
        let team_member_id: Option<Uuid> = row.try_get("team_member_id")?;
        let run_id: Option<Uuid> = row.try_get("run_id")?;
        let slot_order: i32 = row.try_get("slot_order")?;
        let role: String = row.try_get("role")?;
        let event = event_store::insert_event_tx(
            &mut tx,
            ctx.tenant_id,
            conversation_id,
            run_id,
            RunEventInput {
                event_id: Some(format!(
                    "team.member.cancelling.{team_run_id}.{run_member_id}"
                )),
                event_type: "team.member.updated".to_string(),
                payload: Some(json!({
                    "team_id": team_id,
                    "team_run_id": team_run_id,
                    "team_member_id": team_member_id.unwrap_or(run_member_id),
                    "team_run_member_id": run_member_id,
                    "run_id": run_id,
                    "slot_order": slot_order,
                    "role": role,
                    "status": "cancelling",
                    "reason": reason
                })),
                trace_id: rows[0].try_get("trace_id").ok(),
            },
        )
        .await?;
        persisted_events.push(event);
    }

    if target_slot_id.is_none() {
        sqlx::query(
            r#"
            UPDATE agent_team_runs
            SET status = 'cancelling',
                updated_at = CURRENT_TIMESTAMP
            WHERE tenant_id = $1
              AND id = $2
              AND status NOT IN ('completed', 'failed', 'cancelled', 'cancelling')
            "#,
        )
        .bind(ctx.tenant_id)
        .bind(team_run_id)
        .execute(&mut *tx)
        .await?;
        let event = event_store::insert_event_tx(
            &mut tx,
            ctx.tenant_id,
            conversation_id,
            None,
            RunEventInput {
                event_id: Some(format!("team.run.cancelling.{team_run_id}")),
                event_type: "team.run.updated".to_string(),
                payload: Some(json!({
                    "team_id": team_id,
                    "team_run_id": team_run_id,
                    "status": "cancelling",
                    "reason": reason
                })),
                trace_id: rows[0].try_get("trace_id").ok(),
            },
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

    for run_id in run_ids {
        let _ = state
            .agent_runtime_client
            .cancel_run(
                run_id,
                &CancelRunRequest {
                    tenant_id: ctx.tenant_id,
                    conversation_id,
                    trace_id: rows[0].try_get("trace_id").ok(),
                    reason: reason.to_string(),
                },
            )
            .await;
    }
    Ok(())
}

fn biwork_team_target_role(role: &str) -> &'static str {
    if role == "leader" { "lead" } else { "teammate" }
}

pub(super) fn biwork_team_status(status: &str) -> &'static str {
    match status {
        "accepted" => "accepted",
        "queued" | "pending" | "running" | "waiting_approval" | "blocked" => "running",
        "cancelling" | "canceling" => "cancelling",
        "completed" => "completed",
        "cancelled" => "cancelled",
        "failed" => "failed",
        _ => "running",
    }
}

pub(super) fn biwork_team_run_state_status(
    stored_status: &str,
    member_statuses: &[String],
) -> &'static str {
    if matches!(stored_status, "completed" | "failed" | "cancelled") || member_statuses.is_empty() {
        return biwork_team_status(stored_status);
    }
    if member_statuses
        .iter()
        .any(|status| matches!(status.as_str(), "cancelling" | "canceling"))
    {
        return "cancelling";
    }
    let has_active_member = member_statuses.iter().any(|status| {
        matches!(
            status.as_str(),
            "queued" | "pending" | "running" | "waiting_approval" | "blocked"
        )
    });
    if matches!(stored_status, "cancelling" | "canceling") && has_active_member {
        return "cancelling";
    }
    if has_active_member {
        return biwork_team_status(stored_status);
    }
    if member_statuses.iter().any(|status| status == "failed") {
        return "failed";
    }
    if member_statuses.iter().any(|status| status == "cancelled") {
        return "cancelled";
    }
    if member_statuses.iter().all(|status| status == "completed") {
        return "completed";
    }
    biwork_team_status(stored_status)
}

async fn insert_biwork_team_member(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    team_id: Uuid,
    assistant: &Value,
    slot_order: i32,
    is_leader: bool,
) -> Result<Uuid, AppError> {
    let agent_id = value_string(assistant, "assistant_id")
        .or_else(|| value_string(assistant, "id"))
        .ok_or_else(|| AppError::InvalidInput("assistant_id is required".to_string()))
        .and_then(|id| {
            Uuid::parse_str(&id)
                .map_err(|_| AppError::InvalidInput("assistant_id must be a UUID".to_string()))
        })?;
    let agent_row = sqlx::query(
        r#"
        SELECT a.name, a.status, a.metadata,
               (
                   SELECT av.id
                   FROM agent_versions av
                   WHERE av.agent_id = a.id
                     AND av.tenant_id = a.tenant_id
                     AND av.status = 'published'
                   ORDER BY av.created_at DESC
                   LIMIT 1
               ) AS agent_version_id,
               COALESCE((
                   SELECT av.config_snapshot
                   FROM agent_versions av
                   WHERE av.agent_id = a.id AND av.tenant_id = a.tenant_id
                   ORDER BY (av.status = 'published') DESC, av.created_at DESC
                   LIMIT 1
               ), a.draft_config) AS config
        FROM agents a
        WHERE a.id = $1 AND a.tenant_id = $2 AND a.deleted_at IS NULL
        "#,
    )
    .bind(agent_id)
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::NotFound("assistant not found".to_string()))?;
    let agent_name: String = agent_row.try_get("name")?;
    let agent_status: String = agent_row.try_get("status")?;
    let agent_metadata: Value = agent_row.try_get("metadata")?;
    let agent_config: Value = agent_row.try_get("config")?;
    let agent_version_id: Option<Uuid> = agent_row.try_get("agent_version_id")?;
    if let Some(reason) =
        biwork_team_member_block_reason(&agent_status, &agent_config, &agent_metadata)
    {
        return Err(AppError::Conflict(format!(
            "assistant is not selectable for team: {reason}"
        )));
    }
    if is_leader {
        sqlx::query(
            r#"
            UPDATE agent_team_members
            SET role = 'member', updated_at = CURRENT_TIMESTAMP
            WHERE tenant_id = $1
              AND team_id = $2
              AND role = 'leader'
              AND deleted_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(team_id)
        .execute(&mut **tx)
        .await?;
    }
    let display_name = value_string(assistant, "assistant_name")
        .or_else(|| value_string(assistant, "name"))
        .unwrap_or(agent_name);
    let metadata = json!({
        "biwork": {
            "model": value_string(assistant, "model").unwrap_or_else(|| "default".to_string()),
            "assistant_backend": value_string(assistant, "assistant_backend")
                .or_else(|| value_string(assistant, "backend"))
                .unwrap_or_else(|| "deepagents".to_string()),
            "icon": assistant.get("icon").cloned().unwrap_or(Value::Null),
        }
    });
    sqlx::query_scalar(
        r#"
        INSERT INTO agent_team_members (
            tenant_id, team_id, agent_id, agent_version_id, role, display_name, slot_order, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(tenant_id)
    .bind(team_id)
    .bind(agent_id)
    .bind(agent_version_id)
    .bind(if is_leader { "leader" } else { "member" })
    .bind(display_name)
    .bind(slot_order)
    .bind(metadata)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

pub(super) fn biwork_team_member_block_reason(
    status: &str,
    config: &Value,
    metadata: &Value,
) -> Option<String> {
    if status == "disabled" {
        return Some("assistant is disabled".to_string());
    }
    let runtime = runtime_kind(config, metadata);
    if runtime == "biwork_cli" {
        return Some("desktop local agent runtime is not supported for team execution".to_string());
    }
    let agent_type = biwork_agent_type(&runtime, metadata);
    biwork_assistant_runtime_disabled_reason(&runtime, &agent_type)
}
