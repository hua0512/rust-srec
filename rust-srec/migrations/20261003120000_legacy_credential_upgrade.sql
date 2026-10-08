-- Cookies, tokens and account logins stored on platform, template and streamer
-- configuration move into credential profiles. The conversion needs application
-- logic, so it runs right after migrations and drops this marker when done.
CREATE TABLE legacy_credential_upgrade_pending (
    id INTEGER PRIMARY KEY CHECK (id = 1)
);
INSERT INTO legacy_credential_upgrade_pending (id) VALUES (1);
