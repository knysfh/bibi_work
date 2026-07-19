use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::features::{
    agent_platform::{memory_context, rustfs::RustFsClient},
    core::errors::AppError,
};

pub(crate) const AUDIT_SEGMENT_CONTENT_TYPE: &str = "application/vnd.bibi-work.audit-segment+json";
const APPROVAL_EVIDENCE_CONTENT_TYPE: &str = "application/vnd.bibi-work.approval-evidence+json";
const TOOL_CALL_EVIDENCE_CONTENT_TYPE: &str = "application/vnd.bibi-work.tool-call-evidence+json";
const AUDIT_SUMMARY_MAX_CHARS: usize = 4_096;
const AUDIT_EVIDENCE_MAX_DEPTH: usize = 12;
const AUDIT_EVIDENCE_MAX_ARRAY_ITEMS: usize = 100;
const AUDIT_EVIDENCE_MAX_OBJECT_FIELDS: usize = 100;
pub const NO_UNSEALED_AUDIT_ROWS_MESSAGE: &str = "no unsealed audit hash chain rows found";

#[derive(Debug, Clone)]
pub struct NewAuditLog<'a> {
    pub tenant_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub actor_device_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub resource_type: &'a str,
    pub resource_id: &'a str,
    pub action: &'a str,
    pub decision: &'a str,
    pub policy_version: &'a str,
    pub reason_code: Option<&'a str>,
    pub run_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub workflow_run_id: Option<Uuid>,
    pub tool_call_id: Option<Uuid>,
    pub approval_id: Option<Uuid>,
    pub args_hash: Option<&'a str>,
    pub input_summary: Option<&'a str>,
    pub output_summary: Option<&'a str>,
    pub risk_level: Option<&'a str>,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub trace_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct AuditHashChainVerifyResponse {
    pub tenant_id: Uuid,
    pub valid: bool,
    pub rows_checked: i64,
    pub first_audit_id: Option<Uuid>,
    pub last_audit_id: Option<Uuid>,
    pub first_prev_hash: Option<String>,
    pub last_row_hash: Option<String>,
    pub broken_at: Option<AuditHashChainBreak>,
}

