ALTER TABLE tool_calls
    ADD COLUMN IF NOT EXISTS evidence_object_reference_id UUID
        REFERENCES object_references(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_tool_calls_evidence_object_reference
    ON tool_calls (evidence_object_reference_id);
