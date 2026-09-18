-- A failed workflow step job whose step carries a retry policy waits here for
-- its next attempt instead of failing the workflow: milliseconds since the
-- epoch at which the retry sweeper may re-queue it, NULL when none is pending.
ALTER TABLE job ADD COLUMN retry_after INTEGER;

-- The sweeper polls for due retries; keep that scan to the few rows waiting.
CREATE INDEX IF NOT EXISTS idx_job_retry_after ON job(retry_after) WHERE retry_after IS NOT NULL;
