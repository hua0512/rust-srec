-- Marks a streamer as deleted before its row is physically removed, so the
-- runtime owners that hold its id can be stood down and awaited first.
--
-- `StreamerMetadata::is_active` is false while this is set, which is what makes
-- `Scheduler::ensure_streamer_actor_state` retire the actor,
-- `run_live_download_pipeline` refuse to start a recording, and
-- `ConfigEventHandler` run `RuntimeCoordinator::handle_streamer_disabled`.
-- `RuntimeCoordinator::retire_streamer` then waits for those owners and
-- `StreamerManager::reap_deleted` runs the `DELETE FROM streamers`, whose
-- cascade reaches `filters`, `monitor_event_outbox` and
-- `streamer_check_history` and whose `ON DELETE SET NULL` leaves
-- `live_sessions` and everything under it in place.
--
-- A plain `ADD COLUMN` rather than a new `streamers.state` variant: `state`
-- carries a `CHECK` constraint, so a new value would mean rebuilding a table
-- that four `ON DELETE CASCADE` children point at, and
-- `StreamerManager::partial_update_streamer` writes `state` unconditionally on
-- every monitor tick, which would clobber the marker.
--
-- No backfill: no row can carry a marker written before this column exists, so
-- the recovery pass `ServiceContainer` runs at startup finds nothing on the
-- upgrade itself.

ALTER TABLE streamers
    ADD COLUMN deleted_at INTEGER;

-- Serves the startup recovery and the periodic reaper sweep, both of which
-- enumerate only marked rows; partial so it stays empty in the steady state.
CREATE INDEX IF NOT EXISTS idx_streamers_deleted_at
    ON streamers(deleted_at)
    WHERE deleted_at IS NOT NULL;
