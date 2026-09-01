-- SqlxJobRepository::claim_next_pending_job selects the next PENDING job with
-- ORDER BY priority DESC, created_at ASC, id ASC. The previous partial indexes
-- sorted created_at DESC, so the planner had to sort the whole pending set on
-- every claim. Match the claim order exactly, with id as the final tiebreak,
-- so the claim is an index walk to the first row.
DROP INDEX IF EXISTS idx_job_pending_priority_created_at;
DROP INDEX IF EXISTS idx_job_pending_type_priority_created_at;

CREATE INDEX idx_job_pending_priority_created_at
    ON job(priority DESC, created_at ASC, id ASC)
    WHERE status = 'PENDING';

CREATE INDEX idx_job_pending_type_priority_created_at
    ON job(job_type, priority DESC, created_at ASC, id ASC)
    WHERE status = 'PENDING';
