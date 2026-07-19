use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};
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
const MAX_FILE_RANGE_BYTES: usize = 256 * 1024;
const FILE_SEARCH_CHUNK_BYTES: usize = 64 * 1024;
const FILE_SEARCH_MAX_INDEXED_BYTES: usize = 1024 * 1024;
const FILE_SEARCH_BACKFILL_BATCH_SIZE: i64 = 16;
const FILE_SEARCH_BACKFILL_INTERVAL: Duration = Duration::from_secs(60);

struct ResolvedFileContent {
    bytes: Vec<u8>,
    inline_text: Option<String>,
    content_type: String,
    is_binary: bool,
    is_large: bool,
    object_extension: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSearchChunk {
    chunk_index: i32,
    byte_start: i64,
    byte_end: i64,
    content_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileSearchExtraction {
    chunks: Vec<FileSearchChunk>,
    source_size_bytes: i64,
    indexed_bytes: i64,
    is_truncated: bool,
    strategy: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileSearchBackfillSummary {
    pub candidates: usize,
    pub indexed: usize,
    pub skipped_binary: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FileRevisionSelector {
    Latest,
    Revision(i64),
    VersionId(String),
}

pub fn spawn_file_search_backfill_worker(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(FILE_SEARCH_BACKFILL_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match backfill_file_search_chunks(&state, FILE_SEARCH_BACKFILL_BATCH_SIZE).await {
                Ok(summary) if summary.candidates > 0 => debug!(
                    candidates = summary.candidates,
                    indexed = summary.indexed,
                    skipped_binary = summary.skipped_binary,
                    failed = summary.failed,
                    "file search index backfill batch completed"
                ),
                Ok(_) => {}
                Err(err) => warn!("file search index backfill failed: {}", err),
            }
        }
    });
}

pub async fn backfill_file_search_chunks(
    state: &AppState,
    limit: i64,
) -> Result<FileSearchBackfillSummary, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT fr.id, fr.tenant_id, fr.project_id, fr.path, fr.revision, fr.etag,
               fr.content_hash, fr.object_key, fr.object_reference_id,
               obj.bucket, obj.version_id, fr.inline_content, fr.size_bytes,
               fr.reason, fr.run_id, fr.metadata, fr.created_at
        FROM file_revisions fr
        LEFT JOIN object_references obj ON obj.id = fr.object_reference_id
        WHERE fr.metadata #>> '{search_index,status}' IS NULL
           OR (
                fr.metadata #>> '{search_index,status}' = 'indexed'
                AND NOT EXISTS (
                    SELECT 1 FROM file_search_chunks chunk
                    WHERE chunk.file_revision_id = fr.id
                )
           )
        ORDER BY fr.created_at, fr.id
        LIMIT $1
        "#,
    )
    .bind(limit.clamp(1, 256))
    .fetch_all(&state.connect_pool)
    .await?;

    let mut summary = FileSearchBackfillSummary {
        candidates: rows.len(),
        ..FileSearchBackfillSummary::default()
    };
    for row in rows {
        let revision = file_revision_from_row(row)?;
        match backfill_file_search_revision(state, revision).await {
            Ok(true) => summary.indexed += 1,
            Ok(false) => summary.skipped_binary += 1,
            Err(err) => {
                summary.failed += 1;
                warn!("failed to backfill file search revision: {}", err);
            }
        }
    }
    Ok(summary)
}

