-- Adds `source` to `streamer_check_history`: how the monitor obtained the
-- row's status.
--
--   'batch' — a platform batch status call covering many streamers reported
--             the streamer offline, so its own check did not run. Only
--             `outcome = 'offline'` rows carry it.
--   NULL    — the streamer's own check. Every row written before this column
--             existed was one, so existing rows need no backfill.

ALTER TABLE streamer_check_history
    ADD COLUMN source TEXT CHECK (source IN ('batch'));
