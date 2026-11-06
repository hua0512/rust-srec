-- Whether a pool's checks continue without an account when none of its
-- accounts can be used. Only pools set it; other modes keep 0.
ALTER TABLE credential_selections
    ADD COLUMN anonymous_fallback INTEGER NOT NULL DEFAULT 0 CHECK (anonymous_fallback IN (0, 1));
