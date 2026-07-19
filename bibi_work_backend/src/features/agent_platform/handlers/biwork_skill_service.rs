use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{
    collections::HashMap,
    fs,
    path::{Path as FsPath, PathBuf},
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    features::{
        agent_platform::{
            ferriskey_oidc::PlatformRequestContext,
            remote_skill::{self, RemoteSkillDocument},
        },
        core::errors::AppError,
    },
    startup::AppState,
};

use super::{
    biwork_compat_service::{epoch_ms, ok, required_string, trimmed_string, value_string},
    biwork_conversation_support::ensure_conversation_exists,
    biwork_settings_service::{require_biwork_user_settings_update, set_biwork_client_setting},
    support::require_ferriskey_allow,
};

const BIWORK_SKILL_MAX_FILE_BYTES: u64 = 1_048_576;
const BIWORK_SKILL_MAX_TOTAL_BYTES: u64 = 10_485_760;
const BIWORK_SKILL_EXTERNAL_PATHS_KEY: &str = "skills.externalPaths";
const BIWORK_SKILLS_MARKET_ENABLED_KEY: &str = "skillsMarket.enabled";

#[derive(Debug, Deserialize)]
pub struct SkillExternalPathQuery {
    path: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct BiWorkSkillCandidate {
    pub(super) name: String,
    description: String,
    source_path: PathBuf,
    source_name: String,
    content: String,
    total_bytes: u64,
}

#[derive(Debug, Clone)]
pub(super) struct BiWorkSkillImportFailure {
    source_name: String,
    code: String,
    error_path: Option<String>,
    actual_bytes: Option<u64>,
    limit_bytes: Option<u64>,
    line: Option<i32>,
    column: Option<i32>,
}

#[derive(Debug, Clone)]
pub(super) struct BiWorkSkillSource {
    skill_file: PathBuf,
    source_root: PathBuf,
}

pub async fn biwork_skill_import_limits() -> Result<Json<Value>, AppError> {
    Ok(ok(json!({
        "max_file_bytes": BIWORK_SKILL_MAX_FILE_BYTES,
        "max_total_bytes": BIWORK_SKILL_MAX_TOTAL_BYTES,
    })))
}

pub async fn biwork_list_skills(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, description, status, metadata, created_at, updated_at
        FROM skills
        WHERE tenant_id = $1 AND deleted_at IS NULL
        ORDER BY updated_at DESC, created_at DESC
        LIMIT 500
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut skills = Vec::with_capacity(rows.len());
    for row in rows {
        let skill_id: Uuid = row.try_get("id")?;
        let metadata: Value = row.try_get("metadata")?;
        let status: String = row.try_get("status")?;
        skills.push(biwork_skill_info_value(
            skill_id,
            row.try_get::<String, _>("name")?,
            row.try_get::<Option<String>, _>("description")?
                .unwrap_or_default(),
            status,
            metadata,
            row.try_get("created_at")?,
            row.try_get("updated_at")?,
        ));
    }

    Ok(ok(Value::Array(skills)))
}

pub async fn biwork_create_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let candidate = build_biwork_skill_candidate_from_payload(&payload)?;
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "create",
        "skill",
        candidate.name.clone(),
        None,
    )
    .await?;

    let operation_id = Uuid::new_v4();
    let mut tx = state.connect_pool.begin().await?;
    let (skill_id, _) = upsert_biwork_skill_candidate(
        &mut tx,
        ctx.tenant_id,
        ctx.platform_user_id,
        operation_id,
        0,
        &candidate,
    )
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;

    let row = sqlx::query(
        r#"
        SELECT id, name, description, status, metadata, created_at, updated_at
        FROM skills
        WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(skill_id)
    .bind(ctx.tenant_id)
    .fetch_one(&state.connect_pool)
    .await?;

    Ok(ok(biwork_skill_info_value(
        row.try_get("id")?,
        row.try_get::<String, _>("name")?,
        row.try_get::<Option<String>, _>("description")?
            .unwrap_or_default(),
        row.try_get("status")?,
        row.try_get("metadata")?,
        row.try_get("created_at")?,
        row.try_get("updated_at")?,
    )))
}

pub async fn biwork_read_builtin_rule(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Ok(ok(json!(
        read_biwork_builtin_catalog_content(&state, ctx.tenant_id, &payload).await?
    )))
}

pub async fn biwork_read_builtin_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Ok(ok(json!(
        read_biwork_builtin_catalog_content(&state, ctx.tenant_id, &payload).await?
    )))
}

