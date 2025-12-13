-- Monitor event outbox + invariants for transactional monitoring.

-- Enforce at most one active (end_time IS NULL) session per streamer.
CREATE UNIQUE INDEX IF NOT EXISTS live_sessions_one_active_per_streamer
    ON live_sessions (streamer_id)
    WHERE end_time IS NULL;

-- Transactional outbox for monitor events.
-- Events are inserted in the same transaction as state/session updates and
-- published asynchronously after commit.
CREATE TABLE IF NOT EXISTS monitor_event_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    streamer_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at TEXT NOT NULL,
    delivered_at TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS monitor_event_outbox_undelivered
    ON monitor_event_outbox (delivered_at, id);
