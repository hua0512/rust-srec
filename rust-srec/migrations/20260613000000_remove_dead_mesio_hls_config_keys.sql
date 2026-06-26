-- Remove Mesio HLS override keys that no longer affect downloader behavior.
--
-- These keys used to deserialize into rust-srec override structs but were not
-- consulted by the Mesio HLS engine after the lifecycle refactor. Strip them
-- from persisted engine configuration JSON so stored state matches the public
-- API surface.

UPDATE engine_configuration
SET config = json_remove(
    json_remove(
        json_remove(
            json_remove(
                json_remove(
                    config,
                    '$.hls.fetcher_config.segment_raw_cache_ttl_ms'
                ),
                '$.hls.fetcher_config.streaming_threshold_bytes'
            ),
            '$.hls.performance_config.batch_scheduler'
        ),
        '$.hls.performance_config.zero_copy_enabled'
    ),
    '$.hls.performance_config.metrics_enabled'
)
WHERE engine_type = 'MESIO'
  AND json_valid(config);

UPDATE engine_configuration
SET config = json_remove(config, '$.hls.performance_config')
WHERE engine_type = 'MESIO'
  AND json_valid(config)
  AND json_type(config, '$.hls.performance_config') = 'object'
  AND NOT EXISTS (
      SELECT 1
      FROM json_each(config, '$.hls.performance_config')
  );
