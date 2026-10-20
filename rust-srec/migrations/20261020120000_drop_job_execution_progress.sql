-- Live job progress is kept in memory by the job queue and is no longer stored.
-- The triggers on `job` must go with the table: one that still deleted from it
-- would make every later job status change fail with "no such table".
DROP TRIGGER IF EXISTS trg_job_reset_clears_progress;
DROP TRIGGER IF EXISTS trg_job_terminal_clears_progress;
DROP TABLE IF EXISTS job_execution_progress;
