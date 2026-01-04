-- Clear job execution progress when a job is reset back to PENDING.
-- This avoids stale progress snapshots leaking across retries or recovery.

DROP TRIGGER IF EXISTS trg_job_reset_clears_progress;

CREATE TRIGGER trg_job_reset_clears_progress
AFTER UPDATE OF status ON job
WHEN NEW.status = 'PENDING' AND OLD.status != 'PENDING'
BEGIN
    DELETE FROM job_execution_progress WHERE job_id = NEW.id;
END;

