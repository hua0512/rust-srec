-- A streamer stopped by the user is `DISABLED` and nothing writes
-- `CANCELLED`, so rows holding it become `DISABLED`. The error bookkeeping
-- is cleared as on any disable, so re-enabling starts from a clean history
-- instead of a leftover backoff. The `streamers.state` CHECK constraint keeps
-- allowing `CANCELLED`, because narrowing it means rebuilding the table.

UPDATE streamers
SET state = 'DISABLED',
    consecutive_error_count = 0,
    disabled_until = NULL,
    last_error = NULL
WHERE state = 'CANCELLED';
