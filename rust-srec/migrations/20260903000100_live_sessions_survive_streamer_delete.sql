-- no-transaction
--
-- `live_sessions.streamer_id` is nullable with `ON DELETE SET NULL`, so
-- `DELETE FROM streamers` leaves the recording history in place. It matters
-- that this is not a cascade: `live_sessions` is the parent of
-- `media_outputs`, `danmu_statistics`, `session_segments`, `session_events`
-- and `danmu_aggregator_state`, all `ON DELETE CASCADE`, so deleting a session
-- row reaches every recorded file row, segment and danmu statistic under it.
--
-- Why `-- no-transaction`: rebuilding a foreign-key *parent* means
-- `DROP TABLE live_sessions`, which cascades into those five children unless
-- foreign keys are actually off. Connections are opened with
-- `foreign_keys(true)` (`database::sqlite_connect_options`) and
-- `PRAGMA foreign_keys` is a silent no-op inside a transaction, which is the
-- transaction sqlx would otherwise wrap this file in. So the file opts out,
-- toggles the pragma itself, and runs the rebuild in its own BEGIN/COMMIT.
--
-- Replay safety: sqlx records the version in `_sqlx_migrations` only after the
-- script returns, so a crash after COMMIT replays the whole file. The copy
-- lists the same nine columns on both sides, and the source table after a
-- successful run has exactly the target shape, so a second run reproduces the
-- same rows. `DROP TABLE IF EXISTS` and `CREATE ... IF NOT EXISTS` cover the
-- rest.

PRAGMA foreign_keys=OFF;

BEGIN;

DROP TABLE IF EXISTS live_sessions_new;

CREATE TABLE live_sessions_new (
    id TEXT PRIMARY KEY NOT NULL,
    -- NULL once the referenced `streamers` row is deleted; `streamer_name`
    -- keeps the label the session was recorded under.
    streamer_id TEXT,
    streamer_name TEXT,
    start_time INTEGER NOT NULL,
    end_time INTEGER,
    titles TEXT,
    danmu_statistics_id TEXT,
    total_size_bytes BIGINT NOT NULL DEFAULT 0,
    session_complete_dispatched INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE SET NULL,
    FOREIGN KEY (danmu_statistics_id) REFERENCES danmu_statistics(id)
);

INSERT INTO live_sessions_new (
    id,
    streamer_id,
    streamer_name,
    start_time,
    end_time,
    titles,
    danmu_statistics_id,
    total_size_bytes,
    session_complete_dispatched
)
SELECT
    id,
    streamer_id,
    streamer_name,
    start_time,
    end_time,
    titles,
    danmu_statistics_id,
    total_size_bytes,
    session_complete_dispatched
FROM live_sessions;

DROP TABLE live_sessions;

ALTER TABLE live_sessions_new RENAME TO live_sessions;

-- Every index the dropped table carried. `live_sessions_one_active_per_streamer`
-- is what `SessionLifecycleRepository::start_or_resume` relies on to cap a
-- streamer at one row with `end_time IS NULL`; SQLite treats NULL keys as
-- distinct, so it constrains only rows with a non-NULL `streamer_id`. The
-- trigger below is what keeps NULL-owner rows out of the `end_time IS NULL` set.
CREATE UNIQUE INDEX IF NOT EXISTS live_sessions_one_active_per_streamer
    ON live_sessions (streamer_id)
    WHERE end_time IS NULL;

CREATE INDEX IF NOT EXISTS idx_live_session_streamer_id
    ON live_sessions(streamer_id);

CREATE INDEX IF NOT EXISTS idx_live_session_streamer_time
    ON live_sessions(streamer_id, start_time DESC);

CREATE INDEX IF NOT EXISTS idx_live_sessions_empty_ended
    ON live_sessions(end_time)
    WHERE total_size_bytes = 0 AND end_time IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_live_sessions_start_time
    ON live_sessions(start_time DESC);

-- Nothing owns a session whose `streamer_id` the foreign-key action just
-- nulled: no `StreamerActor` polls it and `SessionLifecycle` has no in-memory
-- entry for it, so `end_time` would stay NULL forever and every reader that
-- treats that as "recording in progress" (`SessionResponse::is_live`, the
-- `active_only` branch of `SqlxSessionRepository::list_sessions_filtered`,
-- `PipelineManager` startup recovery) would keep reporting it as live.
-- Foreign-key actions fire row triggers, so closing the session here runs
-- inside the same statement as the `DELETE FROM streamers`.
CREATE TRIGGER IF NOT EXISTS trg_live_session_orphan_ends
AFTER UPDATE OF streamer_id ON live_sessions
FOR EACH ROW
WHEN NEW.streamer_id IS NULL
  AND OLD.streamer_id IS NOT NULL
  AND NEW.end_time IS NULL
BEGIN
    UPDATE live_sessions
    SET end_time = unixepoch('now') * 1000
    WHERE id = NEW.id;
END;

COMMIT;

PRAGMA foreign_keys=ON;
