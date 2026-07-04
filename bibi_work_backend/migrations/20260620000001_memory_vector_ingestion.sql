CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id UUID PRIMARY KEY REFERENCES memory_items(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider TEXT NOT NULL DEFAULT 'external-http',
    embedding_model TEXT,
    vector_dimension INTEGER,
    vector_hash TEXT,
    qdrant_collection TEXT NOT NULL DEFAULT 'bibi_work_memories',
    qdrant_point_id TEXT,
    index_status TEXT NOT NULL DEFAULT 'pending',
    last_error TEXT,
    indexed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_memory_embeddings_tenant_status
    ON memory_embeddings (tenant_id, index_status, updated_at DESC);

CREATE TABLE IF NOT EXISTS memory_ingestion_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    memory_id UUID NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    job_type TEXT NOT NULL DEFAULT 'upsert',
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    scheduled_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_memory_ingestion_jobs_pending
    ON memory_ingestion_jobs (tenant_id, status, scheduled_at)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS memory_feedback (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    memory_id UUID REFERENCES memory_items(id) ON DELETE SET NULL,
    user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    feedback TEXT NOT NULL,
    score DOUBLE PRECISION,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_memory_feedback_memory
    ON memory_feedback (tenant_id, memory_id, created_at DESC);
