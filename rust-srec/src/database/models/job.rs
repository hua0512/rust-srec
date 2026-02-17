//! Job database models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::json::{self, JsonContext};

/// Filter criteria for querying jobs.
#[derive(Debug, Clone, Default)]
pub struct JobFilters {
    /// Filter by job status.
    pub status: Option<JobStatus>,
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by pipeline ID.
    pub pipeline_id: Option<String>,
    /// Filter jobs created after this date.
    pub from_date: Option<DateTime<Utc>>,
    /// Filter jobs created before this date.
    pub to_date: Option<DateTime<Utc>>,
    /// Filter by job type.
    pub job_type: Option<String>,
    /// Filter by multiple job types.
    pub job_types: Option<Vec<String>>,
    /// Search query.
    pub search: Option<String>,
}

impl JobFilters {
    /// Create a new empty filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by status.
    pub fn with_status(mut self, status: JobStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Filter by streamer ID.
    pub fn with_streamer_id(mut self, streamer_id: impl Into<String>) -> Self {
        self.streamer_id = Some(streamer_id.into());
        self
    }

    /// Filter by session ID.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Filter by pipeline ID.
    pub fn with_pipeline_id(mut self, pipeline_id: impl Into<String>) -> Self {
        self.pipeline_id = Some(pipeline_id.into());
        self
    }

    /// Filter by date range.
    pub fn with_date_range(
        mut self,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Self {
        self.from_date = from;
        self.to_date = to;
        self
    }

    /// Filter by job type.
    pub fn with_job_type(mut self, job_type: impl Into<String>) -> Self {
        self.job_type = Some(job_type.into());
        self
    }

    /// Filter by multiple job types.
    pub fn with_job_types(mut self, job_types: Vec<String>) -> Self {
        self.job_types = Some(job_types);
        self
    }

    /// Filter by search query.
    pub fn with_search(mut self, search: impl Into<String>) -> Self {
        self.search = Some(search.into());
        self
    }
}

/// Pagination parameters for list queries.
#[derive(Debug, Clone)]
pub struct Pagination {
    /// Maximum number of items to return.
    pub limit: u32,
    /// Number of items to skip.
    pub offset: u32,
}

impl Pagination {
    /// Create new pagination parameters.
    pub fn new(limit: u32, offset: u32) -> Self {
        Self { limit, offset }
    }
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
        }
    }
}

/// Job counts by status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobCounts {
    /// Number of pending jobs.
    pub pending: u64,
    /// Number of processing jobs.
    pub processing: u64,
    /// Number of completed jobs.
    pub completed: u64,
    /// Number of failed jobs.
    pub failed: u64,
    /// Number of interrupted jobs.
    pub interrupted: u64,
}

impl JobCounts {
    /// Get total count of all jobs.
    pub fn total(&self) -> u64 {
        self.pending + self.processing + self.completed + self.failed + self.interrupted
    }
}

/// Job database model.
/// Represents a single asynchronous task (download or pipeline process).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct JobDbModel {
    pub id: String,
    /// Job type.
    ///
    /// This field is a free-form string used to route work to a processor and to filter/list jobs.
    /// In practice, it is usually:
    /// - A processor name for inline steps (e.g. `remux`, `upload`, `thumbnail`)
    /// - A preset name for preset-driven DAG steps (e.g. `thumbnail_native`, `thumbnail_hd`)
    pub job_type: String,
    /// Status: PENDING, PROCESSING, COMPLETED, FAILED, INTERRUPTED
    pub status: String,
    /// JSON blob for job-specific configuration
    pub config: String,
    /// JSON blob for dynamic job state
    pub state: String,
    /// Unix epoch milliseconds (UTC) when the job was created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when the job was last updated.
    pub updated_at: i64,
    // Pipeline-specific fields
    /// Input path or source for the job (single input file)
    pub input: Option<String>,
    /// Output paths produced by the job (JSON array, can be 0, 1, or more outputs)
    pub outputs: Option<String>,
    /// Job priority (higher values = higher priority)
    pub priority: i32,
    /// Associated streamer ID
    pub streamer_id: Option<String>,
    /// Associated session ID
    pub session_id: Option<String>,
    /// Unix epoch milliseconds (UTC) when the job started processing.
    pub started_at: Option<i64>,
    /// Unix epoch milliseconds (UTC) when the job completed.
    pub completed_at: Option<i64>,
    /// Error message if the job failed
    pub error: Option<String>,
    /// Number of retry attempts
    pub retry_count: i32,
    /// Pipeline ID to group related jobs (first job's ID)
    pub pipeline_id: Option<String>,
    /// Execution information for observability (JSON)
    pub execution_info: Option<String>,
    /// Processing duration in seconds (from processor output)
    pub duration_secs: Option<f64>,
    /// Time spent waiting in queue before processing started (seconds)
    pub queue_wait_secs: Option<f64>,
    /// DAG step execution ID (if this job is part of a DAG pipeline)
    pub dag_step_execution_id: Option<String>,
}

