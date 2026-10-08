ALTER TABLE live_sessions ADD COLUMN credential_binding TEXT;

CREATE TABLE credential_profiles (
    id TEXT PRIMARY KEY NOT NULL,
    platform_config_id TEXT NOT NULL REFERENCES platform_config(id) ON DELETE RESTRICT,
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
    -- When an operation last received the account's material. Unlike health,
    -- it survives revision changes; writes are coalesced to a few minutes.
    last_used_at INTEGER
);
CREATE INDEX idx_credential_profiles_platform ON credential_profiles(platform_config_id);
-- Target of the selection members' composite reference, which keeps every
-- selected profile on the selecting scope's platform.
CREATE UNIQUE INDEX idx_credential_profiles_id_platform ON credential_profiles(id, platform_config_id);

-- Which accounts a scope uses on one platform. A row belongs to the platform
-- itself, to one template for that platform, or to one streamer on it; a scope
-- without a row inherits. Rows of a retired streamer or template are removed.
CREATE TABLE credential_selections (
    id INTEGER PRIMARY KEY,
    platform_config_id TEXT NOT NULL REFERENCES platform_config(id) ON DELETE CASCADE,
    template_config_id TEXT REFERENCES template_config(id) ON DELETE CASCADE,
    streamer_id TEXT REFERENCES streamers(id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK (mode IN ('none', 'fixed', 'pool')),
    strategy TEXT CHECK (strategy IN ('priority', 'round_robin')),
    failover INTEGER CHECK (failover IN (0, 1)),
    max_attempts INTEGER CHECK (max_attempts BETWEEN 1 AND 10),
    CHECK (template_config_id IS NULL OR streamer_id IS NULL),
    CHECK (
        (mode = 'pool' AND strategy IS NOT NULL AND failover IS NOT NULL AND max_attempts IS NOT NULL)
        OR (mode != 'pool' AND strategy IS NULL AND failover IS NULL AND max_attempts IS NULL)
    ),
    UNIQUE (id, platform_config_id)
);
CREATE UNIQUE INDEX idx_credential_selections_platform ON credential_selections(platform_config_id)
    WHERE template_config_id IS NULL AND streamer_id IS NULL;
CREATE UNIQUE INDEX idx_credential_selections_template ON credential_selections(template_config_id, platform_config_id)
    WHERE template_config_id IS NOT NULL;
CREATE UNIQUE INDEX idx_credential_selections_streamer ON credential_selections(streamer_id)
    WHERE streamer_id IS NOT NULL;

-- Ordered accounts of a selection. The platform column ties each member to a
-- profile of the selection's own platform, and a selected profile cannot be
-- deleted.
CREATE TABLE credential_selection_members (
    selection_id INTEGER NOT NULL,
    platform_config_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    profile_id TEXT NOT NULL,
    PRIMARY KEY (selection_id, position),
    UNIQUE (selection_id, profile_id),
    FOREIGN KEY (selection_id, platform_config_id)
        REFERENCES credential_selections(id, platform_config_id) ON DELETE CASCADE,
    FOREIGN KEY (profile_id, platform_config_id)
        REFERENCES credential_profiles(id, platform_config_id) ON DELETE RESTRICT
) WITHOUT ROWID;
CREATE INDEX idx_credential_selection_members_profile ON credential_selection_members(profile_id, platform_config_id);

-- A streamer marked deleted no longer selects accounts, and one that moves to
-- another platform starts inheriting there.
CREATE TRIGGER credential_selections_streamer_retired
AFTER UPDATE OF deleted_at ON streamers
WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM credential_selections WHERE streamer_id = NEW.id;
END;
CREATE TRIGGER credential_selections_streamer_moved
AFTER UPDATE OF platform_config_id ON streamers
WHEN NEW.platform_config_id IS NOT OLD.platform_config_id
BEGIN
    DELETE FROM credential_selections WHERE streamer_id = NEW.id;
END;

CREATE TABLE credential_profile_health (
    profile_id TEXT PRIMARY KEY NOT NULL REFERENCES credential_profiles(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision > 0),
    validity TEXT NOT NULL DEFAULT 'unknown' CHECK (validity IN ('unknown', 'valid', 'needs_refresh', 'invalid')),
    last_check_at INTEGER,
    last_refresh_at INTEGER,
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
