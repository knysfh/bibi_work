ALTER TABLE workflow_node_runs
    ADD COLUMN IF NOT EXISTS max_attempts INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS backoff_sec INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS timeout_sec INTEGER,
    ADD COLUMN IF NOT EXISTS not_before TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS started_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS completed_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_error TEXT;

CREATE INDEX IF NOT EXISTS idx_workflow_node_runs_ready
    ON workflow_node_runs (workflow_run_id, status, not_before);

CREATE INDEX IF NOT EXISTS idx_workflow_node_runs_agent_run
    ON workflow_node_runs (agent_run_id)
    WHERE agent_run_id IS NOT NULL;
