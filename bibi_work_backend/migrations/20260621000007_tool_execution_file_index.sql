CREATE TABLE IF NOT EXISTS file_search_documents (
    file_revision_id UUID PRIMARY KEY REFERENCES file_revisions(id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    path_hash TEXT NOT NULL,
    revision BIGINT NOT NULL,
    content_hash TEXT NOT NULL,
    content_text TEXT NOT NULL,
    search_vector TSVECTOR GENERATED ALWAYS AS (
        to_tsvector('simple', COALESCE(path, '') || ' ' || COALESCE(content_text, ''))
    ) STORED,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_file_search_documents_scope
    ON file_search_documents (tenant_id, project_id, path_hash, revision DESC);

CREATE INDEX IF NOT EXISTS idx_file_search_documents_vector
    ON file_search_documents USING GIN (search_vector);

CREATE INDEX IF NOT EXISTS idx_sql_tool_versions_query_hash
    ON sql_tool_versions (tenant_id, query_hash)
    WHERE status = 'published';
