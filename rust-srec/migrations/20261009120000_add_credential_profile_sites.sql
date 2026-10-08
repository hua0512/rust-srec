-- The sites a Streamlink account is for. A Streamlink streamer without an
-- account of its own uses the account whose site covers its URL's host. A
-- site belongs to at most one account, so that choice is never ambiguous.
CREATE TABLE credential_profile_sites (
    site TEXT PRIMARY KEY NOT NULL CHECK (length(site) BETWEEN 1 AND 253),
    profile_id TEXT NOT NULL REFERENCES credential_profiles(id) ON DELETE CASCADE
) WITHOUT ROWID;
CREATE INDEX idx_credential_profile_sites_profile ON credential_profile_sites(profile_id);
