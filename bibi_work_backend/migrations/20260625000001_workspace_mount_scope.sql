CREATE TABLE IF NOT EXISTS workspaces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    owner_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    remote_project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    default_agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    default_agent_version_id UUID REFERENCES agent_versions(id) ON DELETE SET NULL,
    default_model_profile_id UUID REFERENCES llm_model_profiles(id) ON DELETE SET NULL,
    tool_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    file_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    include_globs JSONB NOT NULL DEFAULT '[]'::jsonb,
    exclude_globs JSONB NOT NULL DEFAULT '[]'::jsonb,
    trust_state TEXT NOT NULL DEFAULT 'untrusted',
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_workspaces_tenant ON workspaces (tenant_id, status);
CREATE INDEX IF NOT EXISTS idx_workspaces_remote_project ON workspaces (remote_project_id);

CREATE TABLE IF NOT EXISTS local_mounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES platform_users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    virtual_path TEXT NOT NULL,
    capabilities JSONB NOT NULL DEFAULT '["read"]'::jsonb,
    include_globs JSONB NOT NULL DEFAULT '[]'::jsonb,
    exclude_globs JSONB NOT NULL DEFAULT '[]'::jsonb,
    trust_state TEXT NOT NULL DEFAULT 'untrusted',
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (user_id, device_id, workspace_id, virtual_path)
);

CREATE INDEX IF NOT EXISTS idx_local_mounts_workspace ON local_mounts (workspace_id, status);
CREATE INDEX IF NOT EXISTS idx_local_mounts_user_device ON local_mounts (user_id, device_id);

ALTER TABLE conversations
    ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_conversations_workspace ON conversations (workspace_id);

ALTER TABLE runs
    ADD COLUMN IF NOT EXISTS workspace_id UUID REFERENCES workspaces(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS run_scope_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX IF NOT EXISTS idx_runs_workspace ON runs (workspace_id);