pub async fn biwork_materialize_skills_for_agent(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    if let Some(conversation_id) = trimmed_string(&payload, "conversation_id") {
        let conversation_id = Uuid::parse_str(&conversation_id)
            .map_err(|_| AppError::InvalidInput("conversation_id must be a UUID".to_string()))?;
        ensure_conversation_exists(&state, ctx.tenant_id, conversation_id).await?;
    }

    let requested = payload
        .get("skills")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if requested.is_empty() {
        return Ok(ok(json!({ "skills": [] })));
    }

    let rows = sqlx::query(
        r#"
        SELECT id, name, metadata
        FROM skills
        WHERE tenant_id = $1
          AND deleted_at IS NULL
          AND status = 'active'
          AND name = ANY($2::text[])
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(&requested)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut by_name = HashMap::with_capacity(rows.len());
    for row in rows {
        let skill_id: Uuid = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let metadata: Value = row.try_get("metadata")?;
        by_name.insert(
            name.clone(),
            json!({
                "name": name,
                "source_path": biwork_skill_location(&metadata, skill_id),
            }),
        );
    }

    let skills = requested
        .iter()
        .filter_map(|name| by_name.get(name).cloned())
        .collect::<Vec<_>>();
    Ok(ok(json!({ "skills": skills })))
}

pub async fn biwork_read_skill_info(Json(payload): Json<Value>) -> Result<Json<Value>, AppError> {
    let skill_path = required_string(&payload, "skill_path")?;
    let sources = discover_biwork_skill_sources(FsPath::new(&skill_path)).map_err(|failure| {
        AppError::InvalidInput(format!("{}: {}", failure.source_name, failure.code))
    })?;
    let source = sources
        .first()
        .ok_or_else(|| AppError::InvalidInput("skill_path contains no skill".to_string()))?;
    let candidate = build_biwork_skill_candidate(source).map_err(|failure| {
        AppError::InvalidInput(format!("{}: {}", failure.source_name, failure.code))
    })?;
    Ok(ok(json!({
        "name": candidate.name,
        "description": candidate.description,
    })))
}

pub async fn biwork_import_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let skill_path = required_string(&payload, "skill_path")?;
    let is_remote = remote_skill::is_remote_skill_url(&skill_path);
    let source_label = if is_remote {
        remote_skill::safe_remote_source_label(&skill_path)
    } else {
        biwork_skill_source_name(FsPath::new(&skill_path))
    };
    let candidates = if is_remote {
        match remote_skill::fetch_remote_skill_documents(&skill_path).await {
            Ok(documents) => Ok(documents
                .iter()
                .map(build_biwork_remote_skill_candidate)
                .collect::<Vec<_>>()),
            Err(error) => Err(skill_failure(
                source_label.clone(),
                remote_skill_failure_code(&error),
                Some(skill_path.clone()),
                None,
                None,
            )),
        }
    } else if is_zip_file(FsPath::new(&skill_path)) {
        load_biwork_zip_candidates(FsPath::new(&skill_path), &source_label)
    } else {
        discover_biwork_skill_sources(FsPath::new(&skill_path)).map(|sources| {
            sources
                .iter()
                .map(build_biwork_skill_candidate)
                .collect::<Vec<_>>()
        })
    };
    persist_biwork_skill_import(
        &state,
        &ctx,
        &skill_path,
        &source_label,
        Some(skill_path.clone()),
        candidates,
    )
    .await
}

pub async fn biwork_import_skill_upload(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::InvalidInput("SKILL_IMPORT_INVALID_ZIP".to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = field
            .file_name()
            .and_then(|value| FsPath::new(value).file_name())
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("skill-upload.zip")
            .to_string();
        if !file_name.to_ascii_lowercase().ends_with(".zip") {
            return Err(AppError::InvalidInput(
                "SKILL_IMPORT_INVALID_ZIP".to_string(),
            ));
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|_| AppError::InvalidInput("SKILL_IMPORT_INVALID_ZIP".to_string()))?;
        if bytes.len() > BIWORK_SKILL_MAX_TOTAL_BYTES as usize {
            return Err(AppError::InvalidInput(
                "SKILL_IMPORT_TOTAL_TOO_LARGE".to_string(),
            ));
        }
        upload = Some((file_name, bytes.to_vec()));
        break;
    }
    let (source_label, bytes) =
        upload.ok_or_else(|| AppError::InvalidInput("SKILL_IMPORT_INVALID_ZIP".to_string()))?;
    let candidates = remote_skill::parse_zip_skill_documents(&bytes, &source_label)
        .map(|documents| {
            documents
                .iter()
                .map(build_biwork_remote_skill_candidate)
                .collect::<Vec<_>>()
        })
        .map_err(|error| {
            skill_failure(
                source_label.clone(),
                remote_skill_failure_code(&error),
                Some(source_label.clone()),
                None,
                None,
            )
        });
    persist_biwork_skill_import(
        &state,
        &ctx,
        &source_label,
        &source_label,
        Some(source_label.clone()),
        candidates,
    )
    .await
}

async fn persist_biwork_skill_import(
    state: &AppState,
    ctx: &PlatformRequestContext,
    authorization_resource: &str,
    source_label: &str,
    failure_source_path: Option<String>,
    candidates: Result<
        Vec<Result<BiWorkSkillCandidate, BiWorkSkillImportFailure>>,
        BiWorkSkillImportFailure,
    >,
) -> Result<Json<Value>, AppError> {
    require_ferriskey_allow(
        state,
        ctx,
        ctx.tenant_id,
        "create",
        "skill",
        authorization_resource.to_string(),
        None,
    )
    .await?;

    let operation_id = Uuid::new_v4();
    let mut imported_names = Vec::new();
    let mut failures = Vec::new();
    let mut tx = state.connect_pool.begin().await?;

    match candidates {
        Ok(candidates) => {
            for (index, candidate) in candidates.into_iter().enumerate() {
                match candidate {
                    Ok(candidate) => {
                        let (skill_id, overwritten) = upsert_biwork_skill_candidate(
                            &mut tx,
                            ctx.tenant_id,
                            ctx.platform_user_id,
                            operation_id,
                            index,
                            &candidate,
                        )
                        .await?;
                        let status = if overwritten {
                            "overwritten"
                        } else {
                            "imported"
                        };
                        insert_biwork_skill_import_history(
                            &mut tx,
                            BiWorkSkillImportHistory {
                                tenant_id: ctx.tenant_id,
                                operation_id,
                                source_label,
                                source_path: Some(path_to_string(&candidate.source_path)),
                                source_name: &candidate.source_name,
                                skill_id: Some(skill_id),
                                skill_name: Some(candidate.name.clone()),
                                status,
                                failure: None,
                            },
                        )
                        .await?;
                        imported_names.push(candidate.name);
                    }
                    Err(failure) => {
                        insert_biwork_skill_import_history(
                            &mut tx,
                            BiWorkSkillImportHistory {
                                tenant_id: ctx.tenant_id,
                                operation_id,
                                source_label,
                                source_path: failure.error_path.clone(),
                                source_name: &failure.source_name,
                                skill_id: None,
                                skill_name: None,
                                status: "failed",
                                failure: Some(&failure),
                            },
                        )
                        .await?;
                        failures.push(skill_failure_json(&failure));
                    }
                }
            }
        }
        Err(failure) => {
            insert_biwork_skill_import_history(
                &mut tx,
                BiWorkSkillImportHistory {
                    tenant_id: ctx.tenant_id,
                    operation_id,
                    source_label,
                    source_path: failure_source_path,
                    source_name: &failure.source_name,
                    skill_id: None,
                    skill_name: None,
                    status: "failed",
                    failure: Some(&failure),
                },
            )
            .await?;
            failures.push(skill_failure_json(&failure));
        }
    }

    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(json!({
        "skill_name": imported_names.first().cloned().unwrap_or_default(),
        "skill_names": imported_names,
        "failed": failures,
    })))
}

