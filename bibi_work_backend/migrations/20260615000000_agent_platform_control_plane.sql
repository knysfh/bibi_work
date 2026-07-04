CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE IF NOT EXISTS tenants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    slug TEXT NOT NULL UNIQUE,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS platform_users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    ferriskey_subject TEXT NOT NULL,
    username TEXT,
    email TEXT,
    display_name TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, ferriskey_subject)
);

CREATE INDEX IF NOT EXISTS idx_platform_users_subject ON platform_users (ferriskey_subject);

CREATE TABLE IF NOT EXISTS user_tenant_memberships (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES platform_users(id) ON DELETE CASCADE,
    role TEXT NOT NULL DEFAULT 'member',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, user_id)
);

CREATE TABLE IF NOT EXISTS devices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES platform_users(id) ON DELETE CASCADE,
    device_fingerprint TEXT NOT NULL,
    device_name TEXT NOT NULL,
    platform TEXT NOT NULL,
    public_key TEXT,
    trust_level TEXT NOT NULL DEFAULT 'standard',
    last_seen_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_devices_tenant_user ON devices (tenant_id, user_id);
CREATE INDEX IF NOT EXISTS idx_devices_user_platform ON devices (user_id, platform);
CREATE UNIQUE INDEX IF NOT EXISTS idx_devices_user_fingerprint
    ON devices (tenant_id, user_id, device_fingerprint);

CREATE TABLE IF NOT EXISTS platform_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES platform_users(id) ON DELETE CASCADE,
    device_id UUID NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    ferriskey_subject TEXT NOT NULL,
    ferriskey_session_state TEXT NOT NULL,
    token_jti TEXT,
    token_exp TIMESTAMPTZ NOT NULL,
    roles_snapshot JSONB NOT NULL DEFAULT '[]'::jsonb,
    token_hash TEXT NOT NULL,
    last_seen_at TIMESTAMPTZ,
    source_ip TEXT,
    user_agent TEXT,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, user_id, ferriskey_session_state)
);

CREATE INDEX IF NOT EXISTS idx_platform_sessions_tenant_user ON platform_sessions (tenant_id, user_id);
CREATE INDEX IF NOT EXISTS idx_platform_sessions_device ON platform_sessions (device_id);
CREATE INDEX IF NOT EXISTS idx_platform_sessions_active ON platform_sessions (user_id, revoked_at, token_exp);
CREATE INDEX IF NOT EXISTS idx_platform_sessions_token_jti ON platform_sessions (token_jti);

CREATE TABLE IF NOT EXISTS ferriskey_role_projection (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    ferriskey_role_id TEXT,
    role_name TEXT NOT NULL,
    role_kind TEXT NOT NULL DEFAULT 'realm',
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, role_name)
);

CREATE TABLE IF NOT EXISTS resource_relations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    relation TEXT NOT NULL,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('user', 'role')),
    subject_id TEXT NOT NULL,
    created_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    disabled_at TIMESTAMPTZ,
    UNIQUE (tenant_id, resource_type, resource_id, relation, subject_type, subject_id)
);

CREATE INDEX IF NOT EXISTS idx_resource_relations_lookup
    ON resource_relations (tenant_id, resource_type, resource_id, relation);

CREATE TABLE IF NOT EXISTS resource_policy_bindings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    action TEXT NOT NULL,
    subject_type TEXT NOT NULL CHECK (subject_type IN ('user', 'role', 'relation')),
    subject_id TEXT NOT NULL,
    effect TEXT NOT NULL CHECK (effect IN ('allow', 'deny', 'review')),
    risk_level TEXT NOT NULL DEFAULT 'low',
    obligations JSONB NOT NULL DEFAULT '{}'::jsonb,
    policy_version TEXT NOT NULL DEFAULT 'local-policy-v1',
    created_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    disabled_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_resource_policy_bindings_lookup
    ON resource_policy_bindings (tenant_id, resource_type, resource_id, action, effect)
    WHERE disabled_at IS NULL;

CREATE TABLE IF NOT EXISTS agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    owner_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    description TEXT,
    draft_config JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'draft',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_agents_tenant ON agents (tenant_id);

CREATE TABLE IF NOT EXISTS agent_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    version_label TEXT NOT NULL,
    config_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    policy_version TEXT NOT NULL DEFAULT 'local-policy-v1',
    schema_hash TEXT,
    status TEXT NOT NULL DEFAULT 'published',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (agent_id, version_label)
);

