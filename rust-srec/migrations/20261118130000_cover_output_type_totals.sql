-- The Outputs totals count rows and sum `size_bytes` per `file_type`. With the
-- size stored in the index, SQLite answers from the index alone instead of
-- fetching every row for its size.
DROP INDEX idx_media_output_file_type;

CREATE INDEX idx_media_output_file_type ON media_outputs(file_type, size_bytes);
