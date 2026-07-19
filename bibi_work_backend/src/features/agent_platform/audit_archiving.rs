use std::time::Duration;

use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use time::{Duration as TimeDuration, OffsetDateTime};
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    configuration::AuditArchiveSettings, features::core::errors::AppError, startup::AppState,
};

use super::{
    audit::{AUDIT_SEGMENT_CONTENT_TYPE, audit_segment_object_key, sha256_hex},
    rustfs::RustFsClient,
};

const ARCHIVE_ERROR_MAX_CHARS: usize = 1_024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditArchiveRunSummary {
    pub candidates: usize,
    pub archived: usize,
    pub failed: usize,
}

#[derive(Debug)]
struct AuditArchiveCandidate {
    id: Uuid,
    tenant_id: Uuid,
    manifest_hash: String,
    manifest: Value,
    object_reference_id: Option<Uuid>,
    object_key: Option<String>,
    object_content_hash: Option<String>,
    sealed_at: OffsetDateTime,
}

pub fn spawn_audit_archive_worker(state: AppState, settings: AuditArchiveSettings) {
    if !settings.enabled {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(
            settings.worker_interval_milliseconds(),
        ));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match process_pending_audit_archives(&state, &settings).await {
                Ok(summary) if summary.candidates > 0 => debug!(
                    candidates = summary.candidates,
                    archived = summary.archived,
                    failed = summary.failed,
                    "audit archive batch completed"
                ),
                Ok(_) => {}
                Err(err) => warn!("audit archive worker failed: {}", err),
            }
        }
    });
}

pub async fn process_pending_audit_archives(
    state: &AppState,
    settings: &AuditArchiveSettings,
) -> Result<AuditArchiveRunSummary, AppError> {
    process_pending_audit_archives_for_scope(
        &state.connect_pool,
        &state.rustfs_client,
        settings,
        None,
    )
    .await
}

pub async fn process_pending_audit_archives_for_tenant(
    pool: &PgPool,
    rustfs_client: &RustFsClient,
    settings: &AuditArchiveSettings,
    tenant_id: Uuid,
) -> Result<AuditArchiveRunSummary, AppError> {
    process_pending_audit_archives_for_scope(pool, rustfs_client, settings, Some(tenant_id)).await
}

async fn process_pending_audit_archives_for_scope(
    pool: &PgPool,
    rustfs_client: &RustFsClient,
    settings: &AuditArchiveSettings,
    tenant_id: Option<Uuid>,
) -> Result<AuditArchiveRunSummary, AppError> {
    let candidates = load_archive_candidates(pool, settings, tenant_id).await?;
    let mut summary = AuditArchiveRunSummary {
        candidates: candidates.len(),
        ..AuditArchiveRunSummary::default()
    };
    for candidate in candidates {
        match archive_segment(pool, rustfs_client, settings, &candidate).await {
            Ok(()) => summary.archived += 1,
            Err(err) => {
                summary.failed += 1;
                record_archive_failure(pool, candidate.id, settings, &err).await?;
            }
        }
    }
    Ok(summary)
}

async fn load_archive_candidates(
    pool: &PgPool,
    settings: &AuditArchiveSettings,
    tenant_id: Option<Uuid>,
) -> Result<Vec<AuditArchiveCandidate>, AppError> {
    let rows = sqlx::query(
        r#"
        WITH candidates AS (
            SELECT segment.id
            FROM audit_hash_chain_segments segment
            WHERE (
                    segment.archive_status IN ('pending', 'failed')
                    OR (
                        segment.archive_status = 'archiving'
                        AND segment.archive_started_at < CURRENT_TIMESTAMP - INTERVAL '15 minutes'
                    )
                  )
              AND segment.archive_attempts < $1
              AND segment.sealed_at <= CURRENT_TIMESTAMP - ($2 * INTERVAL '1 day')
              AND ($4::UUID IS NULL OR segment.tenant_id = $4)
            ORDER BY segment.sealed_at, segment.id
            LIMIT $3
            FOR UPDATE SKIP LOCKED
        ), claimed AS (
            UPDATE audit_hash_chain_segments segment
            SET archive_status = 'archiving',
                archive_started_at = CURRENT_TIMESTAMP,
                archive_error = NULL
            FROM candidates
            WHERE segment.id = candidates.id
            RETURNING segment.*
        )
        SELECT claimed.id, claimed.tenant_id, claimed.manifest_hash, claimed.manifest,
               claimed.object_reference_id, claimed.sealed_at,
               object_ref.object_key, object_ref.content_hash AS object_content_hash
        FROM claimed
        LEFT JOIN object_references object_ref ON object_ref.id = claimed.object_reference_id
        ORDER BY claimed.sealed_at, claimed.id
        "#,
    )
    .bind(settings.max_attempts())
    .bind(settings.minimum_age_days())
    .bind(settings.segment_batch_size())
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(AuditArchiveCandidate {
                id: row.try_get("id")?,
                tenant_id: row.try_get("tenant_id")?,
                manifest_hash: row.try_get("manifest_hash")?,
                manifest: row.try_get("manifest")?,
                object_reference_id: row.try_get("object_reference_id")?,
                object_key: row.try_get("object_key")?,
                object_content_hash: row.try_get("object_content_hash")?,
                sealed_at: row.try_get("sealed_at")?,
            })
        })
        .collect()
}

