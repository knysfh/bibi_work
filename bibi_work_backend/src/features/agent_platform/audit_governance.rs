use serde::Serialize;
use serde_json::Value;
use sqlx::{AssertSqlSafe, PgPool, Row};
use std::time::Duration;
use time::OffsetDateTime;
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::features::{
    agent_platform::{
        audit::{sanitize_audit_evidence_value, sanitize_audit_text},
        models::CreateAuditLegalHoldRequest,
    },
    core::errors::AppError,
};
use crate::{configuration::AuditPartitionSettings, startup::AppState};

const PARTITION_CONTAINS_INELIGIBLE_SQL: &str = r#"
    SELECT EXISTS (
        SELECT 1
        FROM audit_logs candidate
        WHERE candidate.tableoid::BIGINT = $1
          AND NOT EXISTS (
              SELECT 1
              FROM audit_hash_chain_segments covering
              WHERE covering.tenant_id = candidate.tenant_id
                AND covering.archive_status = 'archived'
                AND covering.archive_verified_at IS NOT NULL
                AND covering.retention_until <= CURRENT_TIMESTAMP
                AND EXISTS (
                    SELECT 1
                    FROM jsonb_array_elements(covering.manifest->'audit_rows') item
                    WHERE (item->>'id')::UUID = candidate.id
                )
                AND NOT EXISTS (
                    SELECT 1
                    FROM audit_legal_holds hold
                    WHERE hold.tenant_id = covering.tenant_id
                      AND hold.status = 'active'
                      AND (
                          hold.scope_type = 'tenant'
                          OR (hold.scope_type = 'segment' AND hold.scope_id = covering.id::TEXT)
                          OR (
                              hold.scope_type = 'resource'
                              AND hold.resource_type = candidate.resource_type
                              AND hold.scope_id = candidate.resource_id
                          )
                      )
                )
          )
    )
"#;

#[derive(Debug, Serialize)]
pub struct AuditLegalHoldResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub scope_type: String,
    pub scope_id: Option<String>,
    pub resource_type: Option<String>,
    pub reason: String,
    pub status: String,
    pub created_by_user_id: Option<Uuid>,
    pub released_by_user_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub released_at: Option<OffsetDateTime>,
    pub metadata: Value,
}

#[derive(Debug, Serialize)]
pub struct AuditRetentionEligibilityResponse {
    pub tenant_id: Uuid,
    pub source_table_partitioned: bool,
    pub segments: Vec<AuditRetentionSegmentResponse>,
}

#[derive(Debug, Serialize)]
pub struct AuditRetentionSegmentResponse {
    pub segment_id: Uuid,
    pub partition_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub sealed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub retention_until: Option<OffsetDateTime>,
    pub archive_verified: bool,
    pub active_legal_hold_ids: Vec<Uuid>,
    pub retention_expired: bool,
    pub eligible_for_partition_cleanup: bool,
    pub blocking_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuditPartitionMaintenanceResponse {
    pub partitions_checked: usize,
    pub partitions_created: usize,
}

#[derive(Debug, Serialize)]
pub struct AuditPartitionCleanupResponse {
    pub partition_name: String,
    pub row_count: i64,
    pub eligible: bool,
    pub blocking_reason: Option<String>,
    pub dry_run: bool,
    pub dropped: bool,
}

pub fn spawn_audit_partition_maintenance_worker(state: AppState, settings: AuditPartitionSettings) {
    if !settings.maintenance_enabled {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(
            settings.worker_interval_milliseconds(),
        ));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            match ensure_audit_log_month_partitions(&state.connect_pool, settings.months_ahead())
                .await
            {
                Ok(summary) if summary.partitions_created > 0 => debug!(
                    partitions_created = summary.partitions_created,
                    "created future audit log partitions"
                ),
                Ok(_) => {}
                Err(error) => warn!("audit partition maintenance failed: {}", error),
            }
        }
    });
}

