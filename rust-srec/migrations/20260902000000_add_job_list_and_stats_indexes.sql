-- SqlxJobRepository::list_jobs_filtered orders every page by
-- priority DESC, created_at DESC. With a status filter the only usable index
-- was idx_job_status_created_at, so the planner sorted the whole status
-- partition (most of the table once jobs are COMPLETED) for each page.
CREATE INDEX idx_job_status_priority_created_at
    ON job(status, priority DESC, created_at DESC);

-- SqlxJobRepository::get_avg_processing_time reads duration_secs,
-- started_at and completed_at for every COMPLETED job. Covering those columns
-- keeps the aggregate inside the index instead of fetching each row; the
-- query is polled with the pipeline stats.
CREATE INDEX idx_job_status_duration
    ON job(status, duration_secs, started_at, completed_at);

-- SqlxSessionRepository::list_sessions_filtered orders by start_time DESC.
-- Without a streamer filter no index matched that order, so each page sorted
-- all sessions.
CREATE INDEX idx_live_sessions_start_time
    ON live_sessions(start_time DESC);