fn load_biwork_zip_candidates(
    path: &FsPath,
    source_label: &str,
) -> Result<Vec<Result<BiWorkSkillCandidate, BiWorkSkillImportFailure>>, BiWorkSkillImportFailure> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        skill_failure(
            source_label.to_string(),
            "SKILL_IMPORT_INVALID_SOURCE",
            Some(path_to_string(path)),
            None,
            None,
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(skill_failure(
            source_label.to_string(),
            "SKILL_IMPORT_SYMLINK_ENTRY",
            Some(path_to_string(path)),
            None,
            None,
        ));
    }
    if !metadata.is_file() || metadata.len() > BIWORK_SKILL_MAX_TOTAL_BYTES {
        return Err(skill_failure(
            source_label.to_string(),
            if metadata.len() > BIWORK_SKILL_MAX_TOTAL_BYTES {
                "SKILL_IMPORT_TOTAL_TOO_LARGE"
            } else {
                "SKILL_IMPORT_INVALID_ZIP"
            },
            Some(path_to_string(path)),
            Some(metadata.len()),
            Some(BIWORK_SKILL_MAX_TOTAL_BYTES),
        ));
    }
    let bytes = fs::read(path).map_err(|_| {
        skill_failure(
            source_label.to_string(),
            "SKILL_IMPORT_INVALID_ZIP",
            Some(path_to_string(path)),
            None,
            None,
        )
    })?;
    remote_skill::parse_zip_skill_documents(&bytes, source_label)
        .map(|documents| {
            documents
                .iter()
                .map(build_biwork_remote_skill_candidate)
                .collect()
        })
        .map_err(|error| {
            skill_failure(
                source_label.to_string(),
                remote_skill_failure_code(&error),
                Some(path_to_string(path)),
                None,
                None,
            )
        })
}

pub async fn biwork_scan_skills(Json(payload): Json<Value>) -> Result<Json<Value>, AppError> {
    let folder_path = required_string(&payload, "folder_path")?;
    let Ok(sources) = discover_biwork_skill_sources(FsPath::new(&folder_path)) else {
        return Ok(ok(json!([])));
    };
    let mut skills = Vec::new();
    for source in sources {
        if let Ok(candidate) = build_biwork_skill_candidate(&source) {
            skills.push(json!({
                "name": candidate.name,
                "description": candidate.description,
                "path": path_to_string(&candidate.source_path),
            }));
        }
    }
    Ok(ok(Value::Array(skills)))
}

pub async fn biwork_list_skill_import_history(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, operation_id, source_label, source_path, source_name, skill_id, skill_name,
               status, error_code, error_path, actual_bytes, limit_bytes,
               line_number, column_number, created_at
        FROM biwork_skill_import_history
        WHERE tenant_id = $1
        ORDER BY created_at DESC
        LIMIT 200
        "#,
    )
    .bind(ctx.tenant_id)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut history = Vec::with_capacity(rows.len());
    for row in rows {
        history.push(json!({
            "id": row.try_get::<Uuid, _>("id")?.to_string(),
            "operation_id": row.try_get::<Uuid, _>("operation_id")?.to_string(),
            "source_label": row.try_get::<String, _>("source_label")?,
            "source_path": row.try_get::<Option<String>, _>("source_path")?,
            "source_name": row.try_get::<String, _>("source_name")?,
            "skill_id": row.try_get::<Option<Uuid>, _>("skill_id")?.map(|id| id.to_string()),
            "skill_name": row.try_get::<Option<String>, _>("skill_name")?,
            "status": row.try_get::<String, _>("status")?,
            "error_code": row.try_get::<Option<String>, _>("error_code")?,
            "error_path": row.try_get::<Option<String>, _>("error_path")?,
            "actual_bytes": row.try_get::<Option<i64>, _>("actual_bytes")?,
            "limit_bytes": row.try_get::<Option<i64>, _>("limit_bytes")?,
            "line": row.try_get::<Option<i32>, _>("line_number")?,
            "column": row.try_get::<Option<i32>, _>("column_number")?,
            "created_at": epoch_ms(row.try_get("created_at")?),
        }));
    }
    Ok(ok(Value::Array(history)))
}

pub async fn biwork_get_skill_paths() -> Result<Json<Value>, AppError> {
    Ok(ok(json!({
        "user_skills_dir": "enterprise://skills/custom",
        "builtin_skills_dir": "enterprise://skills/builtin",
    })))
}

pub async fn biwork_detect_skill_paths(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let external_paths = load_biwork_skill_external_paths(&state, &ctx).await?;
    Ok(ok(Value::Array(biwork_skill_detect_path_entries(
        &external_paths,
    ))))
}

pub async fn biwork_detect_skill_external_sources(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let external_paths = load_biwork_skill_external_paths(&state, &ctx).await?;
    let sources = external_paths
        .iter()
        .filter_map(|entry| {
            let path = entry.get("path")?.as_str()?.trim();
            if path.is_empty() {
                return None;
            }
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| external_path_label(path));
            Some(biwork_external_skill_source_response(
                &name,
                path,
                scan_biwork_external_skill_source(path),
            ))
        })
        .collect::<Vec<_>>();
    Ok(ok(Value::Array(sources)))
}

pub async fn biwork_list_skill_external_paths(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    let paths = load_biwork_skill_external_paths(&state, &ctx).await?;
    Ok(ok(Value::Array(paths)))
}

