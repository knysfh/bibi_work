use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext,
            memory_context::{DEFAULT_MAX_MEMORY_CONTEXT_CHARS, memory_context_from_item},
            memory_vector::{MemoryVectorSearchRequest, score_by_memory_id},
            models::*,
        },
        core::{errors::AppError, models::GenericResponse},
    },
    startup::AppState,
};

use super::support::*;

pub async fn upsert_memory(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<CreateMemoryRequest>,
) -> Result<Json<MemoryItemResponse>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let user_id = payload.user_id.unwrap_or(ctx.platform_user_id);
    let layer = normalize_memory_layer(&payload.layer)?;
    let confidence = normalize_memory_confidence(payload.confidence)?;
    let status = normalize_initial_memory_status(
        &layer,
        payload
            .status
            .as_deref()
            .unwrap_or(DEFAULT_WRITE_MEMORY_STATUS),
    )?;
    let visibility = normalize_memory_visibility(
        payload
            .visibility
            .as_deref()
            .unwrap_or(DEFAULT_MEMORY_VISIBILITY),
    )?;
    let sensitivity = normalize_memory_sensitivity(
        payload
            .sensitivity
            .as_deref()
            .unwrap_or(DEFAULT_MEMORY_SENSITIVITY),
    )?;

    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "update",
        "memory",
        user_id.to_string(),
        Some(AuthzContext {
            agent_id: payload.agent_id,
            project_id: payload.project_id,
            run_id: payload.source_run_id,
            ..Default::default()
        }),
    )
    .await?;
    let content_hash = sha256_hex(payload.content.as_bytes());
    let row = sqlx::query(
        r#"
        INSERT INTO memory_items (
            tenant_id, user_id, agent_id, project_id, layer, content, content_hash,
            source_run_id, confidence, status, visibility, retention_policy, sensitivity
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
        RETURNING id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
                  status, visibility, sensitivity, created_at, updated_at
        "#,
    )
    .bind(payload.tenant_id)
    .bind(user_id)
    .bind(payload.agent_id)
    .bind(payload.project_id)
    .bind(layer)
    .bind(payload.content)
    .bind(content_hash)
    .bind(payload.source_run_id)
    .bind(confidence)
    .bind(status)
    .bind(visibility)
    .bind(
        payload
            .retention_policy
            .unwrap_or_else(|| "default".to_string()),
    )
    .bind(sensitivity)
    .fetch_one(&state.connect_pool)
    .await?;

    let memory = memory_from_row(row)?;
    enqueue_memory_indexing(&state.connect_pool, &memory, "upsert").await?;
    write_memory_access_log(
        &state.connect_pool,
        memory.tenant_id,
        Some(memory.id),
        ctx.platform_user_id,
        memory.agent_id,
        payload.source_run_id,
        "create",
    )
    .await?;

    Ok(Json(memory))
}

pub async fn list_memories(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Vec<MemoryItemResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, query.tenant_id, ctx.platform_user_id).await?;
    let user_id = query.user_id.unwrap_or(ctx.platform_user_id);
    let layer = normalize_optional_memory_layer(query.layer.as_deref())?;
    let status = normalize_memory_status(
        query
            .status
            .as_deref()
            .unwrap_or(DEFAULT_READ_MEMORY_STATUS),
    )?;
    let content_query = normalize_memory_query(query.query.as_deref());
    require_ferriskey_allow(
        &state,
        &ctx,
        query.tenant_id,
        "read",
        "memory",
        user_id.to_string(),
        Some(AuthzContext {
            agent_id: query.agent_id,
            project_id: query.project_id,
            ..Default::default()
        }),
    )
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
               status, visibility, sensitivity, created_at, updated_at
        FROM memory_items
        WHERE tenant_id = $1
          AND ($2::uuid IS NULL OR user_id = $2)
          AND ($3::uuid IS NULL OR agent_id = $3)
          AND ($4::uuid IS NULL OR project_id = $4)
          AND ($5::text IS NULL OR layer = $5)
          AND status = $6
          AND ($7::text IS NULL OR content ILIKE '%' || $7 || '%')
          AND deleted_at IS NULL
        ORDER BY updated_at DESC
        LIMIT $8
        "#,
    )
    .bind(query.tenant_id)
    .bind(user_id)
    .bind(query.agent_id)
    .bind(query.project_id)
    .bind(layer)
    .bind(status)
    .bind(content_query)
    .bind(normalize_memory_limit(query.limit))
    .fetch_all(&state.connect_pool)
    .await?;

    let memories = rows
        .into_iter()
        .map(memory_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    write_memory_read_logs(
        &state.connect_pool,
        &memories,
        ctx.platform_user_id,
        query.run_id,
        "read",
    )
    .await?;
    Ok(Json(memories))
}

pub async fn search_memories(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<MemorySearchRequest>,
) -> Result<Json<Vec<MemoryItemResponse>>, AppError> {
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;
    let user_id = payload.user_id.unwrap_or(ctx.platform_user_id);
    let layer = normalize_optional_memory_layer(payload.layer.as_deref())?;
    let status = normalize_memory_status(
        payload
            .status
            .as_deref()
            .unwrap_or(DEFAULT_READ_MEMORY_STATUS),
    )?;
    let content_query = normalize_memory_query(payload.query.as_deref());
    require_ferriskey_allow(
        &state,
        &ctx,
        payload.tenant_id,
        "read",
        "memory",
        user_id.to_string(),
        Some(AuthzContext {
            agent_id: payload.agent_id,
            project_id: payload.project_id,
            run_id: payload.run_id,
            ..Default::default()
        }),
    )
    .await?;

    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
               status, visibility, sensitivity, created_at, updated_at
        FROM memory_items
        WHERE tenant_id = $1
          AND ($2::uuid IS NULL OR user_id = $2)
          AND ($3::uuid IS NULL OR agent_id = $3)
          AND ($4::uuid IS NULL OR project_id = $4)
          AND ($5::text IS NULL OR layer = $5)
          AND status = $6
          AND ($7::text IS NULL OR content ILIKE '%' || $7 || '%')
          AND deleted_at IS NULL
        ORDER BY confidence DESC, updated_at DESC
        LIMIT $8
        "#,
    )
    .bind(payload.tenant_id)
    .bind(user_id)
    .bind(payload.agent_id)
    .bind(payload.project_id)
    .bind(layer)
    .bind(status)
    .bind(content_query)
    .bind(normalize_memory_limit(payload.limit))
    .fetch_all(&state.connect_pool)
    .await?;

    let memories = rows
        .into_iter()
        .map(memory_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;
    write_memory_read_logs(
        &state.connect_pool,
        &memories,
        ctx.platform_user_id,
        payload.run_id,
        "search",
    )
    .await?;
    if memories.is_empty() {
        write_memory_access_log(
            &state.connect_pool,
            payload.tenant_id,
            None,
            ctx.platform_user_id,
            payload.agent_id,
            payload.run_id,
            "search",
        )
        .await?;
    }

    Ok(Json(memories))
}

pub async fn internal_memory_retrieve_for_run(
    State(state): State<AppState>,
    Json(payload): Json<MemoryRetrieveForRunRequest>,
) -> Result<Json<MemoryRetrieveForRunResponse>, AppError> {
    retrieve_memory_context_for_run(&state, payload)
        .await
        .map(Json)
}

pub async fn internal_memory_candidates(
    State(state): State<AppState>,
    Json(payload): Json<MemoryCandidatesRequest>,
) -> Result<Json<MemoryCandidatesResponse>, AppError> {
    create_memory_candidates(&state, payload).await.map(Json)
}

pub async fn internal_memory_access_log(
    State(state): State<AppState>,
    Json(payload): Json<MemoryAccessLogRequest>,
) -> Result<Json<GenericResponse>, AppError> {
    ensure_tenant_member(
        &state.connect_pool,
        payload.tenant_id,
        payload.actor.user_id,
    )
    .await?;
    let action = normalize_memory_access_action(&payload.action)?;
    let mut agent_id = payload.agent_id;

    if let Some(memory_id) = payload.memory_id {
        let memory = load_memory(&state.connect_pool, memory_id).await?;
        if memory.tenant_id != payload.tenant_id {
            return Err(AppError::NotFound("memory not found".to_string()));
        }
        agent_id = agent_id.or(memory.agent_id);
        require_ferriskey_allow_for_actor(
            &state,
            payload.tenant_id,
            payload.actor.clone(),
            "read",
            "memory",
            memory_authz_resource_id(&memory),
            Some(AuthzContext {
                agent_id,
                project_id: memory.project_id,
                run_id: payload.run_id,
                ..Default::default()
            }),
        )
        .await?;
    }

    write_memory_access_log(
        &state.connect_pool,
        payload.tenant_id,
        payload.memory_id,
        payload.actor.user_id,
        agent_id,
        payload.run_id,
        &action,
    )
    .await?;

    Ok(Json(GenericResponse {
        code: "MEMORY_ACCESS_LOGGED".to_string(),
        message: "Memory access log recorded".to_string(),
    }))
}

