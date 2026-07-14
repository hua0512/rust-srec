-- Seed Bigo Live platform config (platform_name is matched case-insensitively
-- against StreamerUrl::platform() == "Bigo")
INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
VALUES ('platform-bigo', 'bigo', NULL, NULL)
ON CONFLICT(platform_name) DO NOTHING;
