use std::time::Duration;

use sqlx::PgPool;
use tokio::time::MissedTickBehavior;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    configuration::AuditHashChainSettings, features::core::errors::AppError, startup::AppState,
};

use super::{
    audit::{NO_UNSEALED_AUDIT_ROWS_MESSAGE, seal_audit_hash_chain},
    rustfs::RustFsClient,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditHashChainSealingRunSummary {
    pub tenants_scanned: usize,
    pub segments_sealed: usize,
    pub failed_tenants: usize,
}

pub fn spawn_audit_hash_chain_sealing_worker(state: AppState, settings: AuditHashChainSettings) {
    if !settings.auto_seal_enabled {
        return;
    }

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(
            settings.worker_interval_milliseconds(),
        ));
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            interval.tick().await;
            match process_pending_audit_hash_chain_seals(
                &state.connect_pool,
                &state.rustfs_client,
                &settings,
            )
            .await
            {
                Ok(summary) if summary.segments_sealed > 0 || summary.failed_tenants > 0 => {
                    debug!(
                        tenants_scanned = summary.tenants_scanned,
                        segments_sealed = summary.segments_sealed,
                        failed_tenants = summary.failed_tenants,
                        "background audit hash-chain sealing completed"
                    );
                }
                Ok(_) => {}
                Err(err) => warn!("background audit hash-chain sealing failed: {}", err),
            }
        }
    });
}

pub async fn process_pending_audit_hash_chain_seals(
    pool: &PgPool,
    rustfs_client: &RustFsClient,
    settings: &AuditHashChainSettings,
) -> Result<AuditHashChainSealingRunSummary, AppError> {
    let tenant_ids =
        find_tenants_with_unsealed_hash_chain_rows(pool, settings.worker_tenant_batch_size())
            .await?;
    process_audit_hash_chain_seals_for_tenants(pool, rustfs_client, settings, tenant_ids).await
}

async fn process_audit_hash_chain_seals_for_tenants(
    pool: &PgPool,
    rustfs_client: &RustFsClient,
    settings: &AuditHashChainSettings,
    tenant_ids: Vec<Uuid>,
) -> Result<AuditHashChainSealingRunSummary, AppError> {
    let mut summary = AuditHashChainSealingRunSummary {
        tenants_scanned: tenant_ids.len(),
        segments_sealed: 0,
        failed_tenants: 0,
    };

    for tenant_id in tenant_ids {
        match seal_audit_hash_chain(
            pool,
            rustfs_client,
            tenant_id,
            None,
            settings.segment_max_rows(),
        )
        .await
        {
            Ok(response) => {
                summary.segments_sealed += 1;
                debug!(
                    tenant_id = %tenant_id,
                    segment_id = %response.segment_id,
                    rows_count = response.rows_count,
                    "sealed audit hash-chain segment"
                );
            }
            Err(err) if is_no_unsealed_conflict(&err) => {
                debug!(
                    tenant_id = %tenant_id,
                    "audit hash-chain rows were already sealed by another worker"
                );
            }
            Err(err) => {
                summary.failed_tenants += 1;
                warn!(
                    tenant_id = %tenant_id,
                    "failed to seal audit hash-chain segment: {}",
                    err
                );
            }
        }
    }

    Ok(summary)
}

