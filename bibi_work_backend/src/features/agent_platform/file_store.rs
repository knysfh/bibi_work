use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use tracing::warn;
use uuid::Uuid;

use crate::{features::core::errors::AppError, startup::AppState};

use super::{
    audit::{self, NewAuditLog},
    event_store, file_lock,
    models::{
        FileEntryResponse, FileReadRequest, FileRevisionResponse, FileWriteRequest, RunEventInput,
    },
};

struct RunEventContext {
    conversation_id: Uuid,
    trace_id: String,
}

const FILE_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
const BINARY_CONTENT_TYPE: &str = "application/octet-stream";
const LARGE_FILE_THRESHOLD_BYTES: i64 = 1024 * 1024;

struct ResolvedFileContent {
    bytes: Vec<u8>,
    inline_text: Option<String>,
    content_type: String,
    is_binary: bool,
    is_large: bool,
    object_extension: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileRevisionSelector {
    Latest,
    Revision(i64),
    VersionId(String),
}

pub fn validate_virtual_path(path: &str) -> Result<(), AppError> {
    if path.contains('\0') {
        return Err(AppError::InvalidInput(
            "path contains null byte".to_string(),
        ));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(AppError::InvalidInput(
            "path may not contain ..".to_string(),
        ));
    }
    if path.starts_with("//") {
        return Err(AppError::InvalidInput(
            "path may not start with //".to_string(),
        ));
    }
    Ok(())
}

pub fn path_hash(path: &str) -> Result<String, AppError> {
    validate_virtual_path(path)?;
    Ok(sha256_hex(path.as_bytes()))
}

fn file_revision_selector(payload: &FileReadRequest) -> Result<FileRevisionSelector, AppError> {
    match (payload.revision, payload.version_id.as_deref()) {
        (None, None) => Ok(FileRevisionSelector::Latest),
        (Some(revision), None) if revision > 0 => Ok(FileRevisionSelector::Revision(revision)),
        (Some(_), None) => Err(AppError::InvalidInput(
            "revision must be greater than zero".to_string(),
        )),
        (None, Some(version_id)) if !version_id.trim().is_empty() => Ok(
            FileRevisionSelector::VersionId(version_id.trim().to_string()),
        ),
        (None, Some(_)) => Err(AppError::InvalidInput(
            "version_id must not be empty".to_string(),
        )),
        (Some(_), Some(_)) => Err(AppError::InvalidInput(
            "revision and version_id are mutually exclusive".to_string(),
        )),
    }
}

pub async fn read_revision(
    state: &AppState,
    payload: FileReadRequest,
) -> Result<FileRevisionResponse, AppError> {
    let path_hash = path_hash(&payload.path)?;
    let selector = file_revision_selector(&payload)?;
    let row = match selector {
        FileRevisionSelector::Latest => {
            load_latest_revision_row(
                &state.connect_pool,
                payload.tenant_id,
                payload.project_id,
                &path_hash,
            )
            .await?
        }
        FileRevisionSelector::Revision(revision) => {
            load_revision_row_by_number(
                &state.connect_pool,
                payload.tenant_id,
                payload.project_id,
                &path_hash,
                revision,
            )
            .await?
        }
        FileRevisionSelector::VersionId(version_id) => {
            load_revision_row_by_version_id(
                &state.connect_pool,
                payload.tenant_id,
                payload.project_id,
                &path_hash,
                &version_id,
            )
            .await?
        }
    }
    .ok_or_else(|| AppError::NotFound("file not found".to_string()))?;

    let mut revision = file_revision_from_row(row)?;
    if payload.include_content.unwrap_or(true) {
        hydrate_revision_content(state, &mut revision, payload.allow_binary.unwrap_or(false))
            .await?;
    }
    Ok(revision)
}

pub async fn write_revision(
    state: &AppState,
    payload: FileWriteRequest,
) -> Result<FileRevisionResponse, AppError> {
    validate_virtual_path(&payload.path)?;
    let resolved_content = resolve_write_content(&payload)?;
    let size_bytes = i64::try_from(resolved_content.bytes.len())?;
    let path_hash = sha256_hex(payload.path.as_bytes());
    let content_hash = sha256_hex(&resolved_content.bytes);

    let mut tx = state.connect_pool.begin().await?;
    lock_file_revision_tx(&mut tx, payload.project_id, &path_hash).await?;
    file_lock::ensure_write_permitted_tx(&mut tx, &payload, &path_hash).await?;

    let current_revision: i64 = sqlx::query(
        r#"
        SELECT COALESCE(MAX(revision), 0) AS revision
        FROM file_revisions
        WHERE tenant_id = $1 AND project_id = $2 AND path_hash = $3
        "#,
    )
    .bind(payload.tenant_id)
    .bind(payload.project_id)
    .bind(&path_hash)
    .fetch_one(&mut *tx)
    .await?
    .try_get("revision")?;

    if current_revision != payload.expected_revision {
        return Err(AppError::Conflict(format!(
            "file revision conflict: expected {}, current {}",
            payload.expected_revision, current_revision
        )));
    }

    let next_revision = current_revision + 1;
    let tenant_slug = load_tenant_slug_in_tx(&mut tx, payload.tenant_id).await?;
    let object_key = object_key_for_revision(
        &tenant_slug,
        payload.project_id,
        &path_hash,
        next_revision,
        &content_hash,
        resolved_content.object_extension,
    );

    let object = state
        .rustfs_client
        .put_file_object(&object_key, resolved_content.bytes.clone())
        .await?;
    let object_key_for_cleanup = object.as_ref().map(|object| object.object_key.clone());
    let result = async {
        let etag = object
            .as_ref()
            .and_then(|object| object.etag.clone())
            .unwrap_or_else(|| format!("sha256:{content_hash}"));
        let object_reference_id = if let Some(object) = object {
            Some(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO object_references (
                        tenant_id, bucket, object_key, version_id, etag, content_hash,
                        size_bytes, content_type, owner_resource_type, owner_resource_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'file_revision', $9)
                    RETURNING id
                    "#,
                )
                .bind(payload.tenant_id)
                .bind(object.bucket)
                .bind(object.object_key)
                .bind(object.version_id)
                .bind(object.etag)
                .bind(&content_hash)
                .bind(size_bytes)
                .bind(&resolved_content.content_type)
                .bind(format!("{}:{}", payload.project_id, &path_hash))
                .fetch_one(&mut *tx)
                .await?,
            )
        } else {
            None
        };
        let stored_inline_content =
            stored_inline_content(object_reference_id.is_some(), &resolved_content);
        let metadata = json!({
            "storage": if object_reference_id.is_some() { "rustfs" } else { "inline" },
            "content_type": resolved_content.content_type,
            "is_binary": resolved_content.is_binary,
            "is_large": resolved_content.is_large,
            "content_encoding": if resolved_content.is_binary { "base64" } else { "utf-8" },
            "large_threshold_bytes": LARGE_FILE_THRESHOLD_BYTES
        });

        let revision_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO file_revisions (
                tenant_id, project_id, path, path_hash, revision, etag, content_hash,
                object_key, object_reference_id, inline_content, size_bytes, reason, run_id,
                last_writer_user_id, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            RETURNING id
            "#,
        )
        .bind(payload.tenant_id)
        .bind(payload.project_id)
        .bind(payload.path.clone())
        .bind(path_hash.clone())
        .bind(next_revision)
        .bind(etag)
        .bind(&content_hash)
        .bind(object_key)
        .bind(object_reference_id)
        .bind(stored_inline_content)
        .bind(size_bytes)
        .bind(payload.reason.clone())
        .bind(payload.run_id)
        .bind(payload.actor_user_id)
        .bind(metadata)
        .fetch_one(&mut *tx)
        .await?;

        upsert_file_search_document_tx(
            &mut tx,
            payload.tenant_id,
            payload.project_id,
            &payload.path,
            &path_hash,
            next_revision,
            &content_hash,
            revision_id,
            &resolved_content,
        )
        .await?;

        let row = load_revision_row_by_id_tx(&mut tx, revision_id).await?;
        let mut revision = file_revision_from_row(row)?;
        populate_revision_content_from_bytes(&mut revision, &resolved_content.bytes)?;

        let run_context = if let Some(run_id) = payload.run_id {
            Some(load_run_context_in_tx(&mut tx, run_id).await?)
        } else {
            None
        };

        insert_file_write_audit_tx(
            &mut tx,
            &payload,
            &revision,
            &path_hash,
            run_context.as_ref(),
        )
        .await?;

        let event = if let Some(run_id) = payload.run_id {
            let run = run_context
                .as_ref()
                .ok_or_else(|| AppError::NotFound("run not found".to_string()))?;
            Some(
                event_store::insert_event_tx(
                    &mut tx,
                    payload.tenant_id,
                    run.conversation_id,
                    Some(run_id),
                    RunEventInput {
                        event_id: Some(format!(
                            "file.changed.{}.{}",
                            revision.id, revision.revision
                        )),
                        event_type: "file.changed".to_string(),
                        payload: Some(json!({
                            "project_id": revision.project_id,
                            "path": revision.path,
                            "revision": revision.revision,
                            "etag": revision.etag,
                            "size_bytes": revision.size_bytes,
                            "content_type": revision.content_type,
                            "is_binary": revision.is_binary,
                            "is_large": revision.is_large
                        })),
                        trace_id: Some(run.trace_id.clone()),
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
            event_store::publish_single_event(state, &event).await;
        }

        Ok(revision)
    }
    .await;

    if result.is_err() {
        cleanup_orphan_file_object(state, object_key_for_cleanup.as_deref()).await;
    }

    result
}

fn resolve_write_content(payload: &FileWriteRequest) -> Result<ResolvedFileContent, AppError> {
    if payload.content_ref.is_some() {
        return Err(AppError::InvalidInput(
            "content_ref writes are not supported until trusted object copy is implemented"
                .to_string(),
        ));
    }

    match (&payload.inline_content, &payload.content_base64) {
        (Some(_), Some(_)) => Err(AppError::InvalidInput(
            "inline_content and content_base64 are mutually exclusive".to_string(),
        )),
        (None, None) => Err(AppError::InvalidInput(
            "inline_content or content_base64 is required".to_string(),
        )),
        (Some(content), None) => {
            let content_type = payload
                .content_type
                .clone()
                .unwrap_or_else(|| FILE_CONTENT_TYPE.to_string());
            let bytes = content.as_bytes().to_vec();
            let is_binary = !is_textual_content_type(&content_type);
            let is_large = i64::try_from(bytes.len())? > LARGE_FILE_THRESHOLD_BYTES;
            Ok(ResolvedFileContent {
                bytes,
                inline_text: if is_binary {
                    None
                } else {
                    Some(content.clone())
                },
                content_type,
                is_binary,
                is_large,
                object_extension: if is_binary { "bin" } else { "txt" },
            })
        }
        (None, Some(content_base64)) => {
            let bytes = BASE64_STANDARD
                .decode(content_base64)
                .map_err(|_| AppError::InvalidInput("content_base64 is invalid".to_string()))?;
            let content_type = payload
                .content_type
                .clone()
                .unwrap_or_else(|| BINARY_CONTENT_TYPE.to_string());
            let textual = is_textual_content_type(&content_type);
            let inline_text = if textual {
                Some(String::from_utf8(bytes.clone()).map_err(|_| {
                    AppError::InvalidInput(
                        "text content_base64 must decode to valid UTF-8".to_string(),
                    )
                })?)
            } else {
                None
            };
            let is_binary = !textual;
            let is_large = i64::try_from(bytes.len())? > LARGE_FILE_THRESHOLD_BYTES;
            Ok(ResolvedFileContent {
                bytes,
                inline_text,
                content_type,
                is_binary,
                is_large,
                object_extension: if is_binary { "bin" } else { "txt" },
            })
        }
    }
}

fn stored_inline_content(
    object_reference_exists: bool,
    content: &ResolvedFileContent,
) -> Option<String> {
    if object_reference_exists {
        return None;
    }

    if content.is_binary {
        Some(BASE64_STANDARD.encode(&content.bytes))
    } else {
        content.inline_text.clone()
    }
}

async fn upsert_file_search_document_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    project_id: Uuid,
    path: &str,
    path_hash: &str,
    revision: i64,
    content_hash: &str,
    file_revision_id: Uuid,
    content: &ResolvedFileContent,
) -> Result<(), AppError> {
    if content.is_binary || content.is_large {
        return Ok(());
    }
    let Some(content_text) = content.inline_text.as_deref() else {
        return Ok(());
    };

    sqlx::query(
        r#"
        INSERT INTO file_search_documents (
            file_revision_id, tenant_id, project_id, path, path_hash, revision,
            content_hash, content_text
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (file_revision_id) DO UPDATE
        SET path = EXCLUDED.path,
            path_hash = EXCLUDED.path_hash,
            revision = EXCLUDED.revision,
            content_hash = EXCLUDED.content_hash,
            content_text = EXCLUDED.content_text,
            updated_at = CURRENT_TIMESTAMP
        "#,
    )
    .bind(file_revision_id)
    .bind(tenant_id)
    .bind(project_id)
    .bind(path)
    .bind(path_hash)
    .bind(revision)
    .bind(content_hash)
    .bind(content_text)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn populate_revision_content_from_bytes(
    revision: &mut FileRevisionResponse,
    content: &[u8],
) -> Result<(), AppError> {
    if revision.is_binary {
        revision.inline_content = None;
        revision.content_base64 = Some(BASE64_STANDARD.encode(content));
        return Ok(());
    }

    let content = String::from_utf8(content.to_vec())
        .map_err(|_| AppError::InvalidInput("file object is not valid UTF-8 text".to_string()))?;
    revision.inline_content = Some(content);
    revision.content_base64 = None;
    Ok(())
}

fn is_textual_content_type(content_type: &str) -> bool {
    let normalized = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    normalized.starts_with("text/")
        || matches!(
            normalized.as_str(),
            "application/json"
                | "application/xml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
                | "application/javascript"
                | "application/sql"
                | "application/jsonl"
                | "application/x-jsonlines"
                | "application/x-ndjson"
        )
        || normalized.ends_with("+json")
        || normalized.ends_with("+xml")
}

pub async fn list_latest_revisions(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    prefix: &str,
) -> Result<Vec<FileRevisionResponse>, AppError> {
    validate_virtual_path(prefix)?;
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT ON (fr.path_hash)
               fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.tenant_id = $1 AND fr.project_id = $2 AND fr.path LIKE $3
        ORDER BY fr.path_hash, fr.revision DESC
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(format!("{}%", prefix))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(file_revision_from_row)
        .collect::<Result<Vec<_>, AppError>>()
}

pub async fn search_latest_revisions(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
    prefix: &str,
    query: &str,
    limit: i64,
) -> Result<Vec<FileRevisionResponse>, AppError> {
    validate_virtual_path(prefix)?;
    if query.trim().is_empty() {
        return Err(AppError::InvalidInput("query is required".to_string()));
    }
    let limit = limit.clamp(1, 200);
    let rows = sqlx::query(
        r#"
        WITH search_query AS (
            SELECT websearch_to_tsquery('simple', $4) AS ts_query,
                   LOWER($4) AS needle
        ),
        latest AS (
            SELECT DISTINCT ON (fr.path_hash)
                   fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
                   fr.content_hash, fr.object_key, fr.object_reference_id,
                   obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
                   fr.reason, fr.run_id, fr.metadata, fr.created_at
            FROM file_revisions fr
            LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
            WHERE fr.tenant_id = $1
              AND fr.project_id = $2
              AND fr.path LIKE $3
            ORDER BY fr.path_hash, fr.revision DESC
        )
        SELECT latest.id, latest.tenant_id, latest.project_id, latest.path, latest.revision,
               latest.etag, latest.content_hash, latest.object_key, latest.object_reference_id,
               latest.bucket, latest.version_id, latest.inline_content, latest.size_bytes,
               latest.reason, latest.run_id, latest.metadata, latest.created_at
        FROM latest
        JOIN file_search_documents doc ON doc.file_revision_id = latest.id
        CROSS JOIN search_query
        WHERE doc.search_vector @@ search_query.ts_query
           OR LOWER(doc.content_text) LIKE '%' || search_query.needle || '%'
           OR LOWER(doc.path) LIKE '%' || search_query.needle || '%'
        ORDER BY ts_rank_cd(doc.search_vector, search_query.ts_query) DESC,
                 latest.path ASC
        LIMIT $5
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(format!("{}%", prefix))
    .bind(query.trim())
    .bind(limit)
    .fetch_all(&state.connect_pool)
    .await?;

    let mut matches = Vec::new();
    for row in rows {
        let mut revision = file_revision_from_row(row)?;
        hydrate_revision_content(state, &mut revision, false).await?;
        matches.push(revision);
    }

    Ok(matches)
}

pub fn directory_entries(
    files: &[FileRevisionResponse],
    prefix: &str,
) -> Result<Vec<FileEntryResponse>, AppError> {
    validate_virtual_path(prefix)?;
    let normalized_prefix = normalize_directory_prefix(prefix);
    let mut directories = BTreeMap::<String, BTreeSet<String>>::new();
    directories.entry(normalized_prefix.clone()).or_default();
    let mut entries = Vec::new();

    for file in files {
        if !file.path.starts_with(&normalized_prefix) {
            continue;
        }
        add_directory_ancestors(&mut directories, &normalized_prefix, &file.path);
        entries.push(FileEntryResponse {
            path: file.path.clone(),
            entry_type: "file".to_string(),
            depth: path_depth(&file.path),
            children_count: 0,
            latest_revision: Some(file.revision),
            size_bytes: Some(file.size_bytes),
        });
    }

    for file in files {
        if !file.path.starts_with(&normalized_prefix) {
            continue;
        }
        add_immediate_child(&mut directories, &normalized_prefix, &file.path);
    }

    let mut directory_entries = directories
        .into_iter()
        .map(|(path, children)| FileEntryResponse {
            depth: path_depth(&path),
            path,
            entry_type: "directory".to_string(),
            children_count: i32::try_from(children.len()).unwrap_or(i32::MAX),
            latest_revision: None,
            size_bytes: None,
        })
        .collect::<Vec<_>>();
    directory_entries.extend(entries);
    directory_entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.entry_type.cmp(&right.entry_type))
    });
    Ok(directory_entries)
}

pub async fn glob_latest_revisions(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    prefix: &str,
    pattern: &str,
) -> Result<Vec<FileRevisionResponse>, AppError> {
    validate_virtual_path(pattern)?;
    let files = list_latest_revisions(pool, tenant_id, project_id, prefix).await?;
    Ok(files
        .into_iter()
        .filter(|file| glob_matches(pattern, &file.path))
        .collect())
}

pub async fn list_revision_history(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    path: &str,
    limit: i64,
) -> Result<Vec<FileRevisionResponse>, AppError> {
    let path_hash = path_hash(path)?;
    let rows = sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.tenant_id = $1
          AND fr.project_id = $2
          AND fr.path_hash = $3
        ORDER BY fr.revision DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(file_revision_from_row)
        .collect::<Result<Vec<_>, AppError>>()
}

pub async fn list_artifact_revisions(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    run_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<FileRevisionResponse>, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.tenant_id = $1
          AND fr.project_id = $2
          AND fr.run_id IS NOT NULL
          AND ($3::uuid IS NULL OR fr.run_id = $3)
        ORDER BY fr.created_at DESC, fr.revision DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(run_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(file_revision_from_row)
        .collect::<Result<Vec<_>, AppError>>()
}

async fn load_latest_revision_row(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    path_hash: &str,
) -> Result<Option<PgRow>, AppError> {
    sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.tenant_id = $1 AND fr.project_id = $2 AND fr.path_hash = $3
        ORDER BY fr.revision DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)
}

async fn load_revision_row_by_number(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    path_hash: &str,
    revision: i64,
) -> Result<Option<PgRow>, AppError> {
    sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.tenant_id = $1
          AND fr.project_id = $2
          AND fr.path_hash = $3
          AND fr.revision = $4
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .bind(revision)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)
}

async fn load_revision_row_by_version_id(
    pool: &PgPool,
    tenant_id: Uuid,
    project_id: Uuid,
    path_hash: &str,
    version_id: &str,
) -> Result<Option<PgRow>, AppError> {
    sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.tenant_id = $1
          AND fr.project_id = $2
          AND fr.path_hash = $3
          AND obj.version_id = $4
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .bind(project_id)
    .bind(path_hash)
    .bind(version_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)
}

async fn load_revision_row_by_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    revision_id: Uuid,
) -> Result<PgRow, AppError> {
    sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.id = $1
        "#,
    )
    .bind(revision_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)
}

