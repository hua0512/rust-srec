-- Default DAG and output pages order by creation time without a leading filter.
CREATE INDEX idx_dag_execution_created_at ON dag_execution(created_at DESC);
CREATE INDEX idx_media_outputs_created_at ON media_outputs(created_at DESC);

-- Duration statistics use the covering idx_job_status_duration. No repository
-- filters/orders jobs by started_at or completed_at; these increase write cost.
DROP INDEX idx_job_started_at;
DROP INDEX idx_job_completed_at;
DROP INDEX idx_jobs_completed_at_status;

-- Maintenance binds terminal statuses, so its predicate cannot imply this partial
-- index's literal IN clause. Keep idx_job_updated_at for its ordered cutoff scan.
DROP INDEX idx_job_terminal_updated_at;