pub async fn biwork_add_skill_external_path(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let path = required_string(&payload, "path")?;
    let name = trimmed_string(&payload, "name").unwrap_or_else(|| external_path_label(&path));
    let mut paths = load_biwork_skill_external_paths(&state, &ctx).await?;
    paths.retain(|entry| entry.get("path").and_then(Value::as_str) != Some(path.as_str()));
    paths.push(json!({ "name": name, "path": path }));
    save_biwork_skill_external_paths(&state, &ctx, &paths).await?;
    Ok(ok(Value::Null))
}

pub async fn biwork_remove_skill_external_path(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Query(query): Query<SkillExternalPathQuery>,
) -> Result<Json<Value>, AppError> {
    let path = query
        .path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("path is required".to_string()))?;
    let mut paths = load_biwork_skill_external_paths(&state, &ctx).await?;
    paths.retain(|entry| entry.get("path").and_then(Value::as_str) != Some(path));
    save_biwork_skill_external_paths(&state, &ctx, &paths).await?;
    Ok(ok(Value::Null))
}

async fn load_biwork_skill_external_paths(
    state: &AppState,
    ctx: &PlatformRequestContext,
) -> Result<Vec<Value>, AppError> {
    let value: Option<Value> = sqlx::query_scalar(
        r#"
        SELECT value
        FROM user_ui_preferences
        WHERE tenant_id = $1 AND user_id = $2 AND key = $3
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.platform_user_id)
    .bind(BIWORK_SKILL_EXTERNAL_PATHS_KEY)
    .fetch_optional(&state.connect_pool)
    .await?;
    Ok(normalize_biwork_skill_external_paths(
        value.as_ref().unwrap_or(&Value::Null),
    ))
}

async fn save_biwork_skill_external_paths(
    state: &AppState,
    ctx: &PlatformRequestContext,
    paths: &[Value],
) -> Result<(), AppError> {
    require_biwork_user_settings_update(state, ctx).await?;
    set_biwork_client_setting(
        state,
        ctx.tenant_id,
        ctx.platform_user_id,
        BIWORK_SKILL_EXTERNAL_PATHS_KEY,
        &Value::Array(paths.to_vec()),
    )
    .await
}

pub(super) fn normalize_biwork_skill_external_paths(value: &Value) -> Vec<Value> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries {
        let Some(path) = entry
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        if paths
            .iter()
            .any(|existing: &Value| existing.get("path").and_then(Value::as_str) == Some(path))
        {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| external_path_label(path));
        paths.push(json!({ "name": name, "path": path }));
    }
    paths
}

fn external_path_label(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("External Skills")
        .to_string()
}

pub(super) fn biwork_skill_detect_path_entries(external_paths: &[Value]) -> Vec<Value> {
    let mut paths = vec![
        json!({ "name": "Enterprise Custom Skills", "path": "enterprise://skills/custom" }),
        json!({ "name": "Enterprise Builtin Skills", "path": "enterprise://skills/builtin" }),
    ];
    paths.extend(external_paths.iter().cloned());
    paths
}

pub(super) fn biwork_external_skill_source_response(
    name: &str,
    path: &str,
    skills: Vec<Value>,
) -> Value {
    json!({
        "name": name,
        "path": path,
        "source": biwork_external_skill_source_id(path),
        "skills": skills,
    })
}

fn biwork_external_skill_source_id(path: &str) -> String {
    format!("custom-{path}")
}

pub(super) fn scan_biwork_external_skill_source(path: &str) -> Vec<Value> {
    let Ok(sources) = discover_biwork_skill_sources(FsPath::new(path)) else {
        return Vec::new();
    };
    sources
        .iter()
        .filter_map(|source| {
            let candidate = build_biwork_skill_candidate(source).ok()?;
            Some(json!({
                "name": candidate.name,
                "description": candidate.description,
                "path": path_to_string(&candidate.source_path),
            }))
        })
        .collect()
}

async fn set_biwork_skills_market_enabled(
    state: &AppState,
    ctx: &PlatformRequestContext,
    enabled: bool,
) -> Result<Json<Value>, AppError> {
    require_biwork_user_settings_update(state, ctx).await?;
    set_biwork_client_setting(
        state,
        ctx.tenant_id,
        ctx.platform_user_id,
        BIWORK_SKILLS_MARKET_ENABLED_KEY,
        &json!(enabled),
    )
    .await?;
    Ok(ok(json!({ "enabled": enabled })))
}

pub async fn biwork_enable_skills_market(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    set_biwork_skills_market_enabled(&state, &ctx, true).await
}

pub async fn biwork_disable_skills_market(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
) -> Result<Json<Value>, AppError> {
    set_biwork_skills_market_enabled(&state, &ctx, false).await
}

pub async fn biwork_delete_skill(
    State(state): State<AppState>,
    Extension(ctx): Extension<PlatformRequestContext>,
    Path(skill_name): Path<String>,
) -> Result<Json<Value>, AppError> {
    let skill_name = skill_name.trim();
    if skill_name.is_empty() {
        return Err(AppError::InvalidInput("skill_name is required".to_string()));
    }
    require_ferriskey_allow(
        &state,
        &ctx,
        ctx.tenant_id,
        "delete",
        "skill",
        skill_name.to_string(),
        None,
    )
    .await?;

    let mut tx = state.connect_pool.begin().await?;
    let deleted: Option<Uuid> = sqlx::query_scalar(
        r#"
        UPDATE skills
        SET status = 'deleted',
            deleted_at = CURRENT_TIMESTAMP,
            updated_at = CURRENT_TIMESTAMP
        WHERE tenant_id = $1
          AND name = $2
          AND deleted_at IS NULL
          AND COALESCE(metadata->>'source', 'custom') NOT IN ('builtin', 'extension', 'cron')
        RETURNING id
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(skill_name)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(skill_id) = deleted else {
        return Err(AppError::NotFound("skill not found".to_string()));
    };
    sqlx::query(
        r#"
        UPDATE skill_versions
        SET status = 'disabled'
        WHERE tenant_id = $1
          AND skill_id = $2
          AND status = 'published'
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(skill_id)
    .execute(&mut *tx)
    .await?;
    tx.commit()
        .await
        .map_err(|_| AppError::DatabaseTransaction)?;
    Ok(ok(Value::Null))
}

fn normalize_biwork_skill_source(source: Option<&str>) -> String {
    match source
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "builtin" => "builtin".to_string(),
        "cron" => "cron".to_string(),
        "extension" => "extension".to_string(),
        _ => "custom".to_string(),
    }
}

fn biwork_skill_location(metadata: &Value, skill_id: Uuid) -> String {
    value_string(metadata, "source_path")
        .or_else(|| value_string(metadata, "location"))
        .unwrap_or_else(|| format!("enterprise://skills/{skill_id}"))
}

fn biwork_skill_info_value(
    skill_id: Uuid,
    name: String,
    description: String,
    status: String,
    metadata: Value,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
) -> Value {
    let source = normalize_biwork_skill_source(value_string(&metadata, "source").as_deref());
    let location = biwork_skill_location(&metadata, skill_id);
    let is_auto_inject = metadata
        .get("is_auto_inject")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "id": skill_id.to_string(),
        "name": name,
        "description": description,
        "location": location,
        "relative_location": value_string(&metadata, "relative_location"),
        "is_auto_inject": is_auto_inject,
        "is_custom": source == "custom",
        "source": source,
        "enabled": status == "active",
        "status": status,
        "metadata": metadata,
        "created_at": epoch_ms(created_at),
        "updated_at": epoch_ms(updated_at),
    })
}

