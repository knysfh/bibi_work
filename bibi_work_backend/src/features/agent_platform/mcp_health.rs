use futures_util::{StreamExt, stream};
use serde_json::Value;
use sqlx::Row;
use tokio::time::{MissedTickBehavior, interval};
use tracing::warn;
use uuid::Uuid;

use crate::{configuration::McpHealthSettings, startup::AppState};

use super::mcp_discovery;

pub fn spawn_mcp_health_worker(state: AppState, settings: McpHealthSettings) {
    if !settings.worker_enabled {
        return;
    }
    tokio::spawn(async move {
        let mut ticker = interval(settings.interval());
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(error) = probe_active_servers(&state, settings.batch_size()).await {
                warn!(error = %error, "MCP health probe batch failed");
            }
        }
    });
}

async fn probe_active_servers(state: &AppState, batch_size: i64) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, tenant_id, transport, config, secret_ref
        FROM mcp_servers
        WHERE status = 'active'
          AND deleted_at IS NULL
          AND transport IN ('http', 'json-rpc', 'streamable-http')
        ORDER BY COALESCE(last_health_check_at, '-infinity'::timestamptz), id
        LIMIT $1
        "#,
    )
    .bind(batch_size)
    .fetch_all(&state.connect_pool)
    .await?;

    let targets = rows
        .into_iter()
        .map(|row| {
            Ok::<_, sqlx::Error>((
                row.try_get::<Uuid, _>("id")?,
                row.try_get::<Uuid, _>("tenant_id")?,
                row.try_get::<String, _>("transport")?,
                row.try_get::<Value, _>("config")?,
                row.try_get::<Option<String>, _>("secret_ref")?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    stream::iter(targets)
        .for_each_concurrent(8, |(id, tenant_id, transport, config, secret_ref)| {
            let state = state.clone();
            async move {
                let result = mcp_discovery::discover_mcp_tools(
                    &state.secret_resolver,
                    &transport,
                    &config,
                    secret_ref.as_deref(),
                )
                .await;
                let update = match result {
                    Ok(_) => record_success(&state, id, tenant_id).await,
                    Err(error) => {
                        let message: String = error.to_string().chars().take(2_000).collect();
                        warn!(mcp_server_id = %id, tenant_id = %tenant_id, "MCP health probe failed");
                        record_failure(&state, id, tenant_id, &message).await
                    }
                };
                if let Err(error) = update {
                    warn!(mcp_server_id = %id, tenant_id = %tenant_id, error = %error, "failed to persist MCP health result");
                }
            }
        })
        .await;
    Ok(())
}

async fn record_success(state: &AppState, id: Uuid, tenant_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE mcp_servers
        SET health_status = 'healthy',
            last_health_check_at = CURRENT_TIMESTAMP,
            last_discovered_at = CURRENT_TIMESTAMP,
            consecutive_failures = 0,
            health_error = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND status = 'active' AND deleted_at IS NULL
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

async fn record_failure(
    state: &AppState,
    id: Uuid,
    tenant_id: Uuid,
    message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE mcp_servers
        SET health_status = 'unhealthy',
            last_health_check_at = CURRENT_TIMESTAMP,
            consecutive_failures = consecutive_failures + 1,
            health_error = $3,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = $1 AND tenant_id = $2 AND status = 'active' AND deleted_at IS NULL
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(message)
    .execute(&state.connect_pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_settings_are_bounded() {
        let settings = McpHealthSettings {
            worker_enabled: true,
            interval_seconds: 1,
            batch_size: 10_000,
        };
        assert_eq!(settings.interval(), std::time::Duration::from_secs(10));
        assert_eq!(settings.batch_size(), 500);
    }
}
