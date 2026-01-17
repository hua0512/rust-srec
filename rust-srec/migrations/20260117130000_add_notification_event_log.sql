-- Add persistent notification event log for UI/debugging/audit.

CREATE TABLE IF NOT EXISTS notification_event_log (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    priority TEXT NOT NULL,
    payload TEXT NOT NULL,
    streamer_id TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_notification_event_log_created_at
    ON notification_event_log(created_at);

CREATE INDEX IF NOT EXISTS idx_notification_event_log_event_type
    ON notification_event_log(event_type);

CREATE INDEX IF NOT EXISTS idx_notification_event_log_streamer_id
    ON notification_event_log(streamer_id);


-- Add notification event log retention to global config
-- This allows configuring how long to keep rows in `notification_event_log`.

ALTER TABLE global_config
ADD COLUMN notification_event_log_retention_days INTEGER NOT NULL DEFAULT 30;

-- Add Web Push subscriptions for browser push notifications

CREATE TABLE web_push_subscription (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    endpoint TEXT NOT NULL UNIQUE,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    -- Minimum priority to send (low|normal|high|critical)
    min_priority TEXT NOT NULL DEFAULT 'critical',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_web_push_subscription_user_id
ON web_push_subscription(user_id);