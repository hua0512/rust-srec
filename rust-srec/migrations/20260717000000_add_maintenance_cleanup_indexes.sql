CREATE INDEX idx_job_terminal_updated_at
    ON job(updated_at)
    WHERE status IN ('COMPLETED', 'FAILED', 'CANCELLED');

CREATE INDEX idx_dag_execution_terminal_updated_at
    ON dag_execution(updated_at)
    WHERE status IN ('COMPLETED', 'FAILED', 'CANCELLED');

CREATE INDEX idx_live_sessions_empty_ended
    ON live_sessions(end_time)
    WHERE total_size_bytes = 0 AND end_time IS NOT NULL;