async fn hydrate_revision_content(
    state: &AppState,
    revision: &mut FileRevisionResponse,
    allow_binary: bool,
) -> Result<(), AppError> {
    if revision.inline_content.is_some() || revision.content_base64.is_some() {
        return Ok(());
    }
    if revision.is_binary && !allow_binary {
        return Ok(());
    }
    let Some(content) = state
        .rustfs_client
        .get_file_object_version(&revision.object_key, revision.version_id.as_deref())
        .await?
    else {
        return Ok(());
    };
    let actual_hash = sha256_hex(&content);
    if actual_hash != revision.content_hash {
        return Err(AppError::ObjectStore(format!(
            "file object content hash mismatch for revision {}",
            revision.id
        )));
    }
    populate_revision_content_from_bytes(revision, &content)?;
    Ok(())
}

async fn cleanup_orphan_file_object(state: &AppState, object_key: Option<&str>) {
    let Some(object_key) = object_key else {
        return;
    };
    if let Err(error) = state.rustfs_client.delete_file_object(object_key).await {
        warn!(
            object_key,
            error = ?error,
            "failed to delete orphan file object after file revision persistence failure"
        );
    }
}

async fn lock_file_revision_tx(
    tx: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    path_hash: &str,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{project_id}:{path_hash}"))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn load_tenant_slug_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<String, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT slug
        FROM tenants
        WHERE id = $1
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::NotFound("tenant not found".to_string()))
}

