-- Add auto_thumbnail column to global_config table
-- Defaults to TRUE to preserve existing behavior (automatic thumbnail generation enabled)
ALTER TABLE global_config ADD COLUMN auto_thumbnail BOOLEAN NOT NULL DEFAULT TRUE;