#[derive(Debug, Serialize)]
pub struct AuditHashChainBreak {
    pub audit_id: Uuid,
    pub reason: String,
    pub expected_prev_hash: Option<String>,
    pub actual_prev_hash: Option<String>,
    pub expected_row_hash: Option<String>,
    pub actual_row_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditHashBackfillReport {
    pub tenant_id: Uuid,
    pub unhashed_rows: i64,
    pub hashed_rows: i64,
    pub sealed_segments: i64,
    pub executable_in_place: bool,
    pub requires_offline_rechain: bool,
    pub dry_run: bool,
    pub updated_rows: i64,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct AuditHashChainSealResponse {
    pub segment_id: Uuid,
    pub tenant_id: Uuid,
    pub rows_count: i64,
    pub first_audit_id: Uuid,
    pub last_audit_id: Uuid,
    pub first_prev_hash: Option<String>,
    pub last_row_hash: String,
    pub manifest_hash: String,
    pub object_reference_id: Option<Uuid>,
    pub object_key: Option<String>,
    pub sealed_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct ApprovalEvidenceInput {
    pub tenant_id: Uuid,
    pub approval_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub tool_call_id: Option<Uuid>,
    pub status: String,
    pub request_payload: Value,
    pub decision_payload: Value,
    pub decided_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct ToolCallEvidenceInput {
    pub tenant_id: Uuid,
    pub tool_call_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub tool_name: String,
    pub resource_type: String,
    pub resource_id: String,
    pub status: String,
    pub decision: String,
    pub policy_version: String,
    pub args_hash: Option<String>,
    pub input_summary: Option<String>,
    pub output_summary: Option<String>,
    pub error_summary: Option<String>,
    pub risk_level: Option<String>,
    pub trace_id: Option<String>,
    pub completed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct ArchivedAuditEvidence {
    pub object_reference_id: Option<Uuid>,
    pub object_key: Option<String>,
}

#[derive(Debug)]
struct LastAuditSegment {
    id: Uuid,
    last_audit_log_id: Uuid,
    last_row_hash: String,
    manifest_hash: String,
}

#[derive(Debug, Clone)]
struct StoredAuditLog {
    id: Uuid,
    tenant_id: Uuid,
    actor_user_id: Option<Uuid>,
    actor_device_id: Option<Uuid>,
    session_id: Option<Uuid>,
    resource_type: String,
    resource_id: String,
    action: String,
    decision: String,
    policy_version: String,
    reason_code: Option<String>,
    run_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    workflow_run_id: Option<Uuid>,
    tool_call_id: Option<Uuid>,
    approval_id: Option<Uuid>,
    args_hash: Option<String>,
    input_summary: Option<String>,
    output_summary: Option<String>,
    risk_level: Option<String>,
    ip: Option<String>,
    user_agent: Option<String>,
    trace_id: Option<String>,
    prev_hash: Option<String>,
    row_hash: String,
    created_at: OffsetDateTime,
}

pub async fn insert_audit_log_tx(
    tx: &mut Transaction<'_, Postgres>,
    entry: NewAuditLog<'_>,
) -> Result<Uuid, AppError> {
    let sanitized_input_summary = entry.input_summary.map(sanitize_audit_text);
    let sanitized_output_summary = entry.output_summary.map(sanitize_audit_text);
    let sanitized_user_agent = entry.user_agent.map(sanitize_audit_text);
    let entry = NewAuditLog {
        input_summary: sanitized_input_summary.as_deref(),
        output_summary: sanitized_output_summary.as_deref(),
        user_agent: sanitized_user_agent.as_deref(),
        ..entry
    };
    lock_tenant_chain(tx, entry.tenant_id).await?;

    let previous_hash: Option<String> = sqlx::query(
        r#"
        SELECT row_hash
        FROM audit_logs
        WHERE tenant_id = $1
          AND row_hash IS NOT NULL
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(entry.tenant_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(|row| row.try_get("row_hash"))
    .transpose()?;

    let id = Uuid::new_v4();
    let created_at = now_truncated_to_pg_micros();
    let row_hash = compute_audit_row_hash(&entry, id, created_at, previous_hash.as_deref())?;

    sqlx::query(
        r#"
        INSERT INTO audit_logs (
            id, tenant_id, actor_user_id, actor_device_id, session_id,
            resource_type, resource_id, action, decision, policy_version, reason_code,
            run_id, conversation_id, workflow_run_id, tool_call_id, approval_id,
            args_hash, input_summary, output_summary, risk_level, ip, user_agent, trace_id,
            prev_hash, row_hash, created_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                $11, $12, $13, $14, $15, $16, $17, $18, $19, $20,
                $21, $22, $23, $24, $25, $26)
        "#,
    )
    .bind(id)
    .bind(entry.tenant_id)
    .bind(entry.actor_user_id)
    .bind(entry.actor_device_id)
    .bind(entry.session_id)
    .bind(entry.resource_type)
    .bind(entry.resource_id)
    .bind(entry.action)
    .bind(entry.decision)
    .bind(entry.policy_version)
    .bind(entry.reason_code)
    .bind(entry.run_id)
    .bind(entry.conversation_id)
    .bind(entry.workflow_run_id)
    .bind(entry.tool_call_id)
    .bind(entry.approval_id)
    .bind(entry.args_hash)
    .bind(entry.input_summary)
    .bind(entry.output_summary)
    .bind(entry.risk_level)
    .bind(entry.ip)
    .bind(entry.user_agent)
    .bind(entry.trace_id)
    .bind(previous_hash)
    .bind(row_hash)
    .bind(created_at)
    .execute(&mut **tx)
    .await?;

    Ok(id)
}

pub async fn verify_audit_hash_chain(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<AuditHashChainVerifyResponse, AppError> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, actor_user_id, actor_device_id, session_id,
               resource_type, resource_id, action, decision, policy_version, reason_code,
               run_id, conversation_id, workflow_run_id, tool_call_id, approval_id,
               args_hash, input_summary, output_summary, risk_level, ip, user_agent, trace_id,
               prev_hash, row_hash, created_at
        FROM (
            SELECT id, tenant_id, actor_user_id, actor_device_id, session_id,
                   resource_type, resource_id, action, decision, policy_version, reason_code,
                   run_id, conversation_id, workflow_run_id, tool_call_id, approval_id,
                   args_hash, input_summary, output_summary, risk_level, ip, user_agent, trace_id,
                   prev_hash, row_hash, created_at
            FROM audit_logs
            WHERE tenant_id = $1
              AND row_hash IS NOT NULL
            ORDER BY created_at DESC, id DESC
            LIMIT $2
        ) recent
        ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(tenant_id)
    .bind(limit)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(stored_audit_log_from_row)
    .collect::<Result<Vec<_>, AppError>>()?;

    verify_stored_audit_rows(tenant_id, rows)
}

pub async fn audit_hash_backfill_status(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<AuditHashBackfillReport, AppError> {
    let mut tx = pool.begin().await?;
    lock_tenant_chain(&mut tx, tenant_id).await?;
    let report = audit_hash_backfill_report_tx(&mut tx, tenant_id, true, 0).await?;
    tx.commit().await?;
    Ok(report)
}

pub async fn backfill_historical_audit_hashes(
    pool: &PgPool,
    tenant_id: Uuid,
    dry_run: bool,
) -> Result<AuditHashBackfillReport, AppError> {
    let mut tx = pool.begin().await?;
    lock_tenant_chain(&mut tx, tenant_id).await?;
    let initial = audit_hash_backfill_report_tx(&mut tx, tenant_id, dry_run, 0).await?;
    if dry_run || initial.unhashed_rows == 0 {
        tx.commit().await?;
        return Ok(initial);
    }
    if !initial.executable_in_place {
        return Err(AppError::Conflict(initial.reason));
    }

    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, actor_user_id, actor_device_id, session_id,
               resource_type, resource_id, action, decision, policy_version, reason_code,
               run_id, conversation_id, workflow_run_id, tool_call_id, approval_id,
               args_hash, input_summary, output_summary, risk_level, ip, user_agent, trace_id,
               created_at
        FROM audit_logs
        WHERE tenant_id = $1 AND row_hash IS NULL
        ORDER BY created_at, id
        "#,
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(historical_audit_log_from_row)
    .collect::<Result<Vec<_>, AppError>>()?;
    let mut previous_hash: Option<String> = None;
    let mut updated_rows = 0_i64;
    for row in rows {
        let row_hash = compute_audit_row_hash(
            &row.as_new_audit_log(),
            row.id,
            row.created_at,
            previous_hash.as_deref(),
        )?;
        let updated = sqlx::query(
            r#"
            UPDATE audit_logs
            SET prev_hash = $2, row_hash = $3
            WHERE id = $1 AND tenant_id = $4 AND row_hash IS NULL
            "#,
        )
        .bind(row.id)
        .bind(previous_hash.as_deref())
        .bind(&row_hash)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated != 1 {
            return Err(AppError::Conflict(
                "historical audit row changed during backfill".to_string(),
            ));
        }
        previous_hash = Some(row_hash);
        updated_rows += 1;
    }
    let report = audit_hash_backfill_report_tx(&mut tx, tenant_id, false, updated_rows).await?;
    tx.commit().await?;
    Ok(report)
}

async fn audit_hash_backfill_report_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    dry_run: bool,
    updated_rows: i64,
) -> Result<AuditHashBackfillReport, AppError> {
    let counts = sqlx::query(
        r#"
        SELECT COUNT(*) FILTER (WHERE row_hash IS NULL) AS unhashed_rows,
               COUNT(*) FILTER (WHERE row_hash IS NOT NULL) AS hashed_rows
        FROM audit_logs
        WHERE tenant_id = $1
        "#,
    )
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await?;
    let unhashed_rows: i64 = counts.try_get("unhashed_rows")?;
    let hashed_rows: i64 = counts.try_get("hashed_rows")?;
    let sealed_segments: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_hash_chain_segments WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&mut **tx)
            .await?;
    let executable_in_place = unhashed_rows > 0 && hashed_rows == 0 && sealed_segments == 0;
    let requires_offline_rechain = unhashed_rows > 0 && !executable_in_place;
    let reason = if unhashed_rows == 0 {
        "no unhashed historical audit rows".to_string()
    } else if executable_in_place {
        "tenant has only unhashed rows and no sealed segments; in-place backfill is safe"
            .to_string()
    } else {
        "tenant has an existing hashed or sealed chain; offline rechain and evidence replacement are required"
            .to_string()
    };
    Ok(AuditHashBackfillReport {
        tenant_id,
        unhashed_rows,
        hashed_rows,
        sealed_segments,
        executable_in_place,
        requires_offline_rechain,
        dry_run,
        updated_rows,
        reason,
    })
}

pub async fn seal_audit_hash_chain(
    pool: &PgPool,
    rustfs_client: &RustFsClient,
    tenant_id: Uuid,
    sealed_by_user_id: Option<Uuid>,
    max_rows: i64,
) -> Result<AuditHashChainSealResponse, AppError> {
    let mut tx = pool.begin().await?;
    lock_tenant_chain(&mut tx, tenant_id).await?;

    let previous_segment = load_last_audit_segment_tx(&mut tx, tenant_id).await?;
    let previous_hash = previous_segment
        .as_ref()
        .map(|segment| segment.last_row_hash.as_str());
    let rows = load_next_chain_rows_tx(&mut tx, tenant_id, previous_hash, max_rows).await?;
    if rows.is_empty() {
        return Err(AppError::Conflict(
            NO_UNSEALED_AUDIT_ROWS_MESSAGE.to_string(),
        ));
    }

    if let Some(expected_first_prev_hash) = previous_hash {
        let actual_first_prev_hash = rows.first().and_then(|row| row.prev_hash.as_deref());
        if actual_first_prev_hash != Some(expected_first_prev_hash) {
            return Err(AppError::Conflict(
                "audit hash chain is not continuous from previous segment".to_string(),
            ));
        }
    }

    let verification = verify_stored_audit_rows(tenant_id, rows.clone())?;
    if !verification.valid {
        return Err(AppError::Conflict(
            "cannot seal invalid audit hash chain segment".to_string(),
        ));
    }

    let segment_id = Uuid::new_v4();
    let sealed_at = now_truncated_to_pg_micros();
    let first = rows
        .first()
        .ok_or_else(|| AppError::Conflict(NO_UNSEALED_AUDIT_ROWS_MESSAGE.to_string()))?;
    let last = rows
        .last()
        .ok_or_else(|| AppError::Conflict(NO_UNSEALED_AUDIT_ROWS_MESSAGE.to_string()))?;
    let rows_count = i64::try_from(rows.len())?;
    let manifest = audit_segment_manifest(
        segment_id,
        tenant_id,
        sealed_by_user_id,
        sealed_at,
        previous_segment.as_ref(),
        &rows,
    );
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|_| {
        AppError::InvalidInput("failed to encode audit segment manifest".to_string())
    })?;
    let manifest_hash = sha256_hex(&manifest_bytes);
    let evidence = json!({
        "manifest_hash": manifest_hash,
        "manifest": manifest,
    });
    let evidence_bytes = serde_json::to_vec(&evidence).map_err(|_| {
        AppError::InvalidInput("failed to encode audit segment evidence".to_string())
    })?;
    let content_hash = sha256_hex(&evidence_bytes);
    let size_bytes = i64::try_from(evidence_bytes.len())?;
    let object_key = audit_segment_object_key(tenant_id, segment_id, sealed_at);

    let object = rustfs_client
        .put_audit_object(&object_key, evidence_bytes)
        .await?;
    let object_key_for_cleanup = object.as_ref().map(|object| object.object_key.clone());

    let insert_result = async {
        let object_reference_id = if let Some(object) = object {
            Some(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO object_references (
                        tenant_id, bucket, object_key, version_id, etag, content_hash,
                        size_bytes, content_type, owner_resource_type, owner_resource_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'audit_hash_chain_segment', $9)
                    RETURNING id
                    "#,
                )
                .bind(tenant_id)
                .bind(object.bucket)
                .bind(object.object_key)
                .bind(object.version_id)
                .bind(object.etag)
                .bind(&content_hash)
                .bind(size_bytes)
                .bind(AUDIT_SEGMENT_CONTENT_TYPE)
                .bind(segment_id.to_string())
                .fetch_one(&mut *tx)
                .await?,
            )
        } else {
            None
        };

        let sealed_at: OffsetDateTime = sqlx::query_scalar(
            r#"
            INSERT INTO audit_hash_chain_segments (
                id, tenant_id, first_audit_log_id, last_audit_log_id, rows_count,
                first_prev_hash, last_row_hash, manifest_hash, manifest,
                object_reference_id, sealed_by_user_id, sealed_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            RETURNING sealed_at
            "#,
        )
        .bind(segment_id)
        .bind(tenant_id)
        .bind(first.id)
        .bind(last.id)
        .bind(rows_count)
        .bind(first.prev_hash.clone())
        .bind(&last.row_hash)
        .bind(&manifest_hash)
        .bind(manifest.clone())
        .bind(object_reference_id)
        .bind(sealed_by_user_id)
        .bind(sealed_at)
        .fetch_one(&mut *tx)
        .await?;

        Ok::<_, AppError>(AuditHashChainSealResponse {
            segment_id,
            tenant_id,
            rows_count,
            first_audit_id: first.id,
            last_audit_id: last.id,
            first_prev_hash: first.prev_hash.clone(),
            last_row_hash: last.row_hash.clone(),
            manifest_hash,
            object_reference_id,
            object_key: object_key_for_cleanup.clone(),
            sealed_at,
        })
    }
    .await;

    match insert_result {
        Ok(response) => {
            if let Err(err) = tx.commit().await {
                if let Some(object_key) = object_key_for_cleanup {
                    let _ = rustfs_client.delete_audit_object(&object_key).await;
                }
                return Err(err.into());
            }
            Ok(response)
        }
        Err(err) => {
            if let Some(object_key) = object_key_for_cleanup {
                let _ = rustfs_client.delete_audit_object(&object_key).await;
            }
            Err(err)
        }
    }
}

pub async fn archive_approval_evidence_tx(
    tx: &mut Transaction<'_, Postgres>,
    rustfs_client: &RustFsClient,
    input: ApprovalEvidenceInput,
) -> Result<ArchivedAuditEvidence, AppError> {
    let archived_at = now_truncated_to_pg_micros();
    let evidence = approval_evidence_payload(&input, archived_at);
    let evidence_bytes = serde_json::to_vec(&evidence)
        .map_err(|_| AppError::InvalidInput("failed to encode approval evidence".to_string()))?;
    let content_hash = sha256_hex(&evidence_bytes);
    let size_bytes = i64::try_from(evidence_bytes.len())?;
    let object_key = approval_evidence_object_key(input.tenant_id, input.approval_id, archived_at);

    let object = rustfs_client
        .put_audit_object(&object_key, evidence_bytes)
        .await?;
    let object_key_for_cleanup = object.as_ref().map(|object| object.object_key.clone());

    let insert_result = async {
        let object_reference_id = if let Some(object) = object {
            Some(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO object_references (
                        tenant_id, bucket, object_key, version_id, etag, content_hash,
                        size_bytes, content_type, owner_resource_type, owner_resource_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'approval_evidence', $9)
                    RETURNING id
                    "#,
                )
                .bind(input.tenant_id)
                .bind(object.bucket)
                .bind(object.object_key)
                .bind(object.version_id)
                .bind(object.etag)
                .bind(&content_hash)
                .bind(size_bytes)
                .bind(APPROVAL_EVIDENCE_CONTENT_TYPE)
                .bind(input.approval_id.to_string())
                .fetch_one(&mut **tx)
                .await?,
            )
        } else {
            None
        };

        sqlx::query(
            r#"
            UPDATE approvals
            SET evidence_object_reference_id = $1,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $2 AND tenant_id = $3
            "#,
        )
        .bind(object_reference_id)
        .bind(input.approval_id)
        .bind(input.tenant_id)
        .execute(&mut **tx)
        .await?;

        Ok::<_, AppError>(ArchivedAuditEvidence {
            object_reference_id,
            object_key: object_key_for_cleanup.clone(),
        })
    }
    .await;

    if insert_result.is_err()
        && let Some(object_key) = object_key_for_cleanup
    {
        let _ = rustfs_client.delete_audit_object(&object_key).await;
    }

    insert_result
}

pub fn should_archive_tool_call_evidence(risk_level: Option<&str>) -> bool {
    matches!(
        risk_level.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if value == "high" || value == "critical"
    )
}

pub async fn archive_tool_call_evidence_tx(
    tx: &mut Transaction<'_, Postgres>,
    rustfs_client: &RustFsClient,
    input: ToolCallEvidenceInput,
) -> Result<ArchivedAuditEvidence, AppError> {
    let archived_at = now_truncated_to_pg_micros();
    let evidence = tool_call_evidence_payload(&input, archived_at);
    let evidence_bytes = serde_json::to_vec(&evidence)
        .map_err(|_| AppError::InvalidInput("failed to encode tool call evidence".to_string()))?;
    let content_hash = sha256_hex(&evidence_bytes);
    let size_bytes = i64::try_from(evidence_bytes.len())?;
    let object_key =
        tool_call_evidence_object_key(input.tenant_id, input.tool_call_id, archived_at);

    let object = rustfs_client
        .put_audit_object(&object_key, evidence_bytes)
        .await?;
    let object_key_for_cleanup = object.as_ref().map(|object| object.object_key.clone());

    let insert_result = async {
        let object_reference_id = if let Some(object) = object {
            Some(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    INSERT INTO object_references (
                        tenant_id, bucket, object_key, version_id, etag, content_hash,
                        size_bytes, content_type, owner_resource_type, owner_resource_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'tool_call_evidence', $9)
                    RETURNING id
                    "#,
                )
                .bind(input.tenant_id)
                .bind(object.bucket)
                .bind(object.object_key)
                .bind(object.version_id)
                .bind(object.etag)
                .bind(&content_hash)
                .bind(size_bytes)
                .bind(TOOL_CALL_EVIDENCE_CONTENT_TYPE)
                .bind(input.tool_call_id.to_string())
                .fetch_one(&mut **tx)
                .await?,
            )
        } else {
            None
        };

        let update_result = sqlx::query(
            r#"
            UPDATE tool_calls
            SET evidence_object_reference_id = COALESCE(evidence_object_reference_id, $1)
            WHERE id = $2 AND tenant_id = $3
            "#,
        )
        .bind(object_reference_id)
        .bind(input.tool_call_id)
        .bind(input.tenant_id)
        .execute(&mut **tx)
        .await?;
        if update_result.rows_affected() == 0 {
            return Err(AppError::NotFound(
                "tool call not found for evidence archive".to_string(),
            ));
        }

        Ok::<_, AppError>(ArchivedAuditEvidence {
            object_reference_id,
            object_key: object_key_for_cleanup.clone(),
        })
    }
    .await;

    if insert_result.is_err()
        && let Some(object_key) = object_key_for_cleanup
    {
        let _ = rustfs_client.delete_audit_object(&object_key).await;
    }

    insert_result
}

async fn load_last_audit_segment_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<Option<LastAuditSegment>, AppError> {
    let row = sqlx::query(
        r#"
        SELECT id, last_audit_log_id, last_row_hash, manifest_hash
        FROM audit_hash_chain_segments
        WHERE tenant_id = $1
        ORDER BY sealed_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?;

    row.map(|row| {
        Ok(LastAuditSegment {
            id: row.try_get("id")?,
            last_audit_log_id: row.try_get("last_audit_log_id")?,
            last_row_hash: row.try_get("last_row_hash")?,
            manifest_hash: row.try_get("manifest_hash")?,
        })
    })
    .transpose()
}

async fn load_next_chain_rows_tx(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    previous_hash: Option<&str>,
    max_rows: i64,
) -> Result<Vec<StoredAuditLog>, AppError> {
    let mut rows = Vec::new();
    let mut cursor = previous_hash.map(str::to_string);

    for _ in 0..max_rows {
        let candidates = sqlx::query(
            r#"
            SELECT id, tenant_id, actor_user_id, actor_device_id, session_id,
                   resource_type, resource_id, action, decision, policy_version, reason_code,
                   run_id, conversation_id, workflow_run_id, tool_call_id, approval_id,
                   args_hash, input_summary, output_summary, risk_level, ip, user_agent, trace_id,
                   prev_hash, row_hash, created_at
            FROM audit_logs
            WHERE tenant_id = $1
              AND row_hash IS NOT NULL
              AND (
                    ($2::text IS NULL AND prev_hash IS NULL)
                    OR prev_hash = $2
              )
            ORDER BY created_at ASC, id ASC
            LIMIT 2
            "#,
        )
        .bind(tenant_id)
        .bind(cursor.as_deref())
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .map(stored_audit_log_from_row)
        .collect::<Result<Vec<_>, AppError>>()?;

        if candidates.is_empty() {
            break;
        }
        if candidates.len() > 1 {
            return Err(AppError::Conflict(
                "audit hash chain fork detected while sealing".to_string(),
            ));
        }

        let next = candidates
            .into_iter()
            .next()
            .ok_or(AppError::DatabaseQuery)?;
        cursor = Some(next.row_hash.clone());
        rows.push(next);
    }

    Ok(rows)
}

fn audit_segment_manifest(
    segment_id: Uuid,
    tenant_id: Uuid,
    sealed_by_user_id: Option<Uuid>,
    sealed_at: OffsetDateTime,
    previous_segment: Option<&LastAuditSegment>,
    rows: &[StoredAuditLog],
) -> Value {
    let first = rows.first().expect("segment rows are non-empty");
    let last = rows.last().expect("segment rows are non-empty");
    let audit_rows = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "prev_hash": row.prev_hash,
                "row_hash": row.row_hash,
                "created_at": row.created_at.unix_timestamp_nanos().to_string(),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schema": "bibi-work.audit_hash_chain_segment.v1",
        "segment_id": segment_id,
        "tenant_id": tenant_id,
        "sealed_by_user_id": sealed_by_user_id,
        "sealed_at": sealed_at.unix_timestamp_nanos().to_string(),
        "previous_segment": previous_segment.map(|segment| json!({
            "id": segment.id,
            "last_audit_log_id": segment.last_audit_log_id,
            "last_row_hash": segment.last_row_hash,
            "manifest_hash": segment.manifest_hash,
        })),
        "first_audit_log_id": first.id,
        "last_audit_log_id": last.id,
        "rows_count": rows.len(),
        "first_prev_hash": first.prev_hash,
        "last_row_hash": last.row_hash,
        "audit_rows": audit_rows,
    })
}

pub(crate) fn audit_segment_object_key(
    tenant_id: Uuid,
    segment_id: Uuid,
    sealed_at: OffsetDateTime,
) -> String {
    let date = sealed_at.date();
    format!(
        "tenants/{tenant_id}/audit/hash-chain/{}/{:02}/{:02}/{segment_id}.json",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn approval_evidence_payload(input: &ApprovalEvidenceInput, archived_at: OffsetDateTime) -> Value {
    json!({
        "schema": "bibi-work.approval_evidence.v1",
        "tenant_id": input.tenant_id,
        "approval_id": input.approval_id,
        "actor_user_id": input.actor_user_id,
        "conversation_id": input.conversation_id,
        "run_id": input.run_id,
        "tool_call_id": input.tool_call_id,
        "status": input.status,
        "request_payload": sanitize_audit_evidence_value(&input.request_payload),
        "decision_payload": sanitize_audit_evidence_value(&input.decision_payload),
        "decided_at": input.decided_at.map(|value| value.unix_timestamp_nanos().to_string()),
        "archived_at": archived_at.unix_timestamp_nanos().to_string(),
    })
}

fn approval_evidence_object_key(
    tenant_id: Uuid,
    approval_id: Uuid,
    archived_at: OffsetDateTime,
) -> String {
    let date = archived_at.date();
    format!(
        "tenants/{tenant_id}/audit/approvals/{}/{:02}/{:02}/{approval_id}.json",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

fn tool_call_evidence_payload(input: &ToolCallEvidenceInput, archived_at: OffsetDateTime) -> Value {
    json!({
        "schema": "bibi-work.tool_call_evidence.v1",
        "tenant_id": input.tenant_id,
        "tool_call_id": input.tool_call_id,
        "actor_user_id": input.actor_user_id,
        "conversation_id": input.conversation_id,
        "run_id": input.run_id,
        "tool_name": input.tool_name,
        "resource_type": input.resource_type,
        "resource_id": input.resource_id,
        "status": input.status,
        "decision": input.decision,
        "policy_version": input.policy_version,
        "args_hash": input.args_hash,
        "input_summary": input.input_summary.as_deref().map(sanitize_audit_text),
        "output_summary": input.output_summary.as_deref().map(sanitize_audit_text),
        "error_summary": input.error_summary.as_deref().map(sanitize_audit_text),
        "risk_level": input.risk_level,
        "trace_id": input.trace_id,
        "completed_at": input.completed_at.map(|value| value.unix_timestamp_nanos().to_string()),
        "archived_at": archived_at.unix_timestamp_nanos().to_string(),
    })
}

pub(crate) fn sanitize_audit_text(input: &str) -> String {
    let redacted = memory_context::redact_sensitive_text(input);
    truncate_chars(&redacted, AUDIT_SUMMARY_MAX_CHARS)
}

pub(crate) fn sanitize_audit_evidence_value(value: &Value) -> Value {
    sanitize_audit_evidence_value_at_depth(value, 0)
}

fn sanitize_audit_evidence_value_at_depth(value: &Value, depth: usize) -> Value {
    if depth >= AUDIT_EVIDENCE_MAX_DEPTH {
        return Value::String("[TRUNCATED_DEPTH]".to_string());
    }
    match value {
        Value::Object(map) => {
            let mut sanitized = serde_json::Map::new();
            for (index, (key, item)) in map.iter().enumerate() {
                if index >= AUDIT_EVIDENCE_MAX_OBJECT_FIELDS {
                    sanitized.insert(
                        "_truncated_fields".to_string(),
                        Value::Number((map.len() - index).into()),
                    );
                    break;
                }
                if is_sensitive_audit_key(key) {
                    sanitized.insert(key.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    sanitized.insert(
                        key.clone(),
                        sanitize_audit_evidence_value_at_depth(item, depth + 1),
                    );
                }
            }
            Value::Object(sanitized)
        }
        Value::Array(items) => {
            let mut sanitized = items
                .iter()
                .take(AUDIT_EVIDENCE_MAX_ARRAY_ITEMS)
                .map(|item| sanitize_audit_evidence_value_at_depth(item, depth + 1))
                .collect::<Vec<_>>();
            if items.len() > AUDIT_EVIDENCE_MAX_ARRAY_ITEMS {
                sanitized.push(json!({
                    "_truncated_items": items.len() - AUDIT_EVIDENCE_MAX_ARRAY_ITEMS
                }));
            }
            Value::Array(sanitized)
        }
        Value::String(text) => Value::String(sanitize_audit_text(text)),
        other => other.clone(),
    }
}

fn is_sensitive_audit_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    normalized.contains("authorization")
        || normalized.contains("api_key")
        || normalized.contains("apikey")
        || normalized.contains("access_token")
        || normalized.contains("refresh_token")
        || normalized.contains("client_secret")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized == "token"
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut truncated = input.chars().take(max_chars).collect::<String>();
    truncated.push_str("...[TRUNCATED]");
    truncated
}

fn tool_call_evidence_object_key(
    tenant_id: Uuid,
    tool_call_id: Uuid,
    archived_at: OffsetDateTime,
) -> String {
    let date = archived_at.date();
    format!(
        "tenants/{tenant_id}/audit/tool-calls/{}/{:02}/{:02}/{tool_call_id}/{}.json",
        date.year(),
        u8::from(date.month()),
        date.day(),
        archived_at.unix_timestamp_nanos()
    )
}

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

async fn lock_tenant_chain(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(tenant_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn now_truncated_to_pg_micros() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    let nanos = now.unix_timestamp_nanos();
    let pg_micros = (nanos / 1_000) * 1_000;
    OffsetDateTime::from_unix_timestamp_nanos(pg_micros).unwrap_or(now)
}

fn compute_audit_row_hash(
    entry: &NewAuditLog<'_>,
    id: Uuid,
    created_at: OffsetDateTime,
    previous_hash: Option<&str>,
) -> Result<String, AppError> {
    let canonical = canonical_audit_json(entry, id, created_at)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical);
    hasher.update(previous_hash.unwrap_or("").as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

fn canonical_audit_json(
    entry: &NewAuditLog<'_>,
    id: Uuid,
    created_at: OffsetDateTime,
) -> Result<Vec<u8>, AppError> {
    let mut row = BTreeMap::new();
    insert_uuid(&mut row, "id", Some(id));
    insert_uuid(&mut row, "tenant_id", Some(entry.tenant_id));
    insert_uuid(&mut row, "actor_user_id", entry.actor_user_id);
    insert_uuid(&mut row, "actor_device_id", entry.actor_device_id);
    insert_uuid(&mut row, "session_id", entry.session_id);
    insert_str(&mut row, "resource_type", Some(entry.resource_type));
    insert_str(&mut row, "resource_id", Some(entry.resource_id));
    insert_str(&mut row, "action", Some(entry.action));
    insert_str(&mut row, "decision", Some(entry.decision));
    insert_str(&mut row, "policy_version", Some(entry.policy_version));
    insert_str(&mut row, "reason_code", entry.reason_code);
    insert_uuid(&mut row, "run_id", entry.run_id);
    insert_uuid(&mut row, "conversation_id", entry.conversation_id);
    insert_uuid(&mut row, "workflow_run_id", entry.workflow_run_id);
    insert_uuid(&mut row, "tool_call_id", entry.tool_call_id);
    insert_uuid(&mut row, "approval_id", entry.approval_id);
    insert_str(&mut row, "args_hash", entry.args_hash);
    insert_str(&mut row, "input_summary", entry.input_summary);
    insert_str(&mut row, "output_summary", entry.output_summary);
    insert_str(&mut row, "risk_level", entry.risk_level);
    insert_str(&mut row, "ip", entry.ip);
    insert_str(&mut row, "user_agent", entry.user_agent);
    insert_str(&mut row, "trace_id", entry.trace_id);
    // Store the canonical timestamp as Unix nanos to avoid RFC3339 formatting drift.
    insert_str(
        &mut row,
        "created_at",
        Some(&created_at.unix_timestamp_nanos().to_string()),
    );

    serde_json::to_vec(&row)
        .map_err(|_| AppError::InvalidInput("failed to encode audit hash row".to_string()))
}

fn insert_uuid(row: &mut BTreeMap<&'static str, Value>, key: &'static str, value: Option<Uuid>) {
    row.insert(
        key,
        value
            .map(|uuid| Value::String(uuid.to_string()))
            .unwrap_or(Value::Null),
    );
}

fn insert_str(row: &mut BTreeMap<&'static str, Value>, key: &'static str, value: Option<&str>) {
    row.insert(
        key,
        value
            .map(|text| Value::String(text.to_string()))
            .unwrap_or(Value::Null),
    );
}

fn verify_stored_audit_rows(
    tenant_id: Uuid,
    rows: Vec<StoredAuditLog>,
) -> Result<AuditHashChainVerifyResponse, AppError> {
    let first_audit_id = rows.first().map(|row| row.id);
    let last_audit_id = rows.last().map(|row| row.id);
    let first_prev_hash = rows.first().and_then(|row| row.prev_hash.clone());
    let mut previous_row_hash: Option<String> = None;

    for (index, row) in rows.iter().enumerate() {
        let rows_checked = (index + 1) as i64;
        if let Some(expected_prev_hash) = previous_row_hash.clone()
            && row.prev_hash.as_deref() != Some(expected_prev_hash.as_str())
        {
            return Ok(AuditHashChainVerifyResponse {
                tenant_id,
                valid: false,
                rows_checked,
                first_audit_id,
                last_audit_id,
                first_prev_hash,
                last_row_hash: Some(expected_prev_hash.clone()),
                broken_at: Some(AuditHashChainBreak {
                    audit_id: row.id,
                    reason: "prev_hash_mismatch".to_string(),
                    expected_prev_hash: Some(expected_prev_hash),
                    actual_prev_hash: row.prev_hash.clone(),
                    expected_row_hash: None,
                    actual_row_hash: Some(row.row_hash.clone()),
                }),
            });
        }

        let expected_row_hash = compute_audit_row_hash(
            &row.as_new_audit_log(),
            row.id,
            row.created_at,
            row.prev_hash.as_deref(),
        )?;
        if expected_row_hash != row.row_hash {
            return Ok(AuditHashChainVerifyResponse {
                tenant_id,
                valid: false,
                rows_checked,
                first_audit_id,
                last_audit_id,
                first_prev_hash,
                last_row_hash: previous_row_hash,
                broken_at: Some(AuditHashChainBreak {
                    audit_id: row.id,
                    reason: "row_hash_mismatch".to_string(),
                    expected_prev_hash: row.prev_hash.clone(),
                    actual_prev_hash: row.prev_hash.clone(),
                    expected_row_hash: Some(expected_row_hash),
                    actual_row_hash: Some(row.row_hash.clone()),
                }),
            });
        }

        previous_row_hash = Some(row.row_hash.clone());
    }

    Ok(AuditHashChainVerifyResponse {
        tenant_id,
        valid: true,
        rows_checked: rows.len() as i64,
        first_audit_id,
        last_audit_id,
        first_prev_hash,
        last_row_hash: previous_row_hash,
        broken_at: None,
    })
}

fn stored_audit_log_from_row(row: PgRow) -> Result<StoredAuditLog, AppError> {
    Ok(StoredAuditLog {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        actor_user_id: row.try_get("actor_user_id")?,
        actor_device_id: row.try_get("actor_device_id")?,
        session_id: row.try_get("session_id")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        action: row.try_get("action")?,
        decision: row.try_get("decision")?,
        policy_version: row.try_get("policy_version")?,
        reason_code: row.try_get("reason_code")?,
        run_id: row.try_get("run_id")?,
        conversation_id: row.try_get("conversation_id")?,
        workflow_run_id: row.try_get("workflow_run_id")?,
        tool_call_id: row.try_get("tool_call_id")?,
        approval_id: row.try_get("approval_id")?,
        args_hash: row.try_get("args_hash")?,
        input_summary: row.try_get("input_summary")?,
        output_summary: row.try_get("output_summary")?,
        risk_level: row.try_get("risk_level")?,
        ip: row.try_get("ip")?,
        user_agent: row.try_get("user_agent")?,
        trace_id: row.try_get("trace_id")?,
        prev_hash: row.try_get("prev_hash")?,
        row_hash: row.try_get("row_hash")?,
        created_at: row.try_get("created_at")?,
    })
}

fn historical_audit_log_from_row(row: PgRow) -> Result<StoredAuditLog, AppError> {
    Ok(StoredAuditLog {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        actor_user_id: row.try_get("actor_user_id")?,
        actor_device_id: row.try_get("actor_device_id")?,
        session_id: row.try_get("session_id")?,
        resource_type: row.try_get("resource_type")?,
        resource_id: row.try_get("resource_id")?,
        action: row.try_get("action")?,
        decision: row.try_get("decision")?,
        policy_version: row.try_get("policy_version")?,
        reason_code: row.try_get("reason_code")?,
        run_id: row.try_get("run_id")?,
        conversation_id: row.try_get("conversation_id")?,
        workflow_run_id: row.try_get("workflow_run_id")?,
        tool_call_id: row.try_get("tool_call_id")?,
        approval_id: row.try_get("approval_id")?,
        args_hash: row.try_get("args_hash")?,
        input_summary: row.try_get("input_summary")?,
        output_summary: row.try_get("output_summary")?,
        risk_level: row.try_get("risk_level")?,
        ip: row.try_get("ip")?,
        user_agent: row.try_get("user_agent")?,
        trace_id: row.try_get("trace_id")?,
        prev_hash: None,
        row_hash: String::new(),
        created_at: row.try_get("created_at")?,
    })
}

impl StoredAuditLog {
    fn as_new_audit_log(&self) -> NewAuditLog<'_> {
        NewAuditLog {
            tenant_id: self.tenant_id,
            actor_user_id: self.actor_user_id,
            actor_device_id: self.actor_device_id,
            session_id: self.session_id,
            resource_type: &self.resource_type,
            resource_id: &self.resource_id,
            action: &self.action,
            decision: &self.decision,
            policy_version: &self.policy_version,
            reason_code: self.reason_code.as_deref(),
            run_id: self.run_id,
            conversation_id: self.conversation_id,
            workflow_run_id: self.workflow_run_id,
            tool_call_id: self.tool_call_id,
            approval_id: self.approval_id,
            args_hash: self.args_hash.as_deref(),
            input_summary: self.input_summary.as_deref(),
            output_summary: self.output_summary.as_deref(),
            risk_level: self.risk_level.as_deref(),
            ip: self.ip.as_deref(),
            user_agent: self.user_agent.as_deref(),
            trace_id: self.trace_id.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use sqlx::{Row, postgres::PgPoolOptions};
    use time::OffsetDateTime;
    use uuid::Uuid;

    use crate::{
        configuration::ObjectStoreSettings, features::agent_platform::rustfs::RustFsClient,
    };
    use secrecy::SecretBox;
    use serde_json::{Value, json};

    use super::{
        ApprovalEvidenceInput, NewAuditLog, ToolCallEvidenceInput, archive_approval_evidence_tx,
        archive_tool_call_evidence_tx, backfill_historical_audit_hashes, compute_audit_row_hash,
        insert_audit_log_tx, sanitize_audit_evidence_value, sanitize_audit_text,
        seal_audit_hash_chain, should_archive_tool_call_evidence, tool_call_evidence_object_key,
        tool_call_evidence_payload, verify_audit_hash_chain,
    };

    fn audit_entry() -> NewAuditLog<'static> {
        NewAuditLog {
            tenant_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            actor_user_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap()),
            actor_device_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap()),
            session_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000004").unwrap()),
            resource_type: "tool",
            resource_id: "tool-1",
            action: "execute",
            decision: "allow",
            policy_version: "local-policy-v1",
            reason_code: None,
            run_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000005").unwrap()),
            conversation_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000006").unwrap()),
            workflow_run_id: None,
            tool_call_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000007").unwrap()),
            approval_id: None,
            args_hash: Some("sha256:args"),
            input_summary: Some("redacted input"),
            output_summary: Some("redacted output"),
            risk_level: Some("low"),
            ip: Some("127.0.0.1"),
            user_agent: Some("test-agent"),
            trace_id: Some("trace-1"),
        }
    }

    fn audit_entry_for_tenant(tenant_id: Uuid, action: &'static str) -> NewAuditLog<'static> {
        NewAuditLog {
            tenant_id,
            actor_user_id: None,
            actor_device_id: None,
            session_id: None,
            resource_type: "tool",
            resource_id: "tool-1",
            action,
            decision: "allow",
            policy_version: "local-policy-v1",
            reason_code: None,
            run_id: None,
            conversation_id: None,
            workflow_run_id: None,
            tool_call_id: None,
            approval_id: None,
            args_hash: Some("sha256:args"),
            input_summary: Some("redacted input"),
            output_summary: Some("redacted output"),
            risk_level: Some("low"),
            ip: Some("127.0.0.1"),
            user_agent: Some("test-agent"),
            trace_id: Some("trace-1"),
        }
    }

    #[test]
    fn audit_hash_is_stable_for_same_row_and_previous_hash() {
        let entry = audit_entry();
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000008").unwrap();
        let created_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

        let first = compute_audit_row_hash(&entry, id, created_at, Some("previous")).unwrap();
        let second = compute_audit_row_hash(&entry, id, created_at, Some("previous")).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn audit_text_redacts_credentials_and_caps_length() {
        let input = format!(
            "authorization: Bearer raw-token api_key=sk-test password=hunter2 {}",
            "x".repeat(5_000)
        );

        let sanitized = sanitize_audit_text(&input);

        assert!(!sanitized.contains("raw-token"));
        assert!(!sanitized.contains("sk-test"));
        assert!(!sanitized.contains("hunter2"));
        assert!(sanitized.contains("[REDACTED]"));
        assert!(sanitized.ends_with("...[TRUNCATED]"));
    }

    #[test]
    fn audit_evidence_recursively_redacts_secret_fields_and_values() {
        let sanitized = sanitize_audit_evidence_value(&json!({
            "authorization": "Bearer raw-token",
            "nested": {
                "client_secret": "secret-value",
                "safe": "password=hunter2",
                "items": [{"apiKey": "sk-test"}]
            }
        }));

        assert_eq!(sanitized["authorization"], json!("[REDACTED]"));
        assert_eq!(sanitized["nested"]["client_secret"], json!("[REDACTED]"));
        assert_eq!(
            sanitized["nested"]["items"][0]["apiKey"],
            json!("[REDACTED]")
        );
        assert_eq!(sanitized["nested"]["safe"], json!("password=[REDACTED]"));
    }

    #[test]
    fn audit_hash_changes_when_previous_hash_changes() {
        let entry = audit_entry();
        let id = Uuid::parse_str("00000000-0000-0000-0000-000000000008").unwrap();
        let created_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

        let first = compute_audit_row_hash(&entry, id, created_at, Some("previous-1")).unwrap();
        let second = compute_audit_row_hash(&entry, id, created_at, Some("previous-2")).unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn tool_call_evidence_archive_policy_only_includes_high_risk_levels() {
        assert!(should_archive_tool_call_evidence(Some("high")));
        assert!(should_archive_tool_call_evidence(Some(" critical ")));
        assert!(should_archive_tool_call_evidence(Some("HIGH")));
        assert!(!should_archive_tool_call_evidence(Some("medium")));
        assert!(!should_archive_tool_call_evidence(Some("low")));
        assert!(!should_archive_tool_call_evidence(None));
    }

    #[test]
    fn tool_call_evidence_payload_uses_summaries_and_stable_identifiers() {
        let tenant_id = Uuid::parse_str("00000000-0000-0000-0000-000000000101").unwrap();
        let tool_call_id = Uuid::parse_str("00000000-0000-0000-0000-000000000102").unwrap();
        let archived_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let completed_at = OffsetDateTime::from_unix_timestamp(1_700_000_001).unwrap();
        let payload = tool_call_evidence_payload(
            &ToolCallEvidenceInput {
                tenant_id,
                tool_call_id,
                actor_user_id: None,
                conversation_id: None,
                run_id: None,
                tool_name: "sql_query".to_string(),
                resource_type: "sql_query".to_string(),
                resource_id: "query-hash".to_string(),
                status: "failed".to_string(),
                decision: "allow".to_string(),
                policy_version: "local-policy-v1".to_string(),
                args_hash: Some("args-hash".to_string()),
                input_summary: Some("{\"query\":\"select <redacted>\"}".to_string()),
                output_summary: None,
                error_summary: Some("permission denied".to_string()),
                risk_level: Some("critical".to_string()),
                trace_id: Some("trace-1".to_string()),
                completed_at: Some(completed_at),
            },
            archived_at,
        );

        assert_eq!(
            payload.get("schema").and_then(Value::as_str),
            Some("bibi-work.tool_call_evidence.v1")
        );
        let tool_call_id_text = tool_call_id.to_string();
        assert_eq!(
            payload.get("tool_call_id").and_then(Value::as_str),
            Some(tool_call_id_text.as_str())
        );
        assert_eq!(
            payload.get("error_summary").and_then(Value::as_str),
            Some("permission denied")
        );
        let completed_at_text = completed_at.unix_timestamp_nanos().to_string();
        assert_eq!(
            payload.get("completed_at").and_then(Value::as_str),
            Some(completed_at_text.as_str())
        );

        let object_key = tool_call_evidence_object_key(tenant_id, tool_call_id, archived_at);
        assert!(object_key.contains("/audit/tool-calls/"));
        assert!(object_key.contains(&tool_call_id.to_string()));
        assert!(object_key.ends_with(".json"));
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn insert_audit_log_tx_chains_rows_per_tenant() -> Result<(), Box<dyn std::error::Error>>
    {
        let pool = test_pool().await?;
        let tenant_id = Uuid::new_v4();
        let tenant_slug = format!("audit-chain-test-{tenant_id}");

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Audit chain test")
            .bind(&tenant_slug)
            .execute(&pool)
            .await?;

        let mut tx = pool.begin().await?;
        let first_id =
            insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "read")).await?;
        let second_id =
            insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "write")).await?;
        tx.commit().await?;

        let first_hash: String = sqlx::query("SELECT row_hash FROM audit_logs WHERE id = $1")
            .bind(first_id)
            .fetch_one(&pool)
            .await?
            .try_get("row_hash")?;

        let second = sqlx::query("SELECT prev_hash, row_hash FROM audit_logs WHERE id = $1")
            .bind(second_id)
            .fetch_one(&pool)
            .await?;
        let second_prev_hash: Option<String> = second.try_get("prev_hash")?;
        let second_hash: String = second.try_get("row_hash")?;

        assert_eq!(second_prev_hash.as_deref(), Some(first_hash.as_str()));
        assert_ne!(first_hash, second_hash);

        let valid = verify_audit_hash_chain(&pool, tenant_id, 100).await?;
        assert!(valid.valid);
        assert_eq!(valid.rows_checked, 2);
        assert_eq!(valid.first_audit_id, Some(first_id));
        assert_eq!(valid.last_audit_id, Some(second_id));
        assert_eq!(valid.last_row_hash.as_deref(), Some(second_hash.as_str()));

        let sealed_by_user_id = seed_platform_user(&pool, tenant_id).await?;
        let sealed = seal_audit_hash_chain(
            &pool,
            &RustFsClient::disabled_for_tests(),
            tenant_id,
            Some(sealed_by_user_id),
            100,
        )
        .await?;
        assert_eq!(sealed.rows_count, 2);
        assert_eq!(sealed.first_audit_id, first_id);
        assert_eq!(sealed.last_audit_id, second_id);
        assert_eq!(sealed.last_row_hash, second_hash);
        assert!(sealed.object_reference_id.is_none());
        assert!(sealed.object_key.is_none());

        let stored_segment_exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM audit_hash_chain_segments
                WHERE id = $1
                  AND tenant_id = $2
                  AND manifest_hash = $3
                  AND object_reference_id IS NULL
            )
            "#,
        )
        .bind(sealed.segment_id)
        .bind(tenant_id)
        .bind(&sealed.manifest_hash)
        .fetch_one(&pool)
        .await?;
        assert!(stored_segment_exists);

        let second_seal = seal_audit_hash_chain(
            &pool,
            &RustFsClient::disabled_for_tests(),
            tenant_id,
            Some(sealed_by_user_id),
            100,
        )
        .await;
        assert!(second_seal.is_err());

        sqlx::query("UPDATE audit_logs SET action = 'tampered' WHERE id = $1")
            .bind(second_id)
            .execute(&pool)
            .await?;

        let invalid = verify_audit_hash_chain(&pool, tenant_id, 100).await?;
        assert!(!invalid.valid);
        assert_eq!(invalid.rows_checked, 2);
        assert_eq!(
            invalid.broken_at.as_ref().map(|item| item.reason.as_str()),
            Some("row_hash_mismatch")
        );
        assert_eq!(
            invalid.broken_at.as_ref().map(|item| item.audit_id),
            Some(second_id)
        );

        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn backfills_only_an_unsealed_unhashed_tenant_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let tenant_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Backfill test', $2)")
            .bind(tenant_id)
            .bind(format!("audit-backfill-test-{tenant_id}"))
            .execute(&pool)
            .await?;
        for (index, action) in ["legacy-read", "legacy-write"].into_iter().enumerate() {
            sqlx::query(
                r#"
                INSERT INTO audit_logs (
                    tenant_id, resource_type, resource_id, action, decision,
                    policy_version, input_summary, created_at
                )
                VALUES ($1, 'legacy', $2, $3, 'allow', 'legacy-v1', $4,
                        CURRENT_TIMESTAMP + ($5 * INTERVAL '1 microsecond'))
                "#,
            )
            .bind(tenant_id)
            .bind(format!("resource-{index}"))
            .bind(action)
            .bind(format!("legacy-{index}"))
            .bind(i64::try_from(index)?)
            .execute(&pool)
            .await?;
        }

        let dry_run = backfill_historical_audit_hashes(&pool, tenant_id, true).await?;
        assert!(dry_run.executable_in_place);
        assert_eq!(dry_run.unhashed_rows, 2);
        let executed = backfill_historical_audit_hashes(&pool, tenant_id, false).await?;
        assert_eq!(executed.updated_rows, 2);
        assert_eq!(executed.unhashed_rows, 0);
        let verification = verify_audit_hash_chain(&pool, tenant_id, 100).await?;
        assert!(verification.valid);
        assert_eq!(verification.rows_checked, 2);

        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn backfill_fails_closed_for_a_mixed_existing_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let tenant_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Mixed backfill test', $2)")
            .bind(tenant_id)
            .bind(format!("audit-mixed-backfill-test-{tenant_id}"))
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                tenant_id, resource_type, resource_id, action, decision, policy_version
            ) VALUES ($1, 'legacy', 'legacy-1', 'read', 'allow', 'legacy-v1')
            "#,
        )
        .bind(tenant_id)
        .execute(&pool)
        .await?;
        let mut tx = pool.begin().await?;
        insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "new-write")).await?;
        tx.commit().await?;

        let report = backfill_historical_audit_hashes(&pool, tenant_id, true).await?;
        assert!(report.requires_offline_rechain);
        assert!(!report.executable_in_place);
        assert!(
            backfill_historical_audit_hashes(&pool, tenant_id, false)
                .await
                .is_err()
        );
        let unhashed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_logs WHERE tenant_id = $1 AND row_hash IS NULL",
        )
        .bind(tenant_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(unhashed, 1);

        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, RustFS, and the bibi_work schema"]
    async fn seal_audit_hash_chain_writes_manifest_object_to_rustfs()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let rustfs = rustfs_client_from_env()?;
        let tenant_id = Uuid::new_v4();
        let tenant_slug = format!("audit-segment-rustfs-test-{tenant_id}");

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Audit segment RustFS test")
            .bind(&tenant_slug)
            .execute(&pool)
            .await?;

        let sealed_by_user_id = seed_platform_user(&pool, tenant_id).await?;
        let mut tx = pool.begin().await?;
        insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "read")).await?;
        insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "write")).await?;
        tx.commit().await?;

        let sealed =
            seal_audit_hash_chain(&pool, &rustfs, tenant_id, Some(sealed_by_user_id), 100).await?;
        let object_reference_id = sealed
            .object_reference_id
            .expect("expected object reference");
        let object_key = sealed.object_key.clone().expect("expected object key");

        let evidence_bytes = rustfs
            .get_audit_object(&object_key)
            .await?
            .expect("expected RustFS evidence object");
        let evidence: Value = serde_json::from_slice(&evidence_bytes)?;
        let segment_id = sealed.segment_id.to_string();
        assert_eq!(
            evidence.get("manifest_hash").and_then(Value::as_str),
            Some(sealed.manifest_hash.as_str())
        );
        assert_eq!(
            evidence
                .pointer("/manifest/segment_id")
                .and_then(Value::as_str),
            Some(segment_id.as_str())
        );

        let object_reference_exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM object_references
                WHERE id = $1
                  AND tenant_id = $2
                  AND object_key = $3
                  AND owner_resource_type = 'audit_hash_chain_segment'
                  AND owner_resource_id = $4
            )
            "#,
        )
        .bind(object_reference_id)
        .bind(tenant_id)
        .bind(&object_key)
        .bind(sealed.segment_id.to_string())
        .fetch_one(&pool)
        .await?;
        assert!(object_reference_exists);

        rustfs.delete_audit_object(&object_key).await?;
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, RustFS, and the bibi_work schema"]
    async fn archive_approval_evidence_writes_audit_bucket_object()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let rustfs = rustfs_client_from_env()?;
        let tenant_id = Uuid::new_v4();
        let tenant_slug = format!("approval-evidence-rustfs-test-{tenant_id}");
        let approval_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Approval evidence RustFS test")
            .bind(&tenant_slug)
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO approvals (id, tenant_id, status, request_payload, decision_payload)
            VALUES ($1, $2, 'approved', $3, $4)
            "#,
        )
        .bind(approval_id)
        .bind(tenant_id)
        .bind(json!({"tool_call_id": "tool-call-1", "risk": "critical"}))
        .bind(json!({"decision": "approved", "reason": "test"}))
        .execute(&pool)
        .await?;

        let mut tx = pool.begin().await?;
        let archived = archive_approval_evidence_tx(
            &mut tx,
            &rustfs,
            ApprovalEvidenceInput {
                tenant_id,
                approval_id,
                actor_user_id: None,
                conversation_id: None,
                run_id: None,
                tool_call_id: None,
                status: "approved".to_string(),
                request_payload: json!({"tool_call_id": "tool-call-1", "risk": "critical"}),
                decision_payload: json!({"decision": "approved", "reason": "test"}),
                decided_at: None,
            },
        )
        .await?;
        tx.commit().await?;

        let object_reference_id = archived
            .object_reference_id
            .expect("expected approval evidence object reference");
        let object_key = archived
            .object_key
            .expect("expected approval evidence object");
        let evidence_bytes = rustfs
            .get_audit_object(&object_key)
            .await?
            .expect("expected approval evidence object content");
        let evidence: Value = serde_json::from_slice(&evidence_bytes)?;
        assert_eq!(
            evidence.get("schema").and_then(Value::as_str),
            Some("bibi-work.approval_evidence.v1")
        );
        assert_eq!(
            evidence.get("approval_id").and_then(Value::as_str),
            Some(approval_id.to_string().as_str())
        );
        assert_eq!(
            evidence
                .pointer("/decision_payload/decision")
                .and_then(Value::as_str),
            Some("approved")
        );

        let row = sqlx::query(
            r#"
            SELECT obj.bucket, obj.owner_resource_type, obj.owner_resource_id,
                   approvals.evidence_object_reference_id
            FROM object_references obj
            JOIN approvals ON approvals.evidence_object_reference_id = obj.id
            WHERE obj.id = $1
            "#,
        )
        .bind(object_reference_id)
        .fetch_one(&pool)
        .await?;
        let bucket: String = row.try_get("bucket")?;
        let owner_resource_type: String = row.try_get("owner_resource_type")?;
        let owner_resource_id: String = row.try_get("owner_resource_id")?;
        let approval_object_reference_id: Uuid = row.try_get("evidence_object_reference_id")?;
        assert_eq!(bucket, rustfs.audit_bucket());
        assert_eq!(owner_resource_type, "approval_evidence");
        assert_eq!(owner_resource_id, approval_id.to_string());
        assert_eq!(approval_object_reference_id, object_reference_id);

        rustfs.delete_audit_object(&object_key).await?;
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, RustFS, and the bibi_work schema"]
    async fn archive_tool_call_evidence_writes_audit_bucket_object()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let rustfs = rustfs_client_from_env()?;
        let tenant_id = Uuid::new_v4();
        let tenant_slug = format!("tool-call-evidence-rustfs-test-{tenant_id}");
        let tool_call_id = Uuid::new_v4();

        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, $2, $3)")
            .bind(tenant_id)
            .bind("Tool call evidence RustFS test")
            .bind(&tenant_slug)
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO tool_calls (
                id, tenant_id, tool_name, status, decision, policy_version,
                input_summary, output_summary, risk_level
            )
            VALUES ($1, $2, 'sql_query', 'completed', 'allow', 'local-policy-v1',
                    $3, $4, 'critical')
            "#,
        )
        .bind(tool_call_id)
        .bind(tenant_id)
        .bind("{\"query\":\"select <redacted>\"}")
        .bind("updated 1 row")
        .execute(&pool)
        .await?;

        let mut tx = pool.begin().await?;
        let archived = archive_tool_call_evidence_tx(
            &mut tx,
            &rustfs,
            ToolCallEvidenceInput {
                tenant_id,
                tool_call_id,
                actor_user_id: None,
                conversation_id: None,
                run_id: None,
                tool_name: "sql_query".to_string(),
                resource_type: "sql_query".to_string(),
                resource_id: "query-hash".to_string(),
                status: "completed".to_string(),
                decision: "allow".to_string(),
                policy_version: "local-policy-v1".to_string(),
                args_hash: Some("args-hash".to_string()),
                input_summary: Some("{\"query\":\"select <redacted>\"}".to_string()),
                output_summary: Some("updated 1 row".to_string()),
                error_summary: None,
                risk_level: Some("critical".to_string()),
                trace_id: Some("trace-1".to_string()),
                completed_at: None,
            },
        )
        .await?;
        tx.commit().await?;

        let object_reference_id = archived
            .object_reference_id
            .expect("expected tool call evidence object reference");
        let object_key = archived
            .object_key
            .expect("expected tool call evidence object");
        let evidence_bytes = rustfs
            .get_audit_object(&object_key)
            .await?
            .expect("expected tool call evidence object content");
        let evidence: Value = serde_json::from_slice(&evidence_bytes)?;
        assert_eq!(
            evidence.get("schema").and_then(Value::as_str),
            Some("bibi-work.tool_call_evidence.v1")
        );
        let tool_call_id_text = tool_call_id.to_string();
        assert_eq!(
            evidence.get("tool_call_id").and_then(Value::as_str),
            Some(tool_call_id_text.as_str())
        );
        assert_eq!(
            evidence.get("output_summary").and_then(Value::as_str),
            Some("updated 1 row")
        );

        let row = sqlx::query(
            r#"
            SELECT obj.bucket, obj.owner_resource_type, obj.owner_resource_id,
                   tool_calls.evidence_object_reference_id
            FROM object_references obj
            JOIN tool_calls ON tool_calls.evidence_object_reference_id = obj.id
            WHERE obj.id = $1
            "#,
        )
        .bind(object_reference_id)
        .fetch_one(&pool)
        .await?;
        let bucket: String = row.try_get("bucket")?;
        let owner_resource_type: String = row.try_get("owner_resource_type")?;
        let owner_resource_id: String = row.try_get("owner_resource_id")?;
        let evidence_object_reference_id: Uuid = row.try_get("evidence_object_reference_id")?;
        assert_eq!(bucket, rustfs.audit_bucket());
        assert_eq!(owner_resource_type, "tool_call_evidence");
        assert_eq!(owner_resource_id, tool_call_id.to_string());
        assert_eq!(evidence_object_reference_id, object_reference_id);

        rustfs.delete_audit_object(&object_key).await?;
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    async fn seed_platform_user(
        pool: &sqlx::PgPool,
        tenant_id: Uuid,
    ) -> Result<Uuid, Box<dyn std::error::Error>> {
        let user_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO platform_users (tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, 'audit-sealer')
            RETURNING id
            "#,
        )
        .bind(tenant_id)
        .bind(format!("audit-sealer-{tenant_id}"))
        .fetch_one(pool)
        .await?;
        Ok(user_id)
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

    fn rustfs_client_from_env() -> Result<RustFsClient, Box<dyn std::error::Error>> {
        Ok(RustFsClient::new(ObjectStoreSettings {
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
        })?)
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
