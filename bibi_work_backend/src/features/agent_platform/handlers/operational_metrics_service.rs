use axum::{
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use sqlx::Row;

use crate::{features::core::errors::AppError, startup::AppState};

use super::biwork_ws_service;
use crate::features::agent_platform::{mcp_http, process_metrics};

pub async fn operational_metrics(State(state): State<AppState>) -> Result<Response, AppError> {
    let row = sqlx::query(
        r#"
        WITH recent_run_dispatch AS (
          SELECT EXTRACT(EPOCH FROM (started_at - queued_at))::double precision AS seconds
          FROM runs
          WHERE started_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours'
            AND started_at >= queued_at
        ),
        recent_run_duration AS (
          SELECT EXTRACT(EPOCH FROM (completed_at - started_at))::double precision AS seconds
          FROM runs
          WHERE completed_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours'
            AND completed_at >= started_at
        ),
        recent_approval_duration AS (
          SELECT EXTRACT(EPOCH FROM (decided_at - created_at))::double precision AS seconds
          FROM approvals
          WHERE decided_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours'
            AND decided_at >= created_at
        ),
        recent_tool_duration AS (
          SELECT EXTRACT(EPOCH FROM (completed_at - created_at))::double precision AS seconds
          FROM tool_calls
          WHERE completed_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours'
            AND completed_at >= created_at
        )
        SELECT
          (SELECT COUNT(*) FROM runs WHERE status = 'queued') AS runs_queued,
          (SELECT COUNT(*) FROM runs WHERE status IN ('running', 'cancelling')) AS runs_active,
          (SELECT COUNT(*) FROM runs WHERE status = 'waiting_approval') AS runs_waiting_approval,
          (SELECT COUNT(*) FROM runs
             WHERE status = 'completed'
               AND completed_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours') AS runs_completed_24h,
          (SELECT COUNT(*) FROM runs
             WHERE status = 'failed'
               AND completed_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours') AS runs_failed_24h,
          (SELECT COUNT(*) FROM runs
             WHERE status = 'cancelled'
               AND completed_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours') AS runs_cancelled_24h,
          (SELECT COUNT(*) FROM approvals WHERE status = 'pending') AS approvals_pending,
          (SELECT COUNT(*) FROM local_exec_requests WHERE status IN ('queued', 'running')) AS local_exec_active,
          (SELECT COUNT(*) FROM event_outbox WHERE status IN ('pending', 'failed')) AS outbox_pending,
          (SELECT COUNT(*) FROM mcp_servers
             WHERE status = 'active' AND deleted_at IS NULL AND health_status = 'unhealthy') AS mcp_unhealthy,
          (SELECT COUNT(*) FROM audit_hash_chain_segments
             WHERE archive_status IN ('pending', 'failed', 'archiving')) AS audit_archive_pending,
          (SELECT COUNT(*) FROM llm_credential_rotation_attempts
             WHERE status = 'failed' AND started_at >= CURRENT_TIMESTAMP - INTERVAL '24 hours') AS rotation_failures_24h,
          COALESCE((SELECT EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - MIN(queued_at)))::double precision
             FROM runs WHERE status = 'queued'), 0) AS oldest_queued_run_seconds,
          COALESCE((SELECT EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - MIN(created_at)))::double precision
             FROM local_exec_requests WHERE status = 'queued'), 0) AS oldest_local_exec_seconds,
          COALESCE((SELECT EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP - MIN(created_at)))::double precision
             FROM event_outbox WHERE status IN ('pending', 'failed')), 0) AS oldest_outbox_seconds,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 0.1) AS run_dispatch_le_0_1,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 0.5) AS run_dispatch_le_0_5,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 1) AS run_dispatch_le_1,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 2.5) AS run_dispatch_le_2_5,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 5) AS run_dispatch_le_5,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 10) AS run_dispatch_le_10,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 30) AS run_dispatch_le_30,
          (SELECT COUNT(*) FROM recent_run_dispatch WHERE seconds <= 60) AS run_dispatch_le_60,
          (SELECT COUNT(*) FROM recent_run_dispatch) AS run_dispatch_count,
          COALESCE((SELECT SUM(seconds) FROM recent_run_dispatch), 0) AS run_dispatch_sum,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 1) AS run_duration_le_1,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 5) AS run_duration_le_5,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 10) AS run_duration_le_10,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 30) AS run_duration_le_30,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 60) AS run_duration_le_60,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 300) AS run_duration_le_300,
          (SELECT COUNT(*) FROM recent_run_duration WHERE seconds <= 900) AS run_duration_le_900,
          (SELECT COUNT(*) FROM recent_run_duration) AS run_duration_count,
          COALESCE((SELECT SUM(seconds) FROM recent_run_duration), 0) AS run_duration_sum,
          (SELECT COUNT(*) FROM recent_approval_duration WHERE seconds <= 5) AS approval_le_5,
          (SELECT COUNT(*) FROM recent_approval_duration WHERE seconds <= 30) AS approval_le_30,
          (SELECT COUNT(*) FROM recent_approval_duration WHERE seconds <= 60) AS approval_le_60,
          (SELECT COUNT(*) FROM recent_approval_duration WHERE seconds <= 300) AS approval_le_300,
          (SELECT COUNT(*) FROM recent_approval_duration WHERE seconds <= 900) AS approval_le_900,
          (SELECT COUNT(*) FROM recent_approval_duration WHERE seconds <= 3600) AS approval_le_3600,
          (SELECT COUNT(*) FROM recent_approval_duration) AS approval_count,
          COALESCE((SELECT SUM(seconds) FROM recent_approval_duration), 0) AS approval_sum,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 0.1) AS tool_le_0_1,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 0.5) AS tool_le_0_5,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 1) AS tool_le_1,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 2.5) AS tool_le_2_5,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 5) AS tool_le_5,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 10) AS tool_le_10,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 30) AS tool_le_30,
          (SELECT COUNT(*) FROM recent_tool_duration WHERE seconds <= 60) AS tool_le_60,
          (SELECT COUNT(*) FROM recent_tool_duration) AS tool_count,
          COALESCE((SELECT SUM(seconds) FROM recent_tool_duration), 0) AS tool_sum
        "#,
    )
    .fetch_one(&state.connect_pool)
    .await?;

    let mcp = mcp_http::metrics_snapshot();
    let ws = biwork_ws_service::metrics_snapshot();
    let control_plane = process_metrics::metrics_snapshot();
    let body = render_metrics(&[
        ("bibi_runs_queued", row.try_get("runs_queued")?),
        ("bibi_runs_active", row.try_get("runs_active")?),
        (
            "bibi_runs_waiting_approval",
            row.try_get("runs_waiting_approval")?,
        ),
        (
            "bibi_runs_completed_24h",
            row.try_get("runs_completed_24h")?,
        ),
        ("bibi_runs_failed_24h", row.try_get("runs_failed_24h")?),
        (
            "bibi_runs_cancelled_24h",
            row.try_get("runs_cancelled_24h")?,
        ),
        ("bibi_approvals_pending", row.try_get("approvals_pending")?),
        (
            "bibi_local_exec_requests_active",
            row.try_get("local_exec_active")?,
        ),
        ("bibi_event_outbox_pending", row.try_get("outbox_pending")?),
        ("bibi_mcp_servers_unhealthy", row.try_get("mcp_unhealthy")?),
        (
            "bibi_audit_archive_segments_pending",
            row.try_get("audit_archive_pending")?,
        ),
        (
            "bibi_llm_credential_rotation_failures_24h",
            row.try_get("rotation_failures_24h")?,
        ),
        (
            "bibi_oldest_queued_run_seconds",
            row.try_get::<f64, _>("oldest_queued_run_seconds")? as i64,
        ),
        (
            "bibi_oldest_local_exec_request_seconds",
            row.try_get::<f64, _>("oldest_local_exec_seconds")? as i64,
        ),
        (
            "bibi_oldest_event_outbox_item_seconds",
            row.try_get::<f64, _>("oldest_outbox_seconds")? as i64,
        ),
        ("bibi_mcp_http_session_slots", mcp.session_slots as i64),
        ("bibi_biwork_ws_connections_active", ws.connections_active),
        (
            "bibi_biwork_ws_subscriptions_active",
            ws.subscriptions_active,
        ),
    ]);
    let body = render_counters(
        body,
        &[
            ("bibi_mcp_http_requests_total", mcp.requests_total),
            (
                "bibi_mcp_http_request_failures_total",
                mcp.request_failures_total,
            ),
            (
                "bibi_mcp_http_session_reuses_total",
                mcp.session_reuses_total,
            ),
            (
                "bibi_mcp_http_session_initializations_total",
                mcp.session_initializations_total,
            ),
            (
                "bibi_mcp_http_session_retries_total",
                mcp.session_retries_total,
            ),
            ("bibi_biwork_ws_connections_total", ws.connections_total),
            ("bibi_biwork_ws_auth_failures_total", ws.auth_failures_total),
            ("bibi_biwork_ws_messages_sent_total", ws.messages_sent_total),
            ("bibi_biwork_ws_send_failures_total", ws.send_failures_total),
            (
                "bibi_ferriskey_oidc_auth_requests_total",
                control_plane.oidc_auth.requests_total,
            ),
            (
                "bibi_ferriskey_oidc_auth_failures_total",
                control_plane.oidc_auth.failures_total,
            ),
            (
                "bibi_ferriskey_jwks_refresh_requests_total",
                control_plane.jwks_refresh.requests_total,
            ),
            (
                "bibi_ferriskey_jwks_refresh_failures_total",
                control_plane.jwks_refresh.failures_total,
            ),
            (
                "bibi_resource_authz_checks_total",
                control_plane.authz_check.requests_total,
            ),
            (
                "bibi_resource_authz_check_failures_total",
                control_plane.authz_check.failures_total,
            ),
        ],
    );
    let body = render_cumulative_histograms(
        body,
        &[
            CumulativeHistogram {
                name: "bibi_mcp_http_request_duration_seconds",
                buckets: mcp.request_duration_buckets,
                count: mcp.requests_total,
                sum: mcp.request_duration_sum_seconds,
            },
            CumulativeHistogram {
                name: "bibi_biwork_ws_send_duration_seconds",
                buckets: ws.send_duration_buckets,
                count: ws.messages_sent_total + ws.send_failures_total,
                sum: ws.send_duration_sum_seconds,
            },
            CumulativeHistogram {
                name: "bibi_ferriskey_oidc_auth_duration_seconds",
                buckets: control_plane.oidc_auth.duration_buckets,
                count: control_plane.oidc_auth.requests_total,
                sum: control_plane.oidc_auth.duration_sum_seconds,
            },
            CumulativeHistogram {
                name: "bibi_ferriskey_jwks_refresh_duration_seconds",
                buckets: control_plane.jwks_refresh.duration_buckets,
                count: control_plane.jwks_refresh.requests_total,
                sum: control_plane.jwks_refresh.duration_sum_seconds,
            },
            CumulativeHistogram {
                name: "bibi_resource_authz_check_duration_seconds",
                buckets: control_plane.authz_check.duration_buckets,
                count: control_plane.authz_check.requests_total,
                sum: control_plane.authz_check.duration_sum_seconds,
            },
        ],
    );
    let body = render_windowed_distributions(
        body,
        &[
            windowed_distribution_from_row(
                &row,
                "bibi_run_dispatch_duration_seconds_24h",
                &[
                    (0.1, "run_dispatch_le_0_1"),
                    (0.5, "run_dispatch_le_0_5"),
                    (1.0, "run_dispatch_le_1"),
                    (2.5, "run_dispatch_le_2_5"),
                    (5.0, "run_dispatch_le_5"),
                    (10.0, "run_dispatch_le_10"),
                    (30.0, "run_dispatch_le_30"),
                    (60.0, "run_dispatch_le_60"),
                ],
                "run_dispatch_count",
                "run_dispatch_sum",
            )?,
            windowed_distribution_from_row(
                &row,
                "bibi_run_duration_seconds_24h",
                &[
                    (1.0, "run_duration_le_1"),
                    (5.0, "run_duration_le_5"),
                    (10.0, "run_duration_le_10"),
                    (30.0, "run_duration_le_30"),
                    (60.0, "run_duration_le_60"),
                    (300.0, "run_duration_le_300"),
                    (900.0, "run_duration_le_900"),
                ],
                "run_duration_count",
                "run_duration_sum",
            )?,
            windowed_distribution_from_row(
                &row,
                "bibi_approval_duration_seconds_24h",
                &[
                    (5.0, "approval_le_5"),
                    (30.0, "approval_le_30"),
                    (60.0, "approval_le_60"),
                    (300.0, "approval_le_300"),
                    (900.0, "approval_le_900"),
                    (3600.0, "approval_le_3600"),
                ],
                "approval_count",
                "approval_sum",
            )?,
            windowed_distribution_from_row(
                &row,
                "bibi_tool_execution_duration_seconds_24h",
                &[
                    (0.1, "tool_le_0_1"),
                    (0.5, "tool_le_0_5"),
                    (1.0, "tool_le_1"),
                    (2.5, "tool_le_2_5"),
                    (5.0, "tool_le_5"),
                    (10.0, "tool_le_10"),
                    (30.0, "tool_le_30"),
                    (60.0, "tool_le_60"),
                ],
                "tool_count",
                "tool_sum",
            )?,
        ],
    );
    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    Ok(response)
}

