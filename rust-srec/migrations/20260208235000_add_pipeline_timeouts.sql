-- Add global-config knobs for pipeline job timeouts.
--
-- These timeouts are consumed by the pipeline worker pools and the execute processor.
-- NOTE: Changing these values at runtime currently requires a service restart.

ALTER TABLE global_config
    ADD COLUMN pipeline_cpu_job_timeout_secs INTEGER NOT NULL DEFAULT 3600;

ALTER TABLE global_config
    ADD COLUMN pipeline_io_job_timeout_secs INTEGER NOT NULL DEFAULT 3600;

ALTER TABLE global_config
    ADD COLUMN pipeline_execute_timeout_secs INTEGER NOT NULL DEFAULT 3600;