pub async fn ensure_audit_log_month_partitions(
    pool: &PgPool,
    months_ahead: i32,
) -> Result<AuditPartitionMaintenanceResponse, AppError> {
    let rows =
        sqlx::query("SELECT partition_name, created FROM ensure_audit_log_month_partitions($1)")
            .bind(months_ahead.clamp(1, 24))
            .fetch_all(pool)
            .await?;
    let partitions_created = rows
        .iter()
        .filter_map(|row| row.try_get::<bool, _>("created").ok())
        .filter(|created| *created)
        .count();
    Ok(AuditPartitionMaintenanceResponse {
        partitions_checked: rows.len(),
        partitions_created,
    })
}

pub async fn audit_partition_cleanup(
    pool: &PgPool,
    partition_name: &str,
    dry_run: bool,
    cleanup_enabled: bool,
) -> Result<AuditPartitionCleanupResponse, AppError> {
    let partition_name = validate_partition_name(partition_name)?;
    let status = audit_partition_cleanup_status(pool, partition_name, dry_run).await?;
    if dry_run || !status.eligible {
        return Ok(status);
    }
    if !cleanup_enabled {
        return Err(AppError::Conflict(
            "audit partition cleanup is disabled by server configuration".to_string(),
        ));
    }

    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('audit_log_partition_cleanup'))")
        .execute(&mut *tx)
        .await?;
    sqlx::query("LOCK TABLE audit_logs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    let partition_oid = partition_oid_tx(&mut tx, partition_name).await?;
    let old_enough: bool = sqlx::query_scalar(
        "SELECT to_date(substring($1 FROM 13 FOR 6), 'YYYYMM') + INTERVAL '1 month' <= date_trunc('month', CURRENT_TIMESTAMP)",
    )
    .bind(partition_name)
    .fetch_one(&mut *tx)
    .await?;
    if !old_enough {
        return Err(AppError::Conflict(
            "current or future audit partitions cannot be removed".to_string(),
        ));
    }
    let contains_ineligible: bool = sqlx::query_scalar(PARTITION_CONTAINS_INELIGIBLE_SQL)
        .bind(partition_oid)
        .fetch_one(&mut *tx)
        .await?;
    if contains_ineligible {
        return Err(AppError::Conflict(
            "audit partition eligibility changed before cleanup".to_string(),
        ));
    }
    sqlx::query(AssertSqlSafe(format!(
        "ALTER TABLE audit_logs DETACH PARTITION {partition_name}"
    )))
    .execute(&mut *tx)
    .await?;
    sqlx::query(AssertSqlSafe(format!("DROP TABLE {partition_name}")))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(AuditPartitionCleanupResponse {
        dropped: true,
        dry_run: false,
        ..status
    })
}

async fn audit_partition_cleanup_status(
    pool: &PgPool,
    partition_name: &str,
    dry_run: bool,
) -> Result<AuditPartitionCleanupResponse, AppError> {
    let partition_oid: i64 = sqlx::query_scalar(
        r#"
        SELECT child.oid::BIGINT
        FROM pg_class child
        JOIN pg_namespace namespace ON namespace.oid = child.relnamespace
        JOIN pg_inherits inheritance ON inheritance.inhrelid = child.oid
        WHERE namespace.nspname = 'public'
          AND child.relname = $1
          AND inheritance.inhparent = 'audit_logs'::regclass
        "#,
    )
    .bind(partition_name)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("audit partition not found".to_string()))?;
    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_logs WHERE tableoid::BIGINT = $1")
            .bind(partition_oid)
            .fetch_one(pool)
            .await?;
    let old_enough: bool = sqlx::query_scalar(
        "SELECT to_date(substring($1 FROM 13 FOR 6), 'YYYYMM') + INTERVAL '1 month' <= date_trunc('month', CURRENT_TIMESTAMP)",
    )
    .bind(partition_name)
    .fetch_one(pool)
    .await?;
    let contains_ineligible: bool = sqlx::query_scalar(PARTITION_CONTAINS_INELIGIBLE_SQL)
        .bind(partition_oid)
        .fetch_one(pool)
        .await?;
    let blocking_reason = if !old_enough {
        Some("current_or_future_partition".to_string())
    } else if contains_ineligible {
        Some("partition_contains_ineligible_rows".to_string())
    } else {
        None
    };
    Ok(AuditPartitionCleanupResponse {
        partition_name: partition_name.to_string(),
        row_count,
        eligible: blocking_reason.is_none(),
        blocking_reason,
        dry_run,
        dropped: false,
    })
}