struct WindowedDistribution {
    name: &'static str,
    buckets: Vec<(f64, i64)>,
    count: i64,
    sum: f64,
}

fn windowed_distribution_from_row(
    row: &sqlx::postgres::PgRow,
    name: &'static str,
    bucket_columns: &[(f64, &str)],
    count_column: &str,
    sum_column: &str,
) -> Result<WindowedDistribution, sqlx::Error> {
    let buckets = bucket_columns
        .iter()
        .map(|(upper_bound, column)| Ok((*upper_bound, row.try_get(*column)?)))
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(WindowedDistribution {
        name,
        buckets,
        count: row.try_get(count_column)?,
        sum: row.try_get(sum_column)?,
    })
}

fn render_windowed_distributions(
    mut body: String,
    distributions: &[WindowedDistribution],
) -> String {
    for distribution in distributions {
        body.push_str("# TYPE ");
        body.push_str(distribution.name);
        body.push_str("_bucket gauge\n");
        for (upper_bound, count) in &distribution.buckets {
            body.push_str(distribution.name);
            body.push_str("_bucket{le=\"");
            body.push_str(&upper_bound.to_string());
            body.push_str("\"} ");
            body.push_str(&count.to_string());
            body.push('\n');
        }
        body.push_str(distribution.name);
        body.push_str("_bucket{le=\"+Inf\"} ");
        body.push_str(&distribution.count.to_string());
        body.push('\n');
        body.push_str("# TYPE ");
        body.push_str(distribution.name);
        body.push_str("_sum gauge\n");
        body.push_str(distribution.name);
        body.push_str("_sum ");
        body.push_str(&distribution.sum.to_string());
        body.push('\n');
        body.push_str("# TYPE ");
        body.push_str(distribution.name);
        body.push_str("_count gauge\n");
        body.push_str(distribution.name);
        body.push_str("_count ");
        body.push_str(&distribution.count.to_string());
        body.push('\n');
    }
    body
}

