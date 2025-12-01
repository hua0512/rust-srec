//! Job database models.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Job database model.
/// Represents a single asynchronous task (download or pipeline process).
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct JobDbModel {
    pub id: String,
    /// Job type: DOWNLOAD, PIPELINE
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
        }
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
}

impl JobExecutionLogDbModel {
    pub fn new(job_id: impl Into<String>, entry: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: job_id.into(),
            entry: entry.into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStep {
    pub processor: String,
    pub config: serde_json::Value,
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
}
