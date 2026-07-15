-- Seed SOOP platform config (platform_name is matched case-insensitively
-- against StreamerUrl::platform() == "SOOP")
INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
VALUES ('platform-soop', 'soop', NULL, NULL)
ON CONFLICT(platform_name) DO NOTHING;
