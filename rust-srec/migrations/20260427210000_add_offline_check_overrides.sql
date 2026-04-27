-- Add per-platform / per-template overrides for offline-check tunables.
--
-- Today `offline_check_count` and `offline_check_delay_ms` live only on
-- `global_config`. This migration extends the 4-layer config hierarchy so
-- platforms and templates can override them. NULL means "inherit from parent
-- layer" (template → platform → global). Streamer-level overrides continue
-- to live in the existing `streamer_specific_config` JSON blob — no schema
-- change for that layer.

ALTER TABLE platform_config ADD COLUMN offline_check_count INTEGER;
ALTER TABLE platform_config ADD COLUMN offline_check_delay_ms BIGINT;

ALTER TABLE template_config ADD COLUMN offline_check_count INTEGER;
ALTER TABLE template_config ADD COLUMN offline_check_delay_ms BIGINT;
