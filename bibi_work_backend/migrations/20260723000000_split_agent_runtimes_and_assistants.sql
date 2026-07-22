-- Split the historical mixed `agents` aggregate into explicit execution
-- runtimes and user-facing assistants. The legacy tables are retained for
-- rollback/audit only; active foreign keys are rewired to the new aggregates.

ALTER TABLE public.agent_versions RENAME TO legacy_agent_versions;
ALTER TABLE public.agents RENAME TO legacy_agents;

CREATE TABLE public.agent_runtimes (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    name text NOT NULL,
    description text,
    runtime_kind text NOT NULL,
    source text DEFAULT 'internal'::text NOT NULL,
    draft_config jsonb DEFAULT '{}'::jsonb NOT NULL,
    capabilities jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT agent_runtimes_status_check
        CHECK (status = ANY (ARRAY['active'::text, 'disabled'::text, 'draft'::text]))
);

CREATE TABLE public.assistants (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid NOT NULL,
    owner_user_id uuid,
    runtime_id uuid NOT NULL REFERENCES public.agent_runtimes(id) ON DELETE RESTRICT,
    name text NOT NULL,
    description text,
    draft_config jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'draft'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    deleted_at timestamp with time zone,
    CONSTRAINT assistants_status_check
        CHECK (status = ANY (ARRAY['active'::text, 'disabled'::text, 'draft'::text, 'archived'::text, 'deleted'::text]))
);

CREATE TABLE public.assistant_versions (
    id uuid DEFAULT gen_random_uuid() PRIMARY KEY,
    tenant_id uuid NOT NULL,
    assistant_id uuid NOT NULL REFERENCES public.assistants(id) ON DELETE CASCADE,
    runtime_id uuid NOT NULL REFERENCES public.agent_runtimes(id) ON DELETE RESTRICT,
    version_label text NOT NULL,
    config_snapshot jsonb DEFAULT '{}'::jsonb NOT NULL,
    policy_version text DEFAULT 'local-policy-v1'::text NOT NULL,
    schema_hash text,
    status text DEFAULT 'published'::text NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT assistant_versions_assistant_label_key UNIQUE (assistant_id, version_label)
);

CREATE UNIQUE INDEX idx_agent_runtimes_builtin_kind
    ON public.agent_runtimes (tenant_id, runtime_kind)
    WHERE metadata->>'builtin_runtime' = 'true' AND deleted_at IS NULL;
CREATE INDEX idx_agent_runtimes_tenant_status
    ON public.agent_runtimes (tenant_id, status, updated_at DESC)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_assistants_tenant_status
    ON public.assistants (tenant_id, status, updated_at DESC)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_assistants_runtime
    ON public.assistants (tenant_id, runtime_id)
    WHERE deleted_at IS NULL;
CREATE INDEX idx_assistant_versions_lookup
    ON public.assistant_versions (tenant_id, assistant_id, status, created_at DESC);

CREATE TEMPORARY TABLE runtime_migration_map (
    legacy_agent_id uuid PRIMARY KEY,
    runtime_id uuid NOT NULL
) ON COMMIT DROP;