pub(super) async fn create_candidates_from_run_completed_event(
    state: &AppState,
    run_id: Option<Uuid>,
    source_event_id: Uuid,
    event_payload: &Value,
) -> Result<Vec<MemoryItemResponse>, AppError> {
    let Some(run_id) = run_id else {
        return Ok(Vec::new());
    };
    let candidates = memory_candidates_from_event_payload(event_payload);
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let run = load_run_memory_scope(&state.connect_pool, run_id).await?;
    let user_id = run.created_by_user_id.ok_or_else(|| {
        AppError::InvalidInput(
            "run is missing created_by_user_id for memory candidates".to_string(),
        )
    })?;
    let response = create_memory_candidates(
        state,
        MemoryCandidatesRequest {
            tenant_id: run.tenant_id,
            actor: ActorRef {
                user_id,
                device_id: None,
                session_id: None,
                roles: Vec::new(),
            },
            run_id: Some(run_id),
            user_id: Some(user_id),
            agent_id: run.agent_id,
            project_id: run.project_id,
            source_event_id: Some(source_event_id),
            candidates,
        },
    )
    .await?;
    Ok(response.memories)
}

pub(super) async fn retrieve_memory_context_for_run(
    state: &AppState,
    payload: MemoryRetrieveForRunRequest,
) -> Result<MemoryRetrieveForRunResponse, AppError> {
    ensure_tenant_member(
        &state.connect_pool,
        payload.tenant_id,
        payload.actor.user_id,
    )
    .await?;
    let user_id = payload.user_id.unwrap_or(payload.actor.user_id);
    let layer = normalize_optional_memory_layer(payload.layer.as_deref())?;
    let query = normalize_required_memory_query(&payload.query)?;
    let limit = normalize_memory_limit(payload.limit);
    let min_score = normalize_min_score(payload.min_score)?;

    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        payload.actor.clone(),
        "read",
        "memory",
        user_id.to_string(),
        Some(AuthzContext {
            agent_id: payload.agent_id,
            project_id: payload.project_id,
            run_id: payload.run_id,
            ..Default::default()
        }),
    )
    .await?;

    let mut vector_attempted = false;
    let mut vector_error = None;
    let mut source = "memory_keyword_search".to_string();
    let mut memories = Vec::new();

    if state.memory_vector_client.is_enabled() {
        vector_attempted = true;
        match state
            .memory_vector_client
            .search_memory_ids(MemoryVectorSearchRequest {
                tenant_id: payload.tenant_id,
                user_id: Some(user_id),
                agent_id: payload.agent_id,
                project_id: payload.project_id,
                layer: layer.clone(),
                query: query.clone(),
                limit: usize::try_from(limit)?,
                min_score,
            })
            .await
        {
            Ok(hits) if !hits.is_empty() => {
                memories = load_memory_contexts_by_ids(
                    &state.connect_pool,
                    MemoryContextScope {
                        tenant_id: payload.tenant_id,
                        user_id,
                        agent_id: payload.agent_id,
                        project_id: payload.project_id,
                        layer: layer.as_deref(),
                        max_context_chars: state.memory_vector_client.max_context_chars(),
                    },
                    hits,
                )
                .await?;
                if !memories.is_empty() {
                    source = "memory_vector_search".to_string();
                }
            }
            Ok(_) => {}
            Err(err) => vector_error = Some(err),
        }
    }

    if memories.is_empty() {
        memories = load_memory_contexts_by_keyword(
            &state.connect_pool,
            MemoryContextScope {
                tenant_id: payload.tenant_id,
                user_id,
                agent_id: payload.agent_id,
                project_id: payload.project_id,
                layer: layer.as_deref(),
                max_context_chars: DEFAULT_MAX_MEMORY_CONTEXT_CHARS,
            },
            &query,
            limit,
        )
        .await?;
    }

    write_memory_context_logs(
        &state.connect_pool,
        payload.tenant_id,
        &memories,
        payload.actor.user_id,
        payload.agent_id,
        payload.run_id,
        &source,
    )
    .await?;

    Ok(MemoryRetrieveForRunResponse {
        memories,
        source,
        vector_attempted,
        vector_error,
    })
}

async fn create_memory_candidates(
    state: &AppState,
    payload: MemoryCandidatesRequest,
) -> Result<MemoryCandidatesResponse, AppError> {
    ensure_tenant_member(
        &state.connect_pool,
        payload.tenant_id,
        payload.actor.user_id,
    )
    .await?;
    if payload.candidates.is_empty() {
        return Ok(MemoryCandidatesResponse {
            memories: Vec::new(),
        });
    }

    let user_id = payload.user_id.unwrap_or(payload.actor.user_id);
    require_ferriskey_allow_for_actor(
        state,
        payload.tenant_id,
        payload.actor.clone(),
        "update",
        "memory",
        user_id.to_string(),
        Some(AuthzContext {
            agent_id: payload.agent_id,
            project_id: payload.project_id,
            run_id: payload.run_id,
            ..Default::default()
        }),
    )
    .await?;

    let mut tx = state.connect_pool.begin().await?;
    let mut memories = Vec::new();
    for candidate in payload.candidates {
        let candidate = normalize_memory_candidate(candidate)?;
        let memory = upsert_memory_candidate_tx(
            &mut tx,
            MemoryCandidateInsert {
                tenant_id: payload.tenant_id,
                user_id,
                agent_id: payload.agent_id,
                project_id: payload.project_id,
                source_run_id: payload.run_id,
                source_event_id: payload.source_event_id,
                layer: candidate.layer,
                content: candidate.content,
                confidence: candidate.confidence,
                visibility: candidate.visibility,
                retention_policy: candidate.retention_policy,
                sensitivity: candidate.sensitivity,
            },
        )
        .await?;
        enqueue_memory_indexing_tx(&mut tx, &memory, "candidate").await?;
        write_memory_access_log_tx(
            &mut tx,
            memory.tenant_id,
            Some(memory.id),
            payload.actor.user_id,
            memory.agent_id,
            payload.run_id,
            "candidate_create",
        )
        .await?;
        memories.push(memory);
    }
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    Ok(MemoryCandidatesResponse { memories })
}

pub async fn activate_memory(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(memory_id): Path<Uuid>,
) -> Result<Json<MemoryItemResponse>, AppError> {
    decide_memory_status(state, ctx, memory_id, MemoryStatusDecision::activate()).await
}

pub async fn reject_memory(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(memory_id): Path<Uuid>,
) -> Result<Json<MemoryItemResponse>, AppError> {
    decide_memory_status(state, ctx, memory_id, MemoryStatusDecision::reject()).await
}

pub async fn archive_memory(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(memory_id): Path<Uuid>,
) -> Result<Json<MemoryItemResponse>, AppError> {
    decide_memory_status(state, ctx, memory_id, MemoryStatusDecision::archive()).await
}

pub async fn batch_decide_memories(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<MemoryBatchDecisionRequest>,
) -> Result<Json<MemoryBatchDecisionResponse>, AppError> {
    batch_decide_memory_status(&state, &ctx, payload)
        .await
        .map(Json)
}

async fn decide_memory_status(
    state: AppState,
    ctx: PlatformRequestContext,
    memory_id: Uuid,
    decision: MemoryStatusDecision,
) -> Result<Json<MemoryItemResponse>, AppError> {
    decide_memory_status_one(&state, &ctx, memory_id, None, decision, None)
        .await
        .map(Json)
}

