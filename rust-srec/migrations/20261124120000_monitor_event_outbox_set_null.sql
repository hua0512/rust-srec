-- no-transaction
--
-- `monitor_event_outbox.streamer_id` becomes nullable with `ON DELETE SET NULL`
-- so reaping a deleted streamer (`DELETE FROM streamers`) preserves undelivered
-- durable notifications (such as fatal errors or offline alerts) for replay.
-- The JSON `payload` retains the original `streamer_id` and `streamer_name`,
-- while the foreign-key column clears to NULL once the streamer row is gone.
--
-- Replay safety: SQLx records this migration only after the `-- no-transaction`
-- script finishes. Both sides of the copy list the same eight columns and the
-- rebuilt table matches the target schema, so replaying the file preserves all
-- rows, IDs, payloads, timestamps, and the undelivered scan index.

PRAGMA foreign_keys=OFF;

BEGIN;

DROP TABLE IF EXISTS monitor_event_outbox_new;

CREATE TABLE monitor_event_outbox_new (
    id INTEGER PRIMARY KEY,
    streamer_id TEXT,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    delivered_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE SET NULL
);

INSERT INTO monitor_event_outbox_new (
    id,
    streamer_id,
    event_type,
    payload,
    created_at,
    delivered_at,
    attempts,
    last_error
)
SELECT
    id,
    streamer_id,
    event_type,
    payload,
    created_at,
    delivered_at,
    attempts,
    last_error
FROM monitor_event_outbox;

DROP TABLE monitor_event_outbox;

ALTER TABLE monitor_event_outbox_new RENAME TO monitor_event_outbox;

CREATE INDEX IF NOT EXISTS monitor_event_outbox_undelivered
    ON monitor_event_outbox (delivered_at, id);

COMMIT;

PRAGMA foreign_keys=ON;