async fn partition_oid_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    partition_name: &str,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        r#"
        SELECT child.oid::BIGINT
        FROM pg_class child
        JOIN pg_namespace namespace ON namespace.oid = child.relnamespace
        JOIN pg_inherits inheritance ON inheritance.inhrelid = child.oid
        WHERE namespace.nspname = 'public' AND child.relname = $1
          AND inheritance.inhparent = 'audit_logs'::regclass
        "#,
    )
    .bind(partition_name)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::NotFound("audit partition not found".to_string()))
}

fn validate_partition_name(value: &str) -> Result<&str, AppError> {
    let valid = value.len() == 18
        && value.starts_with("audit_logs_p")
        && value[12..].bytes().all(|byte| byte.is_ascii_digit());
    if !valid {
        return Err(AppError::InvalidInput(
            "partition_name must match audit_logs_pYYYYMM".to_string(),
        ));
    }
    Ok(value)
}

pub async fn list_audit_legal_holds(
    pool: &PgPool,
    tenant_id: Uuid,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<AuditLegalHoldResponse>, AppError> {
    let status = normalize_hold_status(status)?;
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, scope_type, scope_id, resource_type, reason, status,
               created_by_user_id, released_by_user_id, created_at, released_at, metadata
        FROM audit_legal_holds
        WHERE tenant_id = $1
          AND ($2::TEXT IS NULL OR status = $2)
        ORDER BY created_at DESC, id DESC
        LIMIT $3
        "#,
    )
    .bind(tenant_id)
    .bind(status)
    .bind(limit.clamp(1, 1_000))
    .fetch_all(pool)
    .await?;
    rows.into_iter().map(legal_hold_from_row).collect()
}