async fn batch_decide_memory_status(
    state: &AppState,
    ctx: &PlatformRequestContext,
    payload: MemoryBatchDecisionRequest,
) -> Result<MemoryBatchDecisionResponse, AppError> {
    let decision = normalize_memory_decision(&payload.decision)?;
    let memory_ids = normalize_memory_batch_ids(payload.memory_ids)?;
    ensure_tenant_member(&state.connect_pool, payload.tenant_id, ctx.platform_user_id).await?;

    let mut results = Vec::with_capacity(memory_ids.len());
    for memory_id in memory_ids {
        match decide_memory_status_one(
            state,
            ctx,
            memory_id,
            Some(payload.tenant_id),
            decision,
            payload.run_id,
        )
        .await
        {
            Ok(memory) => results.push(MemoryBatchDecisionResult {
                memory_id,
                status: "succeeded".to_string(),
                memory: Some(memory),
                error_code: None,
                error_message: None,
            }),
            Err(err) if is_memory_batch_item_error(&err) => {
                let (error_code, error_message) = memory_batch_error(&err);
                results.push(MemoryBatchDecisionResult {
                    memory_id,
                    status: "failed".to_string(),
                    memory: None,
                    error_code: Some(error_code),
                    error_message: Some(error_message),
                });
            }
            Err(err) => return Err(err),
        }
    }

    let succeeded = results
        .iter()
        .filter(|result| result.status == "succeeded")
        .count();
    let failed = results.len().saturating_sub(succeeded);

    Ok(MemoryBatchDecisionResponse {
        decision: decision.canonical_decision.to_string(),
        target_status: decision.target_status.to_string(),
        succeeded,
        failed,
        results,
    })
}

async fn decide_memory_status_one(
    state: &AppState,
    ctx: &PlatformRequestContext,
    memory_id: Uuid,
    expected_tenant_id: Option<Uuid>,
    decision: MemoryStatusDecision,
    run_id: Option<Uuid>,
) -> Result<MemoryItemResponse, AppError> {
    let current = load_memory(&state.connect_pool, memory_id).await?;
    if expected_tenant_id.is_some_and(|tenant_id| tenant_id != current.tenant_id) {
        return Err(AppError::NotFound("memory not found".to_string()));
    }
    ensure_tenant_member(&state.connect_pool, current.tenant_id, ctx.platform_user_id).await?;
    require_ferriskey_allow(
        state,
        ctx,
        current.tenant_id,
        "update",
        "memory",
        memory_authz_resource_id(&current),
        Some(AuthzContext {
            agent_id: current.agent_id,
            project_id: current.project_id,
            run_id,
            ..Default::default()
        }),
    )
    .await?;

    let row = sqlx::query(
        r#"
        UPDATE memory_items
        SET status = $2,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1
          AND deleted_at IS NULL
        RETURNING id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
                  status, visibility, sensitivity, created_at, updated_at
    "#,
    )
    .bind(memory_id)
    .bind(decision.target_status)
    .fetch_optional(&state.connect_pool)
    .await?
    .ok_or_else(|| AppError::NotFound("memory not found".to_string()))?;

    let memory = memory_from_row(row)?;
    enqueue_memory_indexing(&state.connect_pool, &memory, "status_changed").await?;
    write_memory_access_log(
        &state.connect_pool,
        memory.tenant_id,
        Some(memory.id),
        ctx.platform_user_id,
        memory.agent_id,
        run_id,
        decision.action,
    )
    .await?;
    Ok(memory)
}

async fn enqueue_memory_indexing(
    pool: &PgPool,
    memory: &MemoryItemResponse,
    job_type: &'static str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO memory_embeddings (
            memory_id, tenant_id, qdrant_point_id, index_status, last_error, updated_at
        )
        VALUES ($1, $2, $3, 'pending', NULL, CURRENT_TIMESTAMP)
        ON CONFLICT (memory_id) DO UPDATE
        SET tenant_id = EXCLUDED.tenant_id,
            qdrant_point_id = EXCLUDED.qdrant_point_id,
            index_status = 'pending',
            last_error = NULL,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(memory.id)
    .bind(memory.tenant_id)
    .bind(memory.id.to_string())
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO memory_ingestion_jobs (tenant_id, memory_id, job_type, status)
        VALUES ($1, $2, $3, 'pending')
        "#,
    )
    .bind(memory.tenant_id)
    .bind(memory.id)
    .bind(job_type)
    .execute(pool)
    .await?;
    Ok(())
}

async fn enqueue_memory_indexing_tx(
    tx: &mut Transaction<'_, Postgres>,
    memory: &MemoryItemResponse,
    job_type: &'static str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO memory_embeddings (
            memory_id, tenant_id, qdrant_point_id, index_status, last_error, updated_at
        )
        VALUES ($1, $2, $3, 'pending', NULL, CURRENT_TIMESTAMP)
        ON CONFLICT (memory_id) DO UPDATE
        SET tenant_id = EXCLUDED.tenant_id,
            qdrant_point_id = EXCLUDED.qdrant_point_id,
            index_status = 'pending',
            last_error = NULL,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(memory.id)
    .bind(memory.tenant_id)
    .bind(memory.id.to_string())
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO memory_ingestion_jobs (tenant_id, memory_id, job_type, status)
        VALUES ($1, $2, $3, 'pending')
        "#,
    )
    .bind(memory.tenant_id)
    .bind(memory.id)
    .bind(job_type)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

struct NormalizedMemoryCandidate {
    content: String,
    layer: String,
    confidence: f64,
    visibility: String,
    retention_policy: String,
    sensitivity: String,
}

struct MemoryCandidateInsert {
    tenant_id: Uuid,
    user_id: Uuid,
    agent_id: Option<Uuid>,
    project_id: Option<Uuid>,
    source_run_id: Option<Uuid>,
    source_event_id: Option<Uuid>,
    layer: String,
    content: String,
    confidence: f64,
    visibility: String,
    retention_policy: String,
    sensitivity: String,
}

async fn upsert_memory_candidate_tx(
    tx: &mut Transaction<'_, Postgres>,
    candidate: MemoryCandidateInsert,
) -> Result<MemoryItemResponse, AppError> {
    let content_hash = sha256_hex(candidate.content.as_bytes());
    if let Some(row) = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
               status, visibility, sensitivity, created_at, updated_at
        FROM memory_items
        WHERE tenant_id = $1
          AND user_id = $2
          AND source_run_id IS NOT DISTINCT FROM $3
          AND content_hash = $4
          AND deleted_at IS NULL
        ORDER BY created_at ASC
        LIMIT 1
        "#,
    )
    .bind(candidate.tenant_id)
    .bind(candidate.user_id)
    .bind(candidate.source_run_id)
    .bind(&content_hash)
    .fetch_optional(&mut **tx)
    .await?
    {
        return memory_from_row(row);
    }

    let row = sqlx::query(
        r#"
        INSERT INTO memory_items (
            tenant_id, user_id, agent_id, project_id, layer, content, content_hash,
            source_run_id, source_event_id, confidence, status, visibility,
            retention_policy, sensitivity
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'candidate', $11, $12, $13)
        RETURNING id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
                  status, visibility, sensitivity, created_at, updated_at
        "#,
    )
    .bind(candidate.tenant_id)
    .bind(candidate.user_id)
    .bind(candidate.agent_id)
    .bind(candidate.project_id)
    .bind(candidate.layer)
    .bind(candidate.content)
    .bind(content_hash)
    .bind(candidate.source_run_id)
    .bind(candidate.source_event_id)
    .bind(candidate.confidence)
    .bind(candidate.visibility)
    .bind(candidate.retention_policy)
    .bind(candidate.sensitivity)
    .fetch_one(&mut **tx)
    .await?;

    memory_from_row(row)
}

struct MemoryContextScope<'a> {
    tenant_id: Uuid,
    user_id: Uuid,
    agent_id: Option<Uuid>,
    project_id: Option<Uuid>,
    layer: Option<&'a str>,
    max_context_chars: usize,
}

async fn load_memory_contexts_by_ids(
    pool: &PgPool,
    scope: MemoryContextScope<'_>,
    hits: Vec<crate::features::agent_platform::memory_vector::MemoryVectorHit>,
) -> Result<Vec<MemoryContextResponse>, AppError> {
    let ids = hits.iter().map(|hit| hit.memory_id).collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let scores = score_by_memory_id(&hits);
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
               status, visibility, sensitivity, created_at, updated_at
        FROM memory_items
        WHERE tenant_id = $1
          AND user_id = $2
          AND ($3::uuid IS NULL OR agent_id = $3)
          AND ($4::uuid IS NULL OR project_id = $4)
          AND ($5::text IS NULL OR layer = $5)
          AND id = ANY($6::uuid[])
          AND status = 'approved'
          AND sensitivity <> 'secret'
          AND deleted_at IS NULL
        "#,
    )
    .bind(scope.tenant_id)
    .bind(scope.user_id)
    .bind(scope.agent_id)
    .bind(scope.project_id)
    .bind(scope.layer)
    .bind(&ids)
    .fetch_all(pool)
    .await?;

    let mut by_id = HashMap::new();
    for row in rows {
        let memory = memory_from_row(row)?;
        by_id.insert(memory.id, memory);
    }

    let mut contexts = Vec::new();
    for id in ids {
        let Some(memory) = by_id.remove(&id) else {
            continue;
        };
        if let Some(context) = memory_context_from_item(
            memory,
            scores.get(&id).copied(),
            "memory_vector_search",
            scope.max_context_chars,
        ) {
            contexts.push(context);
        }
    }

    Ok(contexts)
}