CREATE TABLE IF NOT EXISTS skills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS skill_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    skill_id UUID NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    version_label TEXT NOT NULL,
    manifest JSONB NOT NULL DEFAULT '{}'::jsonb,
    content_hash TEXT,
    source_uri TEXT,
    status TEXT NOT NULL DEFAULT 'published',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (skill_id, version_label)
);

CREATE TABLE IF NOT EXISTS tools (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    tool_type TEXT NOT NULL DEFAULT 'custom',
    schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS tool_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    tool_id UUID NOT NULL REFERENCES tools(id) ON DELETE CASCADE,
    version_label TEXT NOT NULL,
    schema_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    schema_hash TEXT,
    status TEXT NOT NULL DEFAULT 'published',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tool_id, version_label)
);

CREATE TABLE IF NOT EXISTS mcp_servers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    transport TEXT NOT NULL DEFAULT 'http',
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    secret_ref TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS mcp_tools (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    mcp_server_id UUID NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    schema_hash TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (mcp_server_id, name)
);

CREATE TABLE IF NOT EXISTS llm_providers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    base_url TEXT,
    auth_scheme TEXT NOT NULL DEFAULT 'bearer',
    default_headers_template JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, provider_key, display_name)
);

CREATE TABLE IF NOT EXISTS llm_credentials (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider_id UUID NOT NULL REFERENCES llm_providers(id) ON DELETE CASCADE,
    owner_scope TEXT NOT NULL DEFAULT 'tenant',
    owner_resource_id TEXT,
    secret_ref TEXT NOT NULL,
    secret_hash TEXT,
    expires_at TIMESTAMPTZ,
    rotation_status TEXT NOT NULL DEFAULT 'active',
    created_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS llm_model_profiles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    provider_id UUID NOT NULL REFERENCES llm_providers(id) ON DELETE CASCADE,
    credential_id UUID REFERENCES llm_credentials(id) ON DELETE SET NULL,
    profile_name TEXT NOT NULL,
    model_name TEXT NOT NULL,
    context_window BIGINT,
    max_input_tokens BIGINT,
    max_output_tokens BIGINT,
    temperature DOUBLE PRECISION,
    top_p DOUBLE PRECISION,
    reasoning_effort TEXT,
    response_format JSONB NOT NULL DEFAULT '{}'::jsonb,
    tool_choice_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    rate_limit_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    cost_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, profile_name)
);

CREATE TABLE IF NOT EXISTS sql_connections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    database_kind TEXT NOT NULL,
    host TEXT,
    port INTEGER,
    database_name TEXT,
    username_ref TEXT,
    password_secret_ref TEXT,
    tls_config_ref TEXT,
    allowed_schemas JSONB NOT NULL DEFAULT '[]'::jsonb,
    allowed_tables JSONB NOT NULL DEFAULT '[]'::jsonb,
    max_rows INTEGER NOT NULL DEFAULT 1000,
    statement_timeout_ms INTEGER NOT NULL DEFAULT 30000,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS sql_tools (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS sql_tool_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    sql_tool_id UUID NOT NULL REFERENCES sql_tools(id) ON DELETE CASCADE,
    connection_id UUID NOT NULL REFERENCES sql_connections(id) ON DELETE CASCADE,
    version_label TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('read', 'write', 'ddl')),
    parameter_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    sql_template TEXT NOT NULL,
    query_hash TEXT NOT NULL,
    allowed_roles JSONB NOT NULL DEFAULT '[]'::jsonb,
    risk_level TEXT NOT NULL DEFAULT 'medium',
    requires_approval BOOLEAN NOT NULL DEFAULT true,
    status TEXT NOT NULL DEFAULT 'published',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (sql_tool_id, version_label)
);

CREATE TABLE IF NOT EXISTS agent_version_skill_bindings (
    agent_version_id UUID NOT NULL REFERENCES agent_versions(id) ON DELETE CASCADE,
    skill_version_id UUID NOT NULL REFERENCES skill_versions(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_version_id, skill_version_id)
);

CREATE TABLE IF NOT EXISTS agent_version_tool_bindings (
    agent_version_id UUID NOT NULL REFERENCES agent_versions(id) ON DELETE CASCADE,
    tool_version_id UUID NOT NULL REFERENCES tool_versions(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_version_id, tool_version_id)
);

