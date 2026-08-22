-- Durable per-file upload results.
--
-- Rows are written only when an upload job reaches a terminal outcome
-- (JobQueue::complete / worker-pool failure synthesis); live upload state
-- stays in job_execution_progress and the upload WebSocket events.
--
-- job_id is ON DELETE SET NULL so records outlive job pruning; the
-- maintenance sweep bounds this table separately via
-- prune_upload_records_before using job_history_retention_days.
CREATE TABLE upload_records (
    id TEXT PRIMARY KEY,
    job_id TEXT REFERENCES job(id) ON DELETE SET NULL,
    streamer_id TEXT,
    session_id TEXT,
    -- Producing processor kind ('rclone' today); kept generic so future
    -- uploaders reuse this table without a schema change.
    uploader TEXT NOT NULL,
    local_path TEXT NOT NULL,
    remote_path TEXT,
    status TEXT NOT NULL CHECK (status IN ('COMPLETED', 'FAILED', 'SKIPPED')),
    size_bytes INTEGER,
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);

-- Retry idempotency: JobQueue::retry_job reuses the job id, so a retried
-- upload upserts over the failed rows instead of accumulating duplicates.
CREATE UNIQUE INDEX idx_upload_records_job_local ON upload_records (job_id, local_path);
CREATE INDEX idx_upload_records_streamer_created ON upload_records (streamer_id, created_at);
-- list_outputs annotates media outputs by joining on local_path.
CREATE INDEX idx_upload_records_local_path ON upload_records (local_path);
-- Retention sweep deletes oldest-first by created_at.
CREATE INDEX idx_upload_records_created_at ON upload_records (created_at);

-- The tdl/telegram processors were removed; presets pointing at them would
-- produce jobs no processor can claim.
DELETE FROM job_presets WHERE processor IN ('tdl', 'telegram');
