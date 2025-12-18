//! DAG (Directed Acyclic Graph) pipeline database models.
//!
//! These models support fan-in/fan-out pipeline execution where steps
//! can have multiple dependencies and can produce outputs for multiple
//! downstream steps.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use super::job::{DagExecutionStatus, DagPipelineDefinition, DagStepStatus};

// ============================================================================
// DAG Execution Database Model
// ============================================================================

/// DAG execution instance database model.
/// Tracks the overall state of a DAG pipeline execution.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct DagExecutionDbModel {
    /// Unique identifier for this DAG execution.
    pub id: String,
    /// JSON-serialized DAG pipeline definition.
    pub dag_definition: String,
    /// Execution status: PENDING, PROCESSING, COMPLETED, FAILED, INTERRUPTED.
    pub status: String,
    /// Associated streamer ID.
    pub streamer_id: Option<String>,
    /// Associated session ID.
    pub session_id: Option<String>,
    /// ISO 8601 timestamp when the DAG was created.
    pub created_at: String,
    /// ISO 8601 timestamp when the DAG was last updated.
    pub updated_at: String,
    /// ISO 8601 timestamp when the DAG completed (success or failure).
    pub completed_at: Option<String>,
    /// Error message if the DAG failed.
    pub error: Option<String>,
    /// Total number of steps in the DAG.
    pub total_steps: i32,
    /// Number of steps that have completed successfully.
    pub completed_steps: i32,
    /// Number of steps that have failed.
    pub failed_steps: i32,
}

impl DagExecutionDbModel {
    /// Create a new DAG execution record.
    pub fn new(
        dag_definition: &DagPipelineDefinition,
        streamer_id: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let total_steps = dag_definition.steps.len() as i32;

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            dag_definition: serde_json::to_string(dag_definition)
                .unwrap_or_else(|_| "{}".to_string()),
            status: DagExecutionStatus::Processing.as_str().to_string(),
            streamer_id,
            session_id,
            created_at: now.clone(),
            updated_at: now,
            completed_at: None,
            error: None,
            total_steps,
            completed_steps: 0,
            failed_steps: 0,
        }
    }

    /// Get the DAG pipeline definition.
    pub fn get_dag_definition(&self) -> Option<DagPipelineDefinition> {
        serde_json::from_str(&self.dag_definition).ok()
    }

    /// Get the execution status as an enum.
    pub fn get_status(&self) -> Option<DagExecutionStatus> {
        DagExecutionStatus::parse(&self.status)
    }

    /// Mark the DAG as completed successfully.
    pub fn mark_completed(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagExecutionStatus::Completed.as_str().to_string();
        self.completed_at = Some(now.clone());
        self.updated_at = now;
    }

    /// Mark the DAG as failed.
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagExecutionStatus::Failed.as_str().to_string();
        self.completed_at = Some(now.clone());
        self.error = Some(error.into());
        self.updated_at = now;
    }

    /// Mark the DAG as interrupted.
    pub fn mark_interrupted(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagExecutionStatus::Interrupted.as_str().to_string();
        self.updated_at = now;
    }

    /// Increment the completed steps counter.
    pub fn increment_completed(&mut self) {
        self.completed_steps += 1;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Increment the failed steps counter.
    pub fn increment_failed(&mut self) {
        self.failed_steps += 1;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Check if the DAG is complete (all steps finished or failed).
    pub fn is_complete(&self) -> bool {
        self.completed_steps + self.failed_steps >= self.total_steps
    }

    /// Check if the DAG execution has failed.
    pub fn is_failed(&self) -> bool {
        self.failed_steps > 0
    }

    /// Get the progress as a percentage (0-100).
    pub fn progress_percent(&self) -> f64 {
        if self.total_steps == 0 {
            return 100.0;
        }
        ((self.completed_steps + self.failed_steps) as f64 / self.total_steps as f64) * 100.0
    }
}

// ============================================================================
// DAG Step Execution Database Model
// ============================================================================

/// Individual step execution within a DAG.
/// Tracks the state of a single step and its dependencies.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct DagStepExecutionDbModel {
    /// Unique identifier for this step execution.
    pub id: String,
    /// Parent DAG execution ID.
    pub dag_id: String,
    /// Step ID within the DAG definition (e.g., "remux", "upload").
    pub step_id: String,
    /// Associated job ID (NULL until job is created).
    pub job_id: Option<String>,
    /// Step status: BLOCKED, PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED.
    pub status: String,
    /// JSON array of step IDs this step depends on.
    pub depends_on_step_ids: String,
    /// JSON array of output paths produced by this step.
    pub outputs: Option<String>,
    /// ISO 8601 timestamp when the step was created.
    pub created_at: String,
    /// ISO 8601 timestamp when the step was last updated.
    pub updated_at: String,
}

