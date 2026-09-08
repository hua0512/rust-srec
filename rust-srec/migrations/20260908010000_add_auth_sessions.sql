CREATE TABLE auth_sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX idx_auth_sessions_user ON auth_sessions(user_id);
CREATE INDEX idx_auth_sessions_expiry ON auth_sessions(expires_at);

ALTER TABLE refresh_tokens ADD COLUMN session_id TEXT REFERENCES auth_sessions(id) ON DELETE CASCADE;
INSERT INTO auth_sessions(id, user_id, created_at, expires_at, revoked_at)
SELECT id, user_id, created_at, expires_at, revoked_at FROM refresh_tokens;
UPDATE refresh_tokens SET session_id = id;
CREATE INDEX idx_refresh_tokens_session ON refresh_tokens(session_id);

-- Password writes, including imports and repository callers, revoke browser sessions
-- in the same transaction as the credential change. Rotation only revokes a token row.
CREATE TRIGGER revoke_auth_sessions_after_password_change
AFTER UPDATE OF password_hash ON users
WHEN OLD.password_hash != NEW.password_hash
BEGIN
    UPDATE auth_sessions SET revoked_at = COALESCE(revoked_at, CAST(unixepoch('subsec') * 1000 AS INTEGER))
    WHERE user_id = NEW.id;
    UPDATE refresh_tokens SET revoked_at = COALESCE(revoked_at, CAST(unixepoch('subsec') * 1000 AS INTEGER))
    WHERE user_id = NEW.id;
END;

CREATE TRIGGER revoke_auth_sessions_after_account_disable
AFTER UPDATE OF is_active ON users
WHEN OLD.is_active != 0 AND NEW.is_active = 0
BEGIN
    UPDATE auth_sessions SET revoked_at = COALESCE(revoked_at, CAST(unixepoch('subsec') * 1000 AS INTEGER))
    WHERE user_id = NEW.id;
    UPDATE refresh_tokens SET revoked_at = COALESCE(revoked_at, CAST(unixepoch('subsec') * 1000 AS INTEGER))
    WHERE user_id = NEW.id;
END;