pub async fn create_audit_legal_hold(
    pool: &PgPool,
    actor_user_id: Uuid,
    payload: CreateAuditLegalHoldRequest,
) -> Result<AuditLegalHoldResponse, AppError> {
    let scope_type = normalize_scope_type(&payload.scope_type)?;
    let reason = sanitize_audit_text(&required_bounded_text(&payload.reason, "reason", 2_000)?);
    let scope_id = normalize_optional_text(payload.scope_id.as_deref(), 512)?;
    let resource_type = normalize_optional_text(payload.resource_type.as_deref(), 128)?;
    validate_hold_scope(scope_type, scope_id.as_deref(), resource_type.as_deref())?;
    if scope_type == "segment" {
        let segment_id =
            Uuid::parse_str(scope_id.as_deref().unwrap_or_default()).map_err(|_| {
                AppError::InvalidInput("segment legal hold scope_id must be a UUID".to_string())
            })?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM audit_hash_chain_segments WHERE id = $1 AND tenant_id = $2)",
        )
        .bind(segment_id)
        .bind(payload.tenant_id)
        .fetch_one(pool)
        .await?;
        if !exists {
            return Err(AppError::NotFound("audit segment not found".to_string()));
        }
    }

    let row = sqlx::query(
        r#"
        INSERT INTO audit_legal_holds (
            tenant_id, scope_type, scope_id, resource_type, reason,
            created_by_user_id, metadata
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, tenant_id, scope_type, scope_id, resource_type, reason, status,
                  created_by_user_id, released_by_user_id, created_at, released_at, metadata
        "#,
    )
    .bind(payload.tenant_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(resource_type)
    .bind(reason)
    .bind(actor_user_id)
    .bind(sanitize_audit_evidence_value(
        &payload
            .metadata
            .unwrap_or_else(|| Value::Object(Default::default())),
    ))
    .fetch_one(pool)
    .await
    .map_err(|error| {
        if error
            .as_database_error()
            .and_then(|database_error| database_error.constraint())
            == Some("idx_audit_legal_holds_active_scope")
        {
            AppError::Conflict("an active legal hold already exists for this scope".to_string())
        } else {
            AppError::from(error)
        }
    })?;
    legal_hold_from_row(row)
}

pub async fn release_audit_legal_hold(
    pool: &PgPool,
    tenant_id: Uuid,
    hold_id: Uuid,
    actor_user_id: Uuid,
) -> Result<AuditLegalHoldResponse, AppError> {
    let row = sqlx::query(
        r#"
        UPDATE audit_legal_holds
        SET status = 'released',
            released_by_user_id = $3,
            released_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND status = 'active'
        RETURNING id, tenant_id, scope_type, scope_id, resource_type, reason, status,
                  created_by_user_id, released_by_user_id, created_at, released_at, metadata
        "#,
    )
    .bind(hold_id)
    .bind(tenant_id)
    .bind(actor_user_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound("active audit legal hold not found".to_string()))?;
    legal_hold_from_row(row)
}

pub async fn audit_retention_eligibility(
    pool: &PgPool,
    tenant_id: Uuid,
    limit: i64,
) -> Result<AuditRetentionEligibilityResponse, AppError> {
    let source_table_partitioned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_partitioned_table WHERE partrelid = 'audit_logs'::regclass)",
    )
    .fetch_one(pool)
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT segment.id, segment.sealed_at, segment.retention_until,
               segment.archive_verified_at IS NOT NULL AS archive_verified,
               first_audit.tableoid::BIGINT AS first_partition_oid,
               last_audit.tableoid::BIGINT AS last_partition_oid,
               first_audit.tableoid::regclass::TEXT AS first_partition_name,
               COALESCE(ARRAY(
                   SELECT hold.id
                   FROM audit_legal_holds hold
                   WHERE hold.tenant_id = segment.tenant_id
                     AND hold.status = 'active'
                     AND (
                         hold.scope_type = 'tenant'
                         OR (hold.scope_type = 'segment' AND hold.scope_id = segment.id::TEXT)
                         OR (
                             hold.scope_type = 'resource'
                             AND EXISTS (
                                 SELECT 1
                                 FROM jsonb_array_elements(segment.manifest->'audit_rows') item
                                 JOIN audit_logs audit ON audit.id = (item->>'id')::UUID
                                 WHERE audit.resource_type = hold.resource_type
                                   AND audit.resource_id = hold.scope_id
                             )
                         )
                     )
                   ORDER BY hold.created_at, hold.id
               ), ARRAY[]::UUID[]) AS hold_ids
        FROM audit_hash_chain_segments segment
        LEFT JOIN audit_logs first_audit ON first_audit.id = segment.first_audit_log_id
        LEFT JOIN audit_logs last_audit ON last_audit.id = segment.last_audit_log_id
        WHERE segment.tenant_id = $1
          AND segment.archive_status = 'archived'
        ORDER BY segment.retention_until, segment.id
        LIMIT $2
        "#,
    )
    .bind(tenant_id)
    .bind(limit.clamp(1, 1_000))
    .fetch_all(pool)
    .await?;
    let now = OffsetDateTime::now_utc();
    let mut segments = Vec::with_capacity(rows.len());
    for row in rows {
        let retention_until: Option<OffsetDateTime> = row.try_get("retention_until")?;
        let archive_verified: bool = row.try_get("archive_verified")?;
        let active_legal_hold_ids: Vec<Uuid> = row.try_get("hold_ids")?;
        let first_partition_oid: Option<i64> = row.try_get("first_partition_oid")?;
        let last_partition_oid: Option<i64> = row.try_get("last_partition_oid")?;
        let partition_name: Option<String> = row.try_get("first_partition_name")?;
        let retention_expired = retention_until.is_some_and(|deadline| deadline <= now);
        let mut blocking_reason = if !archive_verified {
            Some("archive_not_verified".to_string())
        } else if !active_legal_hold_ids.is_empty() {
            Some("active_legal_hold".to_string())
        } else if !retention_expired {
            Some("retention_not_expired".to_string())
        } else if !source_table_partitioned {
            Some("audit_logs_not_partitioned".to_string())
        } else if first_partition_oid.is_none() || last_partition_oid.is_none() {
            Some("audit_partition_row_missing".to_string())
        } else if first_partition_oid != last_partition_oid {
            Some("segment_spans_partitions".to_string())
        } else {
            None
        };
        if blocking_reason.is_none() {
            blocking_reason = audit_partition_blocking_reason(
                pool,
                first_partition_oid.expect("checked partition oid"),
                partition_name.as_deref(),
            )
            .await?;
        }
        let eligible = blocking_reason.is_none();
        segments.push(AuditRetentionSegmentResponse {
            segment_id: row.try_get("id")?,
            partition_name,
            sealed_at: row.try_get("sealed_at")?,
            retention_until,
            archive_verified,
            active_legal_hold_ids,
            retention_expired,
            eligible_for_partition_cleanup: eligible,
            blocking_reason,
        });
    }
    Ok(AuditRetentionEligibilityResponse {
        tenant_id,
        source_table_partitioned,
        segments,
    })
}

