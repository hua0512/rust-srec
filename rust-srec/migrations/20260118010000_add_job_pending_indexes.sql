-- Add indexes to reduce latency in "claim next pending job" and other queue reads.
--
-- These indexes are intentionally partial to keep write amplification low while
-- optimizing the hot-path queries which filter on `status = 'PENDING'`.

CREATE INDEX IF NOT EXISTS idx_job_pending_priority_created_at
    ON job(priority DESC, created_at DESC)
    WHERE status = 'PENDING';

CREATE INDEX IF NOT EXISTS idx_job_pending_type_priority_created_at
    ON job(job_type, priority DESC, created_at DESC)
    WHERE status = 'PENDING';