impl JobDbModel {
    pub fn new(job_type: impl Into<String>, config: impl Into<String>) -> Self {
        let now = crate::database::time::now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.into(),
            status: JobStatus::Pending.as_str().to_string(),
            config: config.into(),
            state: "{}".to_string(),
            created_at: now,
            updated_at: now,
            input: None,
            outputs: None,
            priority: 0,
            streamer_id: None,
            session_id: None,
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,

            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
        }
    }

    /// Create a new job that has an input path.
    pub fn new_with_input(
        job_type: impl Into<String>,
        input: impl Into<String>,
        priority: i32,
        streamer_id: Option<String>,
        session_id: Option<String>,
        config: impl Into<String>,
    ) -> Self {
        let now = crate::database::time::now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.into(),
            status: JobStatus::Pending.as_str().to_string(),
            config: config.into(),
            state: "{}".to_string(),
            created_at: now,
            updated_at: now,
            input: Some(input.into()),
            outputs: None, // Outputs are populated during/after job execution
            priority,
            streamer_id,
            session_id,
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,

            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
        }
    }

    /// Create a new pipeline step job with pipeline ID.
    /// This is used for DAG pipeline job creation.
    ///
    /// # Arguments
    ///
    /// * `input` - Input paths as JSON array string
    /// * `output` - Output paths as JSON array string (e.g., "[]" for empty)
    pub fn new_pipeline_step(
        job_type: impl Into<String>,
        input: impl Into<String>,
        output: impl Into<String>,
        priority: i32,
        streamer_id: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        let now = crate::database::time::now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        let output_str = output.into();
        Self {
            id: id.clone(),
            job_type: job_type.into(),
            status: JobStatus::Pending.as_str().to_string(),
            config: "{}".to_string(),
            state: "{}".to_string(),
            created_at: now,
            updated_at: now,
            input: Some(input.into()),
            outputs: Some(output_str),
            priority,
            streamer_id,
            session_id,
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
        }
    }

    /// Get the list of output paths produced by this job.
    pub fn get_outputs(&self) -> Vec<String> {
        let Some(raw) = self.outputs.as_deref() else {
            return Vec::new();
        };

        json::parse_or_default(
            raw,
            JsonContext::JobField {
                job_id: &self.id,
                field: "outputs",
            },
            "Invalid job outputs JSON; treating as empty",
        )
    }

    /// Set the output paths for this job.
    pub fn set_outputs(&mut self, outputs: &[String]) {
        self.outputs = Some(json::to_string_or_fallback(
            outputs,
            "[]",
            JsonContext::JobField {
                job_id: &self.id,
                field: "outputs",
            },
            "Failed to serialize job outputs; storing empty list",
        ));
        self.updated_at = crate::database::time::now_ms();
    }

    /// Add an output path to this job.
    pub fn add_output(&mut self, output: impl Into<String>) {
        let mut outputs = self.get_outputs();
        outputs.push(output.into());
        self.set_outputs(&outputs);
    }

    /// Mark the job as started processing.
    pub fn mark_started(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = JobStatus::Processing.as_str().to_string();
        self.started_at = Some(now);
        self.updated_at = now;
    }

    /// Mark the job as completed.
    pub fn mark_completed(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = JobStatus::Completed.as_str().to_string();
        self.completed_at = Some(now);
        self.updated_at = now;
    }

    /// Mark the job as failed with an error message.
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        let now = crate::database::time::now_ms();
        self.status = JobStatus::Failed.as_str().to_string();
        self.completed_at = Some(now);
        self.error = Some(error.into());
        self.updated_at = now;
    }

    /// Reset the job for retry.
    pub fn reset_for_retry(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = JobStatus::Pending.as_str().to_string();
        self.started_at = None;
        self.completed_at = None;
        self.error = None;
        self.retry_count += 1;
        self.updated_at = now;
    }

    /// Get the job status as an enum.
    pub fn get_status(&self) -> Option<JobStatus> {
        JobStatus::parse(&self.status)
    }
}