async fn audit_partition_blocking_reason(
    pool: &PgPool,
    partition_oid: i64,
    partition_name: Option<&str>,
) -> Result<Option<String>, AppError> {
    if partition_name == Some("audit_logs_default") {
        return Ok(Some("default_partition_not_detachable".to_string()));
    }
    let contains_ineligible_rows: bool = sqlx::query_scalar(PARTITION_CONTAINS_INELIGIBLE_SQL)
        .bind(partition_oid)
        .fetch_one(pool)
        .await?;
    Ok(contains_ineligible_rows.then(|| "partition_contains_ineligible_rows".to_string()))
}

fn legal_hold_from_row(row: sqlx::postgres::PgRow) -> Result<AuditLegalHoldResponse, AppError> {
    Ok(AuditLegalHoldResponse {
        id: row.try_get("id")?,
        tenant_id: row.try_get("tenant_id")?,
        scope_type: row.try_get("scope_type")?,
        scope_id: row.try_get("scope_id")?,
        resource_type: row.try_get("resource_type")?,
        reason: row.try_get("reason")?,
        status: row.try_get("status")?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        released_by_user_id: row.try_get("released_by_user_id")?,
        created_at: row.try_get("created_at")?,
        released_at: row.try_get("released_at")?,
        metadata: row.try_get("metadata")?,
    })
}

fn normalize_scope_type(value: &str) -> Result<&'static str, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "tenant" => Ok("tenant"),
        "segment" => Ok("segment"),
        "resource" => Ok("resource"),
        _ => Err(AppError::InvalidInput(
            "scope_type must be tenant, segment, or resource".to_string(),
        )),
    }
}

fn normalize_hold_status(value: Option<&str>) -> Result<Option<&'static str>, AppError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(value) if value.eq_ignore_ascii_case("active") => Ok(Some("active")),
        Some(value) if value.eq_ignore_ascii_case("released") => Ok(Some("released")),
        Some(_) => Err(AppError::InvalidInput(
            "status must be active or released".to_string(),
        )),
    }
}

fn validate_hold_scope(
    scope_type: &str,
    scope_id: Option<&str>,
    resource_type: Option<&str>,
) -> Result<(), AppError> {
    let valid = match scope_type {
        "tenant" => scope_id.is_none() && resource_type.is_none(),
        "segment" => scope_id.is_some() && resource_type.is_none(),
        "resource" => scope_id.is_some() && resource_type.is_some(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidInput(
            "legal hold scope fields do not match scope_type".to_string(),
        ))
    }
}

fn normalize_optional_text(
    value: Option<&str>,
    max_chars: usize,
) -> Result<Option<String>, AppError> {
    value
        .map(|value| required_bounded_text(value, "scope field", max_chars))
        .transpose()
}

