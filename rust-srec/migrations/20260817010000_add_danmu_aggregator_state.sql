-- Checkpoint of a danmu collector's internal counting state, so a collector
-- restarted mid-session continues instead of beginning at zero.
--
-- Needed because the published statistics cannot be reloaded into an aggregator:
-- a HyperLogLog estimate does not yield back its registers, and a truncated
-- top-N does not yield back the Space-Saving counters behind it. Without this,
-- the first snapshot after a restart overwrote `danmu_statistics` with numbers
-- lower than the ones already stored.
--
-- Its own table rather than a column on `danmu_statistics` for two reasons: the
-- blob is far larger than the derived statistics and would otherwise be read by
-- every `SELECT` on that table, and it is transient — only useful while a session
-- is in flight — so it wants an independent lifetime.
--
-- `state` is gzip-compressed JSON written by `StatisticsAggregator::export_state`
-- and read back through `AggregatorState`, whose `version` field guards the
-- layout; an unrecognized version is discarded rather than migrated. `version` is
-- duplicated as a column so a future migration can delete incompatible rows
-- without decompressing them.
--
-- CASCADE ties the checkpoint's lifetime to the session, so deleting a session
-- reclaims it. `DanmuService` also deletes the row when collection stops for a
-- session that is genuinely ending rather than being interrupted by a shutdown.

CREATE TABLE danmu_aggregator_state (
    session_id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    state BLOB NOT NULL,
    FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE
);
