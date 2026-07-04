ALTER TABLE approvals
    ADD COLUMN IF NOT EXISTS evidence_object_reference_id UUID
        REFERENCES object_references(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_approvals_evidence_object_reference
    ON approvals (evidence_object_reference_id);
