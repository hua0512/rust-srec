-- Surface danmu statistics that `StatisticsAggregator` already computed but had
-- nowhere to go.
--
-- `DanmuStatistics` carries a chat/gift split, the session window and the width
-- of one activity-timeline bucket, none of which existed on `danmu_statistics`,
-- so `persist_statistics` dropped them and
-- `GET /api/sessions/{id}/danmu-statistics` could not report them.
--
-- `rate_bucket_secs` is the important one for correctness rather than richness:
-- `StatisticsAggregator::coarsen_rate_data` doubles the bucket width whenever the
-- point count would exceed its budget, so the width varies within a session and a
-- consumer converting `danmu_rate_timeseries` counts into a per-minute rate has
-- no way to know the divisor without it.
--
-- All columns are nullable and stay NULL on rows written before this migration;
-- readers treat NULL as "not tracked". Times are Unix epoch milliseconds, matching
-- `live_sessions.start_time` and `crate::database::time::ms_to_datetime`.

ALTER TABLE danmu_statistics ADD COLUMN chat_count INTEGER;
ALTER TABLE danmu_statistics ADD COLUMN gift_count INTEGER;
ALTER TABLE danmu_statistics ADD COLUMN duration_secs INTEGER;
ALTER TABLE danmu_statistics ADD COLUMN start_time INTEGER;
ALTER TABLE danmu_statistics ADD COLUMN end_time INTEGER;
ALTER TABLE danmu_statistics ADD COLUMN rate_bucket_secs INTEGER;
