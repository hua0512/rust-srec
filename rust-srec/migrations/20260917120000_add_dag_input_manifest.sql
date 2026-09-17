-- Per-segment pairing of the video and danmu inputs of a paired-segment or
-- session-complete pipeline, stored with the DAG so every step job can read it
-- after a restart or retry. NULL for segment, thumbnail and API-created DAGs.
ALTER TABLE dag_execution ADD COLUMN input_manifest TEXT;
