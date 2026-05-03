-- `streamer_check_history` — append-only ring buffer of monitor poll outcomes
-- per streamer. One row per call to `MonitorStatusChecker::check_status`,
-- written best-effort by a background writer task so DB latency never blocks
-- the polling loop.
--
-- Powers the "uptime-bar" check-history strip on the streamer details page,
-- which renders the most-recent rows as colored bars and exposes per-check
-- stream-selection telemetry on hover (how many stream candidates the
-- platform extractor returned, and which one we picked). This is operator-
-- facing diagnostic data — useful when a streamer's stream URL flaps between
-- qualities, or when the extractor occasionally returns zero candidates.
--
-- Retention: the writer task trims to the most-recent 200 rows per streamer
-- on insert; the API caps `?limit=` at 200 and the UI typically displays 60.
-- 200 rows × ~200 B × N streamers stays well under a few MB at typical
-- deployment sizes.
--
-- The `outcome` column is the discriminator; `fatal_kind`, `filter_reason`,
-- and `error_message` carry outcome-specific detail (mutually exclusive).
-- `streams_extracted` is the count of candidates the platform extractor
-- returned BEFORE selection narrowed it to one; `stream_selected` is the
-- JSON-encoded chosen stream descriptor (quality, format, bitrate, codec,
-- fps) or NULL on non-live outcomes.
--
-- Cascade on `streamers` delete: when an operator removes a streamer, the
-- check history goes with it. There is no audit requirement to retain the
-- diagnostic strip after the streamer is gone.

CREATE TABLE streamer_check_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    streamer_id         TEXT    NOT NULL,
    -- Milliseconds since Unix epoch (UTC). Matches `live_sessions.start_time`
    -- and `session_events.occurred_at` units.
    checked_at          INTEGER NOT NULL,
    -- Wall-clock duration of the check, in milliseconds. Useful for spotting
    -- slow extractor responses on the bar's tooltip.
    duration_ms         INTEGER NOT NULL,
    outcome             TEXT    NOT NULL CHECK (outcome IN (
                            'live',
                            'offline',
                            'filtered',
                            'transient_error',
                            'fatal_error'
                        )),
    -- Discriminator detail for fatal outcomes (NotFound|Banned|AgeRestricted
    -- |RegionLocked|Private|UnsupportedPlatform). NULL otherwise.
    fatal_kind          TEXT,
    -- Discriminator detail for filtered outcomes (OutOfSchedule
    -- |TitleMismatch|CategoryMismatch). NULL otherwise.
    filter_reason       TEXT,
    -- Truncated transient-error message (≤ 512 chars at write time). NULL
    -- when `outcome != 'transient_error'`.
    error_message       TEXT,

    -- Stream-selection telemetry. `streams_extracted` is 0 for non-live
    -- outcomes and for filtered outcomes that short-circuit before
    -- extraction (e.g. OutOfSchedule).
    streams_extracted   INTEGER NOT NULL DEFAULT 0,
    -- JSON-encoded descriptor of the selected stream:
    --   { "quality": "...", "stream_format": "...", "media_format": "...",
    --     "bitrate": N, "codec": "...", "fps": F }
    -- NULL on non-live outcomes.
    stream_selected     TEXT,

    -- Snapshot of the live-side metadata for tooltip display. NULL on
    -- non-live outcomes.
    title               TEXT,
    category            TEXT,
    viewer_count        INTEGER,

    FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE
);

-- Primary read pattern: "most-recent N rows for this streamer". The DESC
-- order on `checked_at` matches the API's ordering and the writer's trim
-- query (DELETE … WHERE id NOT IN (… ORDER BY checked_at DESC LIMIT N)).
CREATE INDEX idx_streamer_check_history_streamer_id_checked_at
    ON streamer_check_history(streamer_id, checked_at DESC);
