-- How the monitor obtained the row's status: 'batch' for a batch status call,
-- 'individual' for the streamer's own check (every existing row).

ALTER TABLE streamer_check_history
    ADD COLUMN source TEXT NOT NULL DEFAULT 'individual'
        CHECK (source IN ('individual', 'batch'));