fn build_biwork_skill_candidate_from_payload(
    payload: &Value,
) -> Result<BiWorkSkillCandidate, AppError> {
    let raw_name = trimmed_string(payload, "name")
        .or_else(|| trimmed_string(payload, "skill_name"))
        .ok_or_else(|| AppError::InvalidInput("name is required".to_string()))?;
    let name = slugify_biwork_skill_name(&raw_name);
    if name.is_empty() {
        return Err(AppError::InvalidInput(
            "name must contain an ASCII letter or digit".to_string(),
        ));
    }

    let explicit_description = trimmed_string(payload, "description");
    let content = trimmed_string(payload, "content")
        .or_else(|| trimmed_string(payload, "markdown"))
        .unwrap_or_else(|| {
            let description = explicit_description.clone().unwrap_or_default();
            if description.is_empty() {
                format!("# {raw_name}\n")
            } else {
                format!("# {raw_name}\n\n{description}\n")
            }
        });
    let total_bytes = content.len() as u64;
    if total_bytes > BIWORK_SKILL_MAX_FILE_BYTES {
        return Err(AppError::InvalidInput(format!(
            "content exceeds {} bytes",
            BIWORK_SKILL_MAX_FILE_BYTES
        )));
    }

    let parsed_description = parse_biwork_skill_markdown(&content, &raw_name)
        .map(|(_, description)| description)
        .unwrap_or_default();
    let description = explicit_description
        .filter(|value| !value.is_empty())
        .unwrap_or(parsed_description);
    let source_path = trimmed_string(payload, "location")
        .or_else(|| trimmed_string(payload, "source_path"))
        .unwrap_or_else(|| format!("enterprise://skills/custom/{name}/SKILL.md"));
    let source_name = trimmed_string(payload, "source_name").unwrap_or_else(|| name.clone());

    Ok(BiWorkSkillCandidate {
        name,
        description,
        source_path: PathBuf::from(source_path),
        source_name,
        content,
        total_bytes,
    })
}

async fn read_biwork_builtin_catalog_content(
    state: &AppState,
    tenant_id: Uuid,
    payload: &Value,
) -> Result<String, AppError> {
    let file_name = required_string(payload, "file_name")?;
    let refs = biwork_builtin_skill_ref_candidates(&file_name);
    if refs.is_empty() {
        return Ok(String::new());
    }
    let patterns = biwork_builtin_skill_ref_patterns(&refs);

    let row = sqlx::query(
        r#"
        SELECT latest.manifest
        FROM skills s
        LEFT JOIN LATERAL (
            SELECT manifest, source_uri
            FROM skill_versions sv
            WHERE sv.tenant_id = s.tenant_id
              AND sv.skill_id = s.id
              AND sv.status = 'published'
            ORDER BY sv.created_at DESC
            LIMIT 1
        ) latest ON TRUE
        WHERE s.tenant_id = $1
          AND s.deleted_at IS NULL
          AND (
              lower(s.name) = ANY($2::text[])
              OR lower(coalesce(s.metadata->>'source_name', '')) = ANY($2::text[])
              OR lower(coalesce(s.metadata->>'relative_location', '')) = ANY($2::text[])
              OR lower(coalesce(s.metadata->>'source_path', '')) = ANY($2::text[])
              OR lower(coalesce(latest.source_uri, '')) = ANY($2::text[])
              OR lower(coalesce(latest.manifest->>'source_path', '')) = ANY($2::text[])
              OR EXISTS (
                  SELECT 1
                  FROM unnest($3::text[]) pattern
                  WHERE lower(coalesce(s.metadata->>'relative_location', '')) LIKE pattern
                     OR lower(coalesce(s.metadata->>'source_path', '')) LIKE pattern
                     OR lower(coalesce(latest.source_uri, '')) LIKE pattern
                     OR lower(coalesce(latest.manifest->>'source_path', '')) LIKE pattern
              )
          )
        ORDER BY CASE WHEN lower(coalesce(s.metadata->>'source', '')) = 'builtin' THEN 0 ELSE 1 END,
                 s.updated_at DESC,
                 s.created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(&refs)
    .bind(&patterns)
    .fetch_optional(&state.connect_pool)
    .await?;

    let Some(row) = row else {
        return Ok(String::new());
    };
    let manifest: Option<Value> = row.try_get("manifest")?;
    Ok(manifest
        .as_ref()
        .and_then(|manifest| value_string(manifest, "content"))
        .unwrap_or_default())
}

pub(super) fn biwork_builtin_skill_ref_candidates(file_name: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let normalized = file_name.replace('\\', "/");
    let normalized = normalized.trim().trim_matches('/').to_ascii_lowercase();
    if normalized.is_empty() {
        return refs;
    }

    let mut base = normalized.clone();
    if base.ends_with("/skill.md") {
        base.truncate(base.len() - "/skill.md".len());
    } else if base.ends_with("/skill") {
        base.truncate(base.len() - "/skill".len());
    } else if base.ends_with(".md") {
        base.truncate(base.len() - ".md".len());
    }
    push_unique_skill_ref(&mut refs, base.clone());
    if let Some(last) = base.rsplit('/').next() {
        push_unique_skill_ref(&mut refs, last.to_string());
    }
    push_unique_skill_ref(&mut refs, normalized);
    refs
}

fn biwork_builtin_skill_ref_patterns(refs: &[String]) -> Vec<String> {
    let mut patterns = Vec::new();
    for value in refs {
        push_unique_skill_ref(&mut patterns, format!("%/{value}"));
        push_unique_skill_ref(&mut patterns, format!("%/{value}.md"));
        push_unique_skill_ref(&mut patterns, format!("%/{value}/skill.md"));
    }
    patterns
}

fn push_unique_skill_ref(output: &mut Vec<String>, value: String) {
    let value = value.trim().trim_matches('/').to_ascii_lowercase();
    if !value.is_empty() && !output.iter().any(|existing| existing == &value) {
        output.push(value);
    }
}

pub(super) fn path_to_string(path: &FsPath) -> String {
    path.to_string_lossy().to_string()
}

fn biwork_skill_source_name(path: &FsPath) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path_to_string(path))
}