async fn archive_segment(
    pool: &PgPool,
    rustfs_client: &RustFsClient,
    settings: &AuditArchiveSettings,
    candidate: &AuditArchiveCandidate,
) -> Result<(), AppError> {
    let evidence = json!({
        "manifest_hash": candidate.manifest_hash,
        "manifest": candidate.manifest,
    });
    let evidence_bytes = serde_json::to_vec(&evidence).map_err(|_| {
        AppError::InvalidInput("failed to encode audit archive evidence".to_string())
    })?;
    let content_hash = sha256_hex(&evidence_bytes);
    let object_key =
        audit_segment_object_key(candidate.tenant_id, candidate.id, candidate.sealed_at);

    let object_reference_id = if let Some(existing_id) = candidate.object_reference_id {
        let existing_key = candidate.object_key.as_deref().ok_or_else(|| {
            AppError::ObjectStore("audit archive object reference has no object key".to_string())
        })?;
        let stored = rustfs_client
            .get_audit_object(existing_key)
            .await?
            .ok_or_else(|| {
                AppError::ObjectStore("audit archive object is unavailable".to_string())
            })?;
        let stored_hash = sha256_hex(&stored);
        if candidate.object_content_hash.as_deref() != Some(stored_hash.as_str())
            || stored_hash != content_hash
        {
            return Err(AppError::ObjectStore(
                "audit archive object hash does not match sealed manifest".to_string(),
            ));
        }
        existing_id
    } else {
        let object = rustfs_client
            .put_audit_object(&object_key, evidence_bytes)
            .await?
            .ok_or_else(|| {
                AppError::ObjectStore("audit archive requires enabled object storage".to_string())
            })?;
        let stored = rustfs_client
            .get_audit_object(&object.object_key)
            .await?
            .ok_or_else(|| {
                AppError::ObjectStore("new audit archive object is unavailable".to_string())
            })?;
        if sha256_hex(&stored) != content_hash {
            return Err(AppError::ObjectStore(
                "new audit archive object failed integrity verification".to_string(),
            ));
        }
        let size_bytes = i64::try_from(stored.len())?;
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
        .bind(candidate.tenant_id)
        .bind(object.bucket)
        .bind(object.object_key)
        .bind(object.version_id)
        .bind(object.etag)
        .bind(&content_hash)
        .bind(size_bytes)
        .bind(AUDIT_SEGMENT_CONTENT_TYPE)
        .bind(candidate.id.to_string())
        .fetch_one(pool)
        .await?
    };

    let now = OffsetDateTime::now_utc();
    let retention_until = candidate.sealed_at + TimeDuration::days(settings.retention_days());
    sqlx::query(
        r#"
        UPDATE audit_hash_chain_segments
        SET object_reference_id = $2,
            archive_status = 'archived',
            archive_attempts = archive_attempts + 1,
            archived_at = COALESCE(archived_at, $3),
            archive_verified_at = $3,
            retention_until = $4,
            archive_error = NULL
        WHERE id = $1
          AND archive_status = 'archiving'
        "#,
    )
    .bind(candidate.id)
    .bind(object_reference_id)
    .bind(now)
    .bind(retention_until)
    .execute(pool)
    .await?;
    Ok(())
}

