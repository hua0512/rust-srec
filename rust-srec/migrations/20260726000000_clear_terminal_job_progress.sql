-- Drop a job's live-progress snapshot the moment the job reaches a terminal
-- state. job_execution_progress rows only mean anything while the job is
-- PROCESSING (the API's progress endpoint is polled only then); a leftover
-- terminal row is dead data at best and misleading at worst (a FAILED job
-- frozen at "42%"). Counterpart of trg_job_reset_clears_progress, which
-- clears the row on the way back to PENDING.
--
-- A trigger (rather than per-call cleanup in JobQueue) covers every terminal
-- path with one definition: complete, fail, cancel, bulk pipeline cancels,
-- and DAG fail-fast cancellations. The progress aggregator's flush-time
-- liveness check prevents an in-flight snapshot from re-inserting the row
-- after this fires.
CREATE TRIGGER trg_job_terminal_clears_progress
AFTER UPDATE OF status ON job
WHEN NEW.status IN ('COMPLETED', 'FAILED', 'CANCELLED')
     AND OLD.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
BEGIN
    DELETE FROM job_execution_progress WHERE job_id = NEW.id;
END;

-- One-time sweep of rows that terminal jobs left behind before this trigger
-- existed.
DELETE FROM job_execution_progress
WHERE job_id IN (
    SELECT id FROM job WHERE status IN ('COMPLETED', 'FAILED', 'CANCELLED')
);
