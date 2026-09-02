-- Drop a job's live-progress snapshot when the job is put back into the queue.
-- job_execution_progress describes an attempt that is no longer running once
-- job.status returns to PENDING, so reset_processing_jobs (startup reclaim of
-- jobs left PROCESSING by an unclean stop) and reset_job_for_retry must not
-- leave the previous attempt's percentage readable until the next flush from
-- upsert_job_execution_progress.
--
-- Counterpart of trg_job_terminal_clears_progress, which clears the same row on
-- the way into COMPLETED/FAILED/CANCELLED. The two WHEN clauses are disjoint, so
-- a single status update never fires both.
CREATE TRIGGER IF NOT EXISTS trg_job_reset_clears_progress
AFTER UPDATE OF status ON job
WHEN NEW.status = 'PENDING' AND OLD.status != 'PENDING'
BEGIN
    DELETE FROM job_execution_progress WHERE job_id = NEW.id;
END;

-- One-time sweep of snapshots still attached to jobs that are queued right now.
DELETE FROM job_execution_progress
WHERE job_id IN (SELECT id FROM job WHERE status = 'PENDING');
