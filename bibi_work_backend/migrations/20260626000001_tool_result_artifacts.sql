CREATE TABLE IF NOT EXISTS tool_result_artifacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    tool_call_id UUID REFERENCES tool_calls(id) ON DELETE SET NULL,
    view_kind TEXT NOT NULL,
    ref_kind TEXT NOT NULL,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    revision BIGINT NOT NULL,
    file_revision_id UUID NOT NULL REFERENCES file_revisions(id) ON DELETE CASCADE,
    object_reference_id UUID NOT NULL REFERENCES object_references(id) ON DELETE CASCADE,
    content_hash TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (object_reference_id)
);

CREATE INDEX IF NOT EXISTS idx_tool_result_artifacts_run
    ON tool_result_artifacts (tenant_id, run_id, tool_call_id);

CREATE INDEX IF NOT EXISTS idx_tool_result_artifacts_file_revision
    ON tool_result_artifacts (file_revision_id);
