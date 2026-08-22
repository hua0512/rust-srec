-- Backfill job.pipeline_id for DAG step jobs persisted with a NULL pipeline_id.
-- delete_jobs_by_pipeline, cancel_jobs_by_pipeline, and the pipeline_id job
-- filters match on this column; rows without it survive DAG deletion and keep
-- inflating get_job_counts_by_status.
UPDATE job
SET pipeline_id = (
    SELECT s.dag_id
    FROM dag_step_execution s
    WHERE s.id = job.dag_step_execution_id
)
WHERE pipeline_id IS NULL
  AND dag_step_execution_id IS NOT NULL;

-- One-time cleanup of terminal rows the backfill cannot re-link: their DAG is
-- gone and the dag_step_execution_id FK's ON DELETE SET NULL erased the step
-- link, so they are unreachable through the DAG endpoints and only inflate
-- get_job_counts_by_status. Every insert path (DagScheduler::create_step_job,
-- JobQueue::split_job_for_single_input) stamps pipeline_id, so no new rows can
-- match. Execution logs and progress rows cascade.
DELETE FROM job
WHERE pipeline_id IS NULL
  AND dag_step_execution_id IS NULL
  AND status IN ('COMPLETED', 'FAILED', 'CANCELLED');
