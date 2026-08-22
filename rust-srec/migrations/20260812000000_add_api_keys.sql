-- Long-lived API keys for programmatic access (REST + the /api/mcp endpoint).
--
-- Mirrors the `refresh_tokens` storage model: only the SHA-256 hex digest of
-- the key is stored (`key_hash`); the raw key (`srec_<random>`) is shown once
-- at creation by `AuthService::create_api_key` and cannot be recovered.
-- `key_prefix` keeps the first characters of the raw key for display in the
-- key-management UI.
--
-- `access_level` gates what the key may do: 'read_only' keys are limited to
-- safe requests (GET/HEAD/OPTIONS on REST, read tools on MCP) by the auth
-- middleware; 'full' keys act with the owning user's roles.
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL,
    access_level TEXT NOT NULL,
    -- Milliseconds since Unix epoch (UTC); NULL = never expires.
    expires_at INTEGER,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_api_keys_user_id ON api_keys(user_id);