fn object_key_for_revision(
    tenant_slug: &str,
    project_id: Uuid,
    path_hash: &str,
    revision: i64,
    content_hash: &str,
    extension: &str,
) -> String {
    format!(
        "tenants/{tenant_slug}/projects/{project_id}/workspace-revisions/{path_hash}/{revision}-{content_hash}.{extension}"
    )
}

async fn load_run_context_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> Result<RunEventContext, AppError> {
    let row = sqlx::query(
        r#"
        SELECT conversation_id, trace_id
        FROM runs
        WHERE id = $1
        "#,
    )
    .bind(run_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::NotFound("run not found".to_string()))?;

    Ok(RunEventContext {
        conversation_id: row.try_get("conversation_id")?,
        trace_id: row.try_get("trace_id")?,
    })
}

async fn insert_file_write_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    payload: &FileWriteRequest,
    revision: &FileRevisionResponse,
    path_hash: &str,
    run_context: Option<&RunEventContext>,
) -> Result<(), AppError> {
    let resource_id = format!("{}:{path_hash}", payload.project_id);
    let object_reference_id = revision
        .object_reference_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "inline".to_string());
    let output_summary = format!(
        "revision_id={}; revision={}; size_bytes={}; content_hash={}; content_type={}; object_reference_id={}",
        revision.id,
        revision.revision,
        revision.size_bytes,
        revision.content_hash,
        revision.content_type,
        object_reference_id
    );
    audit::insert_audit_log_tx(
        tx,
        NewAuditLog {
            tenant_id: payload.tenant_id,
            actor_user_id: Some(payload.actor_user_id),
            actor_device_id: payload.actor_device_id,
            session_id: payload.actor_session_id,
            resource_type: "file",
            resource_id: &resource_id,
            action: "write_object",
            decision: "allow",
            policy_version: "local-policy-v1",
            reason_code: Some(&payload.reason),
            run_id: payload.run_id,
            conversation_id: run_context.map(|run| run.conversation_id),
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: Some(&revision.content_hash),
            input_summary: Some(&payload.path),
            output_summary: Some(&output_summary),
            risk_level: Some(if revision.is_binary || revision.is_large {
                "high"
            } else {
                "medium"
            }),
            ip: None,
            user_agent: None,
            trace_id: run_context.map(|run| run.trace_id.as_str()),
        },
    )
    .await?;
    Ok(())
}