fn required_bounded_text(value: &str, field: &str, max_chars: usize) -> Result<String, AppError> {
    let normalized = value.trim();
    if normalized.is_empty() || normalized.chars().count() > max_chars {
        return Err(AppError::InvalidInput(format!(
            "{field} must contain between 1 and {max_chars} characters"
        )));
    }
    Ok(normalized.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use crate::features::{
        agent_platform::models::CreateAuditLegalHoldRequest, core::errors::AppError,
    };

    use super::{
        audit_partition_cleanup, audit_retention_eligibility, create_audit_legal_hold,
        ensure_audit_log_month_partitions, normalize_scope_type, release_audit_legal_hold,
        validate_hold_scope, validate_partition_name,
    };

    #[test]
    fn legal_hold_scope_validation_is_fail_closed() {
        assert_eq!(normalize_scope_type(" TENANT ").unwrap(), "tenant");
        assert!(validate_hold_scope("tenant", None, None).is_ok());
        assert!(validate_hold_scope("segment", Some("id"), None).is_ok());
        assert!(validate_hold_scope("resource", Some("id"), Some("file")).is_ok());
        assert!(validate_hold_scope("tenant", Some("id"), None).is_err());
        assert!(normalize_scope_type("unknown").is_err());
        assert!(validate_partition_name("audit_logs_p200001").is_ok());
        assert!(validate_partition_name("audit_logs_default").is_err());
        assert!(validate_partition_name("audit_logs_p200001;drop").is_err());
    }

    #[tokio::test]
    #[ignore = "requires disposable local Postgres and the bibi_work schema"]
    async fn partition_maintenance_and_cleanup_detach_only_fully_eligible_old_partition()
    -> Result<(), Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let maintenance = ensure_audit_log_month_partitions(&pool, 3).await?;
        assert_eq!(maintenance.partitions_checked, 4);

        sqlx::query(
            "CREATE TABLE audit_logs_p200001 PARTITION OF audit_logs FOR VALUES FROM ('2000-01-01 00:00:00+00') TO ('2000-02-01 00:00:00+00')",
        )
        .execute(&pool)
        .await?;
        let tenant_id = Uuid::new_v4();
        let audit_id = Uuid::new_v4();
        let segment_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tenants (id, name, slug) VALUES ($1, 'Partition cleanup test', $2)",
        )
        .bind(tenant_id)
        .bind(format!("audit-partition-cleanup-{tenant_id}"))
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, tenant_id, resource_type, resource_id, action, decision,
                policy_version, row_hash, created_at
            ) VALUES ($1, $2, 'file', 'old-file', 'read', 'allow', 'test-v1', $3,
                      '2000-01-15 00:00:00+00')
            "#,
        )
        .bind(audit_id)
        .bind(tenant_id)
        .bind("a".repeat(64))
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_hash_chain_segments (
                id, tenant_id, first_audit_log_id, last_audit_log_id, rows_count,
                last_row_hash, manifest_hash, manifest, sealed_at, archive_status,
                archived_at, archive_verified_at, retention_until
            ) VALUES ($1, $2, $3, $3, 1, $4, $5, $6,
                      '2000-01-16 00:00:00+00', 'archived',
                      '2000-01-16 00:00:00+00', '2000-01-16 00:00:00+00',
                      '2000-01-17 00:00:00+00')
            "#,
        )
        .bind(segment_id)
        .bind(tenant_id)
        .bind(audit_id)
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .bind(json!({"audit_rows": [{"id": audit_id}]}))
        .execute(&pool)
        .await?;

        let hold_id: Uuid = sqlx::query_scalar(
            "INSERT INTO audit_legal_holds (tenant_id, scope_type, reason) VALUES ($1, 'tenant', 'cleanup test hold') RETURNING id",
        )
        .bind(tenant_id)
        .fetch_one(&pool)
        .await?;
        let held = audit_partition_cleanup(&pool, "audit_logs_p200001", true, false).await?;
        assert_eq!(
            held.blocking_reason.as_deref(),
            Some("partition_contains_ineligible_rows")
        );
        sqlx::query("UPDATE audit_legal_holds SET status = 'released', released_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(hold_id)
            .execute(&pool)
            .await?;
        let dry_run = audit_partition_cleanup(&pool, "audit_logs_p200001", true, false).await?;
        assert!(dry_run.eligible);
        assert!(!dry_run.dropped);
        assert!(matches!(
            audit_partition_cleanup(&pool, "audit_logs_p200001", false, false).await,
            Err(AppError::Conflict(_))
        ));
        let executed = audit_partition_cleanup(&pool, "audit_logs_p200001", false, true).await?;
        assert!(executed.dropped);
        let still_attached: bool =
            sqlx::query_scalar("SELECT to_regclass('public.audit_logs_p200001') IS NOT NULL")
                .fetch_one(&pool)
                .await?;
        assert!(!still_attached);
        let identity_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log_identities WHERE id = $1)")
                .bind(audit_id)
                .fetch_one(&pool)
                .await?;
        assert!(identity_exists);
        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn legal_hold_blocks_expired_segment_cleanup_until_released()
    -> Result<(), Box<dyn std::error::Error>> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:password@127.0.0.1:5433/bibi_work".to_string()
        });
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let audit_id = Uuid::new_v4();
        let segment_id = Uuid::new_v4();
        sqlx::query("INSERT INTO tenants (id, name, slug) VALUES ($1, 'Legal hold test', $2)")
            .bind(tenant_id)
            .bind(format!("audit-legal-hold-test-{tenant_id}"))
            .execute(&pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO platform_users (id, tenant_id, ferriskey_subject, username)
            VALUES ($1, $2, $3, 'hold-admin')
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .bind(format!("hold-admin-{user_id}"))
        .execute(&pool)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, tenant_id, resource_type, resource_id, action, decision,
                policy_version, row_hash
            ) VALUES ($1, $2, 'file', 'file-1', 'read', 'allow', 'test-v1', $3)
            "#,
        )
        .bind(audit_id)
        .bind(tenant_id)
        .bind("a".repeat(64))
        .execute(&pool)
        .await?;
        let sealed_at = OffsetDateTime::now_utc() - Duration::days(10);
        sqlx::query(
            r#"
            INSERT INTO audit_hash_chain_segments (
                id, tenant_id, first_audit_log_id, last_audit_log_id, rows_count,
                last_row_hash, manifest_hash, manifest, sealed_at, archive_status,
                archived_at, archive_verified_at, retention_until
            ) VALUES ($1, $2, $3, $3, 1, $4, $5, $6, $7, 'archived', $7, $7, $8)
            "#,
        )
        .bind(segment_id)
        .bind(tenant_id)
        .bind(audit_id)
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .bind(json!({"audit_rows": [{"id": audit_id}]}))
        .bind(sealed_at)
        .bind(OffsetDateTime::now_utc() - Duration::days(1))
        .execute(&pool)
        .await?;

        let hold = create_audit_legal_hold(
            &pool,
            user_id,
            CreateAuditLegalHoldRequest {
                tenant_id,
                scope_type: "tenant".to_string(),
                scope_id: None,
                resource_type: None,
                reason: "litigation preservation".to_string(),
                metadata: Some(json!({"authorization": "Bearer raw-secret"})),
            },
        )
        .await?;
        assert_eq!(hold.metadata["authorization"], json!("[REDACTED]"));
        let held = audit_retention_eligibility(&pool, tenant_id, 10).await?;
        assert_eq!(held.segments.len(), 1);
        assert_eq!(held.segments[0].active_legal_hold_ids, vec![hold.id]);
        assert_eq!(
            held.segments[0].blocking_reason.as_deref(),
            Some("active_legal_hold")
        );
        assert!(!held.segments[0].eligible_for_partition_cleanup);

        release_audit_legal_hold(&pool, tenant_id, hold.id, user_id).await?;
        let released = audit_retention_eligibility(&pool, tenant_id, 10).await?;
        assert!(released.segments[0].active_legal_hold_ids.is_empty());
        assert_ne!(
            released.segments[0].blocking_reason.as_deref(),
            Some("active_legal_hold")
        );
        if released.source_table_partitioned {
            assert!(released.segments[0].partition_name.is_some());
        } else {
            assert_eq!(
                released.segments[0].blocking_reason.as_deref(),
                Some("audit_logs_not_partitioned")
            );
        }

        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }
}