-- Non-BiWork runtimes keep their historical ID. This preserves custom CLI,
-- extension and remote runtime references without exposing legacy rows.
INSERT INTO public.agent_runtimes (
    id, tenant_id, owner_user_id, name, description, runtime_kind, source,
    draft_config, capabilities, metadata, status, created_at, updated_at, deleted_at
)
SELECT legacy.id,
       legacy.tenant_id,
       legacy.owner_user_id,
       legacy.name,
       legacy.description,
       COALESCE(
           NULLIF(BTRIM(legacy.draft_config#>>'{runtime,kind}'), ''),
           NULLIF(BTRIM(legacy.draft_config->>'acp_backend'), ''),
           'deepagents'
       ) AS runtime_kind,
       CASE LOWER(COALESCE(legacy.metadata->>'source', 'internal'))
           WHEN 'builtin' THEN 'builtin'
           WHEN 'extension' THEN 'extension'
           WHEN 'remote' THEN 'remote'
           WHEN 'internal' THEN 'internal'
           ELSE 'custom'
       END AS source,
       legacy.draft_config,
       COALESCE(legacy.draft_config#>'{capabilities}', '{}'::jsonb),
       legacy.metadata || jsonb_build_object('legacy_agent_id', legacy.id),
       CASE legacy.status
           WHEN 'disabled' THEN 'disabled'
           WHEN 'draft' THEN 'draft'
           ELSE 'active'
       END,
       legacy.created_at,
       legacy.updated_at,
       legacy.deleted_at
FROM public.legacy_agents legacy
WHERE COALESCE(
          NULLIF(BTRIM(legacy.draft_config#>>'{runtime,kind}'), ''),
          NULLIF(BTRIM(legacy.draft_config->>'acp_backend'), ''),
          'deepagents'
      ) <> 'deepagents';

INSERT INTO runtime_migration_map (legacy_agent_id, runtime_id)
SELECT id, id
FROM public.agent_runtimes
WHERE metadata ? 'legacy_agent_id';

-- Model-driven assistants share one built-in runtime per tenant instead of
-- continuing the historical one-runtime-per-assistant duplication.
INSERT INTO public.agent_runtimes (
    tenant_id, name, description, runtime_kind, source, draft_config,
    capabilities, metadata, status, created_at, updated_at
)
SELECT legacy.tenant_id,
       'BiWork Runtime',
       'Built-in model-driven execution runtime',
       'deepagents',
       'internal',
       jsonb_build_object(
           'acp_backend', 'deepagents',
           'runtime', jsonb_build_object('kind', 'deepagents')
       ),
       '{}'::jsonb,
       jsonb_build_object('builtin_runtime', true, 'migrated_from_legacy_agents', true),
       'active',
       MIN(legacy.created_at),
       MAX(legacy.updated_at)
FROM public.legacy_agents legacy
WHERE COALESCE(
          NULLIF(BTRIM(legacy.draft_config#>>'{runtime,kind}'), ''),
          NULLIF(BTRIM(legacy.draft_config->>'acp_backend'), ''),
          'deepagents'
      ) = 'deepagents'
GROUP BY legacy.tenant_id;

INSERT INTO runtime_migration_map (legacy_agent_id, runtime_id)
SELECT legacy.id, runtime.id
FROM public.legacy_agents legacy
JOIN public.agent_runtimes runtime
  ON runtime.tenant_id = legacy.tenant_id
 AND runtime.runtime_kind = 'deepagents'
 AND runtime.source = 'internal'
 AND runtime.deleted_at IS NULL
WHERE COALESCE(
          NULLIF(BTRIM(legacy.draft_config#>>'{runtime,kind}'), ''),
          NULLIF(BTRIM(legacy.draft_config->>'acp_backend'), ''),
          'deepagents'
      ) = 'deepagents';

INSERT INTO public.assistants (
    id, tenant_id, owner_user_id, runtime_id, name, description, draft_config,
    metadata, status, created_at, updated_at, deleted_at
)
SELECT legacy.id,
       legacy.tenant_id,
       legacy.owner_user_id,
       COALESCE(engine_runtime.runtime_id, own_runtime.runtime_id),
       legacy.name,
       legacy.description,
       jsonb_set(
           legacy.draft_config,
           '{engine_agent_id}',
           to_jsonb(COALESCE(engine_runtime.runtime_id, own_runtime.runtime_id)::text),
           true
       ),
       legacy.metadata || jsonb_build_object('legacy_agent_id', legacy.id),
       legacy.status,
       legacy.created_at,
       legacy.updated_at,
       legacy.deleted_at
FROM public.legacy_agents legacy
JOIN runtime_migration_map own_runtime
  ON own_runtime.legacy_agent_id = legacy.id
LEFT JOIN runtime_migration_map engine_runtime
  ON engine_runtime.legacy_agent_id = CASE
      WHEN COALESCE(legacy.draft_config->>'engine_agent_id', '') ~*
           '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
      THEN (legacy.draft_config->>'engine_agent_id')::uuid
      ELSE NULL
  END;

INSERT INTO public.assistant_versions (
    id, tenant_id, assistant_id, runtime_id, version_label, config_snapshot,
    policy_version, schema_hash, status, created_at
)
SELECT version.id,
       version.tenant_id,
       version.agent_id,
       assistant.runtime_id,
       version.version_label,
       jsonb_set(
           version.config_snapshot,
           '{engine_agent_id}',
           to_jsonb(assistant.runtime_id::text),
           true
       ),
       version.policy_version,
       version.schema_hash,
       version.status,
       version.created_at
FROM public.legacy_agent_versions version
JOIN public.assistants assistant
  ON assistant.id = version.agent_id
 AND assistant.tenant_id = version.tenant_id;

-- Remove foreign keys that PostgreSQL automatically redirected to the renamed
-- legacy tables, then attach active references to the new aggregates.
DO $$
DECLARE
    item record;
BEGIN
    FOR item IN
        SELECT ns.nspname AS schema_name,
               rel.relname AS table_name,
               con.conname AS constraint_name
        FROM pg_constraint con
        JOIN pg_class rel ON rel.oid = con.conrelid
        JOIN pg_namespace ns ON ns.oid = rel.relnamespace
        WHERE con.contype = 'f'
          AND con.confrelid IN (
              'public.legacy_agents'::regclass,
              'public.legacy_agent_versions'::regclass
          )
    LOOP
        EXECUTE format(
            'ALTER TABLE %I.%I DROP CONSTRAINT %I',
            item.schema_name,
            item.table_name,
            item.constraint_name
        );
    END LOOP;
END $$;

ALTER TABLE public.agent_team_members
    ADD CONSTRAINT agent_team_members_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE RESTRICT,
    ADD CONSTRAINT agent_team_members_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE SET NULL;
ALTER TABLE public.agent_team_run_members
    ADD CONSTRAINT agent_team_run_members_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL,
    ADD CONSTRAINT agent_team_run_members_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE SET NULL;
ALTER TABLE public.agent_version_mcp_bindings
    ADD CONSTRAINT agent_version_mcp_bindings_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE CASCADE;
ALTER TABLE public.agent_version_skill_bindings
    ADD CONSTRAINT agent_version_skill_bindings_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE CASCADE;
ALTER TABLE public.agent_version_sql_tool_bindings
    ADD CONSTRAINT agent_version_sql_tool_bindings_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE CASCADE;
ALTER TABLE public.agent_version_tool_bindings
    ADD CONSTRAINT agent_version_tool_bindings_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE CASCADE;
ALTER TABLE public.conversations
    ADD CONSTRAINT conversations_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL;
ALTER TABLE public.memory_access_logs
    ADD CONSTRAINT memory_access_logs_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL;
ALTER TABLE public.memory_feedback
    ADD CONSTRAINT memory_feedback_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL;
ALTER TABLE public.memory_items
    ADD CONSTRAINT memory_items_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL;
ALTER TABLE public.runs
    ADD CONSTRAINT runs_agent_id_fkey
        FOREIGN KEY (agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL,
    ADD CONSTRAINT runs_agent_version_id_fkey
        FOREIGN KEY (agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE SET NULL;
ALTER TABLE public.workspaces
    ADD CONSTRAINT workspaces_default_agent_id_fkey
        FOREIGN KEY (default_agent_id) REFERENCES public.assistants(id) ON DELETE SET NULL,
    ADD CONSTRAINT workspaces_default_agent_version_id_fkey
        FOREIGN KEY (default_agent_version_id) REFERENCES public.assistant_versions(id) ON DELETE SET NULL;

-- Canonical names for new code. Compatibility columns remain populated while
-- older clients still send agent_id / agent_version_id.
ALTER TABLE public.conversations
    ADD COLUMN assistant_id uuid REFERENCES public.assistants(id) ON DELETE SET NULL,
    ADD COLUMN runtime_id uuid REFERENCES public.agent_runtimes(id) ON DELETE SET NULL;
ALTER TABLE public.runs
    ADD COLUMN assistant_id uuid REFERENCES public.assistants(id) ON DELETE SET NULL,
    ADD COLUMN assistant_version_id uuid REFERENCES public.assistant_versions(id) ON DELETE SET NULL,
    ADD COLUMN runtime_id uuid REFERENCES public.agent_runtimes(id) ON DELETE SET NULL;

UPDATE public.conversations conversation
SET assistant_id = conversation.agent_id,
    runtime_id = assistant.runtime_id
FROM public.assistants assistant
WHERE assistant.id = conversation.agent_id;

UPDATE public.runs run
SET assistant_id = run.agent_id,
    assistant_version_id = run.agent_version_id,
    runtime_id = assistant.runtime_id
FROM public.assistants assistant
WHERE assistant.id = run.agent_id;

CREATE OR REPLACE FUNCTION public.normalize_assistant_runtime_binding()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    requested_runtime_id uuid;
BEGIN
    IF COALESCE(NEW.draft_config->>'engine_agent_id', '') ~*
       '^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$' THEN
        requested_runtime_id := (NEW.draft_config->>'engine_agent_id')::uuid;
    END IF;

    IF requested_runtime_id IS NOT NULL AND EXISTS (
        SELECT 1
        FROM public.agent_runtimes runtime
        WHERE runtime.id = requested_runtime_id
          AND runtime.tenant_id = NEW.tenant_id
          AND runtime.deleted_at IS NULL
    ) THEN
        NEW.runtime_id := requested_runtime_id;
    END IF;

    IF NEW.runtime_id IS NULL THEN
        SELECT runtime.id INTO NEW.runtime_id
        FROM public.agent_runtimes runtime
        WHERE runtime.tenant_id = NEW.tenant_id
          AND runtime.runtime_kind = 'deepagents'
          AND runtime.metadata->>'builtin_runtime' = 'true'
          AND runtime.deleted_at IS NULL
        ORDER BY runtime.created_at ASC
        LIMIT 1;
    END IF;

    IF NEW.runtime_id IS NULL THEN
        RAISE EXCEPTION 'assistant requires an active runtime';
    END IF;

    NEW.draft_config := jsonb_set(
        NEW.draft_config,
        '{engine_agent_id}',
        to_jsonb(NEW.runtime_id::text),
        true
    );
    RETURN NEW;
END $$;

CREATE TRIGGER assistants_normalize_runtime_binding
BEFORE INSERT OR UPDATE OF runtime_id, draft_config ON public.assistants
FOR EACH ROW EXECUTE FUNCTION public.normalize_assistant_runtime_binding();

CREATE OR REPLACE FUNCTION public.normalize_assistant_version_runtime()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    SELECT runtime_id INTO NEW.runtime_id
    FROM public.assistants
    WHERE id = NEW.assistant_id AND tenant_id = NEW.tenant_id;

    IF NEW.runtime_id IS NULL THEN
        RAISE EXCEPTION 'assistant version requires an active assistant runtime';
    END IF;

    NEW.config_snapshot := jsonb_set(
        NEW.config_snapshot,
        '{engine_agent_id}',
        to_jsonb(NEW.runtime_id::text),
        true
    );
    RETURN NEW;
END $$;

CREATE TRIGGER assistant_versions_normalize_runtime
BEFORE INSERT OR UPDATE OF assistant_id, runtime_id, config_snapshot ON public.assistant_versions
FOR EACH ROW EXECUTE FUNCTION public.normalize_assistant_version_runtime();

CREATE OR REPLACE FUNCTION public.sync_conversation_assistant_runtime()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.assistant_id IS NULL THEN
        NEW.assistant_id := NEW.agent_id;
    ELSIF NEW.agent_id IS NULL THEN
        NEW.agent_id := NEW.assistant_id;
    ELSIF NEW.assistant_id <> NEW.agent_id THEN
        RAISE EXCEPTION 'assistant_id and compatibility agent_id must match';
    END IF;

    IF NEW.assistant_id IS NOT NULL THEN
        SELECT runtime_id INTO NEW.runtime_id
        FROM public.assistants
        WHERE id = NEW.assistant_id AND tenant_id = NEW.tenant_id;
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER conversations_sync_assistant_runtime
BEFORE INSERT OR UPDATE OF agent_id, assistant_id ON public.conversations
FOR EACH ROW EXECUTE FUNCTION public.sync_conversation_assistant_runtime();

CREATE OR REPLACE FUNCTION public.sync_run_assistant_runtime()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.assistant_id IS NULL THEN
        NEW.assistant_id := NEW.agent_id;
    ELSIF NEW.agent_id IS NULL THEN
        NEW.agent_id := NEW.assistant_id;
    ELSIF NEW.assistant_id <> NEW.agent_id THEN
        RAISE EXCEPTION 'assistant_id and compatibility agent_id must match';
    END IF;

    IF NEW.assistant_version_id IS NULL THEN
        NEW.assistant_version_id := NEW.agent_version_id;
    ELSIF NEW.agent_version_id IS NULL THEN
        NEW.agent_version_id := NEW.assistant_version_id;
    ELSIF NEW.assistant_version_id <> NEW.agent_version_id THEN
        RAISE EXCEPTION 'assistant_version_id and compatibility agent_version_id must match';
    END IF;

    IF NEW.assistant_id IS NOT NULL THEN
        SELECT runtime_id INTO NEW.runtime_id
        FROM public.assistants
        WHERE id = NEW.assistant_id AND tenant_id = NEW.tenant_id;
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER runs_sync_assistant_runtime
BEFORE INSERT OR UPDATE OF agent_id, assistant_id, agent_version_id, assistant_version_id ON public.runs
FOR EACH ROW EXECUTE FUNCTION public.sync_run_assistant_runtime();

-- Compatibility views keep the public platform API and older binaries working
-- while all active data is stored in the split aggregates.
CREATE VIEW public.agents AS
SELECT id,
       tenant_id,
       owner_user_id,
       runtime_id,
       name,
       description,
       draft_config,
       metadata,
       status,
       created_at,
       updated_at,
       deleted_at
FROM public.assistants;

CREATE VIEW public.agent_versions AS
SELECT id,
       tenant_id,
       assistant_id AS agent_id,
       runtime_id,
       version_label,
       config_snapshot,
       policy_version,
       schema_hash,
       status,
       created_at
FROM public.assistant_versions;

COMMENT ON TABLE public.agent_runtimes IS
    'Execution runtimes/connectors. User-facing behavior belongs to assistants.';
COMMENT ON TABLE public.assistants IS
    'User-facing assistant definitions bound to one execution runtime.';
COMMENT ON TABLE public.assistant_versions IS
    'Immutable assistant configuration snapshots used by runs.';
COMMENT ON TABLE public.legacy_agents IS
    'Read-only pre-split compatibility archive; do not use for active writes.';
COMMENT ON TABLE public.legacy_agent_versions IS
    'Read-only pre-split compatibility archive; do not use for active writes.';
