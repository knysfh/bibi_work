CREATE TABLE IF NOT EXISTS audit_hash_chain_segments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    first_audit_log_id UUID NOT NULL REFERENCES audit_logs(id) ON DELETE RESTRICT,
    last_audit_log_id UUID NOT NULL REFERENCES audit_logs(id) ON DELETE RESTRICT,
    rows_count BIGINT NOT NULL CHECK (rows_count > 0),
    first_prev_hash TEXT,
    last_row_hash TEXT NOT NULL,
    manifest_hash TEXT NOT NULL,
    manifest JSONB NOT NULL,
    object_reference_id UUID REFERENCES object_references(id) ON DELETE SET NULL,
    sealed_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    sealed_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, first_audit_log_id, last_audit_log_id),
    UNIQUE (tenant_id, manifest_hash)
);

CREATE INDEX IF NOT EXISTS idx_audit_hash_chain_segments_tenant_sealed
    ON audit_hash_chain_segments (tenant_id, sealed_at DESC, id DESC);
