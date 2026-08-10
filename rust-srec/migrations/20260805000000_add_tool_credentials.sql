-- Opt-in stored credentials for external CLI tools.
--
-- Same intent as `upload_records.uploader`: `tool` is free-form so a future
-- tool reuses this table without a schema change. `account_key` is the
-- per-tool account discriminator ('' = that tool's default account); for
-- `baidupcs` it is the BaiduPCS-Go config directory. `payload` is
-- tool-shaped JSON (`crate::baidupcs::LoginMaterial` for baidupcs).
--
-- Rows are written only when the user opts in via a login dialog's remember
-- flag (`POST /api/tools/baidupcs/login` handles the save) and deleted by
-- the matching logout. `BaiduPcsProcessor` replays the payload through
-- `BaiduPCS-Go login` when a session turns out to be logged out or an
-- upload attempt failed.
--
-- Values are plaintext, matching how `platform_config.cookies` is stored;
-- the API never returns them (`has_stored_credentials` only).
CREATE TABLE tool_credentials (
    tool TEXT NOT NULL,
    account_key TEXT NOT NULL,
    payload TEXT NOT NULL,
    -- Milliseconds since Unix epoch (UTC).
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (tool, account_key)
);