impl DagStepExecutionDbModel {
    /// Create a new step execution record.
    pub fn new(dag_id: &str, step_id: &str, depends_on: &[String]) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let is_root = depends_on.is_empty();

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            dag_id: dag_id.to_string(),
            step_id: step_id.to_string(),
            job_id: None,
            status: if is_root {
                DagStepStatus::Pending.as_str().to_string()
            } else {
                DagStepStatus::Blocked.as_str().to_string()
            },
            depends_on_step_ids: serde_json::to_string(depends_on)
                .unwrap_or_else(|_| "[]".to_string()),
            outputs: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Get the step status as an enum.
    pub fn get_status(&self) -> Option<DagStepStatus> {
        DagStepStatus::parse(&self.status)
    }

    /// Get the dependency step IDs.
    pub fn get_depends_on(&self) -> Vec<String> {
        serde_json::from_str(&self.depends_on_step_ids).unwrap_or_default()
    }

    /// Get the output paths.
    pub fn get_outputs(&self) -> Vec<String> {
        self.outputs
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Set the output paths.
    pub fn set_outputs(&mut self, outputs: &[String]) {
        self.outputs = Some(serde_json::to_string(outputs).unwrap_or_else(|_| "[]".to_string()));
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Check if this step has no dependencies (root step).
    pub fn is_root(&self) -> bool {
        self.get_depends_on().is_empty()
    }

    /// Mark the step as pending (ready to run).
    pub fn mark_pending(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagStepStatus::Pending.as_str().to_string();
        self.updated_at = now;
    }

    /// Mark the step as processing with a job ID.
    pub fn mark_processing(&mut self, job_id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagStepStatus::Processing.as_str().to_string();
        self.job_id = Some(job_id.to_string());
        self.updated_at = now;
    }

    /// Mark the step as completed with outputs.
    pub fn mark_completed(&mut self, outputs: &[String]) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagStepStatus::Completed.as_str().to_string();
        self.set_outputs(outputs);
        self.updated_at = now;
    }

    /// Mark the step as failed.
    pub fn mark_failed(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagStepStatus::Failed.as_str().to_string();
        self.updated_at = now;
    }

    /// Mark the step as cancelled.
    pub fn mark_cancelled(&mut self) {
        let now = chrono::Utc::now().to_rfc3339();
        self.status = DagStepStatus::Cancelled.as_str().to_string();
        self.updated_at = now;
    }
}

// ============================================================================
// Helper Structs for DAG Operations
// ============================================================================

/// Represents a step that is now ready to execute (all dependencies complete).
#[derive(Debug, Clone)]
pub struct ReadyStep {
    /// The step execution record.
    pub step: DagStepExecutionDbModel,
    /// Concatenated outputs from all dependency steps (fan-in merge).
    pub merged_inputs: Vec<String>,
}

/// DAG execution statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DagExecutionStats {
    /// Number of blocked steps (waiting for dependencies).
    pub blocked: u64,
    /// Number of pending steps (ready to run).
    pub pending: u64,
    /// Number of processing steps (currently running).
    pub processing: u64,
    /// Number of completed steps.
    pub completed: u64,
    /// Number of failed steps.
    pub failed: u64,
    /// Number of cancelled steps.
    pub cancelled: u64,
}

impl DagExecutionStats {
    /// Get total number of steps.
    pub fn total(&self) -> u64 {
        self.blocked
            + self.pending
            + self.processing
            + self.completed
            + self.failed
            + self.cancelled
    }

    /// Get number of finished steps (completed + failed + cancelled).
    pub fn finished(&self) -> u64 {
        self.completed + self.failed + self.cancelled
    }

    /// Check if all steps are finished.
    pub fn is_complete(&self) -> bool {
        self.blocked == 0 && self.pending == 0 && self.processing == 0
    }

    /// Check if any step has failed.
    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::job::{DagStep, PipelineStep};

    #[test]
    fn test_dag_execution_new() {
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag_exec = DagExecutionDbModel::new(&dag_def, Some("streamer-1".to_string()), None);

        assert_eq!(dag_exec.total_steps, 2);
        assert_eq!(dag_exec.completed_steps, 0);
        assert_eq!(dag_exec.failed_steps, 0);
        assert_eq!(dag_exec.status, "PROCESSING");
        assert!(dag_exec.get_dag_definition().is_some());
    }

    #[test]
    fn test_dag_execution_lifecycle() {
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );

        let mut dag_exec = DagExecutionDbModel::new(&dag_def, None, None);

