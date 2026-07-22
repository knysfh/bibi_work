CREATE OR REPLACE FUNCTION public.is_valid_secret_ref(value text) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $_$
    SELECT
        value ~ '^env:(//)?[A-Za-z0-9_]{1,128}$'
        OR value ~ '^local://[0-9a-fA-F-]{36}$'
        OR (
            value ~ '^vault://[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)+#[A-Za-z0-9_.:/-]{1,128}$'
            AND position('/../' IN value) = 0
        )
        OR (
            value ~ '^kms://[A-Za-z0-9_.:/-]+#[A-Za-z0-9+/=_-]+$'
            AND length(value) <= 33550
        )
$_$;

COMMENT ON FUNCTION public.is_valid_secret_ref(value text) IS
    'Validates opaque local, env, Vault KV, and KMS decrypt references without resolving secret values.';

CREATE TABLE llm_local_secrets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id UUID NOT NULL,
    ciphertext BYTEA NOT NULL,
    key_version TEXT NOT NULL DEFAULT 'v1',
    created_by_user_id UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX llm_local_secrets_tenant_id_idx
    ON llm_local_secrets (tenant_id, created_at DESC);

ALTER TABLE llm_local_secrets
    ADD CONSTRAINT llm_local_secrets_tenant_id_fkey
        FOREIGN KEY (tenant_id) REFERENCES tenants(id) ON DELETE CASCADE,
    ADD CONSTRAINT llm_local_secrets_created_by_user_id_fkey
        FOREIGN KEY (created_by_user_id) REFERENCES platform_users(id) ON DELETE SET NULL;

ALTER TABLE llm_credentials
    ADD COLUMN secret_mask TEXT,
    ADD COLUMN secret_count INTEGER NOT NULL DEFAULT 1;

ALTER TABLE llm_credentials
    ADD CONSTRAINT llm_credentials_secret_count_check CHECK (secret_count >= 1);

ALTER TABLE llm_credential_rotation_attempts
    DROP CONSTRAINT llm_credential_rotation_attempts_resolver_scheme_check,
    ADD CONSTRAINT llm_credential_rotation_attempts_resolver_scheme_check
        CHECK (resolver_scheme = ANY (ARRAY['local', 'env', 'vault', 'kms']));

COMMENT ON TABLE llm_local_secrets IS
    'Encrypted local secrets. Plaintext is only accepted during replacement and is never returned by APIs.';
COMMENT ON COLUMN llm_credentials.secret_mask IS
    'Non-sensitive fixed mask safe for settings UI display.';
