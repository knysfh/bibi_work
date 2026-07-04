CREATE INDEX IF NOT EXISTS idx_audit_logs_tenant_hash_chain
    ON audit_logs (tenant_id, created_at DESC, id DESC)
    WHERE row_hash IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_audit_logs_tenant_row_hash
    ON audit_logs (tenant_id, row_hash)
    WHERE row_hash IS NOT NULL;