/// Job status values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    /// Job is queued and waiting to be picked up by a worker.
    Pending,
    /// Job is currently being executed.
    Processing,
    /// Job finished successfully.
    Completed,
    /// Job failed after exhausting retries.
    Failed,
    /// Job was interrupted by shutdown; will be reset to PENDING on restart.
    Interrupted,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
            Self::Interrupted => "INTERRUPTED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(Self::Pending),
            "PROCESSING" => Some(Self::Processing),
            "COMPLETED" => Some(Self::Completed),
            "FAILED" => Some(Self::Failed),
            "INTERRUPTED" => Some(Self::Interrupted),
            _ => None,
        }
    }

    /// Check if this is a terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Job execution log database model.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct JobExecutionLogDbModel {
    pub id: String,
    pub job_id: String,
    /// JSON blob for the log entry
    pub entry: String,
    /// Unix epoch milliseconds (UTC).
    pub created_at: i64,
    /// Optional structured level (e.g. "INFO", "WARN", "ERROR").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Optional structured message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl JobExecutionLogDbModel {
    pub fn new(job_id: impl Into<String>, entry: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.into(),
            entry: entry.into(),
            created_at: crate::database::time::now_ms(),
            level: None,
            message: None,
        }
    }
}

/// Job execution progress snapshot (latest only) database model.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct JobExecutionProgressDbModel {
    pub job_id: String,
    /// Progress kind (e.g. "ffmpeg", "rclone").
    pub kind: String,
    /// JSON blob for the progress snapshot.
    pub progress: String,
    /// Unix epoch milliseconds (UTC).
    pub updated_at: i64,
}

/// Log entry structure for job execution logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl LogEntry {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            level: "INFO".to_string(),
            message: message.into(),
            details: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            level: "ERROR".to_string(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// Pipeline job configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineJobConfig {
    pub steps: Vec<PipelineStep>,
}

/// Pipeline step configuration.
/// Uses internally tagged enum to disambiguate step types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PipelineStep {
    /// Reference to a named job preset (e.g., "remux").
    Preset {
        /// Name of the job preset to use.
        name: String,
    },
    /// Reference to a pipeline workflow (expands to multiple steps).
    Workflow {
        /// Name of the pipeline preset/workflow to expand.
        name: String,
    },
    /// Inline definition of a job step.
    Inline {
        /// Processor type (e.g., "remux", "execute").
        processor: String,
        /// Optional configuration for the processor.
        #[serde(default)]
        config: serde_json::Value,
    },
}

impl PipelineStep {
    /// Get the preset name if this is a Preset variant.
    pub fn as_preset(&self) -> Option<&str> {
        match self {
            Self::Preset { name } => Some(name),
            _ => None,
        }
    }

    /// Get the workflow name if this is a Workflow variant.
    pub fn as_workflow(&self) -> Option<&str> {
        match self {
            Self::Workflow { name } => Some(name),
            _ => None,
        }
    }

    /// Get the processor and config if this is an Inline variant.
    pub fn as_inline(&self) -> Option<(&str, &serde_json::Value)> {
        match self {
            Self::Inline { processor, config } => Some((processor, config)),
            _ => None,
        }
    }

    /// Create a new Preset step.
    pub fn preset(name: impl Into<String>) -> Self {
        Self::Preset { name: name.into() }
    }

    /// Create a new Workflow step.
    pub fn workflow(name: impl Into<String>) -> Self {
        Self::Workflow { name: name.into() }
    }

    /// Create a new Inline step.
    pub fn inline(processor: impl Into<String>, config: serde_json::Value) -> Self {
        Self::Inline {
            processor: processor.into(),
            config,
        }
    }
}

/// Pipeline definition with ordered steps.
/// Used to define the sequence of jobs in a pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDefinition {
    /// Ordered list of job types to execute.
    pub steps: Vec<PipelineStep>,
}

impl PipelineDefinition {
    /// Create a new pipeline definition with the given steps.
    pub fn new(steps: Vec<PipelineStep>) -> Self {
        Self { steps }
    }

    /// Create the default pipeline definition: empty (no steps).
    /// Pipeline steps should be explicitly provided or configured per-streamer.
    pub fn default_pipeline() -> Self {
        Self { steps: vec![] }
    }

    /// Get the first step in the pipeline.
    pub fn first_step(&self) -> Option<&PipelineStep> {
        self.steps.first()
    }

    /// Check if the pipeline is empty.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get the number of steps in the pipeline.
    pub fn len(&self) -> usize {
        self.steps.len()
    }
}

impl Default for PipelineDefinition {
    fn default() -> Self {
        Self::default_pipeline()
    }
}

// ============================================================================
// DAG Pipeline Support
// ============================================================================

