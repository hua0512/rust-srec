-- Output retention is opt-in and independent of diagnostic history retention.
ALTER TABLE global_config ADD COLUMN output_retention_days INTEGER NOT NULL DEFAULT 0
    CHECK (output_retention_days >= 0 AND output_retention_days <= 2147483647);
ALTER TABLE global_config ADD COLUMN output_retention_delete_files INTEGER NOT NULL DEFAULT 0
    CHECK (output_retention_delete_files IN (0, 1));

-- Every file deletion checks whether another output still owns the same path.
CREATE INDEX idx_media_outputs_file_path ON media_outputs(file_path);
