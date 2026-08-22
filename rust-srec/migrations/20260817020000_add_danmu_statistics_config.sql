-- Make danmu statistics configurable per layer.
--
-- `record_danmu` was the only danmu setting: how many talkers and words a
-- session reported, how wide an activity bucket was, how many distinct senders
-- were tracked before counts became approximate, and which words were ignored
-- were all compile-time constants in `DanmuService` and `StatisticsAggregator`.
--
-- Stored as a JSON object read by `DanmuStatisticsConfig`, whose fields all
-- default individually, so `{"top_talkers": 200}` is a valid override that
-- inherits the rest. `MergedConfigBuilder` replaces the whole object per layer
-- rather than merging field by field, and `sanitized()` clamps user values.
--
-- Follows the existing nested-config shape: a nullable TEXT column at the global,
-- platform and template layers, and the key `$.danmu_statistics` inside
-- `streamers.streamer_specific_config` for the streamer layer, exactly as
-- `pipeline`, `stream_selection_config` and `download_retry_policy` are stored.
-- NULL means "inherit from the layer above"; the global column is nullable too,
-- and a NULL there resolves to `DanmuStatisticsConfig::default()`.

ALTER TABLE global_config ADD COLUMN danmu_statistics TEXT;
ALTER TABLE platform_config ADD COLUMN danmu_statistics TEXT;
ALTER TABLE template_config ADD COLUMN danmu_statistics TEXT;
