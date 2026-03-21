PRAGMA foreign_keys=OFF;

CREATE TABLE job_new (
    id TEXT PRIMARY KEY,
    job_type TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('PENDING', 'PROCESSING', 'COMPLETED', 'FAILED', 'CANCELLED')),
    config TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    input TEXT,
    outputs TEXT,
    priority INTEGER NOT NULL DEFAULT 0 CHECK (priority >= 0),
    streamer_id TEXT,
    session_id TEXT,
    started_at INTEGER,
    completed_at INTEGER,
    error TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    pipeline_id TEXT,
    execution_info TEXT,
    duration_secs REAL,
    queue_wait_secs REAL,
    dag_step_execution_id TEXT REFERENCES dag_step_execution(id) ON DELETE SET NULL
);

INSERT INTO job_new (
    id, job_type, status, config, state, created_at, updated_at, input, outputs, priority,
    streamer_id, session_id, started_at, completed_at, error, retry_count, pipeline_id,
    execution_info, duration_secs, queue_wait_secs, dag_step_execution_id
)
SELECT
    id,
    job_type,
    CASE WHEN status = 'INTERRUPTED' THEN 'CANCELLED' ELSE status END,
    config,
    state,
    created_at,
    updated_at,
    input,
    outputs,
    priority,
    streamer_id,
    session_id,
    started_at,
    completed_at,
    error,
    retry_count,
    pipeline_id,
    execution_info,
    duration_secs,
    queue_wait_secs,
    dag_step_execution_id
FROM job;

DROP TABLE job;
ALTER TABLE job_new RENAME TO job;

CREATE INDEX idx_job_status_created_at ON job(status, created_at DESC);
CREATE INDEX idx_job_priority_created_at ON job(priority DESC, created_at DESC);
CREATE INDEX idx_job_updated_at ON job(updated_at);
CREATE INDEX idx_job_created_at ON job(created_at);
CREATE INDEX idx_job_streamer_id ON job(streamer_id);
CREATE INDEX idx_job_session_id ON job(session_id);
CREATE INDEX idx_job_started_at ON job(started_at);
CREATE INDEX idx_job_completed_at ON job(completed_at);
CREATE INDEX idx_job_pipeline_id ON job(pipeline_id);
CREATE INDEX idx_jobs_completed_at_status ON job(completed_at) WHERE status IN ('COMPLETED', 'FAILED', 'CANCELLED');
CREATE INDEX idx_job_dag_step ON job(dag_step_execution_id);
CREATE INDEX idx_job_pending_priority_created_at ON job(priority DESC, created_at DESC) WHERE status = 'PENDING';
CREATE INDEX idx_job_pending_type_priority_created_at ON job(job_type, priority DESC, created_at DESC) WHERE status = 'PENDING';

CREATE TABLE dag_execution_new (
    id TEXT PRIMARY KEY,
    dag_definition TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING', 'PROCESSING', 'COMPLETED', 'FAILED', 'CANCELLED')),
    streamer_id TEXT,
    session_id TEXT,
    segment_index INTEGER,
    segment_source TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    error TEXT,
    total_steps INTEGER NOT NULL,
    completed_steps INTEGER NOT NULL DEFAULT 0,
    failed_steps INTEGER NOT NULL DEFAULT 0
);

INSERT INTO dag_execution_new (
    id, dag_definition, status, streamer_id, session_id, segment_index, segment_source,
    created_at, updated_at, completed_at, error, total_steps, completed_steps, failed_steps
)
SELECT
    id,
    dag_definition,
    CASE WHEN status = 'INTERRUPTED' THEN 'CANCELLED' ELSE status END,
    streamer_id,
    session_id,
    segment_index,
    segment_source,
    created_at,
    updated_at,
    completed_at,
    error,
    total_steps,
    completed_steps,
    failed_steps
FROM dag_execution;

DROP TABLE dag_execution;
ALTER TABLE dag_execution_new RENAME TO dag_execution;

CREATE INDEX idx_dag_execution_status_created_at ON dag_execution(status, created_at DESC);
CREATE INDEX idx_dag_execution_session_created_at ON dag_execution(session_id, created_at DESC);
CREATE INDEX idx_dag_execution_streamer_created_at ON dag_execution(streamer_id, created_at DESC);

PRAGMA foreign_keys=ON;