async fn load_memory_contexts_by_keyword(
    pool: &PgPool,
    scope: MemoryContextScope<'_>,
    query: &str,
    limit: i64,
) -> Result<Vec<MemoryContextResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
               status, visibility, sensitivity, created_at, updated_at
        FROM memory_items
        WHERE tenant_id = $1
          AND user_id = $2
          AND ($3::uuid IS NULL OR agent_id = $3)
          AND ($4::uuid IS NULL OR project_id = $4)
          AND ($5::text IS NULL OR layer = $5)
          AND status = 'approved'
          AND sensitivity <> 'secret'
          AND content ILIKE '%' || $6 || '%'
          AND deleted_at IS NULL
        ORDER BY confidence DESC, updated_at DESC
        LIMIT $7
        "#,
    )
    .bind(scope.tenant_id)
    .bind(scope.user_id)
    .bind(scope.agent_id)
    .bind(scope.project_id)
    .bind(scope.layer)
    .bind(query)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(memory_from_row)
        .filter_map(|memory| match memory {
            Ok(memory) => memory_context_from_item(
                memory,
                None,
                "memory_keyword_search",
                scope.max_context_chars,
            )
            .map(Ok),
            Err(err) => Some(Err(err)),
        })
        .collect()
}

async fn write_memory_context_logs(
    pool: &PgPool,
    tenant_id: Uuid,
    memories: &[MemoryContextResponse],
    actor_user_id: Uuid,
    agent_id: Option<Uuid>,
    run_id: Option<Uuid>,
    source: &str,
) -> Result<(), AppError> {
    if memories.is_empty() {
        write_memory_access_log(
            pool,
            tenant_id,
            None,
            actor_user_id,
            agent_id,
            run_id,
            source,
        )
        .await?;
        return Ok(());
    }

    for memory in memories {
        write_memory_access_log(
            pool,
            tenant_id,
            Some(memory.memory_id),
            actor_user_id,
            agent_id,
            run_id,
            source,
        )
        .await?;
    }
    Ok(())
}

async fn load_memory(pool: &PgPool, memory_id: Uuid) -> Result<MemoryItemResponse, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, tenant_id, user_id, agent_id, project_id, source_run_id, layer, content, confidence,
               status, visibility, sensitivity, created_at, updated_at
        FROM memory_items
        WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(memory_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("memory not found".to_string()))?;

    memory_from_row(row)
}

struct RunMemoryScope {
    tenant_id: Uuid,
    created_by_user_id: Option<Uuid>,
    agent_id: Option<Uuid>,
    project_id: Option<Uuid>,
}

async fn load_run_memory_scope(pool: &PgPool, run_id: Uuid) -> Result<RunMemoryScope, AppError> {
    let row = sqlx::query(
        r#"
        SELECT tenant_id, created_by_user_id, agent_id, project_id
        FROM runs
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("run not found".to_string()))?;

    Ok(RunMemoryScope {
        tenant_id: row.try_get("tenant_id")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        agent_id: row.try_get("agent_id")?,
        project_id: row.try_get("project_id")?,
    })
}

fn memory_candidates_from_event_payload(payload: &Value) -> Vec<MemoryCandidateInput> {
    candidate_array(payload)
        .map(|candidates| {
            candidates
                .iter()
                .filter_map(memory_candidate_from_value)
                .collect()
        })
        .unwrap_or_default()
}

fn candidate_array(payload: &Value) -> Option<&Vec<Value>> {
    payload
        .get("memory_candidates")
        .or_else(|| payload.get("candidate_memories"))
        .or_else(|| {
            payload
                .get("memory")
                .and_then(|memory| memory.get("candidates"))
        })
        .and_then(Value::as_array)
}

fn memory_candidate_from_value(value: &Value) -> Option<MemoryCandidateInput> {
    if let Some(content) = value.as_str() {
        return Some(MemoryCandidateInput {
            content: Some(content.to_string()),
            text: None,
            layer: None,
            confidence: None,
            visibility: None,
            sensitivity: None,
            retention_policy: None,
        });
    }
    let object = value.as_object()?;
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string);
    if content.is_none() && text.is_none() {
        return None;
    }

    Some(MemoryCandidateInput {
        content,
        text,
        layer: object
            .get("layer")
            .and_then(Value::as_str)
            .map(str::to_string),
        confidence: object.get("confidence").and_then(Value::as_f64),
        visibility: object
            .get("visibility")
            .and_then(Value::as_str)
            .map(str::to_string),
        sensitivity: object
            .get("sensitivity")
            .and_then(Value::as_str)
            .map(str::to_string),
        retention_policy: object
            .get("retention_policy")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

async fn write_memory_read_logs(
    pool: &PgPool,
    memories: &[MemoryItemResponse],
    actor_user_id: Uuid,
    run_id: Option<Uuid>,
    action: &str,
) -> Result<(), AppError> {
    for memory in memories {
        write_memory_access_log(
            pool,
            memory.tenant_id,
            Some(memory.id),
            actor_user_id,
            memory.agent_id,
            run_id,
            action,
        )
        .await?;
    }
    Ok(())
}

async fn write_memory_access_log(
    pool: &PgPool,
    tenant_id: Uuid,
    memory_id: Option<Uuid>,
    actor_user_id: Uuid,
    agent_id: Option<Uuid>,
    run_id: Option<Uuid>,
    action: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO memory_access_logs (tenant_id, memory_id, user_id, agent_id, run_id, action)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id)
    .bind(memory_id)
    .bind(actor_user_id)
    .bind(agent_id)
    .bind(run_id)
    .bind(action)
    .execute(pool)
    .await?;
    Ok(())
}

async fn write_memory_access_log_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    memory_id: Option<Uuid>,
    actor_user_id: Uuid,
    agent_id: Option<Uuid>,
    run_id: Option<Uuid>,
    action: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO memory_access_logs (tenant_id, memory_id, user_id, agent_id, run_id, action)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id)
    .bind(memory_id)
    .bind(actor_user_id)
    .bind(agent_id)
    .bind(run_id)
    .bind(action)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

const DEFAULT_WRITE_MEMORY_STATUS: &str = "candidate";
const DEFAULT_READ_MEMORY_STATUS: &str = "approved";
const DEFAULT_MEMORY_VISIBILITY: &str = "private";
const DEFAULT_MEMORY_SENSITIVITY: &str = "normal";
const DEFAULT_CANDIDATE_MEMORY_LAYER: &str = "semantic";
const MAX_CANDIDATE_CONTENT_CHARS: usize = 8000;
const MAX_MEMORY_BATCH_DECISION_ITEMS: usize = 200;

#[derive(Debug, Clone, Copy)]
struct MemoryStatusDecision {
    canonical_decision: &'static str,
    target_status: &'static str,
    action: &'static str,
}

impl MemoryStatusDecision {
    fn activate() -> Self {
        Self {
            canonical_decision: "activate",
            target_status: "approved",
            action: "activate",
        }
    }

    fn reject() -> Self {
        Self {
            canonical_decision: "reject",
            target_status: "rejected",
            action: "reject",
        }
    }

    fn archive() -> Self {
        Self {
            canonical_decision: "archive",
            target_status: "archived",
            action: "archive",
        }
    }
}

fn normalize_memory_decision(decision: &str) -> Result<MemoryStatusDecision, AppError> {
    let normalized = decision.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "activate" | "approve" | "approved" => Ok(MemoryStatusDecision::activate()),
        "reject" | "rejected" => Ok(MemoryStatusDecision::reject()),
        "archive" | "archived" => Ok(MemoryStatusDecision::archive()),
        _ => Err(AppError::InvalidInput(
            "memory decision must be activate, reject, or archive".to_string(),
        )),
    }
}

fn normalize_memory_batch_ids(memory_ids: Vec<Uuid>) -> Result<Vec<Uuid>, AppError> {
    if memory_ids.is_empty() {
        return Err(AppError::InvalidInput(
            "memory_ids must contain at least one memory id".to_string(),
        ));
    }
    if memory_ids.len() > MAX_MEMORY_BATCH_DECISION_ITEMS {
        return Err(AppError::InvalidInput(format!(
            "memory_ids supports at most {MAX_MEMORY_BATCH_DECISION_ITEMS} items"
        )));
    }

    let mut seen = HashSet::with_capacity(memory_ids.len());
    Ok(memory_ids
        .into_iter()
        .filter(|memory_id| seen.insert(*memory_id))
        .collect())
}

