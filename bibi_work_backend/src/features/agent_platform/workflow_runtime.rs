use std::collections::HashMap;

pub fn workflow_status_from_counts(counts: &HashMap<String, i64>, total: i64) -> String {
    if total == 0 {
        return "queued".to_string();
    }

    if counts.get("completed").copied().unwrap_or(0) == total {
        return "completed".to_string();
    }

    if counts
        .keys()
        .any(|status| matches!(status.as_str(), "failed" | "blocked"))
    {
        return "failed".to_string();
    }

    if counts.keys().any(|status| status == "cancelled") {
        return "cancelled".to_string();
    }

    if counts.keys().any(|status| {
        matches!(
            status.as_str(),
            "pending" | "ready" | "queued" | "running" | "waiting_approval" | "waiting_user_input"
        )
    }) {
        return "running".to_string();
    }

    "queued".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(entries: &[(&str, i64)]) -> HashMap<String, i64> {
        entries
            .iter()
            .map(|(status, count)| ((*status).to_string(), *count))
            .collect()
    }

    #[test]
    fn workflow_status_prefers_completed_when_all_nodes_completed() {
        assert_eq!(
            workflow_status_from_counts(&counts(&[("completed", 3)]), 3),
            "completed"
        );
    }

    #[test]
    fn workflow_status_fails_when_any_node_is_failed_or_blocked() {
        assert_eq!(
            workflow_status_from_counts(&counts(&[("completed", 1), ("blocked", 1)]), 2),
            "failed"
        );
        assert_eq!(
            workflow_status_from_counts(&counts(&[("failed", 1), ("pending", 1)]), 2),
            "failed"
        );
    }

    #[test]
    fn workflow_status_stays_running_during_retry_backoff() {
        assert_eq!(
            workflow_status_from_counts(&counts(&[("pending", 1)]), 1),
            "running"
        );
    }
}
