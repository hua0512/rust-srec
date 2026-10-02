ALTER TABLE platform_config ADD COLUMN credential_selection TEXT;
ALTER TABLE live_sessions ADD COLUMN credential_binding TEXT;

CREATE TABLE credential_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    platform_config_id TEXT NOT NULL REFERENCES platform_config(id) ON DELETE RESTRICT,
    owner_kind TEXT NOT NULL CHECK (owner_kind IN ('platform', 'template', 'streamer')),
    template_id TEXT REFERENCES template_config(id) ON DELETE RESTRICT,
    streamer_id TEXT REFERENCES streamers(id) ON DELETE RESTRICT,
    label TEXT NOT NULL CHECK (length(trim(label)) BETWEEN 1 AND 128),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    cookies TEXT NOT NULL,
    refresh_token TEXT,
    access_token TEXT,
    reauth_config TEXT CHECK (reauth_config IS NULL OR json_valid(reauth_config)),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (owner_kind = 'platform' AND template_id IS NULL AND streamer_id IS NULL) OR
        (owner_kind = 'template' AND template_id IS NOT NULL AND streamer_id IS NULL) OR
        (owner_kind = 'streamer' AND template_id IS NULL AND streamer_id IS NOT NULL)
    )
);
CREATE INDEX idx_credential_profiles_platform ON credential_profiles(platform_config_id);
CREATE INDEX idx_credential_profiles_template ON credential_profiles(template_id) WHERE template_id IS NOT NULL;
CREATE INDEX idx_credential_profiles_streamer ON credential_profiles(streamer_id) WHERE streamer_id IS NOT NULL;

CREATE TABLE credential_profile_health (
    profile_id TEXT PRIMARY KEY NOT NULL REFERENCES credential_profiles(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision > 0),
    validity TEXT NOT NULL DEFAULT 'unknown' CHECK (validity IN ('unknown', 'valid', 'needs_refresh', 'invalid')),
    last_check_at INTEGER,
    last_refresh_at INTEGER,
    cooldown_until INTEGER,
    throttle_count INTEGER NOT NULL DEFAULT 0 CHECK (throttle_count >= 0),
    refresh_failure_count INTEGER NOT NULL DEFAULT 0 CHECK (refresh_failure_count >= 0),
    last_failure_at INTEGER,
    last_notified_failure_count INTEGER NOT NULL DEFAULT 0,
    reason_code TEXT CHECK (reason_code IS NULL OR length(reason_code) <= 64)
);

CREATE TABLE credential_login_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    principal TEXT NOT NULL,
    platform_config_id TEXT NOT NULL,
    target TEXT NOT NULL CHECK (json_valid(target)),
    provider_auth_code TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'completed', 'conflict', 'expired')),
    result_profile_id TEXT,
    result_version INTEGER,
    completed_at INTEGER
);
CREATE INDEX idx_credential_login_sessions_expiry ON credential_login_sessions(expires_at);
