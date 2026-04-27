-- `session_events` — append-only audit log of `SessionLifecycle` state-machine
-- transitions. One row per transition (`session_started`, `hysteresis_entered`,
-- `session_resumed`, `session_ended`). Powers the Session Detail page's
-- Timeline tab so operators can see *why* a session ended without reading the
-- server log.
--
-- Today the timeline is computed from `live_sessions.titles` alone, so the
-- danmu-driven end of a session (DanmuControlEvent::StreamClosed →
-- TerminalCause::DefinitiveOffline { signal: DanmuStreamClosed }) leaves no
-- UI-visible trace. This table fixes that without changing the existing
-- `live_sessions` shape.
--
-- Writes are issued from `SessionLifecycle`:
--   - session_started / session_ended → atomic with the `live_sessions` write
--     (same `BEGIN IMMEDIATE` tx, in `SessionLifecycleRepository`).
--   - hysteresis_entered / session_resumed → best-effort (no existing tx,
--     in-memory transitions only). A failed write logs and continues; the
--     subsequent `session_ended` row is still atomic and tells the full story.
--
-- The `kind` column is duplicated information vs the JSON `payload` (which
-- carries the typed `SessionEventPayload` via `#[serde(tag = "kind")]`), but
-- having it as a top-level column lets us index/filter without parsing JSON
-- and gives us a `CHECK` constraint that catches typos at insert time.

CREATE TABLE session_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT    NOT NULL,
    streamer_id   TEXT    NOT NULL,
    kind          TEXT    NOT NULL CHECK (kind IN (
                      'session_started',
                      'hysteresis_entered',
                      'session_resumed',
                      'session_ended'
                  )),
    -- Milliseconds since Unix epoch (UTC). Matches `live_sessions.start_time`
    -- and `live_sessions.end_time` units.
    occurred_at   INTEGER NOT NULL,
    -- JSON-encoded `SessionEventPayload`. NULL is reserved for a future event
    -- kind that has no payload fields; today every kind serialises a payload.
    payload       TEXT,
    FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE
);

-- Primary read pattern: "all events for this session, oldest first".
CREATE INDEX idx_session_events_session_id_occurred_at
    ON session_events(session_id, occurred_at ASC);

-- Secondary read pattern: "recent events for this streamer" (e.g. an
-- operator-facing audit of recent sessions across the streamer's history).
CREATE INDEX idx_session_events_streamer_id_occurred_at
    ON session_events(streamer_id, occurred_at DESC);