/// A step within a DAG pipeline with explicit dependencies.
/// Each step has a unique ID and can depend on other steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagStep {
    /// Unique step identifier within the DAG (e.g., "remux", "upload", "notify").
    pub id: String,
    /// The processor or preset to run for this step.
    pub step: PipelineStep,
    /// IDs of steps this depends on (fan-in: waits for all to complete).
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl DagStep {
    /// Create a new DAG step.
    pub fn new(id: impl Into<String>, step: PipelineStep) -> Self {
        Self {
            id: id.into(),
            step,
            depends_on: Vec::new(),
        }
    }

    /// Create a new DAG step with dependencies.
    pub fn with_dependencies(
        id: impl Into<String>,
        step: PipelineStep,
        depends_on: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            step,
            depends_on,
        }
    }

    /// Check if this step has no dependencies (root step).
    pub fn is_root(&self) -> bool {
        self.depends_on.is_empty()
    }
}

/// DAG pipeline definition with named steps and explicit dependencies.
/// Supports fan-in (multiple inputs to one step) and fan-out (one step to multiple).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagPipelineDefinition {
    /// Unique name for the DAG pipeline.
    pub name: String,
    /// All steps in the DAG (order doesn't matter - topology is defined by depends_on).
    pub steps: Vec<DagStep>,
}

impl DagPipelineDefinition {
    /// Create a new DAG pipeline definition.
    pub fn new(name: impl Into<String>, steps: Vec<DagStep>) -> Self {
        Self {
            name: name.into(),
            steps,
        }
    }

    /// Validate the DAG structure.
    /// Returns an error if:
    /// - There are cycles in the dependency graph
    /// - A step references a non-existent dependency
    /// - There are no root steps (steps with no dependencies)
    /// - Step IDs are not unique
    pub fn validate(&self) -> crate::Result<()> {
        use std::collections::{HashMap, HashSet};

        // Check for empty DAG
        if self.steps.is_empty() {
            return Err(crate::Error::validation(
                "DAG pipeline must have at least one step",
            ));
        }

        // Check for unique step IDs
        let mut seen_ids = HashSet::new();
        for step in &self.steps {
            if !seen_ids.insert(&step.id) {
                return Err(crate::Error::validation(format!(
                    "Duplicate step ID: {}",
                    step.id
                )));
            }
        }

        // Check that all dependencies reference existing steps
        let step_ids: HashSet<&str> = self.steps.iter().map(|s| s.id.as_str()).collect();
        for step in &self.steps {
            for dep in &step.depends_on {
                if !step_ids.contains(dep.as_str()) {
                    return Err(crate::Error::validation(format!(
                        "Step '{}' depends on non-existent step '{}'",
                        step.id, dep
                    )));
                }
            }
        }

        // Check for at least one root step
        if !self.steps.iter().any(|s| s.is_root()) {
            return Err(crate::Error::validation(
                "DAG pipeline must have at least one root step (no dependencies)",
            ));
        }

        // Check for cycles using DFS
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let adj: HashMap<&str, Vec<&str>> = self
            .steps
            .iter()
            .map(|s| {
                (
                    s.id.as_str(),
                    s.depends_on.iter().map(|d| d.as_str()).collect(),
                )
            })
            .collect();

        fn has_cycle<'a>(
            node: &'a str,
            adj: &HashMap<&'a str, Vec<&'a str>>,
            visited: &mut HashSet<&'a str>,
            rec_stack: &mut HashSet<&'a str>,
        ) -> bool {
            visited.insert(node);
            rec_stack.insert(node);

            if let Some(deps) = adj.get(node) {
                for dep in deps {
                    if (!visited.contains(dep) && has_cycle(dep, adj, visited, rec_stack))
                        || rec_stack.contains(dep)
                    {
                        return true;
                    }
                }
            }

            rec_stack.remove(node);
            false
        }

        for step in &self.steps {
            if !visited.contains(step.id.as_str())
                && has_cycle(step.id.as_str(), &adj, &mut visited, &mut rec_stack)
            {
                return Err(crate::Error::validation("DAG pipeline contains a cycle"));
            }
        }

