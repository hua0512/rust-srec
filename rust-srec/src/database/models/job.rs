//! Job database models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

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
    /// Job type: DOWNLOAD, PIPELINE, or specific step types like "remux", "upload", "thumbnail"
    pub job_type: String,
    /// Status: PENDING, PROCESSING, COMPLETED, FAILED, INTERRUPTED
    pub status: String,
    /// JSON blob for job-specific configuration
    pub config: String,
    /// JSON blob for dynamic job state
    pub state: String,
    /// ISO 8601 timestamp when the job was created
    pub created_at: String,
    /// ISO 8601 timestamp when the job was last updated
    pub updated_at: String,
    // Pipeline-specific fields (Requirements 6.1, 6.2, 6.4)
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
    /// ISO 8601 timestamp when the job started processing
    pub started_at: Option<String>,
    /// ISO 8601 timestamp when the job completed
    pub completed_at: Option<String>,
    /// Error message if the job failed
    pub error: Option<String>,
    /// Number of retry attempts
    pub retry_count: i32,
    // Pipeline chain fields (Requirements 7.1, 7.2)
    /// Next job type to create on completion (e.g., "upload" after "remux")
    pub next_job_type: Option<String>,
    /// Pipeline steps remaining after this job (JSON array)
    pub remaining_steps: Option<String>,
    /// Pipeline ID to group related jobs (first job's ID)
    pub pipeline_id: Option<String>,
    /// Execution information for observability (JSON)
    /// Requirements: 6.1, 6.2, 6.3, 6.4
    pub execution_info: Option<String>,
    /// Processing duration in seconds (from processor output)
    pub duration_secs: Option<f64>,
    /// Time spent waiting in queue before processing started (seconds)
    pub queue_wait_secs: Option<f64>,
}

impl JobDbModel {
    pub fn new(job_type: JobType, config: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.as_str().to_string(),
            status: JobStatus::Pending.as_str().to_string(),
            config: config.into(),
            state: "{}".to_string(),
            created_at: now.clone(),
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
            next_job_type: None,
            remaining_steps: None,
            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
        }
    }

    /// Create a new pipeline job with all fields.
    pub fn new_pipeline(
        input: impl Into<String>,
        priority: i32,
        streamer_id: Option<String>,
        session_id: Option<String>,
        config: impl Into<String>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: JobType::Pipeline.as_str().to_string(),
            status: JobStatus::Pending.as_str().to_string(),
            config: config.into(),
            state: "{}".to_string(),
            created_at: now.clone(),
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
            next_job_type: None,
            remaining_steps: None,
            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
        }
    }

    /// Create a new pipeline step job with chain information.
    /// This is used for sequential pipeline job creation.
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
        pipeline_id: Option<String>,
        next_job_type: Option<String>,
        remaining_steps: Option<Vec<PipelineStep>>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        let output_str = output.into();
        Self {
            id: id.clone(),
            job_type: job_type.into(),
            status: JobStatus::Pending.as_str().to_string(),
            config: "{}".to_string(),
            state: "{}".to_string(),
            created_at: now.clone(),
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
            next_job_type,
            remaining_steps: remaining_steps
                .map(|steps| serde_json::to_string(&steps).unwrap_or_else(|_| "[]".to_string())),
            pipeline_id,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
        }
    }

    /// Get the remaining pipeline steps.
    pub fn get_remaining_steps(&self) -> Vec<PipelineStep> {
        self.remaining_steps
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Set the remaining pipeline steps.
    pub fn set_remaining_steps(&mut self, steps: &[PipelineStep]) {
        self.remaining_steps =
            Some(serde_json::to_string(steps).unwrap_or_else(|_| "[]".to_string()));
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Get the list of output paths produced by this job.
    pub fn get_outputs(&self) -> Vec<String> {
        self.outputs
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Set the output paths for this job.
    pub fn set_outputs(&mut self, outputs: &[String]) {
        self.outputs = Some(serde_json::to_string(outputs).unwrap_or_else(|_| "[]".to_string()));
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Add an output path to this job.
    pub fn add_output(&mut self, output: impl Into<String>) {
        let mut outputs = self.get_outputs();
        outputs.push(output.into());
        self.set_outputs(&outputs);
    }

    /// Mark the job as started processing.
    pub fn mark_started(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = JobStatus::Processing.as_str().to_string();
        self.started_at = Some(now.clone());
        self.updated_at = now;
    }

    /// Mark the job as completed.
    pub fn mark_completed(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = JobStatus::Completed.as_str().to_string();
        self.completed_at = Some(now.clone());
        self.updated_at = now;
    }

    /// Mark the job as failed with an error message.
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = JobStatus::Failed.as_str().to_string();
        self.completed_at = Some(now.clone());
        self.error = Some(error.into());
        self.updated_at = now;
    }

    /// Reset the job for retry.
    pub fn reset_for_retry(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
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

    /// Get the job type as an enum.
    pub fn get_job_type(&self) -> Option<JobType> {
        JobType::parse(&self.job_type)
    }
}

/// Job types.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobType {
    Download,
    Pipeline,
}

impl JobType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Download => "DOWNLOAD",
            Self::Pipeline => "PIPELINE",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "DOWNLOAD" => Some(Self::Download),
            "PIPELINE" => Some(Self::Pipeline),
            _ => None,
        }
    }
}

/// Job status values.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
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

/// Job execution log database model.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct JobExecutionLogDbModel {
    pub id: String,
    pub job_id: String,
    /// JSON blob for the log entry
    pub entry: String,
    /// ISO 8601 timestamp
    pub created_at: String,
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
            created_at: chrono::Utc::now().to_rfc3339(),
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
    /// ISO 8601 timestamp.
    pub updated_at: String,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
/// Requirements: 6.1, 6.2, 7.1
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
        let job = JobDbModel::new(JobType::Pipeline, r#"{"steps":[]}"#);
        assert_eq!(job.status, "PENDING");
        assert_eq!(job.job_type, "PIPELINE");
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
        let job = JobDbModel::new_pipeline(
            "/input/file.flv",
            10,
            Some("streamer-123".to_string()),
            Some("session-456".to_string()),
            r#"{"steps":[]}"#,
        );
        assert_eq!(job.status, "PENDING");
        assert_eq!(job.job_type, "PIPELINE");
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
        let mut job = JobDbModel::new_pipeline("/input/file.flv", 5, None, None, r#"{"steps":[]}"#);

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
        let mut job = JobDbModel::new(JobType::Pipeline, r#"{"steps":[]}"#);

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
        let mut job = JobDbModel::new(JobType::Pipeline, r#"{"steps":[]}"#);
        job.mark_started();
        job.mark_failed("Something went wrong");

        assert_eq!(job.status, "FAILED");
        assert!(job.completed_at.is_some());
        assert_eq!(job.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_job_reset_for_retry() {
        let mut job = JobDbModel::new(JobType::Pipeline, r#"{"steps":[]}"#);
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
        let job = JobDbModel::new(JobType::Pipeline, r#"{"steps":[]}"#);
        assert_eq!(job.get_status(), Some(JobStatus::Pending));
        assert_eq!(job.get_job_type(), Some(JobType::Pipeline));
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
}