pub async fn find_tenants_with_unsealed_hash_chain_rows(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<Uuid>, AppError> {
    let tenant_ids = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH latest_segments AS (
            SELECT DISTINCT ON (tenant_id)
                tenant_id,
                last_row_hash
            FROM audit_hash_chain_segments
            ORDER BY tenant_id, sealed_at DESC, id DESC
        )
        SELECT DISTINCT audit_logs.tenant_id
        FROM audit_logs
        LEFT JOIN latest_segments
          ON latest_segments.tenant_id = audit_logs.tenant_id
        WHERE audit_logs.row_hash IS NOT NULL
          AND (
                (latest_segments.last_row_hash IS NULL AND audit_logs.prev_hash IS NULL)
                OR audit_logs.prev_hash = latest_segments.last_row_hash
          )
        ORDER BY audit_logs.tenant_id
        LIMIT $1
        "#,
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .await?;

    Ok(tenant_ids)
}

fn is_no_unsealed_conflict(err: &AppError) -> bool {
    matches!(
        err,
        AppError::Conflict(message) if message == NO_UNSEALED_AUDIT_ROWS_MESSAGE
    )
}

#[cfg(test)]
mod tests {
    use sqlx::{Row, postgres::PgPoolOptions};
    use uuid::Uuid;

    use crate::{
        configuration::AuditHashChainSettings,
        features::agent_platform::{
            audit::{NewAuditLog, insert_audit_log_tx},
            rustfs::RustFsClient,
        },
    };

    use super::{
        find_tenants_with_unsealed_hash_chain_rows, process_audit_hash_chain_seals_for_tenants,
    };

    fn audit_settings(segment_max_rows: i64) -> AuditHashChainSettings {
        AuditHashChainSettings {
            auto_seal_enabled: true,
            worker_interval_milliseconds: 1000,
            worker_tenant_batch_size: 10,
            segment_max_rows,
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

    #[tokio::test]
    #[ignore = "requires local Postgres and the bibi_work schema"]
    async fn worker_seals_pending_audit_segments_without_actor_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = test_pool().await?;
        let rustfs = RustFsClient::disabled_for_tests();
        let tenant_id = seed_tenant(&pool).await?;

        let mut tx = pool.begin().await?;
        insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "read")).await?;
        insert_audit_log_tx(&mut tx, audit_entry_for_tenant(tenant_id, "write")).await?;
        tx.commit().await?;

        let pending = find_tenants_with_unsealed_hash_chain_rows(&pool, 10).await?;
        assert!(pending.contains(&tenant_id));

        let first = process_audit_hash_chain_seals_for_tenants(
            &pool,
            &rustfs,
            &audit_settings(1),
            vec![tenant_id],
        )
        .await?;
        assert_eq!(first.tenants_scanned, 1);
        assert_eq!(first.segments_sealed, 1);
        assert_eq!(first.failed_tenants, 0);

        let second = process_audit_hash_chain_seals_for_tenants(
            &pool,
            &rustfs,
            &audit_settings(10),
            vec![tenant_id],
        )
        .await?;
        assert_eq!(second.tenants_scanned, 1);
        assert_eq!(second.segments_sealed, 1);
        assert_eq!(second.failed_tenants, 0);

        let third = process_audit_hash_chain_seals_for_tenants(
            &pool,
            &rustfs,
            &audit_settings(10),
            vec![tenant_id],
        )
        .await?;
        assert_eq!(third.tenants_scanned, 1);
        assert_eq!(third.segments_sealed, 0);
        assert_eq!(third.failed_tenants, 0);

        let segment_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_hash_chain_segments WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(segment_count, 2);

        let sealed_by_values = sqlx::query(
            "SELECT sealed_by_user_id, manifest FROM audit_hash_chain_segments WHERE tenant_id = $1",
        )
        .bind(tenant_id)
        .fetch_all(&pool)
        .await?;
        for row in sealed_by_values {
            let sealed_by_user_id: Option<Uuid> = row.try_get("sealed_by_user_id")?;
            let manifest: serde_json::Value = row.try_get("manifest")?;
            assert!(sealed_by_user_id.is_none());
            assert!(
                manifest
                    .get("sealed_by_user_id")
                    .is_some_and(|value| value.is_null())
            );
        }

        sqlx::query("DELETE FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .execute(&pool)
            .await?;
        Ok(())
    }

    async fn seed_tenant(pool: &sqlx::PgPool) -> Result<Uuid, Box<dyn std::error::Error>> {
        let suffix = Uuid::new_v4();
        let tenant_id: Uuid =
            sqlx::query_scalar("INSERT INTO tenants (name, slug) VALUES ($1, $2) RETURNING id")
                .bind(format!("Audit sealing worker test {suffix}"))
                .bind(format!("audit-sealing-worker-test-{suffix}"))
                .fetch_one(pool)
                .await?;
        Ok(tenant_id)
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
}
