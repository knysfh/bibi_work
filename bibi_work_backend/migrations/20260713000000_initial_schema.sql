-- Canonical PostgreSQL 17 schema for a fresh bibi_work database.
-- Historical backfills and compatibility transitions are intentionally omitted.

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;

COMMENT ON EXTENSION pgcrypto IS 'cryptographic functions';

CREATE FUNCTION public.ensure_audit_log_month_partitions(months_ahead integer) RETURNS TABLE(partition_name text, created boolean)
    LANGUAGE plpgsql
    AS $$
DECLARE
    offset_month INTEGER;
    month_start TIMESTAMPTZ;
    month_end TIMESTAMPTZ;
    target_name TEXT;
BEGIN
    IF months_ahead < 1 OR months_ahead > 24 THEN
        RAISE EXCEPTION 'months_ahead must be between 1 and 24';
    END IF;

    PERFORM pg_advisory_xact_lock(hashtext('audit_log_partition_maintenance'));
    FOR offset_month IN 0..months_ahead LOOP
        month_start := date_trunc('month', CURRENT_TIMESTAMP) + make_interval(months => offset_month);
        month_end := month_start + INTERVAL '1 month';
        target_name := 'audit_logs_p' || to_char(month_start AT TIME ZONE 'UTC', 'YYYYMM');
        partition_name := target_name;
        created := false;

        IF to_regclass('public.' || target_name) IS NULL THEN
            IF EXISTS (
                SELECT 1 FROM audit_logs_default
                WHERE created_at >= month_start AND created_at < month_end
            ) THEN
                RAISE EXCEPTION 'default partition contains rows for %', target_name;
            END IF;
            EXECUTE format(
                'CREATE TABLE %I PARTITION OF audit_logs FOR VALUES FROM (%L) TO (%L)',
                target_name, month_start, month_end
            );
            created := true;
        END IF;
        RETURN NEXT;
    END LOOP;
END
$$;

COMMENT ON FUNCTION public.ensure_audit_log_month_partitions(months_ahead integer) IS 'Creates the current and future UTC monthly audit partitions; fails closed when matching rows already reached the default partition.';

CREATE FUNCTION public.is_valid_secret_ref(value text) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$
    SELECT
        value ~ '^env:(//)?[A-Za-z0-9_]{1,128}$'
        OR (
            value ~ '^vault://[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)+#[A-Za-z0-9_.:/-]{1,128}$'
            AND position('/../' IN value) = 0
        )
        OR (
            value ~ '^kms://[A-Za-z0-9_.:/-]+#[A-Za-z0-9+/=_-]+$'
            AND length(value) <= 33550
        )
$_$;

COMMENT ON FUNCTION public.is_valid_secret_ref(value text) IS 'Validates opaque env, Vault KV, and KMS decrypt references without resolving secret values.';

CREATE FUNCTION public.register_audit_log_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO audit_log_identities (id, tenant_id, created_at)
    VALUES (NEW.id, NEW.tenant_id, NEW.created_at)
    ON CONFLICT (id) DO UPDATE
    SET id = EXCLUDED.id
    WHERE audit_log_identities.tenant_id = EXCLUDED.tenant_id
      AND audit_log_identities.created_at = EXCLUDED.created_at;
    RETURN NEW;
END
$$;

CREATE TABLE public.agent_checkpoint_writes (
    tenant_id uuid NOT NULL,
    thread_id text NOT NULL,
    checkpoint_ns text DEFAULT ''::text NOT NULL,
    checkpoint_id text NOT NULL,
    task_id text NOT NULL,
    idx integer NOT NULL,
    channel text NOT NULL,
    type text NOT NULL,
    value_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agent_checkpoints (
    tenant_id uuid NOT NULL,
    thread_id text NOT NULL,
    checkpoint_ns text DEFAULT ''::text NOT NULL,
    checkpoint_id text NOT NULL,
    parent_checkpoint_id text,
    type text NOT NULL,
    checkpoint_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agent_team_members (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    team_id uuid NOT NULL,
    agent_id uuid NOT NULL,
    agent_version_id uuid,
    role text DEFAULT 'member'::text NOT NULL,
    display_name text NOT NULL,
    slot_order integer NOT NULL,
    policy_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT agent_team_members_role_check CHECK ((role = ANY (ARRAY['leader'::text, 'member'::text]))),
    CONSTRAINT agent_team_members_slot_order_check CHECK ((slot_order >= 0)),
    CONSTRAINT agent_team_members_status_check CHECK ((status = ANY (ARRAY['active'::text, 'disabled'::text])))
);

CREATE TABLE public.agent_team_run_members (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    team_run_id uuid NOT NULL,
    team_member_id uuid,
    run_id uuid,
    agent_id uuid,
    agent_version_id uuid,
    role text NOT NULL,
    display_name text NOT NULL,
    slot_order integer NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    member_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    last_error text,
    queued_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT agent_team_run_members_slot_order_check CHECK ((slot_order >= 0)),
    CONSTRAINT agent_team_run_members_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'waiting_approval'::text, 'blocked'::text, 'cancelling'::text, 'completed'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE TABLE public.agent_team_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    team_id uuid NOT NULL,
    conversation_id uuid NOT NULL,
    workspace_id uuid,
    project_id uuid,
    created_by_user_id uuid,
    status text DEFAULT 'queued'::text NOT NULL,
    input_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    run_config_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    trace_id text NOT NULL,
    thread_id text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    queued_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT agent_team_runs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'waiting_approval'::text, 'cancelling'::text, 'completed'::text, 'failed'::text, 'cancelled'::text])))
);

CREATE TABLE public.agent_teams (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    workspace_id uuid,
    name text NOT NULL,
    description text,
    status text DEFAULT 'active'::text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT agent_teams_status_check CHECK ((status = ANY (ARRAY['active'::text, 'disabled'::text, 'archived'::text])))
);