        Ok(())
    }

    /// Get steps with no dependencies (entry points).
    pub fn root_steps(&self) -> Vec<&DagStep> {
        self.steps.iter().filter(|s| s.is_root()).collect()
    }

    /// Get steps that are not depended on by any other step (exit points).
    pub fn leaf_steps(&self) -> Vec<&DagStep> {
        use std::collections::HashSet;

        let depended_on: HashSet<&str> = self
            .steps
            .iter()
            .flat_map(|s| s.depends_on.iter().map(|d| d.as_str()))
            .collect();

        self.steps
            .iter()
            .filter(|s| !depended_on.contains(s.id.as_str()))
            .collect()
    }

    /// Get a topological ordering of the steps (dependencies before dependents).
    /// Returns an error if the graph contains a cycle.
    pub fn topological_order(&self) -> crate::Result<Vec<&DagStep>> {
        use std::collections::{HashMap, HashSet, VecDeque};

        let step_map: HashMap<&str, &DagStep> =
            self.steps.iter().map(|s| (s.id.as_str(), s)).collect();

        // Calculate in-degrees
        let mut in_degree: HashMap<&str, usize> =
            self.steps.iter().map(|s| (s.id.as_str(), 0)).collect();

        for step in &self.steps {
            for dep in &step.depends_on {
                // dep -> step edge means step has one more incoming edge
                *in_degree.entry(step.id.as_str()).or_insert(0) += 1;
                // Ensure dep is in the map (it should be)
                in_degree.entry(dep.as_str()).or_insert(0);
            }
        }

        // Start with nodes that have no incoming edges (root steps)
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut result = Vec::new();
        let mut processed = HashSet::new();

        while let Some(node) = queue.pop_front() {
            if let Some(step) = step_map.get(node) {
                result.push(*step);
                processed.insert(node);

                // Find all steps that depend on this node
                for step in &self.steps {
                    if step.depends_on.iter().any(|d| d == node)
                        && !processed.contains(step.id.as_str())
                        && let Some(deg) = in_degree.get_mut(step.id.as_str())
                        && *deg > 0
                    {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(step.id.as_str());
                        }
                    }
                }
            }
        }

        if result.len() != self.steps.len() {
            return Err(crate::Error::validation("DAG pipeline contains a cycle"));
        }

        Ok(result)
    }

    /// Get a step by its ID.
    pub fn get_step(&self, id: &str) -> Option<&DagStep> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// Get the number of steps in the DAG.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Check if the DAG is empty.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Status of a DAG step execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DagStepStatus {
    /// Step is waiting for dependencies to complete.
    Blocked,
    /// All dependencies complete, job created and queued.
    Pending,
    /// Job is currently being executed.
    Processing,
    /// Job finished successfully.
    Completed,
    /// Job failed.
    Failed,
    /// Step was cancelled due to fail-fast or user intervention.
    Cancelled,
}

impl DagStepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Blocked => "BLOCKED",
            Self::Pending => "PENDING",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "BLOCKED" => Some(Self::Blocked),
            "PENDING" => Some(Self::Pending),
            "PROCESSING" => Some(Self::Processing),
            "COMPLETED" => Some(Self::Completed),
            "FAILED" => Some(Self::Failed),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Check if this is a terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl std::fmt::Display for DagStepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Status of a DAG execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DagExecutionStatus {
    /// DAG is pending (not yet started).
    Pending,
    /// DAG is currently being executed.
    Processing,
    /// All steps completed successfully.
    Completed,
    /// At least one step failed.
    Failed,
    /// DAG was interrupted by shutdown.
    Interrupted,
}

impl DagExecutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Processing => "PROCESSING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
            Self::Interrupted => "INTERRUPTED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(Self::Pending),
            "PROCESSING" => Some(Self::Processing),
            "COMPLETED" => Some(Self::Completed),
            "FAILED" => Some(Self::Failed),
            "INTERRUPTED" => Some(Self::Interrupted),
            _ => None,
        }
    }

    /// Check if this is a terminal status.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