fn skill_failure(
    source_name: impl Into<String>,
    code: &str,
    error_path: Option<String>,
    actual_bytes: Option<u64>,
    limit_bytes: Option<u64>,
) -> BiWorkSkillImportFailure {
    BiWorkSkillImportFailure {
        source_name: source_name.into(),
        code: code.to_string(),
        error_path,
        actual_bytes,
        limit_bytes,
        line: None,
        column: None,
    }
}

fn skill_failure_json(failure: &BiWorkSkillImportFailure) -> Value {
    json!({
        "source_name": failure.source_name.clone(),
        "code": failure.code.clone(),
        "error_path": failure.error_path.clone(),
        "actual_bytes": failure.actual_bytes,
        "limit_bytes": failure.limit_bytes,
        "line": failure.line,
        "column": failure.column,
    })
}

fn bounded_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn is_markdown_file(path: &FsPath) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("md"))
        .unwrap_or(false)
}

fn is_zip_file(path: &FsPath) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn find_skill_markdown_file(dir: &FsPath) -> Option<PathBuf> {
    ["SKILL.md", "skill.md", "Skill.md"]
        .iter()
        .map(|file_name| dir.join(file_name))
        .find(|path| path.is_file())
}

pub(super) fn discover_biwork_skill_sources(
    path: &FsPath,
) -> Result<Vec<BiWorkSkillSource>, BiWorkSkillImportFailure> {
    let source_name = biwork_skill_source_name(path);
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        skill_failure(
            source_name.clone(),
            "SKILL_IMPORT_INVALID_SOURCE",
            Some(path_to_string(path)),
            None,
            None,
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(skill_failure(
            source_name,
            "SKILL_IMPORT_SYMLINK_ENTRY",
            Some(path_to_string(path)),
            None,
            None,
        ));
    }
    if metadata.is_file() {
        if is_zip_file(path) {
            return Err(skill_failure(
                source_name,
                "SKILL_IMPORT_INVALID_ZIP",
                Some(path_to_string(path)),
                None,
                None,
            ));
        }
        if is_markdown_file(path) {
            return Ok(vec![BiWorkSkillSource {
                skill_file: path.to_path_buf(),
                source_root: path.to_path_buf(),
            }]);
        }
        return Err(skill_failure(
            source_name,
            "SKILL_IMPORT_INVALID_SOURCE",
            Some(path_to_string(path)),
            None,
            None,
        ));
    }
    if !metadata.is_dir() {
        return Err(skill_failure(
            source_name,
            "SKILL_IMPORT_INVALID_SOURCE",
            Some(path_to_string(path)),
            None,
            None,
        ));
    }
    if let Some(skill_file) = find_skill_markdown_file(path) {
        return Ok(vec![BiWorkSkillSource {
            skill_file,
            source_root: path.to_path_buf(),
        }]);
    }

    let mut children = fs::read_dir(path)
        .map_err(|_| {
            skill_failure(
                source_name.clone(),
                "SKILL_IMPORT_INVALID_SOURCE",
                Some(path_to_string(path)),
                None,
                None,
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    children.sort();

    let mut sources = Vec::new();
    for child in children {
        let Ok(child_metadata) = fs::symlink_metadata(&child) else {
            continue;
        };
        if child_metadata.file_type().is_symlink() {
            continue;
        }
        if child_metadata.is_dir() {
            if let Some(skill_file) = find_skill_markdown_file(&child) {
                sources.push(BiWorkSkillSource {
                    skill_file,
                    source_root: child,
                });
            }
        } else if child_metadata.is_file() && is_markdown_file(&child) {
            sources.push(BiWorkSkillSource {
                skill_file: child.clone(),
                source_root: child,
            });
        }
    }

    if sources.is_empty() {
        Err(skill_failure(
            source_name,
            "SKILL_IMPORT_NO_SKILL_FOUND",
            Some(path_to_string(path)),
            None,
            None,
        ))
    } else {
        Ok(sources)
    }
}

fn measure_biwork_skill_source(
    root: &FsPath,
    source_name: &str,
) -> Result<u64, BiWorkSkillImportFailure> {
    let mut total = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            skill_failure(
                source_name.to_string(),
                "SKILL_IMPORT_FAILED",
                Some(path_to_string(&path)),
                None,
                None,
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(skill_failure(
                source_name.to_string(),
                "SKILL_IMPORT_SYMLINK_ENTRY",
                Some(path_to_string(&path)),
                None,
                None,
            ));
        }
        if metadata.is_dir() {
            let entries = fs::read_dir(&path).map_err(|_| {
                skill_failure(
                    source_name.to_string(),
                    "SKILL_IMPORT_FAILED",
                    Some(path_to_string(&path)),
                    None,
                    None,
                )
            })?;
            for entry in entries {
                let entry = entry.map_err(|_| {
                    skill_failure(
                        source_name.to_string(),
                        "SKILL_IMPORT_FAILED",
                        Some(path_to_string(&path)),
                        None,
                        None,
                    )
                })?;
                stack.push(entry.path());
            }
            continue;
        }
        if metadata.is_file() {
            let file_size = metadata.len();
            if file_size > BIWORK_SKILL_MAX_FILE_BYTES {
                return Err(skill_failure(
                    source_name.to_string(),
                    "SKILL_IMPORT_FILE_TOO_LARGE",
                    Some(path_to_string(&path)),
                    Some(file_size),
                    Some(BIWORK_SKILL_MAX_FILE_BYTES),
                ));
            }
            total = total.saturating_add(file_size);
            if total > BIWORK_SKILL_MAX_TOTAL_BYTES {
                return Err(skill_failure(
                    source_name.to_string(),
                    "SKILL_IMPORT_TOTAL_TOO_LARGE",
                    Some(path_to_string(root)),
                    Some(total),
                    Some(BIWORK_SKILL_MAX_TOTAL_BYTES),
                ));
            }
        }
    }
    Ok(total)
}

pub(super) fn build_biwork_skill_candidate(
    source: &BiWorkSkillSource,
) -> Result<BiWorkSkillCandidate, BiWorkSkillImportFailure> {
    let source_name = biwork_skill_source_name(&source.source_root);
    let total_bytes = measure_biwork_skill_source(&source.source_root, &source_name)?;
    let content = fs::read_to_string(&source.skill_file).map_err(|_| {
        skill_failure(
            source_name.clone(),
            "SKILL_IMPORT_FAILED",
            Some(path_to_string(&source.skill_file)),
            None,
            None,
        )
    })?;
    let fallback_name = source
        .source_root
        .file_stem()
        .or_else(|| source.source_root.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or(&source_name);
    let (name, description) =
        parse_biwork_skill_markdown(&content, fallback_name).ok_or_else(|| {
            skill_failure(
                source_name.clone(),
                "SKILL_IMPORT_INVALID_NAME",
                Some(path_to_string(&source.skill_file)),
                None,
                None,
            )
        })?;
    Ok(BiWorkSkillCandidate {
        name,
        description,
        source_path: source.source_root.clone(),
        source_name,
        content,
        total_bytes,
    })
}

fn build_biwork_remote_skill_candidate(
    document: &RemoteSkillDocument,
) -> Result<BiWorkSkillCandidate, BiWorkSkillImportFailure> {
    let (name, description) = parse_biwork_skill_markdown(&document.content, &document.source_name)
        .ok_or_else(|| {
            skill_failure(
                document.source_name.clone(),
                "SKILL_IMPORT_INVALID_NAME",
                Some(document.source_uri.clone()),
                None,
                None,
            )
        })?;
    Ok(BiWorkSkillCandidate {
        name,
        description,
        source_path: PathBuf::from(&document.source_uri),
        source_name: document.source_name.clone(),
        content: document.content.clone(),
        total_bytes: document.total_bytes,
    })
}

fn remote_skill_failure_code(error: &AppError) -> &str {
    let message = error.to_string();
    [
        "SKILL_IMPORT_NO_SKILL_FOUND",
        "SKILL_IMPORT_REMOTE_TREE_TOO_LARGE",
        "SKILL_IMPORT_REMOTE_SKILL_COUNT_EXCEEDED",
        "SKILL_IMPORT_ARCHIVE_ENTRY_LIMIT",
        "SKILL_IMPORT_FILE_TOO_LARGE",
        "SKILL_IMPORT_TOTAL_TOO_LARGE",
        "SKILL_IMPORT_REMOTE_CONTENT_NOT_UTF8",
        "SKILL_IMPORT_REMOTE_LOOKUP_FAILED",
        "SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED",
        "SKILL_IMPORT_REMOTE_REDIRECT_LIMIT",
        "SKILL_IMPORT_REMOTE_URL_INVALID",
        "SKILL_IMPORT_REMOTE_ADDRESS_BLOCKED",
        "SKILL_IMPORT_UNSUPPORTED_REMOTE_CONTENT",
        "SKILL_IMPORT_INVALID_ZIP",
        "SKILL_IMPORT_SYMLINK_ENTRY",
    ]
    .into_iter()
    .find(|code| message.contains(code))
    .unwrap_or("SKILL_IMPORT_REMOTE_FAILED")
}

pub(super) fn parse_biwork_skill_markdown(
    content: &str,
    fallback_name: &str,
) -> Option<(String, String)> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut body_start = 0_usize;
    let mut frontmatter_name = None;
    let mut frontmatter_description = None;

    if lines.first().map(|line| line.trim()) == Some("---") {
        for (index, line) in lines.iter().enumerate().skip(1) {
            if line.trim() == "---" {
                body_start = index + 1;
                break;
            }
            if let Some(value) = parse_frontmatter_field(line, "name") {
                frontmatter_name = Some(value);
            } else if let Some(value) = parse_frontmatter_field(line, "description") {
                frontmatter_description = Some(value);
            }
        }
    }

    let heading_name = lines
        .iter()
        .skip(body_start)
        .map(|line| line.trim())
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let description = frontmatter_description
        .or_else(|| {
            lines
                .iter()
                .skip(body_start)
                .map(|line| line.trim())
                .find(|line| {
                    !line.is_empty()
                        && !line.starts_with('#')
                        && !line.starts_with("```")
                        && *line != "---"
                })
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default();
    let raw_name = frontmatter_name
        .or(heading_name)
        .unwrap_or_else(|| fallback_name.to_string());
    let name = slugify_biwork_skill_name(&raw_name);
    if name.is_empty() {
        None
    } else {
        Some((name, description))
    }
}

fn parse_frontmatter_field(line: &str, key: &str) -> Option<String> {
    let (raw_key, raw_value) = line.split_once(':')?;
    if raw_key.trim().eq_ignore_ascii_case(key) {
        let value = raw_value.trim().trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    } else {
        None
    }
}

fn slugify_biwork_skill_name(value: &str) -> String {
    let mut output = String::new();
    let mut previous_dash = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            output.push(character);
            previous_dash = false;
        } else if (character.is_ascii_whitespace() || character == '-' || character == '_')
            && !output.is_empty()
            && !previous_dash
        {
            output.push('-');
            previous_dash = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    output
}

async fn upsert_biwork_skill_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    actor_user_id: Uuid,
    operation_id: Uuid,
    index: usize,
    candidate: &BiWorkSkillCandidate,
) -> Result<(Uuid, bool), AppError> {
    let mut hasher = Sha256::new();
    hasher.update(candidate.content.as_bytes());
    let content_hash = hex::encode(hasher.finalize());
    let source_path = path_to_string(&candidate.source_path);
    let metadata = json!({
        "source": "custom",
        "source_path": source_path.clone(),
        "source_name": candidate.source_name.clone(),
        "imported_via": "biwork",
        "import_operation_id": operation_id.to_string(),
        "imported_by": actor_user_id.to_string(),
        "content_hash": content_hash.clone(),
        "total_bytes": candidate.total_bytes,
    });

    let existing_id: Option<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM skills
        WHERE tenant_id = $1 AND lower(name) = lower($2)
        ORDER BY (deleted_at IS NULL) DESC, updated_at DESC, created_at DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(&candidate.name)
    .fetch_optional(&mut **tx)
    .await?;

    let overwritten = existing_id.is_some();
    let skill_id = if let Some(skill_id) = existing_id {
        sqlx::query_scalar(
            r#"
            UPDATE skills
            SET description = $3,
                metadata = $4,
                status = 'active',
                deleted_at = NULL,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1 AND tenant_id = $2
            RETURNING id
            "#,
        )
        .bind(skill_id)
        .bind(tenant_id)
        .bind(&candidate.description)
        .bind(&metadata)
        .fetch_one(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar(
            r#"
            INSERT INTO skills (tenant_id, name, description, metadata, status)
            VALUES ($1, $2, $3, $4, 'active')
            ON CONFLICT (tenant_id, lower(name)) WHERE deleted_at IS NULL
            DO UPDATE SET description = EXCLUDED.description,
                          metadata = EXCLUDED.metadata,
                          status = 'active',
                          updated_at = CURRENT_TIMESTAMP
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(&candidate.name)
        .bind(&candidate.description)
        .bind(&metadata)
        .fetch_one(&mut **tx)
        .await?
    };

    sqlx::query(
        r#"
        UPDATE skill_versions
        SET status = 'disabled'
        WHERE tenant_id = $1
          AND skill_id = $2
          AND status = 'published'
        "#,
    )
    .bind(tenant_id)
    .bind(skill_id)
    .execute(&mut **tx)
    .await?;

    let version_label = format!("biwork-import-{}-{index}", operation_id.simple());
    let manifest = json!({
        "kind": "biwork_skill",
        "name": candidate.name.clone(),
        "description": candidate.description.clone(),
        "source_path": source_path.clone(),
        "content": candidate.content.clone(),
        "metadata": metadata.clone(),
    });
    sqlx::query(
        r#"
        INSERT INTO skill_versions
            (tenant_id, skill_id, version_label, manifest, content_hash, source_uri, status)
        VALUES ($1, $2, $3, $4, $5, $6, 'published')
        ON CONFLICT (skill_id, version_label)
        DO UPDATE SET manifest = EXCLUDED.manifest,
                      content_hash = EXCLUDED.content_hash,
                      source_uri = EXCLUDED.source_uri,
                      status = 'published'
        "#,
    )
    .bind(tenant_id)
    .bind(skill_id)
    .bind(version_label)
    .bind(manifest)
    .bind(content_hash)
    .bind(source_path)
    .execute(&mut **tx)
    .await?;

    Ok((skill_id, overwritten))
}

struct BiWorkSkillImportHistory<'a> {
    tenant_id: Uuid,
    operation_id: Uuid,
    source_label: &'a str,
    source_path: Option<String>,
    source_name: &'a str,
    skill_id: Option<Uuid>,
    skill_name: Option<String>,
    status: &'a str,
    failure: Option<&'a BiWorkSkillImportFailure>,
}

async fn insert_biwork_skill_import_history(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    history: BiWorkSkillImportHistory<'_>,
) -> Result<(), AppError> {
    let BiWorkSkillImportHistory {
        tenant_id,
        operation_id,
        source_label,
        source_path,
        source_name,
        skill_id,
        skill_name,
        status,
        failure,
    } = history;
    let error_code = failure.map(|failure| failure.code.clone());
    let error_path = failure.and_then(|failure| failure.error_path.clone());
    let actual_bytes = failure
        .and_then(|failure| failure.actual_bytes)
        .map(bounded_i64);
    let limit_bytes = failure
        .and_then(|failure| failure.limit_bytes)
        .map(bounded_i64);
    let line = failure.and_then(|failure| failure.line);
    let column = failure.and_then(|failure| failure.column);
    sqlx::query(
        r#"
        INSERT INTO biwork_skill_import_history
            (tenant_id, operation_id, source_label, source_path, source_name, skill_id, skill_name,
             status, error_code, error_path, actual_bytes, limit_bytes, line_number, column_number)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(tenant_id)
    .bind(operation_id)
    .bind(source_label)
    .bind(source_path)
    .bind(source_name)
    .bind(skill_id)
    .bind(skill_name)
    .bind(status)
    .bind(error_code)
    .bind(error_path)
    .bind(actual_bytes)
    .bind(limit_bytes)
    .bind(line)
    .bind(column)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