async fn backfill_file_search_revision(
    state: &AppState,
    mut revision: FileRevisionResponse,
) -> Result<bool, AppError> {
    let mut tx = state.connect_pool.begin().await?;
    sqlx::query("SELECT id FROM file_revisions WHERE id = $1 FOR UPDATE")
        .bind(revision.id)
        .fetch_one(&mut *tx)
        .await?;

    if revision.is_binary {
        sqlx::query("DELETE FROM file_search_chunks WHERE file_revision_id = $1")
            .bind(revision.id)
            .execute(&mut *tx)
            .await?;
        let search_index = json!({
            "status": "skipped",
            "reason": "binary_content",
            "source_size_bytes": revision.size_bytes,
            "indexed_bytes": 0,
            "is_truncated": false,
            "chunk_count": 0
        });
        update_file_search_metadata_tx(&mut tx, revision.id, search_index).await?;
        tx.commit().await?;
        return Ok(false);
    }
    drop(tx);

    hydrate_revision_content(state, &mut revision, false, None, None).await?;
    let content_text = revision.inline_content.clone().ok_or_else(|| {
        AppError::ObjectStore(format!(
            "text content is unavailable for file revision {}",
            revision.id
        ))
    })?;
    let content = ResolvedFileContent {
        bytes: content_text.as_bytes().to_vec(),
        inline_text: Some(content_text),
        content_type: revision.content_type.clone(),
        is_binary: false,
        is_large: revision.is_large,
        object_extension: "txt",
    };

    let mut tx = state.connect_pool.begin().await?;
    sqlx::query("SELECT id FROM file_revisions WHERE id = $1 FOR UPDATE")
        .bind(revision.id)
        .fetch_one(&mut *tx)
        .await?;
    let path_hash = sha256_hex(revision.path.as_bytes());
    let search_index = replace_file_search_chunks_tx(
        &mut tx,
        FileSearchChunkSource {
            tenant_id: revision.tenant_id,
            project_id: revision.project_id,
            path: &revision.path,
            path_hash: &path_hash,
            revision: revision.revision,
            content_hash: &revision.content_hash,
            file_revision_id: revision.id,
            content: &content,
        },
    )
    .await?;
    update_file_search_metadata_tx(&mut tx, revision.id, search_index).await?;
    tx.commit().await?;
    Ok(true)
}

async fn update_file_search_metadata_tx(
    tx: &mut Transaction<'_, Postgres>,
    revision_id: Uuid,
    search_index: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE file_revisions
        SET metadata = jsonb_set(metadata, '{search_index}', $2, TRUE)
        WHERE id = $1
        "#,
    )
    .bind(revision_id)
    .bind(search_index)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
        hydrate_revision_content(
            state,
            &mut revision,
            payload.allow_binary.unwrap_or(false),
            payload.offset_bytes,
            payload.limit_bytes,
        )
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
            "large_threshold_bytes": LARGE_FILE_THRESHOLD_BYTES,
            "tool_context": {
                "tool_call_id": payload.tool_call_id.as_deref(),
                "tool_name": payload.tool_name.as_deref(),
                "args_hash": payload.args_hash.as_deref(),
                "parent_tool_call_id": payload.parent_tool_call_id.as_deref(),
                "operation": payload.operation.as_deref()
            }
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

        let search_index = replace_file_search_chunks_tx(
            &mut tx,
            FileSearchChunkSource {
                tenant_id: payload.tenant_id,
                project_id: payload.project_id,
                path: &payload.path,
                path_hash: &path_hash,
                revision: next_revision,
                content_hash: &content_hash,
                file_revision_id: revision_id,
                content: &resolved_content,
            },
        )
        .await?;
        sqlx::query(
            r#"
            UPDATE file_revisions
            SET metadata = jsonb_set(metadata, '{search_index}', $2, TRUE)
            WHERE id = $1
            "#,
        )
        .bind(revision_id)
        .bind(search_index)
        .execute(&mut *tx)
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

struct FileSearchChunkSource<'a> {
    tenant_id: Uuid,
    project_id: Uuid,
    path: &'a str,
    path_hash: &'a str,
    revision: i64,
    content_hash: &'a str,
    file_revision_id: Uuid,
    content: &'a ResolvedFileContent,
}