fn file_revision_from_row(row: PgRow) -> Result<FileRevisionResponse, AppError> {
    let metadata: serde_json::Value = row.try_get("metadata")?;
    let content_type = metadata
        .get("content_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(FILE_CONTENT_TYPE)
        .to_string();
    let is_binary = metadata
        .get("is_binary")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| !is_textual_content_type(&content_type));
    let size_bytes: i64 = row.try_get("size_bytes")?;
    let is_large = metadata
        .get("is_large")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(size_bytes > LARGE_FILE_THRESHOLD_BYTES);
    let stored_inline_content: Option<String> = row.try_get("inline_content")?;
    let (inline_content, content_base64) = if is_binary {
        (None, stored_inline_content)
    } else {
        (stored_inline_content, None)
    };

    Ok(FileRevisionResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        project_id: row.try_get("project_id")?,
        path: row.try_get("path")?,
        revision: row.try_get("revision")?,
        etag: row.try_get("etag")?,
        content_hash: row.try_get("content_hash")?,
        object_key: row.try_get("object_key")?,
        object_reference_id: row.try_get("object_reference_id")?,
        bucket: row.try_get("bucket")?,
        version_id: row.try_get("version_id")?,
        inline_content,
        content_base64,
        size_bytes,
        content_type,
        is_binary,
        is_large,
        reason: row.try_get("reason")?,
        run_id: row.try_get("run_id")?,
        metadata,
        created_at: row.try_get("created_at")?,
    })
}

fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_idx, mut value_idx) = (0_usize, 0_usize);
    let mut star_idx = None;
    let mut star_match_idx = 0_usize;

    while value_idx < value.len() {
        if pattern_idx < pattern.len()
            && (pattern[pattern_idx] == b'?' || pattern[pattern_idx] == value[value_idx])
        {
            pattern_idx += 1;
            value_idx += 1;
        } else if pattern_idx < pattern.len() && pattern[pattern_idx] == b'*' {
            star_idx = Some(pattern_idx);
            star_match_idx = value_idx;
            pattern_idx += 1;
        } else if let Some(star) = star_idx {
            pattern_idx = star + 1;
            star_match_idx += 1;
            value_idx = star_match_idx;
        } else {
            return false;
        }
    }

    while pattern_idx < pattern.len() && pattern[pattern_idx] == b'*' {
        pattern_idx += 1;
    }

    pattern_idx == pattern.len()
}

fn normalize_directory_prefix(prefix: &str) -> String {
    if prefix.is_empty() {
        return "/".to_string();
    }
    if prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    }
}

fn add_directory_ancestors(
    directories: &mut BTreeMap<String, BTreeSet<String>>,
    root_prefix: &str,
    file_path: &str,
) {
    let mut current = root_prefix.to_string();
    directories.entry(current.clone()).or_default();
    let relative = file_path.strip_prefix(root_prefix).unwrap_or(file_path);
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        let next = format!("{current}{part}/");
        directories
            .entry(current.clone())
            .or_default()
            .insert(next.clone());
        directories.entry(next.clone()).or_default();
        current = next;
    }
}

