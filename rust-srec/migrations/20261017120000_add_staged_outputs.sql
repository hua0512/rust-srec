-- Temporary files a pipeline attempt stages before it publishes an output.
-- The attempt records each path before the file exists and removes its rows
-- once the worker finalizes it, so a row that survives a restart names a file
-- that a crash, power loss or forced exit may have left behind. Startup marks
-- those rows interrupted, and only interrupted rows are shown to users.
-- Jobs can be pruned while a leftover is still listed, so job_id is not a
-- foreign key.
CREATE TABLE staged_outputs (
    id TEXT PRIMARY KEY NOT NULL,
    path TEXT NOT NULL UNIQUE,
    output_path TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    interrupted_at INTEGER
);
