-- Migrate notification priority from TEXT to INTEGER.
--
-- Priority mapping (Gotify-compatible 0-10 scale):
--   low      -> 2
--   normal   -> 5
--   high     -> 8
--   critical -> 10

-- ============================================
-- notification_event_log: priority TEXT -> INTEGER
-- ============================================

CREATE TABLE notification_event_log_new (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 5,
    payload TEXT NOT NULL,
    streamer_id TEXT,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE SET NULL
);

INSERT INTO notification_event_log_new (id, event_type, priority, payload, streamer_id, created_at)
SELECT id, event_type,
    CASE LOWER(priority)
        WHEN 'low' THEN 2
        WHEN 'normal' THEN 5
        WHEN 'high' THEN 8
        WHEN 'critical' THEN 10
        ELSE 5
    END,
    payload, streamer_id, created_at
FROM notification_event_log;

DROP TABLE notification_event_log;
ALTER TABLE notification_event_log_new RENAME TO notification_event_log;

CREATE INDEX idx_notification_event_log_created_at ON notification_event_log(created_at);
CREATE INDEX idx_notification_event_log_event_type ON notification_event_log(event_type);
CREATE INDEX idx_notification_event_log_streamer_id ON notification_event_log(streamer_id);

-- ============================================
-- web_push_subscription: min_priority TEXT -> INTEGER
-- ============================================

CREATE TABLE web_push_subscription_new (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    endpoint TEXT NOT NULL UNIQUE,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    min_priority INTEGER NOT NULL DEFAULT 10,
    created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
    next_attempt_at INTEGER,
    last_429_at INTEGER,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

INSERT INTO web_push_subscription_new (id, user_id, endpoint, p256dh, auth, min_priority, created_at, updated_at, next_attempt_at, last_429_at)
SELECT id, user_id, endpoint, p256dh, auth,
    CASE LOWER(min_priority)
        WHEN 'low' THEN 2
        WHEN 'normal' THEN 5
        WHEN 'high' THEN 8
        WHEN 'critical' THEN 10
        ELSE 10
    END,
    created_at, updated_at, next_attempt_at, last_429_at
FROM web_push_subscription;

DROP TABLE web_push_subscription;
ALTER TABLE web_push_subscription_new RENAME TO web_push_subscription;

CREATE INDEX idx_web_push_subscription_user_updated_at
    ON web_push_subscription(user_id, updated_at DESC);

CREATE INDEX idx_web_push_subscription_next_attempt_at
    ON web_push_subscription(next_attempt_at)
    WHERE next_attempt_at IS NOT NULL;

CREATE INDEX idx_web_push_subscription_last_429_at
    ON web_push_subscription(last_429_at)
    WHERE last_429_at IS NOT NULL;