fn is_memory_batch_item_error(err: &AppError) -> bool {
    matches!(
        err,
        AppError::NotFound(_) | AppError::PermissionDenied(_) | AppError::InvalidInput(_)
    )
}

fn memory_batch_error(err: &AppError) -> (String, String) {
    match err {
        AppError::NotFound(message) => ("NOT_FOUND".to_string(), message.clone()),
        AppError::PermissionDenied(message) => ("PERMISSION_DENIED".to_string(), message.clone()),
        AppError::InvalidInput(message) => ("INVALID_INPUT".to_string(), message.clone()),
        _ => ("INTERNAL_ERROR".to_string(), "internal error".to_string()),
    }
}

fn normalize_memory_candidate(
    candidate: MemoryCandidateInput,
) -> Result<NormalizedMemoryCandidate, AppError> {
    let content = candidate
        .content
        .or(candidate.text)
        .map(|content| normalize_candidate_content(&content))
        .transpose()?
        .ok_or_else(|| {
            AppError::InvalidInput("memory candidate content is required".to_string())
        })?;
    let layer = normalize_memory_layer(
        candidate
            .layer
            .as_deref()
            .unwrap_or(DEFAULT_CANDIDATE_MEMORY_LAYER),
    )?;
    let confidence = normalize_memory_confidence(candidate.confidence)?;
    let visibility = normalize_memory_visibility(
        candidate
            .visibility
            .as_deref()
            .unwrap_or(DEFAULT_MEMORY_VISIBILITY),
    )?;
    let sensitivity = normalize_memory_sensitivity(
        candidate
            .sensitivity
            .as_deref()
            .unwrap_or(DEFAULT_MEMORY_SENSITIVITY),
    )?;
    let retention_policy = candidate
        .retention_policy
        .as_deref()
        .map(str::trim)
        .filter(|policy| !policy.is_empty())
        .unwrap_or("default")
        .chars()
        .take(128)
        .collect();

    Ok(NormalizedMemoryCandidate {
        content,
        layer,
        confidence,
        visibility,
        retention_policy,
        sensitivity,
    })
}

fn normalize_candidate_content(content: &str) -> Result<String, AppError> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(AppError::InvalidInput(
            "memory candidate content is required".to_string(),
        ));
    }
    Ok(trimmed.chars().take(MAX_CANDIDATE_CONTENT_CHARS).collect())
}

fn normalize_memory_layer(layer: &str) -> Result<String, AppError> {
    let normalized = layer.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "core_profile" | "episodic" | "semantic" | "procedural" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "memory layer must be core_profile, episodic, semantic, or procedural".to_string(),
        )),
    }
}

fn normalize_optional_memory_layer(layer: Option<&str>) -> Result<Option<String>, AppError> {
    layer.map(normalize_memory_layer).transpose()
}

fn normalize_memory_status(status: &str) -> Result<String, AppError> {
    let normalized = status.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "candidate" | "approved" | "rejected" | "archived" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "memory status must be candidate, approved, rejected, or archived".to_string(),
        )),
    }
}

fn normalize_initial_memory_status(layer: &str, status: &str) -> Result<String, AppError> {
    let status = normalize_memory_status(status)?;
    if is_governed_memory_layer(layer) && status != "candidate" {
        return Err(AppError::InvalidInput(
            "core_profile and procedural memories must be created as candidate before activation"
                .to_string(),
        ));
    }
    Ok(status)
}

fn is_governed_memory_layer(layer: &str) -> bool {
    matches!(layer, "core_profile" | "procedural")
}

fn normalize_memory_visibility(visibility: &str) -> Result<String, AppError> {
    let normalized = visibility.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "private" | "tenant" | "public" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "memory visibility must be private, tenant, or public".to_string(),
        )),
    }
}

fn normalize_memory_sensitivity(sensitivity: &str) -> Result<String, AppError> {
    let normalized = sensitivity.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "normal" | "sensitive" | "secret" => Ok(normalized),
        _ => Err(AppError::InvalidInput(
            "memory sensitivity must be normal, sensitive, or secret".to_string(),
        )),
    }
}

fn normalize_memory_confidence(confidence: Option<f64>) -> Result<f64, AppError> {
    let confidence = confidence.unwrap_or(0.5);
    if confidence.is_finite() && (0.0..=1.0).contains(&confidence) {
        Ok(confidence)
    } else {
        Err(AppError::InvalidInput(
            "memory confidence must be between 0 and 1".to_string(),
        ))
    }
}

fn normalize_memory_query(query: Option<&str>) -> Option<String> {
    query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_required_memory_query(query: &str) -> Result<String, AppError> {
    normalize_memory_query(Some(query))
        .ok_or_else(|| AppError::InvalidInput("memory query is required".to_string()))
}

fn normalize_memory_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(50).clamp(1, 200)
}

fn normalize_min_score(min_score: Option<f64>) -> Result<Option<f64>, AppError> {
    match min_score {
        Some(score) if score.is_finite() => Ok(Some(score)),
        Some(_) => Err(AppError::InvalidInput(
            "memory min_score must be finite".to_string(),
        )),
        None => Ok(None),
    }
}

fn normalize_memory_access_action(action: &str) -> Result<String, AppError> {
    let normalized = action.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized.len() > 64 {
        return Err(AppError::InvalidInput(
            "memory access action must be between 1 and 64 characters".to_string(),
        ));
    }
    if normalized
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '.')
    {
        Ok(normalized)
    } else {
        Err(AppError::InvalidInput(
            "memory access action may only contain lowercase letters, digits, '_' or '.'"
                .to_string(),
        ))
    }
}

