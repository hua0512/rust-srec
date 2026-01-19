-- Add optional per-segment metadata to DAG executions.
--
-- This supports recovering session/paired coordination context (segment index + source)
-- from the database if in-memory context is missing (e.g., restart).

ALTER TABLE dag_execution ADD COLUMN segment_index INTEGER;
ALTER TABLE dag_execution ADD COLUMN segment_source TEXT;