impl std::fmt::Display for DagExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Pipeline job state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineJobState {
    #[serde(default)]
    pub current_step_index: usize,
    #[serde(default)]
    pub items_produced: Vec<String>,
    #[serde(default)]
    pub output_files: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_new() {
        let job = JobDbModel::new("remux", r#"{"steps":[]}"#);
        assert_eq!(job.status, "PENDING");
        assert_eq!(job.job_type, "remux");
        assert_eq!(job.priority, 0);
        assert_eq!(job.retry_count, 0);
        assert!(job.input.is_none());
        assert!(job.outputs.is_none());
        assert!(job.get_outputs().is_empty());
        assert!(job.streamer_id.is_none());
        assert!(job.session_id.is_none());
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
        assert!(job.error.is_none());
    }

    #[test]
    fn test_job_new_pipeline() {
        let job = JobDbModel::new_with_input(
            "remux",
            "/input/file.flv",
            10,
            Some("streamer-123".to_string()),
            Some("session-456".to_string()),
            r#"{"steps":[]}"#,
        );
        assert_eq!(job.status, "PENDING");
        assert_eq!(job.job_type, "remux");
        assert_eq!(job.input, Some("/input/file.flv".to_string()));
        assert!(job.outputs.is_none()); // Outputs start empty
        assert!(job.get_outputs().is_empty());
        assert_eq!(job.priority, 10);
        assert_eq!(job.streamer_id, Some("streamer-123".to_string()));
        assert_eq!(job.session_id, Some("session-456".to_string()));
        assert_eq!(job.retry_count, 0);
    }

    #[test]
    fn test_job_outputs() {
        let mut job = JobDbModel::new_with_input(
            "remux",
            "/input/file.flv",
            5,
            None,
            None,
            r#"{"steps":[]}"#,
        );

        // Initially no outputs
        assert!(job.get_outputs().is_empty());

        // Add single output
        job.add_output("/output/file1.mp4");
        assert_eq!(job.get_outputs(), vec!["/output/file1.mp4".to_string()]);

        // Add more outputs
        job.add_output("/output/file2.mp4");
        job.add_output("/output/thumbnail.jpg");
        assert_eq!(
            job.get_outputs(),
            vec![
                "/output/file1.mp4".to_string(),
                "/output/file2.mp4".to_string(),
                "/output/thumbnail.jpg".to_string(),
            ]
        );

        // Set outputs directly
        job.set_outputs(&["/new/output.mp4".to_string()]);
        assert_eq!(job.get_outputs(), vec!["/new/output.mp4".to_string()]);

        // Set empty outputs
        job.set_outputs(&[]);
        assert!(job.get_outputs().is_empty());
    }

    #[test]
    fn test_job_lifecycle_methods() {
        let mut job = JobDbModel::new("remux", r#"{"steps":[]}"#);

        // Test mark_started
        job.mark_started();
        assert_eq!(job.status, "PROCESSING");
        assert!(job.started_at.is_some());

        // Test mark_completed
        job.mark_completed();
        assert_eq!(job.status, "COMPLETED");
        assert!(job.completed_at.is_some());
    }

    #[test]
    fn test_job_mark_failed() {
        let mut job = JobDbModel::new("remux", r#"{"steps":[]}"#);
        job.mark_started();
        job.mark_failed("Something went wrong");

        assert_eq!(job.status, "FAILED");
        assert!(job.completed_at.is_some());
        assert_eq!(job.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_job_reset_for_retry() {
        let mut job = JobDbModel::new("remux", r#"{"steps":[]}"#);
        job.mark_started();
        job.mark_failed("Error");

        let original_retry_count = job.retry_count;
        job.reset_for_retry();

        assert_eq!(job.status, "PENDING");
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
        assert!(job.error.is_none());
        assert_eq!(job.retry_count, original_retry_count + 1);
    }

    #[test]
    fn test_job_status_terminal() {
        assert!(JobStatus::Completed.is_terminal());
        assert!(JobStatus::Failed.is_terminal());
        assert!(!JobStatus::Pending.is_terminal());
        assert!(!JobStatus::Processing.is_terminal());
        assert!(!JobStatus::Interrupted.is_terminal());
    }

    #[test]
    fn test_log_entry() {
        let entry =
            LogEntry::info("Test message").with_details(serde_json::json!({"key": "value"}));
        assert_eq!(entry.level, "INFO");
        assert!(entry.details.is_some());
    }

    #[test]
    fn test_get_status_and_type() {
        let job = JobDbModel::new("remux", r#"{"steps":[]}"#);
        assert_eq!(job.get_status(), Some(JobStatus::Pending));
        assert_eq!(job.job_type, "remux");
    }

    #[test]
    fn test_pipeline_step_json_format() {
        use serde_json::json;

        // 1. Tagged Preset
        let json = json!({
            "type": "preset",
            "name": "fast-preset"
        });
        let step: PipelineStep = serde_json::from_value(json).unwrap();
        assert_eq!(
            step,
            PipelineStep::Preset {
                name: "fast-preset".to_string()
            }
        );

        // 2. Tagged Workflow
        let json = json!({
            "type": "workflow",
            "name": "full-pipeline"
        });
        let step: PipelineStep = serde_json::from_value(json).unwrap();
        assert_eq!(
            step,
            PipelineStep::Workflow {
                name: "full-pipeline".to_string()
            }
        );

        // 3. Tagged Inline
        let json = json!({
            "type": "inline",
            "processor": "execute",
            "config": { "cmd": "echo" }
        });
        let step: PipelineStep = serde_json::from_value(json).unwrap();
        assert!(matches!(step, PipelineStep::Inline { .. }));
        if let PipelineStep::Inline { processor, config } = step {
            assert_eq!(processor, "execute");
            assert_eq!(config["cmd"], "echo");
        }

        // 4. Invalid/Legacy formats should fail
        let legacy_string = json!("my-preset");
        assert!(serde_json::from_value::<PipelineStep>(legacy_string).is_err());

        let legacy_obj = json!({ "processor": "remux" });
        assert!(serde_json::from_value::<PipelineStep>(legacy_obj).is_err());
    }

    #[test]
    fn test_pipeline_step_serialization() {
        // Serialization should be tagged
        let step = PipelineStep::Preset {
            name: "foo".to_string(),
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["type"], "preset");
        assert_eq!(json["name"], "foo");

        let step = PipelineStep::Workflow {
            name: "bar".to_string(),
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["type"], "workflow");
        assert_eq!(json["name"], "bar");

        let step = PipelineStep::Inline {
            processor: "proc".to_string(),
            config: serde_json::json!({}),
        };
        let json = serde_json::to_value(&step).unwrap();
        assert_eq!(json["type"], "inline");
        assert_eq!(json["processor"], "proc");
    }

    // ========================================================================
    // DAG Pipeline Tests
    // ========================================================================

    #[test]
    fn test_dag_step_creation() {
        let step = DagStep::new("remux", PipelineStep::preset("remux"));
        assert_eq!(step.id, "remux");
        assert!(step.is_root());

        let step_with_deps = DagStep::with_dependencies(
            "upload",
            PipelineStep::preset("upload"),
            vec!["remux".to_string()],
        );
        assert_eq!(step_with_deps.id, "upload");
        assert!(!step_with_deps.is_root());
        assert_eq!(step_with_deps.depends_on, vec!["remux"]);
    }

    #[test]
    fn test_dag_pipeline_validation_valid() {
        // Simple linear DAG: A -> B -> C
        let dag = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("thumbnail"),
                    vec!["B".to_string()],
                ),
            ],
        );
        assert!(dag.validate().is_ok());
    }

    #[test]
    fn test_dag_pipeline_validation_fan_out() {
        // Fan-out: A -> [B, C]
        let dag = DagPipelineDefinition::new(
            "fan-out-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("thumbnail"),
                    vec!["A".to_string()],
                ),
            ],
        );
        assert!(dag.validate().is_ok());
        assert_eq!(dag.root_steps().len(), 1);
        assert_eq!(dag.leaf_steps().len(), 2);
    }

    #[test]
    fn test_dag_pipeline_validation_fan_in() {
        // Fan-in: [A, B] -> C
        let dag = DagPipelineDefinition::new(
            "fan-in-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::new("B", PipelineStep::preset("thumbnail")),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string(), "B".to_string()],
                ),
            ],
        );
        assert!(dag.validate().is_ok());
        assert_eq!(dag.root_steps().len(), 2);
        assert_eq!(dag.leaf_steps().len(), 1);
    }