async fn replace_file_search_chunks_tx(
    tx: &mut Transaction<'_, Postgres>,
    source: FileSearchChunkSource<'_>,
) -> Result<serde_json::Value, AppError> {
    let FileSearchChunkSource {
        tenant_id,
        project_id,
        path,
        path_hash,
        revision,
        content_hash,
        file_revision_id,
        content,
    } = source;
    sqlx::query("DELETE FROM file_search_chunks WHERE file_revision_id = $1")
        .bind(file_revision_id)
        .execute(&mut **tx)
        .await?;
    if content.is_binary {
        return Ok(json!({
            "status": "skipped",
            "reason": "binary_content",
            "source_size_bytes": content.bytes.len(),
            "indexed_bytes": 0,
            "is_truncated": false,
            "chunk_count": 0
        }));
    }
    let Some(content_text) = content.inline_text.as_deref() else {
        return Ok(json!({
            "status": "skipped",
            "reason": "text_content_unavailable",
            "source_size_bytes": content.bytes.len(),
            "indexed_bytes": 0,
            "is_truncated": false,
            "chunk_count": 0
        }));
    };
    let extraction = extract_file_search_chunks(content_text)?;
    for chunk in &extraction.chunks {
        sqlx::query(
            r#"
            INSERT INTO file_search_chunks (
                file_revision_id, tenant_id, project_id, path, path_hash, revision,
                content_hash, chunk_index, byte_start, byte_end, source_size_bytes,
                indexed_bytes, is_truncated, extraction_strategy, content_text
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15
            )
            "#,
        )
        .bind(file_revision_id)
        .bind(tenant_id)
        .bind(project_id)
        .bind(path)
        .bind(path_hash)
        .bind(revision)
        .bind(content_hash)
        .bind(chunk.chunk_index)
        .bind(chunk.byte_start)
        .bind(chunk.byte_end)
        .bind(extraction.source_size_bytes)
        .bind(extraction.indexed_bytes)
        .bind(extraction.is_truncated)
        .bind(extraction.strategy)
        .bind(&chunk.content_text)
        .execute(&mut **tx)
        .await?;
    }
    Ok(json!({
        "status": "indexed",
        "strategy": extraction.strategy,
        "source_size_bytes": extraction.source_size_bytes,
        "indexed_bytes": extraction.indexed_bytes,
        "is_truncated": extraction.is_truncated,
        "chunk_count": extraction.chunks.len(),
        "chunk_bytes": FILE_SEARCH_CHUNK_BYTES,
        "max_indexed_bytes": FILE_SEARCH_MAX_INDEXED_BYTES
    }))
}

fn extract_file_search_chunks(content: &str) -> Result<FileSearchExtraction, AppError> {
    let source_size = content.len();
    if source_size == 0 {
        return Ok(FileSearchExtraction {
            chunks: vec![FileSearchChunk {
                chunk_index: 0,
                byte_start: 0,
                byte_end: 0,
                content_text: String::new(),
            }],
            source_size_bytes: 0,
            indexed_bytes: 0,
            is_truncated: false,
            strategy: "full_chunks",
        });
    }
    let max_chunks = (FILE_SEARCH_MAX_INDEXED_BYTES / FILE_SEARCH_CHUNK_BYTES).max(1);
    let is_truncated = source_size > FILE_SEARCH_MAX_INDEXED_BYTES;
    let starts = if is_truncated {
        let sample_chunks = if max_chunks > 2 && max_chunks.is_multiple_of(2) {
            max_chunks - 1
        } else {
            max_chunks
        };
        let last_start = source_size.saturating_sub(FILE_SEARCH_CHUNK_BYTES);
        (0..sample_chunks)
            .map(|index| {
                if sample_chunks == 1 {
                    0
                } else {
                    index * last_start / (sample_chunks - 1)
                }
            })
            .collect::<Vec<_>>()
    } else {
        (0..source_size)
            .step_by(FILE_SEARCH_CHUNK_BYTES)
            .collect::<Vec<_>>()
    };

    let mut chunks = Vec::with_capacity(starts.len());
    for raw_start in starts {
        let start = next_char_boundary(content, raw_start);
        let raw_end = start
            .saturating_add(FILE_SEARCH_CHUNK_BYTES)
            .min(source_size);
        let end = previous_char_boundary(content, raw_end);
        if end <= start {
            continue;
        }
        chunks.push(FileSearchChunk {
            chunk_index: i32::try_from(chunks.len())?,
            byte_start: i64::try_from(start)?,
            byte_end: i64::try_from(end)?,
            content_text: content[start..end].to_string(),
        });
    }
    let indexed_bytes = chunks.iter().try_fold(0_i64, |total, chunk| {
        total
            .checked_add(i64::try_from(chunk.content_text.len())?)
            .ok_or_else(|| AppError::InvalidInput("indexed byte count is too large".to_string()))
    })?;
    Ok(FileSearchExtraction {
        chunks,
        source_size_bytes: i64::try_from(source_size)?,
        indexed_bytes,
        is_truncated,
        strategy: if is_truncated {
            "uniform_sample"
        } else {
            "full_chunks"
        },
    })
}

