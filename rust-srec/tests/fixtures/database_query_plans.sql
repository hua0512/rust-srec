-- Enough rows and varied predicates for ANALYZE to exercise real list/cleanup choices.
WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM n WHERE value < 5000)
INSERT INTO dag_execution(id, dag_definition, status, created_at, updated_at, total_steps)
SELECT 'plan-dag-' || value, '{}', CASE WHEN value % 10 = 0 THEN 'PENDING' ELSE 'COMPLETED' END,
       value * 1000, value * 1000, 0 FROM n;

INSERT INTO live_sessions(id, start_time) VALUES('plan-session', 0);
WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM n WHERE value < 5000)
INSERT INTO media_outputs(id, session_id, file_path, file_type, size_bytes, created_at)
SELECT 'plan-media-' || value, 'plan-session', 'output-' || value || '.flv',
       'VIDEO', 1024, value * 1000 FROM n;

WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM n WHERE value < 5000)
INSERT INTO job(id, job_type, status, config, state, created_at, updated_at,
                priority, started_at, completed_at, duration_secs)
SELECT 'plan-job-' || value, 'REMUX',
       CASE value % 10 WHEN 0 THEN 'PENDING' WHEN 1 THEN 'FAILED' WHEN 2 THEN 'CANCELLED' ELSE 'COMPLETED' END,
       '{}', '{}', value * 1000, value * 1000, value % 4,
       value * 1000 - 1000, value * 1000, 1.0 FROM n;
