-- Add engagement columns to danmu_statistics.
--
-- `unique_talkers` stores the HyperLogLog distinct-sender estimate computed by
-- `StatisticsAggregator`; `top_gifters` and `top_gifts` store JSON arrays of
-- gift-item tallies extracted from the `gift_name`/`gift_count` message
-- metadata emitted by the platform danmu providers. All three stay NULL on
-- rows written before this migration; readers treat NULL as "not tracked".

ALTER TABLE danmu_statistics ADD COLUMN unique_talkers INTEGER;
ALTER TABLE danmu_statistics ADD COLUMN top_gifters TEXT;
ALTER TABLE danmu_statistics ADD COLUMN top_gifts TEXT;
