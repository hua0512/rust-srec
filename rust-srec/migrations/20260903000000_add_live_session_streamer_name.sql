-- Snapshot the streamer's display name onto every session row.
--
-- `SessionTxOps::create_session` writes this column from the name the monitor
-- already carries on `MonitorEvent::StreamerLive`, so a session keeps a label
-- to render even when `live_sessions.streamer_id` no longer resolves to a
-- `streamers` row. Readers (`api::routes::sessions`,
-- `SqlxSessionRepository::list_sessions_filtered`) prefer the joined
-- `streamers.name` while the streamer exists and fall back to this column
-- otherwise, so a rename after the session started is still reflected live.
--
-- Plain ALTER TABLE on purpose: `live_sessions` is the foreign-key parent of
-- `media_outputs`, `danmu_statistics`, `session_segments`, `session_events`
-- and `danmu_aggregator_state`, and a create-copy-drop-rename on a parent
-- deletes those children unless foreign keys are genuinely off. Adding a
-- plain nullable column needs no rebuild.
ALTER TABLE live_sessions ADD COLUMN streamer_name TEXT;

-- Backfill from the current `streamers` row. A session whose `streamer_id`
-- matches nothing keeps NULL: the name is genuinely unknown, and every reader
-- already renders an empty label when the streamer lookup misses.
UPDATE live_sessions
SET streamer_name = (
        SELECT name FROM streamers WHERE streamers.id = live_sessions.streamer_id
    )
WHERE streamer_name IS NULL;
