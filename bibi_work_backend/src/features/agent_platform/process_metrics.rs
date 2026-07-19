use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const DURATION_BOUNDS_MS: [u64; 10] = [1, 5, 10, 25, 50, 100, 250, 1_000, 2_500, 5_000];

struct OperationMetrics {
    requests_total: AtomicU64,
    failures_total: AtomicU64,
    duration_buckets: [AtomicU64; 10],
    duration_micros: AtomicU64,
}

impl OperationMetrics {
    const fn new() -> Self {
        Self {
            requests_total: AtomicU64::new(0),
            failures_total: AtomicU64::new(0),
            duration_buckets: [const { AtomicU64::new(0) }; 10],
            duration_micros: AtomicU64::new(0),
        }
    }

    fn observe(&self, duration: Duration, success: bool) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if !success {
            self.failures_total.fetch_add(1, Ordering::Relaxed);
        }
        let elapsed_millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        for (bound, bucket) in DURATION_BOUNDS_MS.iter().zip(&self.duration_buckets) {
            if elapsed_millis <= *bound {
                bucket.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.duration_micros.fetch_add(
            u64::try_from(duration.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    fn snapshot(&self) -> OperationMetricsSnapshot {
        OperationMetricsSnapshot {
            requests_total: self.requests_total.load(Ordering::Relaxed),
            failures_total: self.failures_total.load(Ordering::Relaxed),
            duration_buckets: DURATION_BOUNDS_MS
                .iter()
                .zip(&self.duration_buckets)
                .map(|(bound, count)| {
                    (
                        Duration::from_millis(*bound).as_secs_f64(),
                        count.load(Ordering::Relaxed),
                    )
                })
                .collect(),
            duration_sum_seconds: self.duration_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0,
        }
    }
}

static OIDC_AUTH: OperationMetrics = OperationMetrics::new();
static JWKS_REFRESH: OperationMetrics = OperationMetrics::new();
static AUTHZ_CHECK: OperationMetrics = OperationMetrics::new();

pub struct OperationMetricsSnapshot {
    pub requests_total: u64,
    pub failures_total: u64,
    pub duration_buckets: Vec<(f64, u64)>,
    pub duration_sum_seconds: f64,
}

pub struct ControlPlaneMetricsSnapshot {
    pub oidc_auth: OperationMetricsSnapshot,
    pub jwks_refresh: OperationMetricsSnapshot,
    pub authz_check: OperationMetricsSnapshot,
}

pub fn observe_oidc_auth(duration: Duration, success: bool) {
    OIDC_AUTH.observe(duration, success);
}

pub fn observe_jwks_refresh(duration: Duration, success: bool) {
    JWKS_REFRESH.observe(duration, success);
}

pub fn observe_authz_check(duration: Duration, success: bool) {
    AUTHZ_CHECK.observe(duration, success);
}

pub fn metrics_snapshot() -> ControlPlaneMetricsSnapshot {
    ControlPlaneMetricsSnapshot {
        oidc_auth: OIDC_AUTH.snapshot(),
        jwks_refresh: JWKS_REFRESH.snapshot(),
        authz_check: AUTHZ_CHECK.snapshot(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_histogram_is_cumulative_and_counts_failures() {
        let metrics = OperationMetrics::new();
        metrics.observe(Duration::from_millis(20), false);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.requests_total, 1);
        assert_eq!(snapshot.failures_total, 1);
        assert_eq!(snapshot.duration_buckets[2].1, 0);
        assert_eq!(snapshot.duration_buckets[3].1, 1);
        assert_eq!(snapshot.duration_buckets.last().unwrap().1, 1);
    }
}
