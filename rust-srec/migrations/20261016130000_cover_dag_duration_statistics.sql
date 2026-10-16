-- Dashboard statistics average completed DAG durations on every poll. Carrying
-- completed_at in the status index lets that aggregate read the index alone;
-- status-filtered lists keep the same leading (status, created_at) order.
CREATE INDEX idx_dag_execution_status_created_completed
    ON dag_execution(status, created_at DESC, completed_at);
DROP INDEX idx_dag_execution_status_created_at;
