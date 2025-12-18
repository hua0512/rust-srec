-- DAG Pipeline Support Migration
-- Adds tables for Directed Acyclic Graph (DAG) pipeline execution
-- Supports fan-in (multiple inputs to one step) and fan-out (one step to multiple)

-- ============================================
-- DAG EXECUTION TABLE
-- ============================================

-- Tracks the overall state of a DAG pipeline execution
CREATE TABLE dag_execution (
    id TEXT PRIMARY KEY,
    -- JSON-serialized DAG pipeline definition (DagPipelineDefinition)
    dag_definition TEXT NOT NULL,
    -- Execution status: PENDING, PROCESSING, COMPLETED, FAILED, INTERRUPTED
    status TEXT NOT NULL DEFAULT 'PENDING',
    -- Associated streamer ID
    streamer_id TEXT,
    -- Associated session ID
    session_id TEXT,
    -- ISO 8601 timestamp when the DAG was created
    created_at TEXT NOT NULL,
    -- ISO 8601 timestamp when the DAG was last updated
    updated_at TEXT NOT NULL,
    -- ISO 8601 timestamp when the DAG completed (success or failure)
    completed_at TEXT,
    -- Error message if the DAG failed
    error TEXT,
    -- Total number of steps in the DAG
    total_steps INTEGER NOT NULL,
    -- Number of steps that have completed successfully
    completed_steps INTEGER NOT NULL DEFAULT 0,
    -- Number of steps that have failed
    failed_steps INTEGER NOT NULL DEFAULT 0
);

-- ============================================
-- DAG STEP EXECUTION TABLE
-- ============================================

-- Tracks individual step state within a DAG execution
CREATE TABLE dag_step_execution (
    id TEXT PRIMARY KEY,
    -- Parent DAG execution ID
    dag_id TEXT NOT NULL,
    -- Step ID within the DAG definition (e.g., "remux", "upload")
    step_id TEXT NOT NULL,
    -- Associated job ID (NULL until job is created)
    job_id TEXT,
    -- Step status: BLOCKED, PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED
    status TEXT NOT NULL DEFAULT 'BLOCKED',
    -- JSON array of step IDs this step depends on
    depends_on_step_ids TEXT NOT NULL DEFAULT '[]',
    -- JSON array of output paths produced by this step
    outputs TEXT,
    -- ISO 8601 timestamp when the step was created
    created_at TEXT NOT NULL,
    -- ISO 8601 timestamp when the step was last updated
    updated_at TEXT NOT NULL,
    -- Foreign key constraints
    FOREIGN KEY (dag_id) REFERENCES dag_execution(id) ON DELETE CASCADE,
    FOREIGN KEY (job_id) REFERENCES job(id) ON DELETE SET NULL,
    -- Each step_id must be unique within a DAG
    UNIQUE (dag_id, step_id)
);

-- ============================================
-- ADD DAG REFERENCE TO JOB TABLE
-- ============================================

-- Link jobs to their DAG step execution (if part of a DAG)
ALTER TABLE job ADD COLUMN dag_step_execution_id TEXT REFERENCES dag_step_execution(id) ON DELETE SET NULL;

-- ============================================
-- INDEXES
-- ============================================

-- DAG execution indexes
CREATE INDEX idx_dag_execution_status ON dag_execution(status);
CREATE INDEX idx_dag_execution_session ON dag_execution(session_id);
CREATE INDEX idx_dag_execution_streamer ON dag_execution(streamer_id);
CREATE INDEX idx_dag_execution_created_at ON dag_execution(created_at);

-- DAG step execution indexes
CREATE INDEX idx_dag_step_dag_id ON dag_step_execution(dag_id);
CREATE INDEX idx_dag_step_job_id ON dag_step_execution(job_id);
CREATE INDEX idx_dag_step_status ON dag_step_execution(status);
-- Index for finding blocked steps that might be ready
CREATE INDEX idx_dag_step_dag_status ON dag_step_execution(dag_id, status);

-- Job table index for DAG reference
CREATE INDEX idx_job_dag_step ON job(dag_step_execution_id);