CREATE TABLE IF NOT EXISTS agent_version_mcp_bindings (
    agent_version_id UUID NOT NULL REFERENCES agent_versions(id) ON DELETE CASCADE,
    mcp_tool_id UUID NOT NULL REFERENCES mcp_tools(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (agent_version_id, mcp_tool_id)
);

CREATE TABLE IF NOT EXISTS projects (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    owner_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    description TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_projects_tenant ON projects (tenant_id);

CREATE TABLE IF NOT EXISTS project_mounts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    virtual_path TEXT NOT NULL,
    backend_type TEXT NOT NULL,
    backend_ref TEXT,
    mount_config JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (project_id, virtual_path)
);

CREATE TABLE IF NOT EXISTS conversations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    created_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    title TEXT NOT NULL DEFAULT 'Untitled conversation',
    status TEXT NOT NULL DEFAULT 'active',
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_conversations_tenant ON conversations (tenant_id);

CREATE TABLE IF NOT EXISTS runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    agent_version_id UUID REFERENCES agent_versions(id) ON DELETE SET NULL,
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    created_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    status TEXT NOT NULL,
    idempotency_key TEXT,
    input JSONB NOT NULL DEFAULT '{}'::jsonb,
    run_config_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    policy_version TEXT NOT NULL DEFAULT 'local-policy-v1',
    risk_policy_version TEXT NOT NULL DEFAULT 'local-risk-v1',
    trace_id TEXT NOT NULL,
    thread_id TEXT,
    checkpoint_id TEXT,
    queued_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_runs_conversation ON runs (conversation_id);
CREATE INDEX IF NOT EXISTS idx_runs_tenant_status ON runs (tenant_id, status);
CREATE UNIQUE INDEX IF NOT EXISTS idx_runs_tenant_idempotency_key
    ON runs (tenant_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS conversation_event_sequences (
    conversation_id UUID PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    next_seq BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS run_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    seq BIGINT NOT NULL,
    event_id TEXT NOT NULL,
    type TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    trace_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (conversation_id, seq),
    UNIQUE (run_id, event_id)
);

CREATE INDEX IF NOT EXISTS idx_run_events_conversation_seq ON run_events (conversation_id, seq);
CREATE INDEX IF NOT EXISTS idx_run_events_run_seq ON run_events (run_id, seq);

CREATE TABLE IF NOT EXISTS event_outbox (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    event_row_id UUID NOT NULL REFERENCES run_events(id) ON DELETE CASCADE,
    target TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    published_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_event_outbox_pending ON event_outbox (status, next_attempt_at);

CREATE TABLE IF NOT EXISTS tool_calls (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    tool_id UUID REFERENCES tools(id) ON DELETE SET NULL,
    tool_name TEXT NOT NULL,
    resource_type TEXT,
    resource_id TEXT,
    args_hash TEXT,
    risk_level TEXT NOT NULL DEFAULT 'low',
    status TEXT NOT NULL,
    decision TEXT NOT NULL,
    policy_version TEXT NOT NULL DEFAULT 'local-policy-v1',
    input_summary TEXT,
    output_summary TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_tool_calls_run ON tool_calls (run_id);

CREATE TABLE IF NOT EXISTS approvals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    tool_call_id UUID REFERENCES tool_calls(id) ON DELETE SET NULL,
    approval_policy_id TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    requested_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    approver_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    request_payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    decision_payload JSONB,
    expires_at TIMESTAMPTZ,
    decided_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_approvals_tenant_status ON approvals (tenant_id, status);

CREATE TABLE IF NOT EXISTS interrupts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    approval_id UUID REFERENCES approvals(id) ON DELETE SET NULL,
    type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    resolved_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    actor_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    actor_device_id UUID,
    session_id UUID,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    action TEXT NOT NULL,
    decision TEXT NOT NULL,
    policy_version TEXT NOT NULL DEFAULT 'local-policy-v1',
    reason_code TEXT,
    run_id UUID,
    conversation_id UUID,
    workflow_run_id UUID,
    tool_call_id UUID,
    approval_id UUID,
    args_hash TEXT,
    input_summary TEXT,
    output_summary TEXT,
    risk_level TEXT,
    ip TEXT,
    user_agent TEXT,
    trace_id TEXT,
    prev_hash TEXT,
    row_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_tenant_created ON audit_logs (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS authz_decisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    actor_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    actor_device_id UUID,
    session_id UUID,
    resource_type TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    action TEXT NOT NULL,
    decision TEXT NOT NULL,
    policy_version TEXT NOT NULL,
    reason_code TEXT,
    obligations JSONB NOT NULL DEFAULT '{}'::jsonb,
    context JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_authz_decisions_tenant_created
    ON authz_decisions (tenant_id, created_at DESC);

CREATE TABLE IF NOT EXISTS object_references (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    bucket TEXT NOT NULL,
    object_key TEXT NOT NULL,
    version_id TEXT,
    etag TEXT,
    content_hash TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    content_type TEXT,
    owner_resource_type TEXT NOT NULL,
    owner_resource_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_object_references_unique_version
    ON object_references (tenant_id, bucket, object_key, (COALESCE(version_id, '')));

CREATE TABLE IF NOT EXISTS file_revisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    path_hash TEXT NOT NULL,
    revision BIGINT NOT NULL,
    etag TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    object_key TEXT NOT NULL,
    object_reference_id UUID REFERENCES object_references(id) ON DELETE SET NULL,
    inline_content TEXT,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    last_writer_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (project_id, path_hash, revision)
);

CREATE INDEX IF NOT EXISTS idx_file_revisions_project_path ON file_revisions (project_id, path_hash, revision DESC);

CREATE TABLE IF NOT EXISTS file_locks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    path_hash TEXT NOT NULL,
    holder_run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    holder_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (tenant_id, project_id, path_hash)
);

CREATE TABLE IF NOT EXISTS memory_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    layer TEXT NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    source_run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    source_event_id UUID REFERENCES run_events(id) ON DELETE SET NULL,
    confidence DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    status TEXT NOT NULL DEFAULT 'candidate',
    visibility TEXT NOT NULL DEFAULT 'private',
    retention_policy TEXT NOT NULL DEFAULT 'default',
    sensitivity TEXT NOT NULL DEFAULT 'normal',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_memory_items_scope ON memory_items (tenant_id, user_id, layer, status);

CREATE TABLE IF NOT EXISTS memory_access_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    memory_id UUID REFERENCES memory_items(id) ON DELETE SET NULL,
    user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    agent_id UUID REFERENCES agents(id) ON DELETE SET NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    action TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS workflow_designs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    owner_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    description TEXT,
    design JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'draft',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS workflow_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workflow_design_id UUID NOT NULL REFERENCES workflow_designs(id) ON DELETE CASCADE,
    version_label TEXT NOT NULL,
    compiled_plan JSONB NOT NULL DEFAULT '{}'::jsonb,
    policy_version TEXT NOT NULL DEFAULT 'local-policy-v1',
    status TEXT NOT NULL DEFAULT 'published',
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (workflow_design_id, version_label)
);

CREATE TABLE IF NOT EXISTS workflow_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workflow_version_id UUID REFERENCES workflow_versions(id) ON DELETE SET NULL,
    conversation_id UUID REFERENCES conversations(id) ON DELETE SET NULL,
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    created_by_user_id UUID REFERENCES platform_users(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    input JSONB NOT NULL DEFAULT '{}'::jsonb,
    trace_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS workflow_node_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    workflow_run_id UUID NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    node_key TEXT NOT NULL,
    agent_run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    input JSONB NOT NULL DEFAULT '{}'::jsonb,
    output JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (workflow_run_id, node_key)
);

CREATE TABLE IF NOT EXISTS workflow_run_dependencies (
    workflow_run_id UUID NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    from_node_key TEXT NOT NULL,
    to_node_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (workflow_run_id, from_node_key, to_node_key)
);

CREATE TABLE IF NOT EXISTS local_exec_requests (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
    project_id UUID REFERENCES projects(id) ON DELETE SET NULL,
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    command JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'queued',
    execution_token_hash TEXT,
    timeout_ms INTEGER NOT NULL DEFAULT 300000,
    max_output_bytes INTEGER NOT NULL DEFAULT 1048576,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS local_exec_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    local_exec_request_id UUID NOT NULL REFERENCES local_exec_requests(id) ON DELETE CASCADE,
    type TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS agent_checkpoints (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL,
    checkpoint_ns TEXT NOT NULL DEFAULT '',
    checkpoint_id TEXT NOT NULL,
    parent_checkpoint_id TEXT,
    type TEXT NOT NULL,
    checkpoint_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, thread_id, checkpoint_ns, checkpoint_id)
);

CREATE TABLE IF NOT EXISTS agent_checkpoint_writes (
    tenant_id UUID NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL,
    checkpoint_ns TEXT NOT NULL DEFAULT '',
    checkpoint_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    idx INTEGER NOT NULL,
    channel TEXT NOT NULL,
    type TEXT NOT NULL,
    value_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, thread_id, checkpoint_ns, checkpoint_id, task_id, idx)
);
