-- Remove the danmu sampling configuration layer.
--
-- The sampler was never wired into collection: `DanmuService` always ran a
-- no-op sampler and nothing consumed sampling decisions, so
-- `template_config.danmu_sampling_config` and the streamer-level
-- `$.danmu_sampling_config` key no longer have a reader. Statistics are
-- aggregated for every message by `StatisticsAggregator` (bounded
-- heavy-hitter structures), which makes sampling unnecessary.
--
-- ALTER TABLE ... DROP COLUMN applies in place here: the column is a plain
-- nullable TEXT with no index, constraint, or foreign key touching it.

ALTER TABLE template_config DROP COLUMN danmu_sampling_config;

-- The streamer layer kept its copy under `$.danmu_sampling_config` in the
-- untyped blob read by `MergedConfigBuilder::with_streamer`.
UPDATE streamers
SET streamer_specific_config = json_remove(streamer_specific_config, '$.danmu_sampling_config')
WHERE streamer_specific_config IS NOT NULL
  AND json_valid(streamer_specific_config)
  AND json_type(streamer_specific_config, '$.danmu_sampling_config') IS NOT NULL;

-- `live_sessions.danmu_statistics_id` has always been written as NULL; danmu
-- statistics link to sessions through `danmu_statistics.session_id` (UNIQUE)
-- instead. The column itself cannot be dropped in place: it is indexed and
-- declares a foreign key, both of which ALTER TABLE ... DROP COLUMN rejects.
-- A live_sessions rebuild is not usable either -- media_outputs,
-- danmu_statistics, session_events and session_segments hold foreign keys
-- into live_sessions, connections run with `PRAGMA foreign_keys = ON`, and
-- that pragma is a no-op inside the transaction sqlx wraps each migration in,
-- so DROP TABLE on the parent aborts while any child row exists. Drop the
-- pointless index; the NULL column stays behind as an inert vestige that no
-- code reads or writes.
DROP INDEX idx_live_session_danmu_statistics_id;