        // Increment completed
        dag_exec.increment_completed();
        assert_eq!(dag_exec.completed_steps, 1);
        assert!(dag_exec.is_complete());

        // Mark completed
        dag_exec.mark_completed();
        assert_eq!(dag_exec.get_status(), Some(DagExecutionStatus::Completed));
        assert!(dag_exec.completed_at.is_some());
    }

    #[test]
    fn test_dag_execution_failure() {
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );

        let mut dag_exec = DagExecutionDbModel::new(&dag_def, None, None);

        dag_exec.increment_failed();
        dag_exec.mark_failed("Step A failed");

        assert!(dag_exec.is_failed());
        assert_eq!(dag_exec.get_status(), Some(DagExecutionStatus::Failed));
        assert_eq!(dag_exec.error, Some("Step A failed".to_string()));
    }

    #[test]
    fn test_dag_execution_progress() {
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::new("B", PipelineStep::preset("upload")),
                DagStep::new("C", PipelineStep::preset("thumbnail")),
                DagStep::new("D", PipelineStep::preset("notify")),
            ],
        );

        let mut dag_exec = DagExecutionDbModel::new(&dag_def, None, None);
        assert_eq!(dag_exec.progress_percent(), 0.0);

        dag_exec.increment_completed();
        assert_eq!(dag_exec.progress_percent(), 25.0);

        dag_exec.increment_completed();
        dag_exec.increment_failed();
        assert_eq!(dag_exec.progress_percent(), 75.0);
    }

    #[test]
    fn test_dag_step_execution_root() {
        let step = DagStepExecutionDbModel::new("dag-1", "remux", &[]);

        assert!(step.is_root());
        assert_eq!(step.get_status(), Some(DagStepStatus::Pending)); // Root starts as Pending
        assert!(step.get_depends_on().is_empty());
    }

    #[test]
    fn test_dag_step_execution_with_deps() {
        let step = DagStepExecutionDbModel::new(
            "dag-1",
            "upload",
            &["remux".to_string(), "thumbnail".to_string()],
        );

        assert!(!step.is_root());
        assert_eq!(step.get_status(), Some(DagStepStatus::Blocked)); // Non-root starts as Blocked
        assert_eq!(step.get_depends_on(), vec!["remux", "thumbnail"]);
    }

    #[test]
    fn test_dag_step_execution_lifecycle() {
        let mut step = DagStepExecutionDbModel::new("dag-1", "upload", &["remux".to_string()]);

        // Initially blocked
        assert_eq!(step.get_status(), Some(DagStepStatus::Blocked));

        // Mark pending
        step.mark_pending();
        assert_eq!(step.get_status(), Some(DagStepStatus::Pending));

        // Mark processing
        step.mark_processing("job-123");
        assert_eq!(step.get_status(), Some(DagStepStatus::Processing));
        assert_eq!(step.job_id, Some("job-123".to_string()));

        // Mark completed
        step.mark_completed(&["/output/file.mp4".to_string()]);
        assert_eq!(step.get_status(), Some(DagStepStatus::Completed));
        assert_eq!(step.get_outputs(), vec!["/output/file.mp4"]);
    }

    #[test]
    fn test_dag_step_execution_failure() {
        let mut step = DagStepExecutionDbModel::new("dag-1", "remux", &[]);

        step.mark_processing("job-123");
        step.mark_failed();

        assert_eq!(step.get_status(), Some(DagStepStatus::Failed));
    }

    #[test]
    fn test_dag_step_execution_cancelled() {
        let mut step = DagStepExecutionDbModel::new("dag-1", "upload", &["remux".to_string()]);

        step.mark_cancelled();
        assert_eq!(step.get_status(), Some(DagStepStatus::Cancelled));
    }

    #[test]
    fn test_dag_execution_stats() {
        let mut stats = DagExecutionStats::default();

        stats.blocked = 2;
        stats.pending = 1;
        stats.processing = 1;
        stats.completed = 3;
        stats.failed = 1;
        stats.cancelled = 0;

        assert_eq!(stats.total(), 8);
        assert_eq!(stats.finished(), 4);
        assert!(!stats.is_complete());
        assert!(stats.has_failures());
    }

    #[test]
    fn test_ready_step() {
        let step = DagStepExecutionDbModel::new("dag-1", "notify", &["upload".to_string()]);
        let ready = ReadyStep {
            step,
            merged_inputs: vec!["/path/a.mp4".to_string(), "/path/b.jpg".to_string()],
        };

        assert_eq!(ready.merged_inputs.len(), 2);
        assert_eq!(ready.step.step_id, "notify");
    }
}
