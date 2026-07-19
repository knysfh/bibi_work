ALTER TABLE agent_version_mcp_bindings
    ADD COLUMN IF NOT EXISTS schema_hash_at_publish text,
    ADD COLUMN IF NOT EXISTS binding_mode text NOT NULL DEFAULT 'optional';

UPDATE agent_version_mcp_bindings AS binding
SET schema_hash_at_publish = tool.schema_hash
FROM mcp_tools AS tool
WHERE binding.mcp_tool_id = tool.id
  AND binding.schema_hash_at_publish IS NULL;

ALTER TABLE agent_version_mcp_bindings
    DROP CONSTRAINT IF EXISTS agent_version_mcp_bindings_binding_mode_check;

ALTER TABLE agent_version_mcp_bindings
    ADD CONSTRAINT agent_version_mcp_bindings_binding_mode_check
    CHECK (binding_mode IN ('required', 'optional'));

CREATE INDEX IF NOT EXISTS idx_agent_version_mcp_bindings_tool_schema
    ON agent_version_mcp_bindings (mcp_tool_id, schema_hash_at_publish);
