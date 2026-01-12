-- Add a pseudo-platform for Streamlink-based extraction fallback.
--
-- This enables selecting `{platform}` = "streamlink" via platform_config_id and
-- provides a dedicated platform layer for Streamlink extractor tuning.

INSERT INTO platform_config (id, platform_name, download_engine, platform_specific_config)
VALUES (
    'platform-streamlink',
    'streamlink',
    'default-streamlink',
    '{"streamlink":{}}'
)
ON CONFLICT(platform_name) DO UPDATE SET
    download_engine = COALESCE(platform_config.download_engine, excluded.download_engine),
    platform_specific_config = COALESCE(platform_config.platform_specific_config, excluded.platform_specific_config);

