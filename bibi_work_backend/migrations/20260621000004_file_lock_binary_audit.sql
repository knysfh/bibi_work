ALTER TABLE file_locks
ADD COLUMN IF NOT EXISTS path TEXT,
ADD COLUMN IF NOT EXISTS lock_token TEXT NOT NULL DEFAULT gen_random_uuid()::text,
ADD COLUMN IF NOT EXISTS reason TEXT,
ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP;

CREATE INDEX IF NOT EXISTS idx_file_locks_expiry
    ON file_locks (tenant_id, expires_at);

CREATE INDEX IF NOT EXISTS idx_file_revisions_metadata_content
    ON file_revisions USING GIN (metadata);

CREATE INDEX IF NOT EXISTS idx_object_references_owner
    ON object_references (tenant_id, owner_resource_type, owner_resource_id);