    #[test]
    fn test_dag_pipeline_validation_diamond() {
        // Diamond pattern: A -> [B, C] -> D
        let dag = DagPipelineDefinition::new(
            "diamond-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("thumbnail"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "D",
                    PipelineStep::preset("notify"),
                    vec!["B".to_string(), "C".to_string()],
                ),
            ],
        );
        assert!(dag.validate().is_ok());
        assert_eq!(dag.root_steps().len(), 1);
        assert_eq!(dag.leaf_steps().len(), 1);
    }

    #[test]
    fn test_dag_pipeline_validation_empty() {
        let dag = DagPipelineDefinition::new("empty-dag", vec![]);
        assert!(dag.validate().is_err());
        assert!(
            dag.validate()
                .unwrap_err()
                .to_string()
                .contains("at least one step")
        );
    }

    #[test]
    fn test_dag_pipeline_validation_duplicate_ids() {
        let dag = DagPipelineDefinition::new(
            "dup-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::new("A", PipelineStep::preset("upload")),
            ],
        );
        assert!(dag.validate().is_err());
        assert!(
            dag.validate()
                .unwrap_err()
                .to_string()
                .contains("Duplicate")
        );
    }

    #[test]
    fn test_dag_pipeline_validation_missing_dependency() {
        let dag = DagPipelineDefinition::new(
            "missing-dep-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["X".to_string()],
                ),
            ],
        );
        assert!(dag.validate().is_err());
        assert!(
            dag.validate()
                .unwrap_err()
                .to_string()
                .contains("non-existent")
        );
    }

    #[test]
    fn test_dag_pipeline_validation_no_root() {
        // All steps have dependencies (cycle)
        let dag = DagPipelineDefinition::new(
            "no-root-dag",
            vec![
                DagStep::with_dependencies(
                    "A",
                    PipelineStep::preset("remux"),
                    vec!["B".to_string()],
                ),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );
        assert!(dag.validate().is_err());
    }

    #[test]
    fn test_dag_pipeline_validation_cycle() {
        // A -> B -> C -> A (cycle)
        let dag = DagPipelineDefinition::new(
            "cycle-dag",
            vec![
                DagStep::with_dependencies("A", PipelineStep::preset("a"), vec!["C".to_string()]),
                DagStep::with_dependencies("B", PipelineStep::preset("b"), vec!["A".to_string()]),
                DagStep::with_dependencies("C", PipelineStep::preset("c"), vec!["B".to_string()]),
            ],
        );
        assert!(dag.validate().is_err());
    }

    #[test]
    fn test_dag_pipeline_topological_order() {
        // A -> B -> C
        let dag = DagPipelineDefinition::new(
            "topo-dag",
            vec![
                DagStep::with_dependencies("C", PipelineStep::preset("c"), vec!["B".to_string()]),
                DagStep::new("A", PipelineStep::preset("a")),
                DagStep::with_dependencies("B", PipelineStep::preset("b"), vec!["A".to_string()]),
            ],
        );

        let order = dag.topological_order().unwrap();
        let ids: Vec<&str> = order.iter().map(|s| s.id.as_str()).collect();

        // A must come before B, B must come before C
        let a_pos = ids.iter().position(|&x| x == "A").unwrap();
        let b_pos = ids.iter().position(|&x| x == "B").unwrap();
        let c_pos = ids.iter().position(|&x| x == "C").unwrap();

        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_dag_pipeline_get_step() {
        let dag = DagPipelineDefinition::new(
            "get-step-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::new("B", PipelineStep::preset("upload")),
            ],
        );

        assert!(dag.get_step("A").is_some());
        assert!(dag.get_step("B").is_some());
        assert!(dag.get_step("C").is_none());
    }

    #[test]
    fn test_dag_step_status() {
        assert_eq!(DagStepStatus::Blocked.as_str(), "BLOCKED");
        assert_eq!(
            DagStepStatus::parse("BLOCKED"),
            Some(DagStepStatus::Blocked)
        );
        assert_eq!(DagStepStatus::parse("INVALID"), None);

        assert!(!DagStepStatus::Blocked.is_terminal());
        assert!(!DagStepStatus::Pending.is_terminal());
        assert!(!DagStepStatus::Processing.is_terminal());
        assert!(DagStepStatus::Completed.is_terminal());
        assert!(DagStepStatus::Failed.is_terminal());
        assert!(DagStepStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_dag_execution_status() {
        assert_eq!(DagExecutionStatus::Processing.as_str(), "PROCESSING");
        assert_eq!(
            DagExecutionStatus::parse("PROCESSING"),
            Some(DagExecutionStatus::Processing)
        );
        assert_eq!(DagExecutionStatus::parse("INVALID"), None);

        assert!(!DagExecutionStatus::Pending.is_terminal());
        assert!(!DagExecutionStatus::Processing.is_terminal());
        assert!(DagExecutionStatus::Completed.is_terminal());
        assert!(DagExecutionStatus::Failed.is_terminal());
        assert!(!DagExecutionStatus::Interrupted.is_terminal());
    }

    #[test]
    fn test_dag_step_json_serialization() {
        use serde_json::json;

        let step = DagStep::with_dependencies(
            "upload",
            PipelineStep::preset("upload"),
            vec!["remux".to_string()],
        );

        let json_val = serde_json::to_value(&step).unwrap();
        assert_eq!(json_val["id"], "upload");
        assert_eq!(json_val["step"]["type"], "preset");
        assert_eq!(json_val["step"]["name"], "upload");
        assert_eq!(json_val["depends_on"], json!(["remux"]));

        // Deserialize back
        let deserialized: DagStep = serde_json::from_value(json_val).unwrap();
        assert_eq!(deserialized.id, "upload");
        assert_eq!(deserialized.depends_on, vec!["remux"]);
    }

    #[test]
    fn test_dag_pipeline_json_serialization() {
        let dag = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("remux", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "upload",
                    PipelineStep::preset("upload"),
                    vec!["remux".to_string()],
                ),
            ],
        );

        let json_str = serde_json::to_string(&dag).unwrap();
        let deserialized: DagPipelineDefinition = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.name, "test-dag");
        assert_eq!(deserialized.steps.len(), 2);
        assert!(deserialized.validate().is_ok());
    }
}