CREATE TABLE public.agent_version_mcp_bindings (
    agent_version_id uuid NOT NULL,
    mcp_tool_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agent_version_skill_bindings (
    agent_version_id uuid NOT NULL,
    skill_version_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agent_version_sql_tool_bindings (
    agent_version_id uuid NOT NULL,
    sql_tool_version_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agent_version_tool_bindings (
    agent_version_id uuid NOT NULL,
    tool_version_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agent_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    agent_id uuid NOT NULL,
    version_label text NOT NULL,
    config_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    schema_hash text,
    status text DEFAULT 'published'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.agents (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    name text NOT NULL,
    description text,
    draft_config jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'draft'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.approvals (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    conversation_id uuid,
    run_id uuid,
    tool_call_id uuid,
    approval_policy_id text,
    status text DEFAULT 'pending'::text NOT NULL,
    requested_by_user_id uuid,
    approver_user_id uuid,
    request_payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    decision_payload jsonb,
    expires_at timestamp with time zone,
    decided_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    evidence_object_reference_id uuid
);

CREATE TABLE public.artifact_previews (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    artifact_id uuid NOT NULL,
    kind text NOT NULL,
    title text NOT NULL,
    source jsonb DEFAULT '{}'::jsonb NOT NULL,
    content_hash text,
    size_bytes bigint,
    content_type text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.audit_hash_chain_segments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    first_audit_log_id uuid NOT NULL,
    last_audit_log_id uuid NOT NULL,
    rows_count bigint NOT NULL,
    first_prev_hash text,
    last_row_hash text NOT NULL,
    manifest_hash text NOT NULL,
    manifest jsonb NOT NULL,
    object_reference_id uuid,
    sealed_by_user_id uuid,
    sealed_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    archive_status text DEFAULT 'pending'::text NOT NULL,
    archive_attempts integer DEFAULT 0 NOT NULL,
    archived_at timestamp with time zone,
    archive_verified_at timestamp with time zone,
    retention_until timestamp with time zone,
    archive_error text,
    archive_started_at timestamp with time zone,
    CONSTRAINT audit_hash_chain_segments_archive_status_check CHECK ((archive_status = ANY (ARRAY['pending'::text, 'archiving'::text, 'archived'::text, 'failed'::text]))),
    CONSTRAINT audit_hash_chain_segments_rows_count_check CHECK ((rows_count > 0))
);

CREATE TABLE public.audit_legal_holds (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    scope_type text NOT NULL,
    scope_id text,
    resource_type text,
    reason text NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_by_user_id uuid,
    released_by_user_id uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    released_at timestamp with time zone,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT audit_legal_holds_check CHECK ((((scope_type = 'tenant'::text) AND (scope_id IS NULL) AND (resource_type IS NULL)) OR ((scope_type = 'segment'::text) AND (scope_id IS NOT NULL) AND (resource_type IS NULL)) OR ((scope_type = 'resource'::text) AND (scope_id IS NOT NULL) AND (resource_type IS NOT NULL)))),
    CONSTRAINT audit_legal_holds_check1 CHECK ((((status = 'active'::text) AND (released_at IS NULL)) OR ((status = 'released'::text) AND (released_at IS NOT NULL)))),
    CONSTRAINT audit_legal_holds_reason_check CHECK (((length(TRIM(BOTH FROM reason)) >= 1) AND (length(TRIM(BOTH FROM reason)) <= 2000))),
    CONSTRAINT audit_legal_holds_scope_type_check CHECK ((scope_type = ANY (ARRAY['tenant'::text, 'segment'::text, 'resource'::text]))),
    CONSTRAINT audit_legal_holds_status_check CHECK ((status = ANY (ARRAY['active'::text, 'released'::text])))
);

CREATE TABLE public.audit_log_identities (
    id uuid NOT NULL,
    tenant_id uuid NOT NULL,
    created_at timestamp with time zone NOT NULL
);

COMMENT ON TABLE public.audit_log_identities IS 'Stable globally unique audit IDs retained independently from detachable time partitions.';

CREATE TABLE public.audit_logs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    actor_user_id uuid,
    actor_device_id uuid,
    session_id uuid,
    resource_type text NOT NULL,
    resource_id text NOT NULL,
    action text NOT NULL,
    decision text NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    reason_code text,
    run_id uuid,
    conversation_id uuid,
    workflow_run_id uuid,
    tool_call_id uuid,
    approval_id uuid,
    args_hash text,
    input_summary text,
    output_summary text,
    risk_level text,
    ip text,
    user_agent text,
    trace_id text,
    prev_hash text,
    row_hash text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
)
PARTITION BY RANGE (created_at);

COMMENT ON TABLE public.audit_logs IS 'Monthly range-partitioned audit payloads; partitions may only be detached after archive, retention, and legal-hold checks.';

CREATE TABLE public.audit_logs_default (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    actor_user_id uuid,
    actor_device_id uuid,
    session_id uuid,
    resource_type text NOT NULL,
    resource_id text NOT NULL,
    action text NOT NULL,
    decision text NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    reason_code text,
    run_id uuid,
    conversation_id uuid,
    workflow_run_id uuid,
    tool_call_id uuid,
    approval_id uuid,
    args_hash text,
    input_summary text,
    output_summary text,
    risk_level text,
    ip text,
    user_agent text,
    trace_id text,
    prev_hash text,
    row_hash text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.authz_decisions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    actor_user_id uuid,
    actor_device_id uuid,
    session_id uuid,
    resource_type text NOT NULL,
    resource_id text NOT NULL,
    action text NOT NULL,
    decision text NOT NULL,
    policy_version text NOT NULL,
    reason_code text,
    obligations jsonb DEFAULT '{}'::jsonb NOT NULL,
    context jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.biwork_skill_import_history (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    operation_id uuid NOT NULL,
    source_label text NOT NULL,
    source_path text,
    source_name text NOT NULL,
    skill_id uuid,
    skill_name text,
    status text NOT NULL,
    error_code text,
    error_path text,
    actual_bytes bigint,
    limit_bytes bigint,
    line_number integer,
    column_number integer,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.channel_authorized_users (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    platform text NOT NULL,
    platform_user_id text NOT NULL,
    display_name text,
    status text DEFAULT 'active'::text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    authorized_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_active_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.channel_connectors (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    connector_key text NOT NULL,
    source_extension_package_id uuid,
    runtime_kind text DEFAULT 'builtin'::text NOT NULL,
    status text DEFAULT 'disconnected'::text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    connected boolean DEFAULT false NOT NULL,
    config_ref jsonb DEFAULT '{}'::jsonb NOT NULL,
    policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    last_connected_at timestamp with time zone,
    last_error text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.channel_pairing_requests (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    platform text NOT NULL,
    code text NOT NULL,
    platform_user_id text NOT NULL,
    display_name text,
    status text DEFAULT 'pending'::text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    decided_by_user_id uuid,
    decided_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.channel_platform_settings (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    platform text NOT NULL,
    assistant_profile_id uuid,
    default_model_profile_id uuid,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.channel_sessions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    platform text NOT NULL,
    channel_user_id uuid,
    agent_type text DEFAULT 'acp'::text NOT NULL,
    conversation_id uuid,
    workspace text,
    chat_id text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_activity_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    ended_at timestamp with time zone
);

CREATE TABLE public.conversation_event_sequences (
    conversation_id uuid NOT NULL,
    next_seq bigint NOT NULL
);

CREATE TABLE public.conversations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    created_by_user_id uuid,
    project_id uuid,
    agent_id uuid,
    title text DEFAULT 'Untitled conversation'::text NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone,
    workspace_id uuid
);

CREATE TABLE public.device_extension_states (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    device_id uuid NOT NULL,
    extension_package_id uuid NOT NULL,
    installed boolean DEFAULT false NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    install_status text DEFAULT 'not_installed'::text NOT NULL,
    last_error text,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.devices (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    device_fingerprint text NOT NULL,
    device_name text NOT NULL,
    platform text NOT NULL,
    public_key text,
    trust_level text DEFAULT 'standard'::text NOT NULL,
    last_seen_at timestamp with time zone,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    app_kind text DEFAULT 'unknown'::text NOT NULL
);

CREATE TABLE public.event_outbox (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    event_row_id uuid NOT NULL,
    target text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    next_attempt_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    published_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.extension_contributions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    extension_package_id uuid NOT NULL,
    contribution_type text NOT NULL,
    contribution_key text NOT NULL,
    manifest jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.extension_packages (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    extension_name text NOT NULL,
    source text DEFAULT 'local'::text NOT NULL,
    version text,
    integrity text,
    manifest jsonb DEFAULT '{}'::jsonb NOT NULL,
    risk_level text DEFAULT 'moderate'::text NOT NULL,
    status text DEFAULT 'discovered'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.ferriskey_role_projection (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    ferriskey_role_id text,
    role_name text NOT NULL,
    role_kind text DEFAULT 'realm'::text NOT NULL,
    last_seen_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.file_locks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    project_id uuid NOT NULL,
    path_hash text NOT NULL,
    holder_run_id uuid,
    holder_user_id uuid,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    path text,
    lock_token text DEFAULT (gen_random_uuid())::text NOT NULL,
    reason text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.file_revisions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    project_id uuid NOT NULL,
    path text NOT NULL,
    path_hash text NOT NULL,
    revision bigint NOT NULL,
    etag text NOT NULL,
    content_hash text NOT NULL,
    object_key text NOT NULL,
    object_reference_id uuid,
    inline_content text,
    size_bytes bigint DEFAULT 0 NOT NULL,
    reason text NOT NULL,
    run_id uuid,
    last_writer_user_id uuid,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.file_search_chunks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    file_revision_id uuid NOT NULL,
    tenant_id uuid NOT NULL,
    project_id uuid NOT NULL,
    path text NOT NULL,
    path_hash text NOT NULL,
    revision bigint NOT NULL,
    content_hash text NOT NULL,
    chunk_index integer NOT NULL,
    byte_start bigint NOT NULL,
    byte_end bigint NOT NULL,
    source_size_bytes bigint NOT NULL,
    indexed_bytes bigint NOT NULL,
    is_truncated boolean DEFAULT false NOT NULL,
    extraction_strategy text NOT NULL,
    content_text text NOT NULL,
    search_vector tsvector GENERATED ALWAYS AS (to_tsvector('simple'::regconfig, ((COALESCE(path, ''::text) || ' '::text) || COALESCE(content_text, ''::text)))) STORED,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT file_search_chunks_check CHECK (((byte_start >= 0) AND (byte_end >= byte_start))),
    CONSTRAINT file_search_chunks_check1 CHECK (((source_size_bytes >= 0) AND (indexed_bytes >= 0))),
    CONSTRAINT file_search_chunks_chunk_index_check CHECK ((chunk_index >= 0)),
    CONSTRAINT file_search_chunks_extraction_strategy_check CHECK ((extraction_strategy = ANY (ARRAY['full_chunks'::text, 'uniform_sample'::text])))
);

CREATE TABLE public.interrupts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    conversation_id uuid,
    run_id uuid,
    approval_id uuid,
    type text NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    resolved_at timestamp with time zone
);

CREATE TABLE public.llm_credential_rotation_attempts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    credential_id uuid NOT NULL,
    status text NOT NULL,
    resolver_scheme text NOT NULL,
    previous_ref_hash text NOT NULL,
    new_ref_hash text,
    error_summary text,
    started_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    completed_at timestamp with time zone,
    CONSTRAINT llm_credential_rotation_attempts_resolver_scheme_check CHECK ((resolver_scheme = ANY (ARRAY['env'::text, 'vault'::text, 'kms'::text]))),
    CONSTRAINT llm_credential_rotation_attempts_status_check CHECK ((status = ANY (ARRAY['running'::text, 'succeeded'::text, 'failed'::text])))
);

CREATE TABLE public.llm_credentials (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    provider_id uuid NOT NULL,
    owner_scope text DEFAULT 'tenant'::text NOT NULL,
    owner_resource_id text,
    secret_ref text NOT NULL,
    secret_hash text,
    expires_at timestamp with time zone,
    rotation_status text DEFAULT 'active'::text NOT NULL,
    created_by_user_id uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    revoked_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_rotated_at timestamp with time zone,
    rotated_by_user_id uuid,
    auto_rotation_enabled boolean DEFAULT false NOT NULL,
    rotation_interval_seconds bigint,
    rotate_before_seconds bigint DEFAULT 86400 NOT NULL,
    next_rotation_at timestamp with time zone,
    rotation_started_at timestamp with time zone,
    rotation_claim_id uuid,
    rotation_attempts integer DEFAULT 0 NOT NULL,
    rotation_error text,
    CONSTRAINT llm_credentials_rotation_interval_check CHECK ((((NOT auto_rotation_enabled) AND (rotation_interval_seconds IS NULL)) OR (auto_rotation_enabled AND ((rotation_interval_seconds >= 300) AND (rotation_interval_seconds <= 31536000)) AND ((rotate_before_seconds >= 0) AND (rotate_before_seconds <= 2592000)) AND (next_rotation_at IS NOT NULL)))),
    CONSTRAINT llm_credentials_secret_ref_scheme_check CHECK (public.is_valid_secret_ref(secret_ref))
);

CREATE TABLE public.llm_model_profiles (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    provider_id uuid NOT NULL,
    credential_id uuid,
    profile_name text NOT NULL,
    model_name text NOT NULL,
    context_window bigint,
    max_input_tokens bigint,
    max_output_tokens bigint,
    temperature double precision,
    top_p double precision,
    reasoning_effort text,
    response_format jsonb DEFAULT '{}'::jsonb NOT NULL,
    tool_choice_policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    rate_limit_policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    cost_policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.llm_providers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    provider_key text NOT NULL,
    display_name text NOT NULL,
    base_url text,
    auth_scheme text DEFAULT 'bearer'::text NOT NULL,
    default_headers_template jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.local_exec_events (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    local_exec_request_id uuid NOT NULL,
    type text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.local_exec_requests (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    device_id uuid,
    project_id uuid,
    run_id uuid,
    command jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    timeout_ms integer DEFAULT 300000 NOT NULL,
    max_output_bytes integer DEFAULT 1048576 NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT local_exec_requests_max_output_bytes_check CHECK (((max_output_bytes >= 1) AND (max_output_bytes <= 8388608))),
    CONSTRAINT local_exec_requests_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'dispatching'::text, 'completed'::text, 'failed'::text, 'cancelled'::text, 'timed_out'::text]))),
    CONSTRAINT local_exec_requests_timeout_ms_check CHECK (((timeout_ms >= 1000) AND (timeout_ms <= 300000)))
);

CREATE TABLE public.local_mounts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    device_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    display_name text NOT NULL,
    virtual_path text NOT NULL,
    capabilities jsonb DEFAULT '["read"]'::jsonb NOT NULL,
    include_globs jsonb DEFAULT '[]'::jsonb NOT NULL,
    exclude_globs jsonb DEFAULT '[]'::jsonb NOT NULL,
    trust_state text DEFAULT 'untrusted'::text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.mcp_servers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    transport text DEFAULT 'http'::text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    secret_ref text,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone,
    health_status text DEFAULT 'unknown'::text NOT NULL,
    last_health_check_at timestamp with time zone,
    last_discovered_at timestamp with time zone,
    consecutive_failures integer DEFAULT 0 NOT NULL,
    health_error text,
    CONSTRAINT mcp_servers_consecutive_failures_check CHECK ((consecutive_failures >= 0)),
    CONSTRAINT mcp_servers_health_error_length_check CHECK (((health_error IS NULL) OR (char_length(health_error) <= 2000))),
    CONSTRAINT mcp_servers_health_status_check CHECK ((health_status = ANY (ARRAY['unknown'::text, 'healthy'::text, 'unhealthy'::text, 'unsupported'::text]))),
    CONSTRAINT mcp_servers_secret_ref_scheme_check CHECK (((secret_ref IS NULL) OR public.is_valid_secret_ref(secret_ref))),
    CONSTRAINT mcp_servers_status_check CHECK ((status = ANY (ARRAY['active'::text, 'disabled'::text, 'deleted'::text]))),
    CONSTRAINT mcp_servers_transport_check CHECK ((transport = ANY (ARRAY['stdio'::text, 'http'::text, 'sse'::text, 'streamable-http'::text, 'json-rpc'::text])))
);

CREATE TABLE public.mcp_tools (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    mcp_server_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    schema jsonb DEFAULT '{}'::jsonb NOT NULL,
    schema_hash text,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.memory_access_logs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    memory_id uuid,
    user_id uuid,
    agent_id uuid,
    run_id uuid,
    action text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.memory_embeddings (
    memory_id uuid NOT NULL,
    tenant_id uuid NOT NULL,
    provider text DEFAULT 'external-http'::text NOT NULL,
    embedding_model text,
    vector_dimension integer,
    vector_hash text,
    qdrant_collection text DEFAULT 'bibi_work_memories'::text NOT NULL,
    qdrant_point_id text,
    index_status text DEFAULT 'pending'::text NOT NULL,
    last_error text,
    indexed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.memory_feedback (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    memory_id uuid,
    user_id uuid,
    agent_id uuid,
    run_id uuid,
    feedback text NOT NULL,
    score double precision,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.memory_ingestion_jobs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    memory_id uuid NOT NULL,
    job_type text DEFAULT 'upsert'::text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    last_error text,
    scheduled_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.memory_items (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    user_id uuid,
    agent_id uuid,
    project_id uuid,
    layer text NOT NULL,
    content text NOT NULL,
    content_hash text NOT NULL,
    source_run_id uuid,
    source_event_id uuid,
    confidence double precision DEFAULT 0.0 NOT NULL,
    status text DEFAULT 'candidate'::text NOT NULL,
    visibility text DEFAULT 'private'::text NOT NULL,
    retention_policy text DEFAULT 'default'::text NOT NULL,
    sensitivity text DEFAULT 'normal'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.object_references (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    bucket text NOT NULL,
    object_key text NOT NULL,
    version_id text,
    etag text,
    content_hash text NOT NULL,
    size_bytes bigint DEFAULT 0 NOT NULL,
    content_type text,
    owner_resource_type text NOT NULL,
    owner_resource_id text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.platform_sessions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    device_id uuid NOT NULL,
    ferriskey_subject text NOT NULL,
    ferriskey_session_state text NOT NULL,
    token_jti text,
    token_exp timestamp with time zone NOT NULL,
    roles_snapshot jsonb DEFAULT '[]'::jsonb NOT NULL,
    token_hash text NOT NULL,
    last_seen_at timestamp with time zone,
    source_ip text,
    user_agent text,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    client_kind text DEFAULT 'desktop'::text NOT NULL,
    revocation_reason text
);

CREATE TABLE public.platform_users (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    ferriskey_subject text NOT NULL,
    username text,
    email text,
    display_name text,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.project_mounts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    project_id uuid NOT NULL,
    virtual_path text NOT NULL,
    backend_type text NOT NULL,
    backend_ref text,
    mount_config jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.projects (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    name text NOT NULL,
    description text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.resource_policy_bindings (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    resource_type text NOT NULL,
    resource_id text NOT NULL,
    action text NOT NULL,
    subject_type text NOT NULL,
    subject_id text NOT NULL,
    effect text NOT NULL,
    risk_level text DEFAULT 'low'::text NOT NULL,
    obligations jsonb DEFAULT '{}'::jsonb NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    created_by_user_id uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    disabled_at timestamp with time zone,
    CONSTRAINT resource_policy_bindings_effect_check CHECK ((effect = ANY (ARRAY['allow'::text, 'deny'::text, 'review'::text]))),
    CONSTRAINT resource_policy_bindings_subject_type_check CHECK ((subject_type = ANY (ARRAY['user'::text, 'role'::text, 'relation'::text])))
);

CREATE TABLE public.resource_relations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    resource_type text NOT NULL,
    resource_id text NOT NULL,
    relation text NOT NULL,
    subject_type text NOT NULL,
    subject_id text NOT NULL,
    created_by_user_id uuid,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    disabled_at timestamp with time zone,
    CONSTRAINT resource_relations_subject_type_check CHECK ((subject_type = ANY (ARRAY['user'::text, 'role'::text])))
);

CREATE TABLE public.run_event_links (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    run_event_id uuid NOT NULL,
    link_type text NOT NULL,
    link_id uuid,
    link_key text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.run_events (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    conversation_id uuid NOT NULL,
    run_id uuid,
    seq bigint NOT NULL,
    event_id text NOT NULL,
    type text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    trace_id text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    stream_offset bigint GENERATED BY DEFAULT AS IDENTITY (
        SEQUENCE NAME public.run_events_stream_offset_seq
        START WITH 1
        INCREMENT BY 1
        NO MINVALUE
        NO MAXVALUE
        CACHE 1
    ) NOT NULL
);

CREATE TABLE public.runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    conversation_id uuid NOT NULL,
    agent_id uuid,
    agent_version_id uuid,
    project_id uuid,
    created_by_user_id uuid,
    status text NOT NULL,
    idempotency_key text,
    input jsonb DEFAULT '{}'::jsonb NOT NULL,
    run_config_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    risk_policy_version text DEFAULT 'local-risk-v1'::text NOT NULL,
    trace_id text NOT NULL,
    thread_id text,
    checkpoint_id text,
    queued_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    workspace_id uuid,
    run_scope_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL
);

CREATE TABLE public.scheduled_job_artifacts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    scheduled_job_id uuid NOT NULL,
    artifact_kind text NOT NULL,
    object_reference_id uuid,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    conversation_id uuid,
    source_event_id uuid,
    artifact_key text DEFAULT ''::text NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.scheduled_job_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    scheduled_job_id uuid NOT NULL,
    run_id uuid,
    workflow_run_id uuid,
    status text NOT NULL,
    triggered_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    completed_at timestamp with time zone,
    summary jsonb DEFAULT '{}'::jsonb NOT NULL
);

CREATE TABLE public.scheduled_jobs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    name text NOT NULL,
    source_conversation_id uuid,
    target_mode text DEFAULT 'existing'::text NOT NULL,
    target_conversation_id uuid,
    assistant_profile_id uuid,
    agent_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    prompt_template text NOT NULL,
    workspace_id uuid,
    model_profile_id uuid,
    schedule_kind text NOT NULL,
    schedule_expr text NOT NULL,
    timezone text,
    enabled boolean DEFAULT true NOT NULL,
    created_by_user_id uuid NOT NULL,
    created_from text DEFAULT 'user'::text NOT NULL,
    description text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    skill_content text,
    next_run_at timestamp with time zone,
    last_run_at timestamp with time zone,
    last_status text,
    last_error text,
    run_count integer DEFAULT 0 NOT NULL,
    retry_count integer DEFAULT 0 NOT NULL,
    max_retries integer DEFAULT 3 NOT NULL,
    deleted_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.skill_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    skill_id uuid NOT NULL,
    version_label text NOT NULL,
    manifest jsonb DEFAULT '{}'::jsonb NOT NULL,
    content_hash text,
    source_uri text,
    status text DEFAULT 'published'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.skills (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.sql_connections (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    name text NOT NULL,
    database_kind text NOT NULL,
    host text,
    port integer,
    database_name text,
    username_ref text,
    password_secret_ref text,
    tls_config_ref text,
    allowed_schemas jsonb DEFAULT '[]'::jsonb NOT NULL,
    allowed_tables jsonb DEFAULT '[]'::jsonb NOT NULL,
    max_rows integer DEFAULT 1000 NOT NULL,
    statement_timeout_ms integer DEFAULT 30000 NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT sql_connections_password_secret_ref_scheme_check CHECK (((password_secret_ref IS NULL) OR public.is_valid_secret_ref(password_secret_ref))),
    CONSTRAINT sql_connections_tls_config_ref_scheme_check CHECK (((tls_config_ref IS NULL) OR public.is_valid_secret_ref(tls_config_ref)))
);

CREATE TABLE public.sql_tool_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    sql_tool_id uuid NOT NULL,
    connection_id uuid NOT NULL,
    version_label text NOT NULL,
    operation text NOT NULL,
    parameter_schema jsonb DEFAULT '{}'::jsonb NOT NULL,
    sql_template text NOT NULL,
    query_hash text NOT NULL,
    allowed_roles jsonb DEFAULT '[]'::jsonb NOT NULL,
    risk_level text DEFAULT 'medium'::text NOT NULL,
    requires_approval boolean DEFAULT true NOT NULL,
    status text DEFAULT 'published'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT sql_tool_versions_operation_check CHECK ((operation = ANY (ARRAY['read'::text, 'write'::text, 'ddl'::text])))
);

CREATE TABLE public.sql_tools (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.tenants (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.tool_calls (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    conversation_id uuid,
    run_id uuid,
    tool_id uuid,
    tool_name text NOT NULL,
    resource_type text,
    resource_id text,
    args_hash text,
    risk_level text DEFAULT 'low'::text NOT NULL,
    status text NOT NULL,
    decision text NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    input_summary text,
    output_summary text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    completed_at timestamp with time zone,
    error_summary text,
    evidence_object_reference_id uuid
);

CREATE TABLE public.tool_result_artifacts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    run_id uuid,
    tool_call_id uuid,
    view_kind text NOT NULL,
    ref_kind text NOT NULL,
    project_id uuid NOT NULL,
    path text NOT NULL,
    revision bigint NOT NULL,
    file_revision_id uuid NOT NULL,
    object_reference_id uuid NOT NULL,
    content_hash text NOT NULL,
    content_type text NOT NULL,
    size_bytes bigint NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.tool_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    tool_id uuid NOT NULL,
    version_label text NOT NULL,
    schema_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    schema_hash text,
    status text DEFAULT 'published'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    secret_ref text,
    CONSTRAINT tool_versions_secret_ref_scheme_check CHECK (((secret_ref IS NULL) OR public.is_valid_secret_ref(secret_ref)))
);

CREATE TABLE public.tools (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    name text NOT NULL,
    description text,
    tool_type text DEFAULT 'custom'::text NOT NULL,
    schema jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.user_tenant_memberships (
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text DEFAULT 'member'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.user_ui_preferences (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    key text NOT NULL,
    value jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.workflow_designs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    name text NOT NULL,
    description text,
    design jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'draft'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

CREATE TABLE public.workflow_node_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    workflow_run_id uuid NOT NULL,
    node_key text NOT NULL,
    agent_run_id uuid,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    input jsonb DEFAULT '{}'::jsonb NOT NULL,
    output jsonb,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    max_attempts integer DEFAULT 1 NOT NULL,
    backoff_sec integer DEFAULT 0 NOT NULL,
    timeout_sec integer,
    not_before timestamp with time zone,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    last_error text
);

CREATE TABLE public.workflow_run_dependencies (
    workflow_run_id uuid NOT NULL,
    from_node_key text NOT NULL,
    to_node_key text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.workflow_runs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    workflow_version_id uuid,
    conversation_id uuid,
    project_id uuid,
    created_by_user_id uuid,
    status text DEFAULT 'queued'::text NOT NULL,
    input jsonb DEFAULT '{}'::jsonb NOT NULL,
    trace_id text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.workflow_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    workflow_design_id uuid NOT NULL,
    version_label text NOT NULL,
    compiled_plan jsonb DEFAULT '{}'::jsonb NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    status text DEFAULT 'published'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.workspace_pins (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    user_id uuid NOT NULL,
    target_type text NOT NULL,
    target_id uuid NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE public.workspaces (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    name text NOT NULL,
    remote_project_id uuid,
    default_agent_id uuid,
    default_agent_version_id uuid,
    default_model_profile_id uuid,
    tool_policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    file_policy jsonb DEFAULT '{}'::jsonb NOT NULL,
    include_globs jsonb DEFAULT '[]'::jsonb NOT NULL,
    exclude_globs jsonb DEFAULT '[]'::jsonb NOT NULL,
    trust_state text DEFAULT 'untrusted'::text NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone
);

ALTER TABLE ONLY public.audit_logs ATTACH PARTITION public.audit_logs_default DEFAULT;

ALTER TABLE ONLY public.agent_checkpoint_writes
    ADD CONSTRAINT agent_checkpoint_writes_pkey PRIMARY KEY (tenant_id, thread_id, checkpoint_ns, checkpoint_id, task_id, idx);

ALTER TABLE ONLY public.agent_checkpoints
    ADD CONSTRAINT agent_checkpoints_pkey PRIMARY KEY (tenant_id, thread_id, checkpoint_ns, checkpoint_id);

ALTER TABLE ONLY public.agent_team_members
    ADD CONSTRAINT agent_team_members_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_teams
    ADD CONSTRAINT agent_teams_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agent_version_mcp_bindings
    ADD CONSTRAINT agent_version_mcp_bindings_pkey PRIMARY KEY (agent_version_id, mcp_tool_id);

ALTER TABLE ONLY public.agent_version_skill_bindings
    ADD CONSTRAINT agent_version_skill_bindings_pkey PRIMARY KEY (agent_version_id, skill_version_id);

ALTER TABLE ONLY public.agent_version_sql_tool_bindings
    ADD CONSTRAINT agent_version_sql_tool_bindings_pkey PRIMARY KEY (agent_version_id, sql_tool_version_id);

ALTER TABLE ONLY public.agent_version_tool_bindings
    ADD CONSTRAINT agent_version_tool_bindings_pkey PRIMARY KEY (agent_version_id, tool_version_id);

ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_agent_id_version_label_key UNIQUE (agent_id, version_label);

ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.agents
    ADD CONSTRAINT agents_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.artifact_previews
    ADD CONSTRAINT artifact_previews_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.artifact_previews
    ADD CONSTRAINT artifact_previews_tenant_id_artifact_id_key UNIQUE (tenant_id, artifact_id);

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_tenant_id_first_audit_log_id_last_key UNIQUE (tenant_id, first_audit_log_id, last_audit_log_id);

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_tenant_id_manifest_hash_key UNIQUE (tenant_id, manifest_hash);

ALTER TABLE ONLY public.audit_legal_holds
    ADD CONSTRAINT audit_legal_holds_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.audit_log_identities
    ADD CONSTRAINT audit_log_identities_id_created_at_key UNIQUE (id, created_at);

ALTER TABLE ONLY public.audit_log_identities
    ADD CONSTRAINT audit_log_identities_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_pkey PRIMARY KEY (created_at, id);

ALTER TABLE ONLY public.audit_logs_default
    ADD CONSTRAINT audit_logs_default_pkey PRIMARY KEY (created_at, id);

ALTER TABLE ONLY public.authz_decisions
    ADD CONSTRAINT authz_decisions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.biwork_skill_import_history
    ADD CONSTRAINT biwork_skill_import_history_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_authorized_users
    ADD CONSTRAINT channel_authorized_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_authorized_users
    ADD CONSTRAINT channel_authorized_users_tenant_id_platform_platform_user_i_key UNIQUE (tenant_id, platform, platform_user_id);

ALTER TABLE ONLY public.channel_connectors
    ADD CONSTRAINT channel_connectors_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_connectors
    ADD CONSTRAINT channel_connectors_tenant_id_connector_key_key UNIQUE (tenant_id, connector_key);

ALTER TABLE ONLY public.channel_pairing_requests
    ADD CONSTRAINT channel_pairing_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_pairing_requests
    ADD CONSTRAINT channel_pairing_requests_tenant_id_code_key UNIQUE (tenant_id, code);

ALTER TABLE ONLY public.channel_platform_settings
    ADD CONSTRAINT channel_platform_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.channel_platform_settings
    ADD CONSTRAINT channel_platform_settings_tenant_id_platform_key UNIQUE (tenant_id, platform);

ALTER TABLE ONLY public.channel_sessions
    ADD CONSTRAINT channel_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.conversation_event_sequences
    ADD CONSTRAINT conversation_event_sequences_pkey PRIMARY KEY (conversation_id);

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.device_extension_states
    ADD CONSTRAINT device_extension_states_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.device_extension_states
    ADD CONSTRAINT device_extension_states_tenant_id_device_id_extension_packa_key UNIQUE (tenant_id, device_id, extension_package_id);

ALTER TABLE ONLY public.devices
    ADD CONSTRAINT devices_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.event_outbox
    ADD CONSTRAINT event_outbox_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.extension_contributions
    ADD CONSTRAINT extension_contributions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.extension_contributions
    ADD CONSTRAINT extension_contributions_tenant_id_extension_package_id_cont_key UNIQUE (tenant_id, extension_package_id, contribution_type, contribution_key);

ALTER TABLE ONLY public.extension_packages
    ADD CONSTRAINT extension_packages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.extension_packages
    ADD CONSTRAINT extension_packages_tenant_id_extension_name_key UNIQUE (tenant_id, extension_name);

ALTER TABLE ONLY public.ferriskey_role_projection
    ADD CONSTRAINT ferriskey_role_projection_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.ferriskey_role_projection
    ADD CONSTRAINT ferriskey_role_projection_tenant_id_role_name_key UNIQUE (tenant_id, role_name);

ALTER TABLE ONLY public.file_locks
    ADD CONSTRAINT file_locks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.file_locks
    ADD CONSTRAINT file_locks_tenant_id_project_id_path_hash_key UNIQUE (tenant_id, project_id, path_hash);

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_project_id_path_hash_revision_key UNIQUE (project_id, path_hash, revision);

ALTER TABLE ONLY public.file_search_chunks
    ADD CONSTRAINT file_search_chunks_file_revision_id_chunk_index_key UNIQUE (file_revision_id, chunk_index);

ALTER TABLE ONLY public.file_search_chunks
    ADD CONSTRAINT file_search_chunks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.interrupts
    ADD CONSTRAINT interrupts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.llm_credential_rotation_attempts
    ADD CONSTRAINT llm_credential_rotation_attempts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.llm_credentials
    ADD CONSTRAINT llm_credentials_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.llm_model_profiles
    ADD CONSTRAINT llm_model_profiles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.llm_model_profiles
    ADD CONSTRAINT llm_model_profiles_tenant_id_profile_name_key UNIQUE (tenant_id, profile_name);

ALTER TABLE ONLY public.llm_providers
    ADD CONSTRAINT llm_providers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.llm_providers
    ADD CONSTRAINT llm_providers_tenant_id_provider_key_display_name_key UNIQUE (tenant_id, provider_key, display_name);

ALTER TABLE ONLY public.local_exec_events
    ADD CONSTRAINT local_exec_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.local_exec_requests
    ADD CONSTRAINT local_exec_requests_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.local_mounts
    ADD CONSTRAINT local_mounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.local_mounts
    ADD CONSTRAINT local_mounts_user_id_device_id_workspace_id_virtual_path_key UNIQUE (user_id, device_id, workspace_id, virtual_path);

ALTER TABLE ONLY public.mcp_servers
    ADD CONSTRAINT mcp_servers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.mcp_tools
    ADD CONSTRAINT mcp_tools_mcp_server_id_name_key UNIQUE (mcp_server_id, name);

ALTER TABLE ONLY public.mcp_tools
    ADD CONSTRAINT mcp_tools_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.memory_embeddings
    ADD CONSTRAINT memory_embeddings_pkey PRIMARY KEY (memory_id);

ALTER TABLE ONLY public.memory_feedback
    ADD CONSTRAINT memory_feedback_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.memory_ingestion_jobs
    ADD CONSTRAINT memory_ingestion_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.object_references
    ADD CONSTRAINT object_references_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.platform_sessions
    ADD CONSTRAINT platform_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.platform_sessions
    ADD CONSTRAINT platform_sessions_tenant_id_user_id_ferriskey_session_state_key UNIQUE (tenant_id, user_id, ferriskey_session_state);

ALTER TABLE ONLY public.platform_users
    ADD CONSTRAINT platform_users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.platform_users
    ADD CONSTRAINT platform_users_tenant_id_ferriskey_subject_key UNIQUE (tenant_id, ferriskey_subject);

ALTER TABLE ONLY public.project_mounts
    ADD CONSTRAINT project_mounts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.project_mounts
    ADD CONSTRAINT project_mounts_project_id_virtual_path_key UNIQUE (project_id, virtual_path);

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.resource_policy_bindings
    ADD CONSTRAINT resource_policy_bindings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.resource_relations
    ADD CONSTRAINT resource_relations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.resource_relations
    ADD CONSTRAINT resource_relations_tenant_id_resource_type_resource_id_rela_key UNIQUE (tenant_id, resource_type, resource_id, relation, subject_type, subject_id);

ALTER TABLE ONLY public.run_event_links
    ADD CONSTRAINT run_event_links_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.run_events
    ADD CONSTRAINT run_events_conversation_id_seq_key UNIQUE (conversation_id, seq);

ALTER TABLE ONLY public.run_events
    ADD CONSTRAINT run_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.run_events
    ADD CONSTRAINT run_events_run_id_event_id_key UNIQUE (run_id, event_id);

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.scheduled_job_artifacts
    ADD CONSTRAINT scheduled_job_artifacts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.scheduled_job_runs
    ADD CONSTRAINT scheduled_job_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.skill_versions
    ADD CONSTRAINT skill_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.skill_versions
    ADD CONSTRAINT skill_versions_skill_id_version_label_key UNIQUE (skill_id, version_label);

ALTER TABLE ONLY public.skills
    ADD CONSTRAINT skills_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sql_connections
    ADD CONSTRAINT sql_connections_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sql_connections
    ADD CONSTRAINT sql_connections_tenant_id_name_key UNIQUE (tenant_id, name);

ALTER TABLE ONLY public.sql_tool_versions
    ADD CONSTRAINT sql_tool_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sql_tool_versions
    ADD CONSTRAINT sql_tool_versions_sql_tool_id_version_label_key UNIQUE (sql_tool_id, version_label);

ALTER TABLE ONLY public.sql_tools
    ADD CONSTRAINT sql_tools_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.sql_tools
    ADD CONSTRAINT sql_tools_tenant_id_name_key UNIQUE (tenant_id, name);

ALTER TABLE ONLY public.tenants
    ADD CONSTRAINT tenants_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tenants
    ADD CONSTRAINT tenants_slug_key UNIQUE (slug);

ALTER TABLE ONLY public.tool_calls
    ADD CONSTRAINT tool_calls_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_object_reference_id_key UNIQUE (object_reference_id);

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tool_versions
    ADD CONSTRAINT tool_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tool_versions
    ADD CONSTRAINT tool_versions_tool_id_version_label_key UNIQUE (tool_id, version_label);

ALTER TABLE ONLY public.tools
    ADD CONSTRAINT tools_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_tenant_memberships
    ADD CONSTRAINT user_tenant_memberships_pkey PRIMARY KEY (tenant_id, user_id);

ALTER TABLE ONLY public.user_ui_preferences
    ADD CONSTRAINT user_ui_preferences_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.user_ui_preferences
    ADD CONSTRAINT user_ui_preferences_tenant_id_user_id_key_key UNIQUE (tenant_id, user_id, key);

ALTER TABLE ONLY public.workflow_designs
    ADD CONSTRAINT workflow_designs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.workflow_node_runs
    ADD CONSTRAINT workflow_node_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.workflow_node_runs
    ADD CONSTRAINT workflow_node_runs_workflow_run_id_node_key_key UNIQUE (workflow_run_id, node_key);

ALTER TABLE ONLY public.workflow_run_dependencies
    ADD CONSTRAINT workflow_run_dependencies_pkey PRIMARY KEY (workflow_run_id, from_node_key, to_node_key);

ALTER TABLE ONLY public.workflow_runs
    ADD CONSTRAINT workflow_runs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_workflow_design_id_version_label_key UNIQUE (workflow_design_id, version_label);

ALTER TABLE ONLY public.workspace_pins
    ADD CONSTRAINT workspace_pins_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.workspace_pins
    ADD CONSTRAINT workspace_pins_tenant_id_user_id_target_type_target_id_key UNIQUE (tenant_id, user_id, target_type, target_id);

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_pkey PRIMARY KEY (id);

CREATE INDEX idx_audit_logs_id ON ONLY public.audit_logs USING btree (id);

CREATE INDEX audit_logs_default_id_idx ON public.audit_logs_default USING btree (id);

CREATE INDEX idx_audit_logs_tenant_created ON ONLY public.audit_logs USING btree (tenant_id, created_at DESC, id DESC);

CREATE INDEX audit_logs_default_tenant_id_created_at_id_idx ON public.audit_logs_default USING btree (tenant_id, created_at DESC, id DESC);

CREATE INDEX idx_agent_team_members_agent ON public.agent_team_members USING btree (tenant_id, agent_id) WHERE (deleted_at IS NULL);

CREATE UNIQUE INDEX idx_agent_team_members_leader_active ON public.agent_team_members USING btree (tenant_id, team_id) WHERE ((role = 'leader'::text) AND (deleted_at IS NULL));

CREATE UNIQUE INDEX idx_agent_team_members_slot_active ON public.agent_team_members USING btree (tenant_id, team_id, slot_order) WHERE (deleted_at IS NULL);

CREATE INDEX idx_agent_team_members_team_status ON public.agent_team_members USING btree (tenant_id, team_id, status, slot_order) WHERE (deleted_at IS NULL);

CREATE INDEX idx_agent_team_run_members_run ON public.agent_team_run_members USING btree (tenant_id, run_id) WHERE (run_id IS NOT NULL);

CREATE UNIQUE INDEX idx_agent_team_run_members_slot ON public.agent_team_run_members USING btree (tenant_id, team_run_id, slot_order);

CREATE INDEX idx_agent_team_run_members_status ON public.agent_team_run_members USING btree (team_run_id, status);

CREATE INDEX idx_agent_team_runs_conversation ON public.agent_team_runs USING btree (tenant_id, conversation_id, updated_at DESC);

CREATE INDEX idx_agent_team_runs_status ON public.agent_team_runs USING btree (tenant_id, status, updated_at DESC);

CREATE INDEX idx_agent_team_runs_team_updated ON public.agent_team_runs USING btree (tenant_id, team_id, updated_at DESC);

CREATE INDEX idx_agent_teams_owner ON public.agent_teams USING btree (tenant_id, owner_user_id, updated_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_agent_teams_tenant_status_updated ON public.agent_teams USING btree (tenant_id, status, updated_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_agent_teams_workspace ON public.agent_teams USING btree (tenant_id, workspace_id, updated_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_agent_version_sql_tool_bindings_version ON public.agent_version_sql_tool_bindings USING btree (sql_tool_version_id);

CREATE INDEX idx_agents_tenant_status_updated ON public.agents USING btree (tenant_id, status, updated_at DESC, created_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_approvals_evidence_object_reference ON public.approvals USING btree (evidence_object_reference_id);

CREATE INDEX idx_approvals_status_created ON public.approvals USING btree (tenant_id, status, created_at DESC);

CREATE INDEX idx_audit_hash_chain_segments_archive_pending ON public.audit_hash_chain_segments USING btree (archive_status, sealed_at, id) WHERE (archive_status <> 'archived'::text);

CREATE INDEX idx_audit_hash_chain_segments_archive_recovery ON public.audit_hash_chain_segments USING btree (archive_started_at, id) WHERE (archive_status = 'archiving'::text);

CREATE INDEX idx_audit_hash_chain_segments_retention ON public.audit_hash_chain_segments USING btree (tenant_id, retention_until, id) WHERE (archive_status = 'archived'::text);

CREATE INDEX idx_audit_hash_chain_segments_tenant_sealed ON public.audit_hash_chain_segments USING btree (tenant_id, sealed_at DESC, id DESC);

CREATE UNIQUE INDEX idx_audit_legal_holds_active_scope ON public.audit_legal_holds USING btree (tenant_id, scope_type, COALESCE(scope_id, ''::text), COALESCE(resource_type, ''::text)) WHERE (status = 'active'::text);

CREATE INDEX idx_audit_legal_holds_tenant_status ON public.audit_legal_holds USING btree (tenant_id, status, created_at DESC, id DESC);

CREATE INDEX idx_authz_decisions_tenant_created ON public.authz_decisions USING btree (tenant_id, created_at DESC);

CREATE INDEX idx_biwork_skill_import_history_tenant_created ON public.biwork_skill_import_history USING btree (tenant_id, created_at DESC);

CREATE INDEX idx_channel_pairings_pending ON public.channel_pairing_requests USING btree (tenant_id, status, expires_at);

CREATE INDEX idx_channel_sessions_active ON public.channel_sessions USING btree (tenant_id, platform, last_activity_at DESC) WHERE (ended_at IS NULL);

CREATE INDEX idx_conversations_owner_updated ON public.conversations USING btree (tenant_id, created_by_user_id, updated_at DESC, created_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_conversations_workspace_updated ON public.conversations USING btree (tenant_id, workspace_id, updated_at DESC) WHERE ((deleted_at IS NULL) AND (workspace_id IS NOT NULL));

CREATE INDEX idx_devices_tenant_user ON public.devices USING btree (tenant_id, user_id);

CREATE UNIQUE INDEX idx_devices_user_fingerprint ON public.devices USING btree (tenant_id, user_id, device_fingerprint);

CREATE INDEX idx_devices_user_platform ON public.devices USING btree (user_id, platform);

CREATE INDEX idx_event_outbox_pending ON public.event_outbox USING btree (status, next_attempt_at);

CREATE INDEX idx_extension_contributions_type ON public.extension_contributions USING btree (tenant_id, contribution_type) WHERE (enabled = true);

CREATE INDEX idx_file_locks_expiry ON public.file_locks USING btree (tenant_id, expires_at);

CREATE INDEX idx_file_revisions_metadata_content ON public.file_revisions USING gin (metadata);

CREATE INDEX idx_file_revisions_project_path ON public.file_revisions USING btree (project_id, path_hash, revision DESC);

CREATE INDEX idx_file_search_chunks_scope ON public.file_search_chunks USING btree (tenant_id, project_id, path_hash, revision DESC);

CREATE INDEX idx_file_search_chunks_vector ON public.file_search_chunks USING gin (search_vector);

CREATE INDEX idx_llm_credential_rotation_attempts_failed ON public.llm_credential_rotation_attempts USING btree (started_at DESC) WHERE (status = 'failed'::text);

CREATE INDEX idx_llm_credential_rotation_attempts_tenant_started ON public.llm_credential_rotation_attempts USING btree (tenant_id, started_at DESC);

CREATE INDEX idx_llm_credentials_rotation_due ON public.llm_credentials USING btree (next_rotation_at, id) WHERE (auto_rotation_enabled AND (revoked_at IS NULL));

CREATE INDEX idx_local_exec_events_request_created ON public.local_exec_events USING btree (tenant_id, local_exec_request_id, created_at DESC, id DESC);

CREATE INDEX idx_local_exec_requests_active_device ON public.local_exec_requests USING btree (tenant_id, device_id, status, created_at, id) WHERE (status = ANY (ARRAY['queued'::text, 'dispatching'::text]));

CREATE INDEX idx_local_exec_requests_run_active ON public.local_exec_requests USING btree (tenant_id, run_id, status) WHERE ((run_id IS NOT NULL) AND (status = ANY (ARRAY['queued'::text, 'dispatching'::text])));

CREATE INDEX idx_local_mounts_user_device ON public.local_mounts USING btree (user_id, device_id);

CREATE INDEX idx_local_mounts_workspace ON public.local_mounts USING btree (workspace_id, status);

CREATE INDEX idx_mcp_servers_tenant_health ON public.mcp_servers USING btree (tenant_id, health_status, last_health_check_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_memory_embeddings_tenant_status ON public.memory_embeddings USING btree (tenant_id, index_status, updated_at DESC);

CREATE INDEX idx_memory_feedback_memory ON public.memory_feedback USING btree (tenant_id, memory_id, created_at DESC);

CREATE INDEX idx_memory_ingestion_jobs_pending ON public.memory_ingestion_jobs USING btree (tenant_id, status, scheduled_at) WHERE (status = 'pending'::text);

CREATE INDEX idx_memory_items_agent_updated ON public.memory_items USING btree (tenant_id, agent_id, status, updated_at DESC) WHERE ((deleted_at IS NULL) AND (agent_id IS NOT NULL));

CREATE INDEX idx_memory_items_project_updated ON public.memory_items USING btree (tenant_id, project_id, status, updated_at DESC) WHERE ((deleted_at IS NULL) AND (project_id IS NOT NULL));

CREATE INDEX idx_memory_items_scope_updated ON public.memory_items USING btree (tenant_id, user_id, layer, status, updated_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_object_references_owner ON public.object_references USING btree (tenant_id, owner_resource_type, owner_resource_id);

CREATE UNIQUE INDEX idx_object_references_unique_version ON public.object_references USING btree (tenant_id, bucket, object_key, COALESCE(version_id, ''::text));

CREATE INDEX idx_platform_sessions_active ON public.platform_sessions USING btree (user_id, revoked_at, token_exp);

CREATE INDEX idx_platform_sessions_device ON public.platform_sessions USING btree (device_id);

CREATE INDEX idx_platform_sessions_tenant_user ON public.platform_sessions USING btree (tenant_id, user_id);

CREATE INDEX idx_platform_sessions_token_jti ON public.platform_sessions USING btree (token_jti);

CREATE INDEX idx_platform_users_subject ON public.platform_users USING btree (ferriskey_subject);

CREATE INDEX idx_projects_tenant_created ON public.projects USING btree (tenant_id, created_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_resource_policy_bindings_lookup ON public.resource_policy_bindings USING btree (tenant_id, resource_type, resource_id, action, effect) WHERE (disabled_at IS NULL);

CREATE INDEX idx_resource_relations_lookup ON public.resource_relations USING btree (tenant_id, resource_type, resource_id, relation);

CREATE INDEX idx_run_event_links_lookup ON public.run_event_links USING btree (tenant_id, link_type, link_id, link_key);

CREATE INDEX idx_run_events_conversation_seq ON public.run_events USING btree (conversation_id, seq);

CREATE UNIQUE INDEX idx_run_events_event_id_idempotent ON public.run_events USING btree (tenant_id, conversation_id, event_id);

CREATE INDEX idx_run_events_run_seq ON public.run_events USING btree (run_id, seq);

CREATE UNIQUE INDEX idx_run_events_stream_offset ON public.run_events USING btree (stream_offset);

CREATE INDEX idx_run_events_tenant_conversation_seq ON public.run_events USING btree (tenant_id, conversation_id, seq);

CREATE INDEX idx_run_events_tenant_stream_offset ON public.run_events USING btree (tenant_id, stream_offset) INCLUDE (conversation_id);

CREATE INDEX idx_runs_conversation ON public.runs USING btree (conversation_id, queued_at DESC);

CREATE UNIQUE INDEX idx_runs_tenant_idempotency_key ON public.runs USING btree (tenant_id, idempotency_key) WHERE (idempotency_key IS NOT NULL);

CREATE INDEX idx_runs_tenant_queued ON public.runs USING btree (tenant_id, queued_at DESC);

CREATE INDEX idx_runs_tenant_status ON public.runs USING btree (tenant_id, status, queued_at DESC);

CREATE INDEX idx_runs_workspace ON public.runs USING btree (workspace_id);

CREATE INDEX idx_scheduled_job_artifacts_conversation ON public.scheduled_job_artifacts USING btree (tenant_id, conversation_id, created_at) WHERE (conversation_id IS NOT NULL);

CREATE UNIQUE INDEX idx_scheduled_job_artifacts_key ON public.scheduled_job_artifacts USING btree (tenant_id, scheduled_job_id, artifact_kind, artifact_key);

CREATE INDEX idx_scheduled_job_runs_job ON public.scheduled_job_runs USING btree (tenant_id, scheduled_job_id, triggered_at DESC);

CREATE INDEX idx_scheduled_jobs_conversation ON public.scheduled_jobs USING btree (tenant_id, source_conversation_id) WHERE (deleted_at IS NULL);

CREATE INDEX idx_scheduled_jobs_owner_due ON public.scheduled_jobs USING btree (tenant_id, created_by_user_id, next_run_at, created_at) WHERE ((enabled IS TRUE) AND (deleted_at IS NULL) AND (next_run_at IS NOT NULL));

CREATE INDEX idx_scheduled_jobs_owner_updated ON public.scheduled_jobs USING btree (tenant_id, created_by_user_id, updated_at DESC, created_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_scheduled_jobs_target_conversation ON public.scheduled_jobs USING btree (tenant_id, target_conversation_id) WHERE ((deleted_at IS NULL) AND (target_conversation_id IS NOT NULL));

CREATE INDEX idx_scheduled_jobs_tenant_enabled ON public.scheduled_jobs USING btree (tenant_id, enabled, next_run_at) WHERE (deleted_at IS NULL);

CREATE UNIQUE INDEX idx_skill_versions_one_published ON public.skill_versions USING btree (skill_id) WHERE (status = 'published'::text);

CREATE UNIQUE INDEX idx_skills_tenant_active_name ON public.skills USING btree (tenant_id, lower(name)) WHERE (deleted_at IS NULL);

CREATE INDEX idx_sql_tool_versions_query_hash ON public.sql_tool_versions USING btree (tenant_id, query_hash) WHERE (status = 'published'::text);

CREATE INDEX idx_tool_calls_evidence_object_reference ON public.tool_calls USING btree (evidence_object_reference_id);

CREATE INDEX idx_tool_calls_run ON public.tool_calls USING btree (run_id);

CREATE INDEX idx_tool_result_artifacts_file_revision ON public.tool_result_artifacts USING btree (file_revision_id);

CREATE INDEX idx_tool_result_artifacts_run ON public.tool_result_artifacts USING btree (tenant_id, run_id, tool_call_id);

CREATE INDEX idx_tools_tenant_status_updated ON public.tools USING btree (tenant_id, status, updated_at DESC, created_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_workflow_designs_tenant_status_updated ON public.workflow_designs USING btree (tenant_id, status, updated_at DESC, created_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_workflow_node_runs_agent_run ON public.workflow_node_runs USING btree (agent_run_id) WHERE (agent_run_id IS NOT NULL);

CREATE INDEX idx_workflow_node_runs_ready ON public.workflow_node_runs USING btree (workflow_run_id, status, not_before);

CREATE INDEX idx_workspaces_owner_updated ON public.workspaces USING btree (tenant_id, owner_user_id, updated_at DESC) WHERE (deleted_at IS NULL);

CREATE INDEX idx_workspaces_remote_project ON public.workspaces USING btree (tenant_id, remote_project_id) WHERE ((deleted_at IS NULL) AND (remote_project_id IS NOT NULL));

CREATE INDEX idx_workspaces_tenant_updated ON public.workspaces USING btree (tenant_id, updated_at DESC) WHERE (deleted_at IS NULL);

ALTER INDEX public.idx_audit_logs_id ATTACH PARTITION public.audit_logs_default_id_idx;

ALTER INDEX public.audit_logs_pkey ATTACH PARTITION public.audit_logs_default_pkey;

ALTER INDEX public.idx_audit_logs_tenant_created ATTACH PARTITION public.audit_logs_default_tenant_id_created_at_id_idx;

CREATE TRIGGER trg_register_audit_log_identity BEFORE INSERT ON public.audit_logs FOR EACH ROW EXECUTE FUNCTION public.register_audit_log_identity();

ALTER TABLE ONLY public.agent_checkpoint_writes
    ADD CONSTRAINT agent_checkpoint_writes_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_checkpoints
    ADD CONSTRAINT agent_checkpoints_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_members
    ADD CONSTRAINT agent_team_members_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE RESTRICT;

ALTER TABLE ONLY public.agent_team_members
    ADD CONSTRAINT agent_team_members_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_members
    ADD CONSTRAINT agent_team_members_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.agent_teams(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_members
    ADD CONSTRAINT agent_team_members_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_team_member_id_fkey FOREIGN KEY (team_member_id) REFERENCES public.agent_team_members(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_team_run_id_fkey FOREIGN KEY (team_run_id) REFERENCES public.agent_team_runs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_team_id_fkey FOREIGN KEY (team_id) REFERENCES public.agent_teams(id);

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_team_runs
    ADD CONSTRAINT agent_team_runs_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_teams
    ADD CONSTRAINT agent_teams_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_teams
    ADD CONSTRAINT agent_teams_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_teams
    ADD CONSTRAINT agent_teams_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agent_version_mcp_bindings
    ADD CONSTRAINT agent_version_mcp_bindings_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_mcp_bindings
    ADD CONSTRAINT agent_version_mcp_bindings_mcp_tool_id_fkey FOREIGN KEY (mcp_tool_id) REFERENCES public.mcp_tools(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_skill_bindings
    ADD CONSTRAINT agent_version_skill_bindings_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_skill_bindings
    ADD CONSTRAINT agent_version_skill_bindings_skill_version_id_fkey FOREIGN KEY (skill_version_id) REFERENCES public.skill_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_sql_tool_bindings
    ADD CONSTRAINT agent_version_sql_tool_bindings_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_sql_tool_bindings
    ADD CONSTRAINT agent_version_sql_tool_bindings_sql_tool_version_id_fkey FOREIGN KEY (sql_tool_version_id) REFERENCES public.sql_tool_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_tool_bindings
    ADD CONSTRAINT agent_version_tool_bindings_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_version_tool_bindings
    ADD CONSTRAINT agent_version_tool_bindings_tool_version_id_fkey FOREIGN KEY (tool_version_id) REFERENCES public.tool_versions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agent_versions
    ADD CONSTRAINT agent_versions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.agents
    ADD CONSTRAINT agents_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.agents
    ADD CONSTRAINT agents_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_approver_user_id_fkey FOREIGN KEY (approver_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_evidence_object_reference_id_fkey FOREIGN KEY (evidence_object_reference_id) REFERENCES public.object_references(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_requested_by_user_id_fkey FOREIGN KEY (requested_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.approvals
    ADD CONSTRAINT approvals_tool_call_id_fkey FOREIGN KEY (tool_call_id) REFERENCES public.tool_calls(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.artifact_previews
    ADD CONSTRAINT artifact_previews_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_first_audit_log_id_fkey FOREIGN KEY (first_audit_log_id) REFERENCES public.audit_log_identities(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_last_audit_log_id_fkey FOREIGN KEY (last_audit_log_id) REFERENCES public.audit_log_identities(id) ON DELETE RESTRICT DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_object_reference_id_fkey FOREIGN KEY (object_reference_id) REFERENCES public.object_references(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_sealed_by_user_id_fkey FOREIGN KEY (sealed_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.audit_hash_chain_segments
    ADD CONSTRAINT audit_hash_chain_segments_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.audit_legal_holds
    ADD CONSTRAINT audit_legal_holds_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.audit_legal_holds
    ADD CONSTRAINT audit_legal_holds_released_by_user_id_fkey FOREIGN KEY (released_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.audit_legal_holds
    ADD CONSTRAINT audit_legal_holds_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.audit_log_identities
    ADD CONSTRAINT audit_log_identities_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE public.audit_logs
    ADD CONSTRAINT audit_logs_actor_user_id_fkey1 FOREIGN KEY (actor_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE public.audit_logs
    ADD CONSTRAINT audit_logs_id_created_at_fkey FOREIGN KEY (id, created_at) REFERENCES public.audit_log_identities(id, created_at) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE public.audit_logs
    ADD CONSTRAINT audit_logs_tenant_id_fkey1 FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.authz_decisions
    ADD CONSTRAINT authz_decisions_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.authz_decisions
    ADD CONSTRAINT authz_decisions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.biwork_skill_import_history
    ADD CONSTRAINT biwork_skill_import_history_skill_id_fkey FOREIGN KEY (skill_id) REFERENCES public.skills(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.biwork_skill_import_history
    ADD CONSTRAINT biwork_skill_import_history_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_authorized_users
    ADD CONSTRAINT channel_authorized_users_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_connectors
    ADD CONSTRAINT channel_connectors_source_extension_package_id_fkey FOREIGN KEY (source_extension_package_id) REFERENCES public.extension_packages(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_connectors
    ADD CONSTRAINT channel_connectors_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_pairing_requests
    ADD CONSTRAINT channel_pairing_requests_decided_by_user_id_fkey FOREIGN KEY (decided_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_pairing_requests
    ADD CONSTRAINT channel_pairing_requests_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_platform_settings
    ADD CONSTRAINT channel_platform_settings_assistant_profile_id_fkey FOREIGN KEY (assistant_profile_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_platform_settings
    ADD CONSTRAINT channel_platform_settings_default_model_profile_id_fkey FOREIGN KEY (default_model_profile_id) REFERENCES public.llm_model_profiles(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_platform_settings
    ADD CONSTRAINT channel_platform_settings_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.channel_platform_settings
    ADD CONSTRAINT channel_platform_settings_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_sessions
    ADD CONSTRAINT channel_sessions_channel_user_id_fkey FOREIGN KEY (channel_user_id) REFERENCES public.channel_authorized_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_sessions
    ADD CONSTRAINT channel_sessions_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.channel_sessions
    ADD CONSTRAINT channel_sessions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.conversation_event_sequences
    ADD CONSTRAINT conversation_event_sequences_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.device_extension_states
    ADD CONSTRAINT device_extension_states_device_id_fkey FOREIGN KEY (device_id) REFERENCES public.devices(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.device_extension_states
    ADD CONSTRAINT device_extension_states_extension_package_id_fkey FOREIGN KEY (extension_package_id) REFERENCES public.extension_packages(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.device_extension_states
    ADD CONSTRAINT device_extension_states_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.devices
    ADD CONSTRAINT devices_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.devices
    ADD CONSTRAINT devices_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.event_outbox
    ADD CONSTRAINT event_outbox_event_row_id_fkey FOREIGN KEY (event_row_id) REFERENCES public.run_events(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.event_outbox
    ADD CONSTRAINT event_outbox_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.extension_contributions
    ADD CONSTRAINT extension_contributions_extension_package_id_fkey FOREIGN KEY (extension_package_id) REFERENCES public.extension_packages(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.extension_contributions
    ADD CONSTRAINT extension_contributions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.extension_packages
    ADD CONSTRAINT extension_packages_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.ferriskey_role_projection
    ADD CONSTRAINT ferriskey_role_projection_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_locks
    ADD CONSTRAINT file_locks_holder_run_id_fkey FOREIGN KEY (holder_run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.file_locks
    ADD CONSTRAINT file_locks_holder_user_id_fkey FOREIGN KEY (holder_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.file_locks
    ADD CONSTRAINT file_locks_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_locks
    ADD CONSTRAINT file_locks_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_last_writer_user_id_fkey FOREIGN KEY (last_writer_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_object_reference_id_fkey FOREIGN KEY (object_reference_id) REFERENCES public.object_references(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.file_revisions
    ADD CONSTRAINT file_revisions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_search_chunks
    ADD CONSTRAINT file_search_chunks_file_revision_id_fkey FOREIGN KEY (file_revision_id) REFERENCES public.file_revisions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_search_chunks
    ADD CONSTRAINT file_search_chunks_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.file_search_chunks
    ADD CONSTRAINT file_search_chunks_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.interrupts
    ADD CONSTRAINT interrupts_approval_id_fkey FOREIGN KEY (approval_id) REFERENCES public.approvals(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.interrupts
    ADD CONSTRAINT interrupts_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.interrupts
    ADD CONSTRAINT interrupts_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.interrupts
    ADD CONSTRAINT interrupts_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_credential_rotation_attempts
    ADD CONSTRAINT llm_credential_rotation_attempts_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.llm_credentials(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_credential_rotation_attempts
    ADD CONSTRAINT llm_credential_rotation_attempts_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_credentials
    ADD CONSTRAINT llm_credentials_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.llm_credentials
    ADD CONSTRAINT llm_credentials_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.llm_providers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_credentials
    ADD CONSTRAINT llm_credentials_rotated_by_user_id_fkey FOREIGN KEY (rotated_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.llm_credentials
    ADD CONSTRAINT llm_credentials_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_model_profiles
    ADD CONSTRAINT llm_model_profiles_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES public.llm_credentials(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.llm_model_profiles
    ADD CONSTRAINT llm_model_profiles_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.llm_providers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_model_profiles
    ADD CONSTRAINT llm_model_profiles_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.llm_providers
    ADD CONSTRAINT llm_providers_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_exec_events
    ADD CONSTRAINT local_exec_events_local_exec_request_id_fkey FOREIGN KEY (local_exec_request_id) REFERENCES public.local_exec_requests(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_exec_events
    ADD CONSTRAINT local_exec_events_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_exec_requests
    ADD CONSTRAINT local_exec_requests_device_id_fkey FOREIGN KEY (device_id) REFERENCES public.devices(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.local_exec_requests
    ADD CONSTRAINT local_exec_requests_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.local_exec_requests
    ADD CONSTRAINT local_exec_requests_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.local_exec_requests
    ADD CONSTRAINT local_exec_requests_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_mounts
    ADD CONSTRAINT local_mounts_device_id_fkey FOREIGN KEY (device_id) REFERENCES public.devices(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_mounts
    ADD CONSTRAINT local_mounts_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_mounts
    ADD CONSTRAINT local_mounts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.local_mounts
    ADD CONSTRAINT local_mounts_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mcp_servers
    ADD CONSTRAINT mcp_servers_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mcp_tools
    ADD CONSTRAINT mcp_tools_mcp_server_id_fkey FOREIGN KEY (mcp_server_id) REFERENCES public.mcp_servers(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.mcp_tools
    ADD CONSTRAINT mcp_tools_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_memory_id_fkey FOREIGN KEY (memory_id) REFERENCES public.memory_items(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_embeddings
    ADD CONSTRAINT memory_embeddings_memory_id_fkey FOREIGN KEY (memory_id) REFERENCES public.memory_items(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_embeddings
    ADD CONSTRAINT memory_embeddings_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_feedback
    ADD CONSTRAINT memory_feedback_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_feedback
    ADD CONSTRAINT memory_feedback_memory_id_fkey FOREIGN KEY (memory_id) REFERENCES public.memory_items(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_feedback
    ADD CONSTRAINT memory_feedback_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_feedback
    ADD CONSTRAINT memory_feedback_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_feedback
    ADD CONSTRAINT memory_feedback_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_ingestion_jobs
    ADD CONSTRAINT memory_ingestion_jobs_memory_id_fkey FOREIGN KEY (memory_id) REFERENCES public.memory_items(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_ingestion_jobs
    ADD CONSTRAINT memory_ingestion_jobs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_source_event_id_fkey FOREIGN KEY (source_event_id) REFERENCES public.run_events(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_source_run_id_fkey FOREIGN KEY (source_run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.memory_items
    ADD CONSTRAINT memory_items_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.object_references
    ADD CONSTRAINT object_references_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.platform_sessions
    ADD CONSTRAINT platform_sessions_device_id_fkey FOREIGN KEY (device_id) REFERENCES public.devices(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.platform_sessions
    ADD CONSTRAINT platform_sessions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.platform_sessions
    ADD CONSTRAINT platform_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.platform_users
    ADD CONSTRAINT platform_users_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_mounts
    ADD CONSTRAINT project_mounts_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.project_mounts
    ADD CONSTRAINT project_mounts_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.projects
    ADD CONSTRAINT projects_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.resource_policy_bindings
    ADD CONSTRAINT resource_policy_bindings_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.resource_policy_bindings
    ADD CONSTRAINT resource_policy_bindings_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.resource_relations
    ADD CONSTRAINT resource_relations_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.resource_relations
    ADD CONSTRAINT resource_relations_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.run_event_links
    ADD CONSTRAINT run_event_links_run_event_id_fkey FOREIGN KEY (run_event_id) REFERENCES public.run_events(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.run_event_links
    ADD CONSTRAINT run_event_links_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.run_events
    ADD CONSTRAINT run_events_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.run_events
    ADD CONSTRAINT run_events_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.run_events
    ADD CONSTRAINT run_events_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_agent_id_fkey FOREIGN KEY (agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_agent_version_id_fkey FOREIGN KEY (agent_version_id) REFERENCES public.agent_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_job_artifacts
    ADD CONSTRAINT scheduled_job_artifacts_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_job_artifacts
    ADD CONSTRAINT scheduled_job_artifacts_object_reference_id_fkey FOREIGN KEY (object_reference_id) REFERENCES public.object_references(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_job_artifacts
    ADD CONSTRAINT scheduled_job_artifacts_scheduled_job_id_fkey FOREIGN KEY (scheduled_job_id) REFERENCES public.scheduled_jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_job_artifacts
    ADD CONSTRAINT scheduled_job_artifacts_source_event_id_fkey FOREIGN KEY (source_event_id) REFERENCES public.run_events(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_job_artifacts
    ADD CONSTRAINT scheduled_job_artifacts_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_job_runs
    ADD CONSTRAINT scheduled_job_runs_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_job_runs
    ADD CONSTRAINT scheduled_job_runs_scheduled_job_id_fkey FOREIGN KEY (scheduled_job_id) REFERENCES public.scheduled_jobs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_job_runs
    ADD CONSTRAINT scheduled_job_runs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_job_runs
    ADD CONSTRAINT scheduled_job_runs_workflow_run_id_fkey FOREIGN KEY (workflow_run_id) REFERENCES public.workflow_runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_assistant_profile_id_fkey FOREIGN KEY (assistant_profile_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_model_profile_id_fkey FOREIGN KEY (model_profile_id) REFERENCES public.llm_model_profiles(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_source_conversation_id_fkey FOREIGN KEY (source_conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_target_conversation_id_fkey FOREIGN KEY (target_conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.scheduled_jobs
    ADD CONSTRAINT scheduled_jobs_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES public.workspaces(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.skill_versions
    ADD CONSTRAINT skill_versions_skill_id_fkey FOREIGN KEY (skill_id) REFERENCES public.skills(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.skill_versions
    ADD CONSTRAINT skill_versions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.skills
    ADD CONSTRAINT skills_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sql_connections
    ADD CONSTRAINT sql_connections_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sql_tool_versions
    ADD CONSTRAINT sql_tool_versions_connection_id_fkey FOREIGN KEY (connection_id) REFERENCES public.sql_connections(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sql_tool_versions
    ADD CONSTRAINT sql_tool_versions_sql_tool_id_fkey FOREIGN KEY (sql_tool_id) REFERENCES public.sql_tools(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sql_tool_versions
    ADD CONSTRAINT sql_tool_versions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.sql_tools
    ADD CONSTRAINT sql_tools_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_calls
    ADD CONSTRAINT tool_calls_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.tool_calls
    ADD CONSTRAINT tool_calls_evidence_object_reference_id_fkey FOREIGN KEY (evidence_object_reference_id) REFERENCES public.object_references(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.tool_calls
    ADD CONSTRAINT tool_calls_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.tool_calls
    ADD CONSTRAINT tool_calls_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_calls
    ADD CONSTRAINT tool_calls_tool_id_fkey FOREIGN KEY (tool_id) REFERENCES public.tools(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_file_revision_id_fkey FOREIGN KEY (file_revision_id) REFERENCES public.file_revisions(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_object_reference_id_fkey FOREIGN KEY (object_reference_id) REFERENCES public.object_references(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_result_artifacts
    ADD CONSTRAINT tool_result_artifacts_tool_call_id_fkey FOREIGN KEY (tool_call_id) REFERENCES public.tool_calls(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.tool_versions
    ADD CONSTRAINT tool_versions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tool_versions
    ADD CONSTRAINT tool_versions_tool_id_fkey FOREIGN KEY (tool_id) REFERENCES public.tools(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.tools
    ADD CONSTRAINT tools_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_tenant_memberships
    ADD CONSTRAINT user_tenant_memberships_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_tenant_memberships
    ADD CONSTRAINT user_tenant_memberships_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_ui_preferences
    ADD CONSTRAINT user_ui_preferences_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.user_ui_preferences
    ADD CONSTRAINT user_ui_preferences_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_designs
    ADD CONSTRAINT workflow_designs_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workflow_designs
    ADD CONSTRAINT workflow_designs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_node_runs
    ADD CONSTRAINT workflow_node_runs_agent_run_id_fkey FOREIGN KEY (agent_run_id) REFERENCES public.runs(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workflow_node_runs
    ADD CONSTRAINT workflow_node_runs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_node_runs
    ADD CONSTRAINT workflow_node_runs_workflow_run_id_fkey FOREIGN KEY (workflow_run_id) REFERENCES public.workflow_runs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_run_dependencies
    ADD CONSTRAINT workflow_run_dependencies_workflow_run_id_fkey FOREIGN KEY (workflow_run_id) REFERENCES public.workflow_runs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_runs
    ADD CONSTRAINT workflow_runs_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workflow_runs
    ADD CONSTRAINT workflow_runs_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workflow_runs
    ADD CONSTRAINT workflow_runs_project_id_fkey FOREIGN KEY (project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workflow_runs
    ADD CONSTRAINT workflow_runs_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_runs
    ADD CONSTRAINT workflow_runs_workflow_version_id_fkey FOREIGN KEY (workflow_version_id) REFERENCES public.workflow_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workflow_versions
    ADD CONSTRAINT workflow_versions_workflow_design_id_fkey FOREIGN KEY (workflow_design_id) REFERENCES public.workflow_designs(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workspace_pins
    ADD CONSTRAINT workspace_pins_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workspace_pins
    ADD CONSTRAINT workspace_pins_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.platform_users(id) ON DELETE CASCADE;

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_default_agent_id_fkey FOREIGN KEY (default_agent_id) REFERENCES public.agents(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_default_agent_version_id_fkey FOREIGN KEY (default_agent_version_id) REFERENCES public.agent_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_default_model_profile_id_fkey FOREIGN KEY (default_model_profile_id) REFERENCES public.llm_model_profiles(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.platform_users(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_remote_project_id_fkey FOREIGN KEY (remote_project_id) REFERENCES public.projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY public.workspaces
    ADD CONSTRAINT workspaces_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenants(id) ON DELETE CASCADE;

SELECT public.ensure_audit_log_month_partitions(2);

