ALTER TABLE platform_sessions
    ADD COLUMN IF NOT EXISTS last_user_activity_at timestamp with time zone,
    ADD COLUMN IF NOT EXISTS idle_expires_at timestamp with time zone;

UPDATE platform_sessions
SET client_kind = 'web'
WHERE client_kind = 'desktop'
  AND COALESCE(user_agent, '') NOT ILIKE '%Electron/%';

UPDATE platform_sessions
SET last_user_activity_at = COALESCE(last_user_activity_at, last_seen_at, CURRENT_TIMESTAMP),
    idle_expires_at = COALESCE(
        idle_expires_at,
        COALESCE(last_user_activity_at, last_seen_at, CURRENT_TIMESTAMP) + INTERVAL '30 minutes'
    )
WHERE last_user_activity_at IS NULL OR idle_expires_at IS NULL;

ALTER TABLE platform_sessions
    ALTER COLUMN last_user_activity_at SET DEFAULT CURRENT_TIMESTAMP,
    ALTER COLUMN last_user_activity_at SET NOT NULL,
    ALTER COLUMN idle_expires_at SET DEFAULT (CURRENT_TIMESTAMP + INTERVAL '30 minutes'),
    ALTER COLUMN idle_expires_at SET NOT NULL;

CREATE INDEX IF NOT EXISTS idx_platform_sessions_idle_expiry
    ON platform_sessions (idle_expires_at)
    WHERE revoked_at IS NULL;