fn next_char_boundary(content: &str, mut offset: usize) -> usize {
    offset = offset.min(content.len());
    while offset < content.len() && !content.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn previous_char_boundary(content: &str, mut offset: usize) -> usize {
    offset = offset.min(content.len());
    while offset > 0 && !content.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
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

fn apply_revision_content_range(
    revision: &mut FileRevisionResponse,
    offset_bytes: Option<i64>,
    limit_bytes: Option<i64>,
) -> Result<(), AppError> {
    if offset_bytes.is_none() && limit_bytes.is_none() {
        return Ok(());
    }
    if revision.is_binary {
        return Err(AppError::InvalidInput(
            "range reads are only supported for text files".to_string(),
        ));
    }
    let Some(content) = revision.inline_content.as_deref() else {
        return Ok(());
    };
    let (slice, offset, limit, truncated) =
        text_byte_range(content, offset_bytes.unwrap_or(0), limit_bytes)?;
    revision.inline_content = Some(slice);
    revision.content_offset_bytes = Some(offset);
    revision.content_limit_bytes = Some(limit);
    revision.content_truncated = Some(truncated);
    Ok(())
}

fn normalized_file_range(
    size_bytes: i64,
    offset_bytes: Option<i64>,
    limit_bytes: Option<i64>,
) -> Result<Option<(i64, usize)>, AppError> {
    let offset = offset_bytes.unwrap_or(0);
    if offset < 0 {
        return Err(AppError::InvalidInput(
            "offset_bytes must be non-negative".to_string(),
        ));
    }
    if matches!(limit_bytes, Some(limit) if limit < 0) {
        return Err(AppError::InvalidInput(
            "limit_bytes must be non-negative".to_string(),
        ));
    }
    let size = size_bytes.max(0);
    if offset >= size {
        return Ok(None);
    }
    let requested_limit = limit_bytes
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(MAX_FILE_RANGE_BYTES)
        .min(MAX_FILE_RANGE_BYTES);
    Ok(Some((offset, requested_limit)))
}

fn text_byte_range(
    value: &str,
    offset_bytes: i64,
    limit_bytes: Option<i64>,
) -> Result<(String, i64, i64, bool), AppError> {
    if offset_bytes < 0 {
        return Err(AppError::InvalidInput(
            "offset_bytes must be non-negative".to_string(),
        ));
    }
    if matches!(limit_bytes, Some(limit) if limit < 0) {
        return Err(AppError::InvalidInput(
            "limit_bytes must be non-negative".to_string(),
        ));
    }
    let value_bytes = value.as_bytes();
    let mut start = usize::try_from(offset_bytes)
        .map_err(|_| AppError::InvalidInput("offset_bytes is invalid".to_string()))?
        .min(value_bytes.len());
    let requested_limit = limit_bytes
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(MAX_FILE_RANGE_BYTES)
        .min(MAX_FILE_RANGE_BYTES);
    let mut end = start.saturating_add(requested_limit).min(value_bytes.len());
    while start < value_bytes.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    while end > start && !value.is_char_boundary(end) {
        end -= 1;
    }
    let slice = value[start..end].to_string();
    let limit = i64::try_from(slice.len())
        .map_err(|_| AppError::InvalidInput("range length is invalid".to_string()))?;
    Ok((
        slice,
        start as i64,
        limit,
        start > 0 || end < value_bytes.len(),
    ))
}

fn text_byte_range_from_fetched_bytes(
    bytes: &[u8],
    fetch_start: i64,
    requested_offset: i64,
    requested_limit: usize,
    total_size_bytes: i64,
) -> Result<(String, i64, i64, bool), AppError> {
    if bytes.is_empty() || requested_limit == 0 {
        let actual_offset = requested_offset.min(total_size_bytes.max(0));
        return Ok((String::new(), actual_offset, 0, actual_offset > 0));
    }
    let preferred_start =
        usize::try_from(requested_offset.saturating_sub(fetch_start))?.min(bytes.len());
    let mut start = preferred_start;
    while start < bytes.len() && is_utf8_continuation_byte(bytes[start]) {
        start += 1;
    }
    let mut end = preferred_start
        .saturating_add(requested_limit)
        .min(bytes.len());
    if end < start {
        end = start;
    }
    while end < bytes.len() && end > start && is_utf8_continuation_byte(bytes[end]) {
        end -= 1;
    }
    while end > start && std::str::from_utf8(&bytes[start..end]).is_err() {
        end -= 1;
    }
    let text = std::str::from_utf8(&bytes[start..end])
        .map_err(|_| AppError::InvalidInput("file range is not valid UTF-8 text".to_string()))?
        .to_string();
    let actual_offset = fetch_start + i64::try_from(start)?;
    let actual_limit = i64::try_from(text.len())?;
    let truncated = actual_offset > 0 || actual_offset + actual_limit < total_size_bytes;
    Ok((text, actual_offset, actual_limit, truncated))
}

fn is_utf8_continuation_byte(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
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
        ),
        candidate_matches AS (
            SELECT chunk.file_revision_id,
                   chunk.content_text AS search_snippet,
                   chunk.byte_start AS search_byte_start,
                   chunk.byte_end AS search_byte_end,
                   GREATEST(
                       ts_rank_cd(chunk.search_vector, search_query.ts_query),
                       CASE
                           WHEN LOWER(chunk.content_text) LIKE '%' || search_query.needle || '%'
                           THEN 0.1
                           ELSE 0.0
                       END,
                       CASE
                           WHEN LOWER(chunk.path) LIKE '%' || search_query.needle || '%'
                           THEN 0.2
                           ELSE 0.0
                       END
                   ) AS relevance
            FROM file_search_chunks chunk
            CROSS JOIN search_query
            WHERE chunk.search_vector @@ search_query.ts_query
               OR LOWER(chunk.content_text) LIKE '%' || search_query.needle || '%'
               OR LOWER(chunk.path) LIKE '%' || search_query.needle || '%'
            UNION ALL
            SELECT latest.id AS file_revision_id,
                   NULL::TEXT AS search_snippet,
                   NULL::BIGINT AS search_byte_start,
                   NULL::BIGINT AS search_byte_end,
                   0.2::REAL AS relevance
            FROM latest
            CROSS JOIN search_query
            WHERE LOWER(latest.path) LIKE '%' || search_query.needle || '%'
        ),
        matched AS (
            SELECT DISTINCT ON (file_revision_id)
                   file_revision_id, search_snippet, search_byte_start, search_byte_end,
                   relevance
            FROM candidate_matches
            ORDER BY file_revision_id, relevance DESC, search_byte_start ASC NULLS LAST
        )
        SELECT latest.id, latest.tenant_id, latest.project_id, latest.path, latest.revision,
               latest.etag, latest.content_hash, latest.object_key, latest.object_reference_id,
               latest.bucket, latest.version_id, latest.inline_content, latest.size_bytes,
               latest.reason, latest.run_id, latest.metadata, latest.created_at,
               matched.search_snippet, matched.search_byte_start, matched.search_byte_end
        FROM latest
        JOIN matched ON matched.file_revision_id = latest.id
        ORDER BY matched.relevance DESC, latest.path ASC
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
        let search_snippet: Option<String> = row.try_get("search_snippet")?;
        let search_byte_start: Option<i64> = row.try_get("search_byte_start")?;
        let search_byte_end: Option<i64> = row.try_get("search_byte_end")?;
        let mut revision = file_revision_from_row(row)?;
        if revision.is_large && !revision.is_binary {
            revision.inline_content = search_snippet;
            revision.content_base64 = None;
            revision.content_offset_bytes = search_byte_start;
            revision.content_limit_bytes = match (search_byte_start, search_byte_end) {
                (Some(start), Some(end)) => Some(end.saturating_sub(start)),
                _ => None,
            };
            revision.content_truncated = Some(true);
        } else {
            hydrate_revision_content(state, &mut revision, false, None, None).await?;
        }
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
    offset_bytes: Option<i64>,
    limit_bytes: Option<i64>,
) -> Result<(), AppError> {
    let range_requested = offset_bytes.is_some() || limit_bytes.is_some();
    if range_requested && revision.is_binary {
        return Err(AppError::InvalidInput(
            "range reads are only supported for text files".to_string(),
        ));
    }
    if revision.inline_content.is_some() || revision.content_base64.is_some() {
        apply_revision_content_range(revision, offset_bytes, limit_bytes)?;
        return Ok(());
    }
    if revision.is_binary && !allow_binary {
        return Ok(());
    }
    if range_requested
        && hydrate_revision_content_range(state, revision, offset_bytes, limit_bytes).await?
    {
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
    apply_revision_content_range(revision, offset_bytes, limit_bytes)?;
    Ok(())
}

async fn hydrate_revision_content_range(
    state: &AppState,
    revision: &mut FileRevisionResponse,
    offset_bytes: Option<i64>,
    limit_bytes: Option<i64>,
) -> Result<bool, AppError> {
    let Some((requested_offset, requested_limit)) =
        normalized_file_range(revision.size_bytes, offset_bytes, limit_bytes)?
    else {
        let offset = offset_bytes.unwrap_or(0).min(revision.size_bytes.max(0));
        revision.inline_content = Some(String::new());
        revision.content_base64 = None;
        revision.content_offset_bytes = Some(offset);
        revision.content_limit_bytes = Some(0);
        revision.content_truncated = Some(offset > 0);
        return Ok(true);
    };
    let fetch_start = requested_offset.saturating_sub(3);
    let prefix_bytes = usize::try_from(requested_offset - fetch_start)?;
    let fetch_limit = requested_limit
        .saturating_add(prefix_bytes)
        .saturating_add(4)
        .min(usize::try_from(
            revision.size_bytes.saturating_sub(fetch_start),
        )?);
    let Some(content) = state
        .rustfs_client
        .get_file_object_range_version(
            &revision.object_key,
            revision.version_id.as_deref(),
            u64::try_from(fetch_start)?,
            fetch_limit,
        )
        .await?
    else {
        return Ok(false);
    };
    let (slice, actual_offset, actual_limit, truncated) = text_byte_range_from_fetched_bytes(
        &content,
        fetch_start,
        requested_offset,
        requested_limit,
        revision.size_bytes,
    )?;
    revision.inline_content = Some(slice);
    revision.content_base64 = None;
    revision.content_offset_bytes = Some(actual_offset);
    revision.content_limit_bytes = Some(actual_limit);
    revision.content_truncated = Some(truncated);
    Ok(true)
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
    let tool_call_id = payload
        .tool_call_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok());
    let args_hash = payload
        .args_hash
        .as_deref()
        .unwrap_or(&revision.content_hash);
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
            tool_call_id,
            approval_id: None,
            args_hash: Some(args_hash),
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
        content_offset_bytes: None,
        content_limit_bytes: None,
        content_truncated: None,
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
    fn text_byte_range_preserves_utf8_boundaries() {
        let (slice, offset, limit, truncated) =
            text_byte_range("abc公积金def", 4, Some(8)).expect("range");

        assert_eq!(slice, "积金");
        assert_eq!(offset, 6);
        assert_eq!(limit, 6);
        assert!(truncated);
    }

    #[test]
    fn fetched_text_byte_range_preserves_utf8_boundaries() {
        let source = "abc公积金def".as_bytes();
        let fetch_start = 1;
        let fetched = &source[fetch_start as usize..source.len()];
        let (slice, offset, limit, truncated) =
            text_byte_range_from_fetched_bytes(fetched, fetch_start, 4, 8, source.len() as i64)
                .expect("range");

        assert_eq!(slice, "积金");
        assert_eq!(offset, 6);
        assert_eq!(limit, 6);
        assert!(truncated);
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

    #[test]
    fn small_text_search_extraction_covers_all_content() {
        let content = format!("{}{}", "a".repeat(FILE_SEARCH_CHUNK_BYTES), "尾部内容");

        let extraction = extract_file_search_chunks(&content).expect("extraction");

        assert_eq!(extraction.strategy, "full_chunks");
        assert!(!extraction.is_truncated);
        assert_eq!(extraction.source_size_bytes, content.len() as i64);
        assert_eq!(extraction.indexed_bytes, content.len() as i64);
        assert_eq!(
            extraction
                .chunks
                .iter()
                .map(|chunk| chunk.content_text.as_str())
                .collect::<String>(),
            content
        );
    }

    #[test]
    fn large_text_search_extraction_samples_head_middle_and_tail() {
        let half = FILE_SEARCH_MAX_INDEXED_BYTES * 2;
        let content = format!(
            "HEAD_TOKEN{}MIDDLE_TOKEN{}TAIL_TOKEN",
            "a".repeat(half),
            "b".repeat(half)
        );

        let extraction = extract_file_search_chunks(&content).expect("extraction");

        assert_eq!(extraction.strategy, "uniform_sample");
        assert!(extraction.is_truncated);
        assert_eq!(
            extraction.chunks.len(),
            FILE_SEARCH_MAX_INDEXED_BYTES / FILE_SEARCH_CHUNK_BYTES - 1
        );
        assert!(extraction.indexed_bytes <= FILE_SEARCH_MAX_INDEXED_BYTES as i64);
        assert!(
            extraction
                .chunks
                .first()
                .unwrap()
                .content_text
                .contains("HEAD_TOKEN")
        );
        assert!(
            extraction
                .chunks
                .iter()
                .any(|chunk| chunk.content_text.contains("MIDDLE_TOKEN"))
        );
        assert!(
            extraction
                .chunks
                .last()
                .unwrap()
                .content_text
                .contains("TAIL_TOKEN")
        );
    }

    #[test]
    fn large_text_search_chunks_preserve_utf8_boundaries() {
        let unit = "公积金检索";
        let content = unit.repeat(FILE_SEARCH_MAX_INDEXED_BYTES * 2 / unit.len());

        let extraction = extract_file_search_chunks(&content).expect("extraction");

        assert!(extraction.is_truncated);
        assert!(extraction.chunks.iter().all(|chunk| {
            content.is_char_boundary(chunk.byte_start as usize)
                && content.is_char_boundary(chunk.byte_end as usize)
                && chunk.content_text.len() <= FILE_SEARCH_CHUNK_BYTES
        }));
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
                tool_call_id: None,
                tool_name: None,
                args_hash: None,
                parent_tool_call_id: None,
                operation: None,
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
                tool_call_id: None,
                tool_name: None,
                args_hash: None,
                parent_tool_call_id: None,
                operation: None,
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
                offset_bytes: None,
                limit_bytes: None,
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
                offset_bytes: None,
                limit_bytes: None,
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
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn backfills_missing_inline_file_search_index() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = test_state().await?;
        let (tenant_id, user_id, project_id) = seed_file_context(&state.connect_pool).await?;
        let path = "/workspace/backfill-report.txt".to_string();
        let revision = write_revision(
            &state,
            FileWriteRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                content_ref: None,
                inline_content: Some("historical backfill token".to_string()),
                content_base64: None,
                content_type: None,
                expected_revision: 0,
                reason: "backfill fixture".to_string(),
                run_id: None,
                lock_token: None,
                tool_call_id: None,
                tool_name: None,
                args_hash: None,
                parent_tool_call_id: None,
                operation: None,
            },
        )
        .await?;
        sqlx::query("DELETE FROM file_search_chunks WHERE file_revision_id = $1")
            .bind(revision.id)
            .execute(&state.connect_pool)
            .await?;
        sqlx::query("UPDATE file_revisions SET metadata = metadata - 'search_index' WHERE id = $1")
            .bind(revision.id)
            .execute(&state.connect_pool)
            .await?;

        let summary = backfill_file_search_chunks(&state, 256).await?;
        assert!(summary.candidates >= 1);
        let matches = search_latest_revisions(
            &state,
            tenant_id,
            project_id,
            "/workspace/",
            "historical backfill token",
            10,
        )
        .await?;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, path);

        cleanup_tenant(&state.connect_pool, tenant_id).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn large_text_revision_is_uniformly_chunk_indexed()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = test_state().await?;
        let (tenant_id, user_id, project_id) = seed_file_context(&state.connect_pool).await?;
        let path = "/workspace/large-indexed-report.txt".to_string();
        let half = FILE_SEARCH_MAX_INDEXED_BYTES * 2;
        let content = format!(
            "HEAD_INDEX_TOKEN{}MIDDLE_INDEX_TOKEN{}TAIL_INDEX_TOKEN",
            "a".repeat(half),
            "b".repeat(half)
        );

        let revision = write_revision(
            &state,
            FileWriteRequest {
                tenant_id,
                actor_user_id: user_id,
                actor_device_id: None,
                actor_session_id: None,
                project_id,
                path: path.clone(),
                content_ref: None,
                inline_content: Some(content),
                content_base64: None,
                content_type: None,
                expected_revision: 0,
                reason: "large indexed write".to_string(),
                run_id: None,
                lock_token: None,
                tool_call_id: None,
                tool_name: None,
                args_hash: None,
                parent_tool_call_id: None,
                operation: None,
            },
        )
        .await?;

        assert!(revision.is_large);
        assert_eq!(
            revision.metadata.pointer("/search_index/status"),
            Some(&json!("indexed"))
        );
        assert_eq!(
            revision.metadata.pointer("/search_index/strategy"),
            Some(&json!("uniform_sample"))
        );
        assert_eq!(
            revision.metadata.pointer("/search_index/is_truncated"),
            Some(&json!(true))
        );
        let chunk_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM file_search_chunks WHERE file_revision_id = $1",
        )
        .bind(revision.id)
        .fetch_one(&state.connect_pool)
        .await?;
        assert_eq!(
            chunk_count,
            i64::try_from(FILE_SEARCH_MAX_INDEXED_BYTES / FILE_SEARCH_CHUNK_BYTES - 1)?
        );

        for token in ["HEAD_INDEX_TOKEN", "MIDDLE_INDEX_TOKEN", "TAIL_INDEX_TOKEN"] {
            let matches =
                search_latest_revisions(&state, tenant_id, project_id, "/workspace/", token, 10)
                    .await?;
            assert_eq!(matches.len(), 1, "missing indexed token {token}");
            assert_eq!(matches[0].path, path);
            assert!(matches[0].content_truncated.unwrap_or(false));
            assert!(
                matches[0]
                    .inline_content
                    .as_deref()
                    .is_some_and(|snippet| snippet.contains(token))
            );
            assert!(
                matches[0]
                    .inline_content
                    .as_ref()
                    .is_some_and(|snippet| snippet.len() <= FILE_SEARCH_CHUNK_BYTES)
            );
        }

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
                tool_call_id: None,
                tool_name: None,
                args_hash: None,
                parent_tool_call_id: None,
                operation: None,
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
                tool_call_id: None,
                tool_name: None,
                args_hash: None,
                parent_tool_call_id: None,
                operation: None,
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
                offset_bytes: None,
                limit_bytes: None,
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
                offset_bytes: None,
                limit_bytes: None,
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
                    offset_bytes: None,
                    limit_bytes: None,
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
            offset_bytes: None,
            limit_bytes: None,
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
            audit_partition_cleanup_enabled: false,
            secret_resolver:
                crate::features::agent_platform::secret_resolver::SecretResolver::env_only_for_tests(
                ),
            credential_rotation_worker_enabled: false,
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
            content_offset_bytes: None,
            content_limit_bytes: None,
            content_truncated: None,
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