async fn record_archive_failure(
    pool: &PgPool,
    segment_id: Uuid,
    settings: &AuditArchiveSettings,
    error: &AppError,
) -> Result<(), AppError> {
    let message = truncate_error(&error.to_string());
    sqlx::query(
        r#"
        UPDATE audit_hash_chain_segments
        SET archive_status = 'failed',
            archive_attempts = LEAST(archive_attempts + 1, $2),
            archive_started_at = NULL,
            archive_error = $3
        WHERE id = $1
          AND archive_status = 'archiving'
        "#,
    )
    .bind(segment_id)
    .bind(settings.max_attempts())
    .bind(message)
    .execute(pool)
    .await?;
    Ok(())
}

fn truncate_error(input: &str) -> String {
    let mut output = input
        .chars()
        .take(ARCHIVE_ERROR_MAX_CHARS)
        .collect::<String>();
    if input.chars().count() > ARCHIVE_ERROR_MAX_CHARS {
        output.push_str("...[TRUNCATED]");
    }
    output
}

#[cfg(test)]
mod tests {
    use secrecy::SecretBox;
    use sqlx::{Row, postgres::PgPoolOptions};
    use uuid::Uuid;

    use crate::{
        configuration::{AuditArchiveSettings, ObjectStoreSettings},
        features::agent_platform::{
            audit::{NewAuditLog, insert_audit_log_tx, seal_audit_hash_chain},
            rustfs::RustFsClient,
        },
    };

    use super::{process_pending_audit_archives_for_tenant, truncate_error};

    #[test]
    fn archive_errors_are_bounded() {
        let error = truncate_error(&"x".repeat(2_000));
        assert!(error.ends_with("...[TRUNCATED]"));
        assert!(error.len() < 1_100);
    }

    #[tokio::test]
    #[ignore = "requires local Postgres, RustFS, and the bibi_work schema"]
    async fn archives_and_verifies_a_sealed_segment() -> Result<(), Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let tenant_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, $2, 'Audit archive test')")
            .bind(tenant_id)
            .bind(format!("audit-archive-test-{tenant_id}"))
            .execute(&pool)
            .await?;

        let mut tx = pool.begin().await?;
        insert_audit_log_tx(
            &mut tx,
            NewAuditLog {
                tenant_id,
                actor_user_id: None,
                actor_device_id: None,
                session_id: None,
                resource_type: "archive-test",
                resource_id: "fixture",
                action: "verify",
                decision: "allow",
                policy_version: "test-v1",
                reason_code: None,
                run_id: None,
                conversation_id: None,
                workflow_run_id: None,
                tool_call_id: None,
                approval_id: None,
                args_hash: None,
                input_summary: Some("api_key=must-not-leak"),
                output_summary: Some("ok"),
                risk_level: Some("high"),
                ip: None,
                user_agent: None,
                trace_id: Some("archive-test"),
            },
        )
        .await?;
        tx.commit().await?;
        let stored_summary: String = sqlx::query_scalar(
            "SELECT input_summary FROM audit_logs WHERE tenant_id = $1 ORDER BY created_at DESC LIMIT 1",
        )
        .bind(tenant_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(stored_summary, "api_key=[REDACTED]");

        let rustfs = RustFsClient::new(ObjectStoreSettings {
            enabled: true,
            endpoint: std::env::var("RUSTFS_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:9004".to_string()),
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
            timeout_milliseconds: 5_000,
        })?;
        let sealed = seal_audit_hash_chain(&pool, &rustfs, tenant_id, None, 100).await?;
        let settings = AuditArchiveSettings {
            enabled: true,
            worker_interval_milliseconds: 1_000,
            segment_batch_size: 10,
            minimum_age_days: 0,
            retention_days: 2_555,
            max_attempts: 3,
        };

        let summary =
            process_pending_audit_archives_for_tenant(&pool, &rustfs, &settings, tenant_id).await?;
        assert_eq!(summary.archived, 1);
        assert_eq!(summary.failed, 0);
        let row = sqlx::query(
            r#"
            SELECT archive_status, archive_verified_at, retention_until, archive_error
            FROM audit_hash_chain_segments WHERE id = $1
            "#,
        )
        .bind(sealed.segment_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.try_get::<String, _>("archive_status")?, "archived");
        assert!(
            row.try_get::<Option<time::OffsetDateTime>, _>("archive_verified_at")?
                .is_some()
        );
        assert!(
            row.try_get::<Option<time::OffsetDateTime>, _>("retention_until")?
                .is_some()
        );
        assert!(row.try_get::<Option<String>, _>("archive_error")?.is_none());

        if let Some(object_key) = sealed.object_key {
            rustfs.delete_audit_object(&object_key).await?;
        }
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::from(value.to_string().into_boxed_str())
    }
}
