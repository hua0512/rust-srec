-- Remove the `cookies` and `proxy_config` columns: accounts are credential
-- profiles and connections are proxy routes.
--
-- The startup conversions that turn the old values into profiles and routes
-- run after every migration, so values they have not converted yet move into
-- `legacy_cookies` and `legacy_proxy_settings` first. Each conversion reads its
-- table and drops it together with its marker.
--
-- ALTER TABLE ... DROP COLUMN applies in place: no index, constraint, trigger
-- or foreign key touches these columns, and rebuilding `platform_config` or
-- `template_config` inside the migration transaction would fail on the
-- streamers and account selections that reference them.

CREATE TABLE legacy_cookies (
    scope TEXT NOT NULL CHECK (scope IN ('platform', 'template')),
    id TEXT NOT NULL,
    cookies TEXT NOT NULL,
    PRIMARY KEY (scope, id)
);
INSERT INTO legacy_cookies (scope, id, cookies)
SELECT 'platform', id, cookies FROM platform_config WHERE cookies IS NOT NULL;
INSERT INTO legacy_cookies (scope, id, cookies)
SELECT 'template', id, cookies FROM template_config WHERE cookies IS NOT NULL;

CREATE TABLE legacy_proxy_settings (
    scope TEXT NOT NULL CHECK (scope IN ('global', 'platform', 'template')),
    id TEXT NOT NULL,
    proxy_config TEXT NOT NULL,
    PRIMARY KEY (scope, id)
);
INSERT INTO legacy_proxy_settings (scope, id, proxy_config)
SELECT 'global', id, proxy_config FROM global_config WHERE proxy_config IS NOT NULL;
INSERT INTO legacy_proxy_settings (scope, id, proxy_config)
SELECT 'platform', id, proxy_config FROM platform_config WHERE proxy_config IS NOT NULL;
INSERT INTO legacy_proxy_settings (scope, id, proxy_config)
SELECT 'template', id, proxy_config FROM template_config WHERE proxy_config IS NOT NULL;

ALTER TABLE global_config DROP COLUMN proxy_config;
ALTER TABLE platform_config DROP COLUMN cookies;
ALTER TABLE platform_config DROP COLUMN proxy_config;
ALTER TABLE template_config DROP COLUMN cookies;
ALTER TABLE template_config DROP COLUMN proxy_config;