fn render_metrics(metrics: &[(&str, i64)]) -> String {
    let mut body = String::new();
    for (name, value) in metrics {
        body.push_str("# TYPE ");
        body.push_str(name);
        body.push_str(" gauge\n");
        body.push_str(name);
        body.push(' ');
        body.push_str(&value.to_string());
        body.push('\n');
    }
    body
}

struct CumulativeHistogram {
    name: &'static str,
    buckets: Vec<(f64, u64)>,
    count: u64,
    sum: f64,
}

fn render_counters(mut body: String, counters: &[(&str, u64)]) -> String {
    for (name, value) in counters {
        body.push_str("# TYPE ");
        body.push_str(name);
        body.push_str(" counter\n");
        body.push_str(name);
        body.push(' ');
        body.push_str(&value.to_string());
        body.push('\n');
    }
    body
}

fn render_cumulative_histograms(mut body: String, histograms: &[CumulativeHistogram]) -> String {
    for histogram in histograms {
        body.push_str("# TYPE ");
        body.push_str(histogram.name);
        body.push_str(" histogram\n");
        for (upper_bound, count) in &histogram.buckets {
            body.push_str(histogram.name);
            body.push_str("_bucket{le=\"");
            body.push_str(&upper_bound.to_string());
            body.push_str("\"} ");
            body.push_str(&count.to_string());
            body.push('\n');
        }
        body.push_str(histogram.name);
        body.push_str("_bucket{le=\"+Inf\"} ");
        body.push_str(&histogram.count.to_string());
        body.push('\n');
        body.push_str(histogram.name);
        body.push_str("_sum ");
        body.push_str(&histogram.sum.to_string());
        body.push('\n');
        body.push_str(histogram.name);
        body.push_str("_count ");
        body.push_str(&histogram.count.to_string());
        body.push('\n');
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prometheus_output_uses_stable_low_cardinality_names() {
        let body = render_metrics(&[("bibi_runs_active", 2), ("bibi_mcp_servers_unhealthy", 1)]);
        assert_eq!(
            body,
            "# TYPE bibi_runs_active gauge\nbibi_runs_active 2\n# TYPE bibi_mcp_servers_unhealthy gauge\nbibi_mcp_servers_unhealthy 1\n"
        );
        assert!(!body.contains("tenant"));
    }

    #[test]
    fn prometheus_windowed_distribution_uses_gauges_and_cumulative_buckets() {
        let body = render_windowed_distributions(
            String::new(),
            &[WindowedDistribution {
                name: "bibi_run_dispatch_duration_seconds_24h",
                buckets: vec![(0.1, 2), (1.0, 4)],
                count: 5,
                sum: 2.75,
            }],
        );
        assert_eq!(
            body,
            "# TYPE bibi_run_dispatch_duration_seconds_24h_bucket gauge\n\
bibi_run_dispatch_duration_seconds_24h_bucket{le=\"0.1\"} 2\n\
bibi_run_dispatch_duration_seconds_24h_bucket{le=\"1\"} 4\n\
bibi_run_dispatch_duration_seconds_24h_bucket{le=\"+Inf\"} 5\n\
# TYPE bibi_run_dispatch_duration_seconds_24h_sum gauge\n\
bibi_run_dispatch_duration_seconds_24h_sum 2.75\n\
# TYPE bibi_run_dispatch_duration_seconds_24h_count gauge\n\
bibi_run_dispatch_duration_seconds_24h_count 5\n"
        );
        assert!(!body.contains("tenant"));
    }

    #[test]
    fn prometheus_process_metrics_use_counters_and_histograms() {
        let body = render_counters(String::new(), &[("bibi_mcp_http_requests_total", 7)]);
        let body = render_cumulative_histograms(
            body,
            &[CumulativeHistogram {
                name: "bibi_mcp_http_request_duration_seconds",
                buckets: vec![(0.1, 4), (1.0, 6)],
                count: 7,
                sum: 2.5,
            }],
        );
        assert_eq!(
            body,
            "# TYPE bibi_mcp_http_requests_total counter\n\
bibi_mcp_http_requests_total 7\n\
# TYPE bibi_mcp_http_request_duration_seconds histogram\n\
bibi_mcp_http_request_duration_seconds_bucket{le=\"0.1\"} 4\n\
bibi_mcp_http_request_duration_seconds_bucket{le=\"1\"} 6\n\
bibi_mcp_http_request_duration_seconds_bucket{le=\"+Inf\"} 7\n\
bibi_mcp_http_request_duration_seconds_sum 2.5\n\
bibi_mcp_http_request_duration_seconds_count 7\n"
        );
        assert!(!body.contains("tenant"));
    }
}
