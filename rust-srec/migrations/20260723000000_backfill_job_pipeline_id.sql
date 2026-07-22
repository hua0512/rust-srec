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
