-- Retain authentication material until recordings bound to an omitted profile settle.
CREATE TABLE retirement_credential_profiles (
    profile_id TEXT PRIMARY KEY NOT NULL REFERENCES credential_profiles(id) ON DELETE CASCADE
);
