-- Add a runtime-tunable knob for the download-queue freshness threshold.
--
-- When a download is queued waiting for a concurrency slot and waits
-- longer than this many milliseconds, the pipeline re-checks the
-- streamer with the monitor service to refresh stream URLs and
-- headers before starting the engine. Below the threshold, the URLs
-- captured at the original live event are reused.
--
-- 60_000 ms (1 minute) preserves the previous compiled-in default and
-- the RUST_SREC_QUEUE_FRESHNESS_MS env override behaviour. Settings
-- below the cell value via UI now apply at runtime without restart.

ALTER TABLE global_config
    ADD COLUMN queue_freshness_threshold_ms INTEGER NOT NULL DEFAULT 60000;