fn memory_authz_resource_id(memory: &MemoryItemResponse) -> String {
    memory.user_id.unwrap_or(memory.id).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use serde_json::{Value, json};
    use sqlx::postgres::PgPoolOptions;
    use time::{Duration, OffsetDateTime};

    use crate::{
        configuration::{AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings},
        features::agent_platform::{
            authz::ResourceAuthzService, event_store, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_ingestion, memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient,
            rustfs::RustFsClient,
        },
    };

    fn memory_with_user(user_id: Option<Uuid>) -> MemoryItemResponse {
        MemoryItemResponse {
            id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            user_id,
            agent_id: None,
            project_id: None,
            source_run_id: None,
            layer: "episodic".to_string(),
            content: "remember this".to_string(),
            confidence: 0.5,
            status: "candidate".to_string(),
            visibility: "private".to_string(),
            sensitivity: "normal".to_string(),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn normalize_memory_layer_accepts_planned_layers() {
        assert_eq!(
            normalize_memory_layer(" Core_Profile ").expect("valid layer"),
            "core_profile"
        );
        assert_eq!(
            normalize_memory_layer("episodic").expect("valid layer"),
            "episodic"
        );
        assert_eq!(
            normalize_memory_layer("semantic").expect("valid layer"),
            "semantic"
        );
        assert_eq!(
            normalize_memory_layer("procedural").expect("valid layer"),
            "procedural"
        );
    }

    #[test]
    fn normalize_memory_layer_rejects_unknown_layer() {
        assert!(normalize_memory_layer("misc").is_err());
    }

    #[test]
    fn normalize_memory_status_allows_review_lifecycle() {
        assert_eq!(
            normalize_memory_status(DEFAULT_WRITE_MEMORY_STATUS).expect("valid status"),
            "candidate"
        );
        assert_eq!(
            normalize_memory_status(DEFAULT_READ_MEMORY_STATUS).expect("valid status"),
            "approved"
        );
        assert!(normalize_memory_status("published").is_err());
    }

    #[test]
    fn governed_layers_must_start_as_candidates() {
        assert_eq!(
            normalize_initial_memory_status("core_profile", "candidate").expect("candidate"),
            "candidate"
        );
        assert!(normalize_initial_memory_status("core_profile", "approved").is_err());
        assert!(normalize_initial_memory_status("procedural", "archived").is_err());
        assert_eq!(
            normalize_initial_memory_status("semantic", "approved").expect("approved"),
            "approved"
        );
    }

    #[test]
    fn normalize_memory_confidence_rejects_invalid_values() {
        assert_eq!(normalize_memory_confidence(None).expect("default"), 0.5);
        assert_eq!(normalize_memory_confidence(Some(1.0)).expect("max"), 1.0);
        assert!(normalize_memory_confidence(Some(-0.1)).is_err());
        assert!(normalize_memory_confidence(Some(f64::NAN)).is_err());
    }

    #[test]
    fn normalize_memory_limit_clamps_to_supported_range() {
        assert_eq!(normalize_memory_limit(Some(-5)), 1);
        assert_eq!(normalize_memory_limit(Some(500)), 200);
        assert_eq!(normalize_memory_limit(None), 50);
    }

    #[test]
    fn normalize_required_memory_query_rejects_blank_query() {
        assert_eq!(
            normalize_required_memory_query(" sales ").expect("query"),
            "sales"
        );
        assert!(normalize_required_memory_query("   ").is_err());
    }

    #[test]
    fn normalize_min_score_rejects_nan() {
        assert_eq!(normalize_min_score(Some(0.42)).expect("score"), Some(0.42));
        assert!(normalize_min_score(Some(f64::NAN)).is_err());
    }

    #[test]
    fn memory_status_authz_uses_owner_user_when_available() {
        let user_id = Uuid::new_v4();
        let memory = memory_with_user(Some(user_id));
        assert_eq!(memory_authz_resource_id(&memory), user_id.to_string());

        let tenant_memory = memory_with_user(None);
        assert_eq!(
            memory_authz_resource_id(&tenant_memory),
            tenant_memory.id.to_string()
        );
    }

    #[test]
    fn completed_event_payload_extracts_candidate_shapes() {
        let candidates = memory_candidates_from_event_payload(&json!({
            "memory_candidates": [
                "remember sales revenue preference",
                {
                    "text": "prefer concise weekly summaries",
                    "layer": "procedural",
                    "confidence": 0.8,
                    "visibility": "private"
                }
            ]
        }));

        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates[0].content.as_deref(),
            Some("remember sales revenue preference")
        );
        assert_eq!(
            candidates[1].text.as_deref(),
            Some("prefer concise weekly summaries")
        );
        assert_eq!(candidates[1].layer.as_deref(), Some("procedural"));
    }

    #[test]
    fn memory_candidate_normalization_defaults_to_semantic_candidate() {
        let normalized = normalize_memory_candidate(MemoryCandidateInput {
            content: Some("  remember this  ".to_string()),
            text: None,
            layer: None,
            confidence: None,
            visibility: None,
            sensitivity: None,
            retention_policy: None,
        })
        .expect("candidate");

        assert_eq!(normalized.content, "remember this");
        assert_eq!(normalized.layer, "semantic");
        assert_eq!(normalized.confidence, 0.5);
        assert_eq!(normalized.visibility, "private");
        assert_eq!(normalized.sensitivity, "normal");
        assert_eq!(normalized.retention_policy, "default");
    }

    #[test]
    fn memory_access_action_rejects_unsafe_names() {
        assert_eq!(
            normalize_memory_access_action(" memory.read ").expect("valid"),
            "memory.read"
        );
        assert!(normalize_memory_access_action("Memory Read").is_err());
        assert!(normalize_memory_access_action("").is_err());
    }

    #[test]
    fn normalize_memory_decision_accepts_governance_aliases() {
        let activate = normalize_memory_decision(" approve ").expect("activate");
        assert_eq!(activate.canonical_decision, "activate");
        assert_eq!(activate.target_status, "approved");
        assert_eq!(activate.action, "activate");

        let reject = normalize_memory_decision("rejected").expect("reject");
        assert_eq!(reject.canonical_decision, "reject");
        assert_eq!(reject.target_status, "rejected");

        let archive = normalize_memory_decision("ARCHIVED").expect("archive");
        assert_eq!(archive.canonical_decision, "archive");
        assert_eq!(archive.target_status, "archived");

        assert!(normalize_memory_decision("publish").is_err());
    }

    #[test]
    fn normalize_memory_batch_ids_deduplicates_and_limits() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        assert_eq!(
            normalize_memory_batch_ids(vec![first, second, first]).expect("ids"),
            vec![first, second]
        );
        assert!(normalize_memory_batch_ids(Vec::new()).is_err());
        assert!(
            normalize_memory_batch_ids(vec![Uuid::new_v4(); MAX_MEMORY_BATCH_DECISION_ITEMS + 1])
                .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn completed_event_creates_candidate_once() -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, conversation_id, run_id) =
            seed_run_context(&state.connect_pool).await?;
        let event = {
            let mut tx = state.connect_pool.begin().await?;
            let event = event_store::insert_event_tx(
                &mut tx,
                tenant_id,
                conversation_id,
                Some(run_id),
                RunEventInput {
                    event_id: Some(format!("run.completed.{run_id}")),
                    event_type: "run.completed".to_string(),
                    payload: Some(json!({
                        "run_id": run_id,
                        "memory_candidates": [
                            {"content": "remember quarterly sales forecast", "layer": "semantic"}
                        ]
                    })),
                    trace_id: Some(format!("trace-{run_id}")),
                },
            )
            .await?;
            tx.commit()
                .await
                .map_err(|_| AppError::DatabaseTransaction)?;
            event
        };
        let payload = json!({
            "run_id": run_id,
            "memory_candidates": [
                {"content": "remember quarterly sales forecast", "layer": "semantic"}
            ]
        });

        let first =
            create_candidates_from_run_completed_event(&state, Some(run_id), event.id, &payload)
                .await?;
        let second =
            create_candidates_from_run_completed_event(&state, Some(run_id), event.id, &payload)
                .await?;

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 1);
        assert_eq!(first[0].id, second[0].id);
        assert_eq!(first[0].status, "candidate");
        assert_eq!(first[0].user_id, Some(user_id));

        let memory_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM memory_items
            WHERE tenant_id = $1 AND source_run_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(memory_count, 1);

        let access_logged: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM memory_access_logs
                WHERE tenant_id = $1
                  AND memory_id = $2
                  AND user_id = $3
                  AND run_id = $4
                  AND action = 'candidate_create'
            )
            "#,
        )
        .bind(tenant_id)
        .bind(first[0].id)
        .bind(user_id)
        .bind(run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert!(access_logged);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn batch_decision_activates_valid_memories_and_reports_item_failures()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, _, run_id) = seed_run_context(&state.connect_pool).await?;
        let first = seed_candidate_memory(
            &state.connect_pool,
            tenant_id,
            user_id,
            "remember batch governance first",
        )
        .await?;
        let second = seed_candidate_memory(
            &state.connect_pool,
            tenant_id,
            user_id,
            "remember batch governance second",
        )
        .await?;
        let missing = Uuid::new_v4();

        let response = batch_decide_memory_status(
            &state,
            &test_platform_context(tenant_id, user_id),
            MemoryBatchDecisionRequest {
                tenant_id,
                decision: "approve".to_string(),
                run_id: Some(run_id),
                memory_ids: vec![first, missing, second, first],
            },
        )
        .await?;

        assert_eq!(response.decision, "activate");
        assert_eq!(response.target_status, "approved");
        assert_eq!(response.succeeded, 2);
        assert_eq!(response.failed, 1);
        assert_eq!(response.results.len(), 3);
        assert_eq!(response.results[0].memory_id, first);
        assert_eq!(response.results[0].status, "succeeded");
        assert_eq!(
            response.results[0]
                .memory
                .as_ref()
                .map(|memory| memory.status.as_str()),
            Some("approved")
        );
        assert_eq!(response.results[1].memory_id, missing);
        assert_eq!(response.results[1].status, "failed");
        assert_eq!(response.results[1].error_code.as_deref(), Some("NOT_FOUND"));
        assert_eq!(response.results[2].memory_id, second);
        assert_eq!(response.results[2].status, "succeeded");

        let memory_ids = vec![first, second];
        let approved_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM memory_items
            WHERE id = ANY($1::uuid[])
              AND status = 'approved'
            "#,
        )
        .bind(&memory_ids)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(approved_count, 2);

        let job_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM memory_ingestion_jobs
            WHERE memory_id = ANY($1::uuid[])
              AND job_type = 'status_changed'
              AND status = 'pending'
            "#,
        )
        .bind(&memory_ids)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(job_count, 2);

        let access_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM memory_access_logs
            WHERE tenant_id = $1
              AND memory_id = ANY($2::uuid[])
              AND user_id = $3
              AND run_id = $4
              AND action = 'activate'
            "#,
        )
        .bind(tenant_id)
        .bind(&memory_ids)
        .bind(user_id)
        .bind(run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(access_count, 2);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn governed_layers_activate_after_candidate_and_scope_filters_apply()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, _, run_id) = seed_run_context(&state.connect_pool).await?;
        let ctx = test_platform_context(tenant_id, user_id);

        let direct_approved = upsert_memory(
            State(state.clone()),
            Extension(ctx.clone()),
            Json(CreateMemoryRequest {
                tenant_id,
                user_id: Some(user_id),
                agent_id: None,
                project_id: None,
                layer: "core_profile".to_string(),
                content: "用户长期偏好必须先审核".to_string(),
                source_run_id: Some(run_id),
                confidence: Some(0.9),
                status: Some("approved".to_string()),
                visibility: None,
                retention_policy: None,
                sensitivity: None,
            }),
        )
        .await;
        assert!(matches!(direct_approved, Err(AppError::InvalidInput(_))));

        let Json(candidate) = upsert_memory(
            State(state.clone()),
            Extension(ctx.clone()),
            Json(CreateMemoryRequest {
                tenant_id,
                user_id: Some(user_id),
                agent_id: None,
                project_id: None,
                layer: "core_profile".to_string(),
                content: "用户长期偏好需要双周经营摘要".to_string(),
                source_run_id: Some(run_id),
                confidence: Some(0.9),
                status: None,
                visibility: None,
                retention_policy: None,
                sensitivity: None,
            }),
        )
        .await?;
        assert_eq!(candidate.status, "candidate");
        assert_eq!(candidate.layer, "core_profile");

        let activated = decide_memory_status_one(
            &state,
            &ctx,
            candidate.id,
            Some(tenant_id),
            MemoryStatusDecision::activate(),
            Some(run_id),
        )
        .await?;
        assert_eq!(activated.status, "approved");

        let response = retrieve_memory_context_for_run(
            &state,
            MemoryRetrieveForRunRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                run_id: Some(run_id),
                user_id: Some(user_id),
                agent_id: None,
                project_id: None,
                layer: Some("core_profile".to_string()),
                query: "双周经营摘要".to_string(),
                limit: Some(5),
                min_score: None,
            },
        )
        .await?;
        assert_eq!(
            response.memories.first().map(|memory| memory.memory_id),
            Some(candidate.id)
        );

        let other_user_id = seed_platform_user(&state.connect_pool, tenant_id).await?;
        let other_user_memory = seed_memory_for_scope(
            &state.connect_pool,
            MemorySeed {
                tenant_id,
                user_id: other_user_id,
                project_id: None,
                layer: "semantic",
                status: "approved",
                content: "scope isolation other user memory",
                sensitivity: "normal",
            },
        )
        .await?;

        let cross_user = retrieve_memory_context_for_run(
            &state,
            MemoryRetrieveForRunRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                run_id: Some(run_id),
                user_id: Some(other_user_id),
                agent_id: None,
                project_id: None,
                layer: Some("semantic".to_string()),
                query: "scope isolation".to_string(),
                limit: Some(5),
                min_score: None,
            },
        )
        .await;
        assert!(matches!(cross_user, Err(AppError::PermissionDenied(_))));

        let project_a =
            seed_project(&state.connect_pool, tenant_id, user_id, "Memory Scope A").await?;
        let project_b =
            seed_project(&state.connect_pool, tenant_id, user_id, "Memory Scope B").await?;
        let project_a_memory = seed_memory_for_scope(
            &state.connect_pool,
            MemorySeed {
                tenant_id,
                user_id,
                project_id: Some(project_a),
                layer: "semantic",
                status: "approved",
                content: "scope isolation project memory",
                sensitivity: "normal",
            },
        )
        .await?;
        let project_b_memory = seed_memory_for_scope(
            &state.connect_pool,
            MemorySeed {
                tenant_id,
                user_id,
                project_id: Some(project_b),
                layer: "semantic",
                status: "approved",
                content: "scope isolation project memory",
                sensitivity: "normal",
            },
        )
        .await?;

        let scoped_response = retrieve_memory_context_for_run(
            &state,
            MemoryRetrieveForRunRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                run_id: Some(run_id),
                user_id: Some(user_id),
                agent_id: None,
                project_id: Some(project_a),
                layer: Some("semantic".to_string()),
                query: "scope isolation project".to_string(),
                limit: Some(5),
                min_score: None,
            },
        )
        .await?;
        let scoped_ids = scoped_response
            .memories
            .iter()
            .map(|memory| memory.memory_id)
            .collect::<Vec<_>>();
        assert!(scoped_ids.contains(&project_a_memory));
        assert!(!scoped_ids.contains(&project_b_memory));
        assert!(!scoped_ids.contains(&other_user_memory));

        cleanup_tenant(&state.connect_pool, tenant_id).await?;

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, embed endpoint, and Qdrant"]
    async fn approved_memory_indexes_to_qdrant_and_retrieves_via_vector()
    -> Result<(), Box<dyn std::error::Error>> {
        let collection = format!("bibi_work_memories_e2e_{}", Uuid::new_v4().simple());
        let state = test_state_with_memory_vector(collection.clone()).await?;
        let (tenant_id, user_id, _, run_id) = seed_run_context(&state.connect_pool).await?;
        let memory_id = seed_approved_memory_for_index(
            &state.connect_pool,
            tenant_id,
            user_id,
            "销售额数据需要优先用于企业经营分析",
        )
        .await?;

        let processed =
            memory_ingestion::process_pending_memory_ingestion_for_tenant(&state, tenant_id)
                .await?;
        assert_eq!(processed, 1);

        let index_status: String = sqlx::query_scalar(
            r#"
            SELECT index_status
            FROM memory_embeddings
            WHERE memory_id = $1
            "#,
        )
        .bind(memory_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(index_status, "indexed");

        let response = retrieve_memory_context_for_run(
            &state,
            MemoryRetrieveForRunRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                run_id: Some(run_id),
                user_id: Some(user_id),
                agent_id: None,
                project_id: None,
                layer: Some("semantic".to_string()),
                query: "销售额数据".to_string(),
                limit: Some(3),
                min_score: None,
            },
        )
        .await?;

        assert_eq!(response.source, "memory_vector_search");
        assert!(response.vector_attempted);
        assert_eq!(
            response.memories.first().map(|memory| memory.memory_id),
            Some(memory_id)
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
                  AND action = 'memory_vector_search'
            )
            "#,
        )
        .bind(tenant_id)
        .bind(memory_id)
        .bind(user_id)
        .bind(run_id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert!(access_logged);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        delete_qdrant_collection(&collection).await?;

        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, embed endpoint, and Qdrant"]
    async fn archive_and_reject_remove_indexed_memories_from_qdrant()
    -> Result<(), Box<dyn std::error::Error>> {
        let collection = format!("bibi_work_memories_e2e_{}", Uuid::new_v4().simple());
        let state = test_state_with_memory_vector(collection.clone()).await?;
        let (tenant_id, user_id, _, run_id) = seed_run_context(&state.connect_pool).await?;
        let archived_memory_id = seed_approved_memory_for_index(
            &state.connect_pool,
            tenant_id,
            user_id,
            "archive 删除 Qdrant 记忆",
        )
        .await?;
        let rejected_memory_id = seed_approved_memory_for_index(
            &state.connect_pool,
            tenant_id,
            user_id,
            "reject 删除 Qdrant 记忆",
        )
        .await?;

        let processed =
            memory_ingestion::process_pending_memory_ingestion_for_tenant(&state, tenant_id)
                .await?;
        assert_eq!(processed, 2);
        assert!(qdrant_point_exists(&collection, archived_memory_id).await?);
        assert!(qdrant_point_exists(&collection, rejected_memory_id).await?);

        let ctx = test_platform_context(tenant_id, user_id);
        decide_memory_status_one(
            &state,
            &ctx,
            archived_memory_id,
            Some(tenant_id),
            MemoryStatusDecision::archive(),
            Some(run_id),
        )
        .await?;
        decide_memory_status_one(
            &state,
            &ctx,
            rejected_memory_id,
            Some(tenant_id),
            MemoryStatusDecision::reject(),
            Some(run_id),
        )
        .await?;

        let processed =
            memory_ingestion::process_pending_memory_ingestion_for_tenant(&state, tenant_id)
                .await?;
        assert_eq!(processed, 2);

        let skipped_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM memory_embeddings
            WHERE memory_id = ANY($1::uuid[])
              AND index_status = 'skipped'
            "#,
        )
        .bind(vec![archived_memory_id, rejected_memory_id])
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(skipped_count, 2);
        assert!(!qdrant_point_exists(&collection, archived_memory_id).await?);
        assert!(!qdrant_point_exists(&collection, rejected_memory_id).await?);

        let response = retrieve_memory_context_for_run(
            &state,
            MemoryRetrieveForRunRequest {
                tenant_id,
                actor: ActorRef {
                    user_id,
                    device_id: None,
                    session_id: None,
                    roles: Vec::new(),
                },
                run_id: Some(run_id),
                user_id: Some(user_id),
                agent_id: None,
                project_id: None,
                layer: Some("semantic".to_string()),
                query: "删除 Qdrant 记忆".to_string(),
                limit: Some(5),
                min_score: None,
            },
        )
        .await?;
        assert!(response.memories.is_empty());

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        delete_qdrant_collection(&collection).await?;

        Ok(())
    }

    async fn test_state() -> Result<AppState, Box<dyn std::error::Error>> {
        test_state_with_memory_vector_settings(MemoryVectorSettings {
            enabled: false,
            embedding_endpoint: None,
            qdrant_rest_url: None,
            qdrant_collection: "test_memories".to_string(),
            timeout_milliseconds: 1000,
            max_context_chars: 1200,
            worker_interval_milliseconds: 1000,
            worker_batch_size: 1,
            worker_max_attempts: 1,
        })
        .await
    }

    async fn test_state_with_memory_vector(
        collection: String,
    ) -> Result<AppState, Box<dyn std::error::Error>> {
        test_state_with_memory_vector_settings(MemoryVectorSettings {
            enabled: true,
            embedding_endpoint: Some(
                std::env::var("BIBI_TEST_EMBEDDING_ENDPOINT")
                    .unwrap_or_else(|_| "http://172.24.250.231:8335/embed".to_string()),
            ),
            qdrant_rest_url: Some(
                std::env::var("BIBI_TEST_QDRANT_REST_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:6337".to_string()),
            ),
            qdrant_collection: collection,
            timeout_milliseconds: 10_000,
            max_context_chars: 1200,
            worker_interval_milliseconds: 1000,
            worker_batch_size: 10,
            worker_max_attempts: 1,
        })
        .await
    }

    async fn test_state_with_memory_vector_settings(
        memory_vector: MemoryVectorSettings,
    ) -> Result<AppState, Box<dyn std::error::Error>> {
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
            memory_vector_client: MemoryVectorClient::new(memory_vector)?,
            internal_shared_token: "test-internal-token".to_string(),
        })
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }

    fn test_platform_context(tenant_id: Uuid, user_id: Uuid) -> PlatformRequestContext {
        PlatformRequestContext {
            tenant_id,
            platform_user_id: user_id,
            ferriskey_subject: format!("test-subject-{user_id}"),
            preferred_username: Some(format!("test-user-{user_id}")),
            email: None,
            roles: Vec::new(),
            session_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            token_jti: None,
            token_exp: OffsetDateTime::now_utc() + Duration::hours(1),
        }
    }

    async fn seed_run_context(
        pool: &PgPool,
    ) -> Result<(Uuid, Uuid, Uuid, Uuid), Box<dyn std::error::Error>> {
        let suffix = Uuid::new_v4();
        let tenant_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tenants (name, slug, metadata)
            VALUES ($1, $2, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(format!("Memory Candidate Test {suffix}"))
        .bind(format!("memory-candidate-test-{suffix}"))
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
        .bind(format!("memory-candidate-subject-{suffix}"))
        .bind(format!("memory-candidate-user-{suffix}"))
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

        let conversation_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO conversations (tenant_id, created_by_user_id, title, metadata)
            VALUES ($1, $2, 'Memory candidate test', '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_one(pool)
        .await?;

        let run_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO runs (
                tenant_id, conversation_id, created_by_user_id, status,
                input, run_config_snapshot, trace_id
            )
            VALUES ($1, $2, $3, 'running', '{}'::jsonb, '{}'::jsonb, $4)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(conversation_id)
        .bind(user_id)
        .bind(format!("trace-{suffix}"))
        .fetch_one(pool)
        .await?;

        Ok((tenant_id, user_id, conversation_id, run_id))
    }

    async fn seed_candidate_memory(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        content: &str,
    ) -> Result<Uuid, sqlx::Error> {
        let content_hash = sha256_hex(content.as_bytes());
        sqlx::query_scalar(
            r#"
            INSERT INTO memory_items (
                tenant_id, user_id, layer, content, content_hash,
                confidence, status, visibility, retention_policy, sensitivity
            )
            VALUES ($1, $2, 'semantic', $3, $4, 0.7, 'candidate', 'private', 'default', 'normal')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(content)
        .bind(content_hash)
        .fetch_one(pool)
        .await
    }

    async fn seed_platform_user(pool: &PgPool, tenant_id: Uuid) -> Result<Uuid, sqlx::Error> {
        let suffix = Uuid::new_v4();
        let user_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO platform_users (tenant_id, ferriskey_subject, username, status)
            VALUES ($1, $2, $3, 'active')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("memory-scope-subject-{suffix}"))
        .bind(format!("memory-scope-user-{suffix}"))
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

        Ok(user_id)
    }

    async fn seed_project(
        pool: &PgPool,
        tenant_id: Uuid,
        owner_user_id: Uuid,
        name: &str,
    ) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            INSERT INTO projects (tenant_id, owner_user_id, name, metadata)
            VALUES ($1, $2, $3, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(owner_user_id)
        .bind(format!("{name} {}", Uuid::new_v4()))
        .fetch_one(pool)
        .await
    }

    struct MemorySeed<'a> {
        tenant_id: Uuid,
        user_id: Uuid,
        project_id: Option<Uuid>,
        layer: &'a str,
        status: &'a str,
        content: &'a str,
        sensitivity: &'a str,
    }

    async fn seed_memory_for_scope(
        pool: &PgPool,
        seed: MemorySeed<'_>,
    ) -> Result<Uuid, sqlx::Error> {
        let content_hash = sha256_hex(seed.content.as_bytes());
        sqlx::query_scalar(
            r#"
            INSERT INTO memory_items (
                tenant_id, user_id, project_id, layer, content, content_hash,
                confidence, status, visibility, retention_policy, sensitivity
            )
            VALUES ($1, $2, $3, $4, $5, $6, 0.8, $7, 'private', 'default', $8)
            RETURNING id
            "#,
        )
        .bind(seed.tenant_id)
        .bind(seed.user_id)
        .bind(seed.project_id)
        .bind(seed.layer)
        .bind(seed.content)
        .bind(content_hash)
        .bind(seed.status)
        .bind(seed.sensitivity)
        .fetch_one(pool)
        .await
    }

    async fn seed_approved_memory_for_index(
        pool: &PgPool,
        tenant_id: Uuid,
        user_id: Uuid,
        content: &str,
    ) -> Result<Uuid, sqlx::Error> {
        let content_hash = sha256_hex(content.as_bytes());
        let memory_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO memory_items (
                tenant_id, user_id, layer, content, content_hash,
                confidence, status, visibility, retention_policy, sensitivity
            )
            VALUES ($1, $2, 'semantic', $3, $4, 0.95, 'approved', 'private', 'default', 'normal')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(content)
        .bind(content_hash)
        .fetch_one(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO memory_embeddings (
                memory_id, tenant_id, qdrant_point_id, index_status
            )
            VALUES ($1, $2, $3, 'pending')
            "#,
        )
        .bind(memory_id)
        .bind(tenant_id)
        .bind(memory_id.to_string())
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO memory_ingestion_jobs (tenant_id, memory_id, job_type, status)
            VALUES ($1, $2, 'test-upsert', 'pending')
            "#,
        )
        .bind(tenant_id)
        .bind(memory_id)
        .execute(pool)
        .await?;

        Ok(memory_id)
    }

    async fn cleanup_tenant(pool: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    async fn qdrant_point_exists(
        collection: &str,
        memory_id: Uuid,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let rest_url = std::env::var("BIBI_TEST_QDRANT_REST_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:6337".to_string());
        let url = format!(
            "{}/collections/{}/points",
            rest_url.trim_end_matches('/'),
            collection
        );
        let response = reqwest::Client::new()
            .post(url)
            .json(&json!({ "ids": [memory_id.to_string()] }))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        let body: Value = response.error_for_status()?.json().await?;
        Ok(body
            .get("result")
            .and_then(Value::as_array)
            .is_some_and(|points| !points.is_empty()))
    }

    async fn delete_qdrant_collection(collection: &str) -> Result<(), Box<dyn std::error::Error>> {
        let rest_url = std::env::var("BIBI_TEST_QDRANT_REST_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:6337".to_string());
        let url = format!(
            "{}/collections/{}",
            rest_url.trim_end_matches('/'),
            collection
        );
        let response = reqwest::Client::new().delete(url).send().await?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(format!("qdrant collection cleanup failed: {}", response.status()).into())
        }
    }
}
