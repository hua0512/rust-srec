-- How the monitor obtained the row's status: 'batch' for a batch status call,
-- NULL for the streamer's own check (every existing row, so no backfill).

ALTER TABLE streamer_check_history
    ADD COLUMN source TEXT CHECK (source IN ('batch'));
