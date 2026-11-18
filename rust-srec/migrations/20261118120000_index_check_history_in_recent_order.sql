-- The history list and the retention trim both read a streamer's rows newest
-- first with `id` breaking ties (ORDER BY checked_at DESC, id DESC). An index
-- keeps the rowid ascending after its own columns, so only an ascending
-- `checked_at` read backwards yields that order; with `checked_at DESC` the
-- tiebreak needs a sort.
DROP INDEX idx_streamer_check_history_streamer_id_checked_at;

CREATE INDEX idx_streamer_check_history_streamer_id_checked_at
    ON streamer_check_history(streamer_id, checked_at);
