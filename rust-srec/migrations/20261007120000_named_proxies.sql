-- Saved proxies that global, platform, template, streamer and account settings
-- refer to by ID. One entry is one exit: the URL and username identify it, and
-- the password only authenticates it.
CREATE TABLE proxies (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 64),
    -- Canonical `scheme://host[:port]` without a login.
    url TEXT NOT NULL CHECK (length(url) BETWEEN 1 AND 2048),
    username TEXT,
    password TEXT,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK ((username IS NULL) = (password IS NULL)),
    CHECK (username IS NULL OR length(username) > 0)
);
CREATE UNIQUE INDEX idx_proxies_name ON proxies(name COLLATE NOCASE);
CREATE UNIQUE INDEX idx_proxies_endpoint ON proxies(url, COALESCE(username, ''));

-- How each scope connects. `proxy` names an entry, which cannot be deleted
-- while a route uses it; `inherit` follows the next scope out, and the global
-- route always decides. An account that inherits follows the route of the
-- operation using it.
ALTER TABLE global_config ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'direct'
    CHECK (proxy_route IN ('direct', 'system', 'proxy'));
ALTER TABLE global_config ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));

ALTER TABLE platform_config ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_route IN ('inherit', 'direct', 'system', 'proxy'));
ALTER TABLE platform_config ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));
CREATE INDEX idx_platform_config_proxy ON platform_config(proxy_id) WHERE proxy_id IS NOT NULL;

ALTER TABLE template_config ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_route IN ('inherit', 'direct', 'system', 'proxy'));
ALTER TABLE template_config ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));
CREATE INDEX idx_template_config_proxy ON template_config(proxy_id) WHERE proxy_id IS NOT NULL;

ALTER TABLE streamers ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_route IN ('inherit', 'direct', 'system', 'proxy'));
ALTER TABLE streamers ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));
CREATE INDEX idx_streamers_proxy ON streamers(proxy_id) WHERE proxy_id IS NOT NULL;

ALTER TABLE credential_profiles ADD COLUMN proxy_route TEXT NOT NULL DEFAULT 'inherit'
    CHECK (proxy_route IN ('inherit', 'direct', 'system', 'proxy'));
ALTER TABLE credential_profiles ADD COLUMN proxy_id TEXT REFERENCES proxies(id) ON DELETE RESTRICT
    CHECK ((proxy_route = 'proxy') = (proxy_id IS NOT NULL));
CREATE INDEX idx_credential_profiles_proxy ON credential_profiles(proxy_id) WHERE proxy_id IS NOT NULL;

-- A streamer marked deleted no longer connects anywhere, so it stops holding
-- a proxy entry in use.
CREATE TRIGGER streamers_retired_proxy_route
AFTER UPDATE OF deleted_at ON streamers
WHEN NEW.deleted_at IS NOT NULL AND NEW.proxy_route != 'inherit'
BEGIN
    UPDATE streamers SET proxy_route = 'inherit', proxy_id = NULL WHERE id = NEW.id;
END;

-- Converting the `proxy_config` JSON stored on each scope into entries and
-- routes needs application logic, so it runs right after migrations and drops
-- this marker when done.
CREATE TABLE legacy_proxy_upgrade_pending (
    id INTEGER PRIMARY KEY CHECK (id = 1)
);
INSERT INTO legacy_proxy_upgrade_pending (id) VALUES (1);