fn add_immediate_child(
    directories: &mut BTreeMap<String, BTreeSet<String>>,
    root_prefix: &str,
    file_path: &str,
) {
    let mut current = root_prefix.to_string();
    let relative = file_path.strip_prefix(root_prefix).unwrap_or(file_path);
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            directories
                .entry(current)
                .or_default()
                .insert(file_path.to_string());
            return;
        }
        let next = format!("{current}{part}/");
        directories
            .entry(current.clone())
            .or_default()
            .insert(next.clone());
        current = next;
    }
}

fn path_depth(path: &str) -> i32 {
    path.trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .count()
        .try_into()
        .unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::Client as RedisClient;
    use secrecy::SecretBox;
    use sqlx::{PgPool, postgres::PgPoolOptions};

    use crate::{
        configuration::{
            AgentRuntimeSettings, FerrisKeySettings, MemoryVectorSettings, ObjectStoreSettings,
        },
        features::agent_platform::{
            authz::ResourceAuthzService, ferriskey_oidc::FerrisKeyOidcVerifier,
            memory_vector::MemoryVectorClient, runtime::AgentRuntimeClient, rustfs::RustFsClient,
        },
        startup::AppState,
    };

    #[test]
    fn file_revision_selector_defaults_to_latest() {
        assert_eq!(
            file_revision_selector(&file_read_request(None, None)).expect("selector"),
            FileRevisionSelector::Latest
        );
    }

    #[test]
    fn file_revision_selector_accepts_exact_revision() {
        assert_eq!(
            file_revision_selector(&file_read_request(Some(3), None)).expect("selector"),
            FileRevisionSelector::Revision(3)
        );
    }

    #[test]
    fn file_revision_selector_accepts_version_id() {
        assert_eq!(
            file_revision_selector(&file_read_request(None, Some(" version-1 ")))
                .expect("selector"),
            FileRevisionSelector::VersionId("version-1".to_string())
        );
    }

    #[test]
    fn file_revision_selector_rejects_ambiguous_or_invalid_values() {
        assert!(file_revision_selector(&file_read_request(Some(0), None)).is_err());
        assert!(file_revision_selector(&file_read_request(None, Some(" "))).is_err());
        assert!(file_revision_selector(&file_read_request(Some(1), Some("v1"))).is_err());
    }

    #[test]
    fn glob_matches_supports_star_and_question_mark() {
        assert!(glob_matches("/workspace/*.txt", "/workspace/report.txt"));
        assert!(glob_matches("/workspace/file-?.md", "/workspace/file-a.md"));
        assert!(glob_matches(
            "/workspace/*/report.txt",
            "/workspace/q1/report.txt"
        ));
        assert!(!glob_matches(
            "/workspace/file-?.md",
            "/workspace/file-aa.md"
        ));
        assert!(!glob_matches("/workspace/*.txt", "/scratch/report.txt"));
    }

    #[test]
    fn validate_virtual_path_rejects_unsafe_patterns() {
        assert!(validate_virtual_path("/workspace/*.txt").is_ok());
        assert!(validate_virtual_path("/workspace/../*.txt").is_err());
        assert!(validate_virtual_path("//workspace/*.txt").is_err());
    }

    #[test]
    fn object_key_for_revision_is_project_scoped_and_immutable() {
        let project_id = Uuid::new_v4();
        let key = object_key_for_revision(
            "bibi-work",
            project_id,
            "path-hash",
            3,
            "content-hash",
            "txt",
        );

        assert_eq!(
            key,
            format!(
                "tenants/bibi-work/projects/{project_id}/workspace-revisions/path-hash/3-content-hash.txt"
            )
        );
    }

    #[test]
    fn directory_entries_include_virtual_directories_and_files() {
        let tenant_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let files = vec![
            file_revision_for_path(tenant_id, project_id, "/workspace/docs/a.txt", 2, 12),
            file_revision_for_path(tenant_id, project_id, "/workspace/docs/nested/b.txt", 1, 34),
        ];

        let entries = directory_entries(&files, "/workspace/").expect("directory entries");
        let paths = entries
            .iter()
            .map(|entry| {
                (
                    entry.path.as_str(),
                    entry.entry_type.as_str(),
                    entry.children_count,
                )
            })
            .collect::<Vec<_>>();

        assert!(paths.contains(&("/workspace/", "directory", 1)));
        assert!(paths.contains(&("/workspace/docs/", "directory", 2)));
        assert!(paths.contains(&("/workspace/docs/nested/", "directory", 1)));
        assert!(paths.contains(&("/workspace/docs/a.txt", "file", 0)));
        assert!(paths.contains(&("/workspace/docs/nested/b.txt", "file", 0)));
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn writes_and_reads_historical_inline_revisions() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = test_state().await?;
        let (tenant_id, user_id, project_id) = seed_file_context(&state.connect_pool).await?;
        let path = "/workspace/report.txt".to_string();

        let first = write_revision(
            &state,
            FileWriteRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                content_ref: None,
                inline_content: Some("first revision".to_string()),
                content_base64: None,
                content_type: None,
                expected_revision: 0,
                reason: "initial write".to_string(),
                run_id: None,
                lock_token: None,
            },
        )
        .await?;
        let second = write_revision(
            &state,
            FileWriteRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                content_ref: None,
                inline_content: Some("second revision".to_string()),
                content_base64: None,
                content_type: None,
                expected_revision: 1,
                reason: "update".to_string(),
                run_id: None,
                lock_token: None,
            },
        )
        .await?;

        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);

        let latest = read_revision(
            &state,
            FileReadRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                revision: None,
                version_id: None,
                run_id: None,
                include_content: None,
                allow_binary: None,
            },
        )
        .await?;
        assert_eq!(latest.revision, 2);
        assert_eq!(latest.inline_content.as_deref(), Some("second revision"));

        let historical = read_revision(
            &state,
            FileReadRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                revision: Some(1),
                version_id: None,
                run_id: None,
                include_content: None,
                allow_binary: None,
            },
        )
        .await?;
        assert_eq!(historical.revision, 1);
        assert_eq!(historical.inline_content.as_deref(), Some("first revision"));

        let matches =
            search_latest_revisions(&state, tenant_id, project_id, "/workspace/", "second", 10)
                .await?;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, path);
        assert_eq!(matches[0].revision, 2);
        assert_eq!(
            matches[0].inline_content.as_deref(),
            Some("second revision")
        );

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, Redis, RustFS, and the bibi_work schema"]
    async fn writes_and_reads_rustfs_historical_revision() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = test_state_with_rustfs().await?;
        let (tenant_id, user_id, project_id) = seed_file_context(&state.connect_pool).await?;
        let path = "/workspace/rustfs-report.txt".to_string();

        let first = write_revision(
            &state,
            FileWriteRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                content_ref: None,
                inline_content: Some("rustfs first revision".to_string()),
                content_base64: None,
                content_type: None,
                expected_revision: 0,
                reason: "initial rustfs write".to_string(),
                run_id: None,
                lock_token: None,
            },
        )
        .await?;

        let second = write_revision(
            &state,
            FileWriteRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                content_ref: None,
                inline_content: Some("rustfs second revision".to_string()),
                content_base64: None,
                content_type: None,
                expected_revision: 1,
                reason: "rustfs update".to_string(),
                run_id: None,
                lock_token: None,
            },
        )
        .await?;

        assert!(first.object_reference_id.is_some());
        assert!(second.object_reference_id.is_some());

        let latest = read_revision(
            &state,
            FileReadRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                revision: None,
                version_id: None,
                run_id: None,
                include_content: None,
                allow_binary: None,
            },
        )
        .await?;
        assert_eq!(latest.revision, 2);
        assert_eq!(
            latest.inline_content.as_deref(),
            Some("rustfs second revision")
        );

        let historical = read_revision(
            &state,
            FileReadRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                revision: Some(1),
                version_id: None,
                run_id: None,
                include_content: None,
                allow_binary: None,
            },
        )
        .await?;
        assert_eq!(historical.revision, 1);
        assert_eq!(
            historical.inline_content.as_deref(),
            Some("rustfs first revision")
        );

        if let Some(first_version_id) = first.version_id.clone() {
            let by_version = read_revision(
                &state,
                FileReadRequest {
                    tenant_id,
                    actor_user_id: user_id,
                    actor_device_id: None,
                    actor_session_id: None,
                    project_id,
                    path,
                    revision: None,
                    version_id: Some(first_version_id),
                    run_id: None,
                    include_content: None,
                    allow_binary: None,
                },
            )
            .await?;
            assert_eq!(by_version.revision, 1);
            assert_eq!(
                by_version.inline_content.as_deref(),
                Some("rustfs first revision")
            );
        }

        state
            .rustfs_client
            .delete_file_object(&first.object_key)
            .await?;
        state
            .rustfs_client
            .delete_file_object(&second.object_key)
            .await?;
        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    fn file_read_request(revision: Option<i64>, version_id: Option<&str>) -> FileReadRequest {
        FileReadRequest {
            tenant_id: Uuid::new_v4(),
            actor_user_id: Uuid::new_v4(),
            actor_device_id: None,
            actor_session_id: None,
            project_id: Uuid::new_v4(),
            path: "/workspace/report.txt".to_string(),
            revision,
            version_id: version_id.map(str::to_string),
            run_id: None,
            include_content: None,
            allow_binary: None,
        }
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

    async fn seed_file_context(
        pool: &PgPool,
    ) -> Result<(Uuid, Uuid, Uuid), Box<dyn std::error::Error>> {
        let suffix = Uuid::new_v4();
        let tenant_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO tenants (name, slug, metadata)
            VALUES ($1, $2, '{}'::jsonb)
            RETURNING id
            "#,
        )
        .bind(format!("File revision test {suffix}"))
        .bind(format!("file-revision-test-{suffix}"))
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
        .bind(format!("file-revision-subject-{suffix}"))
        .bind(format!("file-revision-user-{suffix}"))
        .fetch_one(pool)
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
        .bind(format!("File revision project {suffix}"))
        .fetch_one(pool)
        .await?;

        Ok((tenant_id, user_id, project_id))
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

    fn file_revision_for_path(
        tenant_id: Uuid,
        project_id: Uuid,
        path: &str,
        revision: i64,
        size_bytes: i64,
    ) -> FileRevisionResponse {
        FileRevisionResponse {
            id: Uuid::new_v4(),
            tenant_id,
            project_id,
            path: path.to_string(),
            revision,
            etag: format!("etag-{revision}"),
            content_hash: format!("hash-{revision}"),
            object_key: format!("object-{revision}"),
            object_reference_id: None,
            bucket: None,
            version_id: None,
            inline_content: Some("content".to_string()),
            content_base64: None,
            size_bytes,
            content_type: FILE_CONTENT_TYPE.to_string(),
            is_binary: false,
            is_large: false,
            reason: "test".to_string(),
            run_id: None,
            metadata: json!({}),
            created_at: time::OffsetDateTime::now_utc(),
        }
    }
}
