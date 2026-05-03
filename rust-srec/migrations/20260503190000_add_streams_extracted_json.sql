-- Adds the `streams_extracted_json` column to `streamer_check_history`,
-- carrying the full list of candidate descriptors the platform extractor
-- returned for a live observation (before selection narrowed the list to
-- one). The check-history strip's tooltip surfaces this so operators can
-- see all available qualities/formats at a glance, with the selected one
-- marked.
--
-- Schema shape (when present):
--   [
--     { "quality": "best",  "stream_format": "Flv", "media_format": "Flv",
--       "bitrate": 5000000, "codec": "h264", "fps": 30.0 },
--     { "quality": "high",  ... },
--     ...
--   ]
--
-- NULL when:
--   - outcome != 'live' (no candidates to record)
--   - the row predates this migration (back-compat — existing rows stay
--     valid; the tooltip just won't render the candidate list for them)
--
-- The existing `streams_extracted INTEGER` count column stays — it lets
-- callers cheaply filter/aggregate without parsing JSON, and matches what
-- live-update consumers already see on the WebSocket.

ALTER TABLE streamer_check_history
    ADD COLUMN streams_extracted_json TEXT;
