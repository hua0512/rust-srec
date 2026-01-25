//! DAG (Directed Acyclic Graph) pipeline database models.
//!
//! These models support fan-in/fan-out pipeline execution where steps
//! can have multiple dependencies and can produce outputs for multiple
//! downstream steps.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::json::{self, JsonContext};

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
    /// Segment index when this DAG is tied to a specific session segment.
    pub segment_index: Option<i64>,
    /// Segment source ("video", "danmu", or "paired") for segment-related DAGs.
    pub segment_source: Option<String>,
    /// Unix epoch milliseconds (UTC) when the DAG was created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when the DAG was last updated.
    pub updated_at: i64,
    /// Unix epoch milliseconds (UTC) when the DAG completed (success or failure).
    pub completed_at: Option<i64>,
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
        let now = crate::database::time::now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        let total_steps = dag_definition.steps.len() as i32;

        Self {
            id: id.clone(),
            dag_definition: json::to_string_or_fallback(
                dag_definition,
                "{}",
                JsonContext::DagExecutionField {
                    dag_execution_id: &id,
                    field: "dag_definition",
                },
                "Failed to serialize dag_definition; storing empty object",
            ),
            status: DagExecutionStatus::Pending.as_str().to_string(),
            streamer_id,
            session_id,
            segment_index: None,
            segment_source: None,
            created_at: now,
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
        json::parse_optional(
            Some(self.dag_definition.as_str()),
            JsonContext::DagExecutionField {
                dag_execution_id: &self.id,
                field: "dag_definition",
            },
            "Invalid dag_definition JSON",
        )
    }

    /// Get the execution status as an enum.
    pub fn get_status(&self) -> Option<DagExecutionStatus> {
        DagExecutionStatus::parse(&self.status)
    }

    /// Mark the DAG as completed successfully.
    pub fn mark_completed(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = DagExecutionStatus::Completed.as_str().to_string();
        self.completed_at = Some(now);
        self.updated_at = now;
    }

    /// Mark the DAG as failed.
    pub fn mark_failed(&mut self, error: impl Into<String>) {
        let now = crate::database::time::now_ms();
        self.status = DagExecutionStatus::Failed.as_str().to_string();
        self.completed_at = Some(now);
        self.error = Some(error.into());
        self.updated_at = now;
    }

    /// Mark the DAG as interrupted.
    pub fn mark_interrupted(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = DagExecutionStatus::Interrupted.as_str().to_string();
        self.updated_at = now;
    }

    /// Increment the completed steps counter.
    pub fn increment_completed(&mut self) {
        self.completed_steps += 1;
        self.updated_at = crate::database::time::now_ms();
    }

    /// Increment the failed steps counter.
    pub fn increment_failed(&mut self) {
        self.failed_steps += 1;
        self.updated_at = crate::database::time::now_ms();
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
    /// Unix epoch milliseconds (UTC) when the step was created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when the step was last updated.
    pub updated_at: i64,
}

impl DagStepExecutionDbModel {
    /// Create a new step execution record.
    pub fn new(dag_id: &str, step_id: &str, depends_on: &[String]) -> Self {
        let now = crate::database::time::now_ms();
        let is_root = depends_on.is_empty();
        let id = uuid::Uuid::new_v4().to_string();

        Self {
            id: id.clone(),
            dag_id: dag_id.to_string(),
            step_id: step_id.to_string(),
            job_id: None,
            status: if is_root {
                DagStepStatus::Pending.as_str().to_string()
            } else {
                DagStepStatus::Blocked.as_str().to_string()
            },
            depends_on_step_ids: json::to_string_or_fallback(
                depends_on,
                "[]",
                JsonContext::DagStepExecutionField {
                    dag_step_execution_id: &id,
                    dag_execution_id: dag_id,
                    step_id,
                    field: "depends_on_step_ids",
                },
                "Failed to serialize depends_on_step_ids; storing empty list",
            ),
            outputs: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Get the step status as an enum.
    pub fn get_status(&self) -> Option<DagStepStatus> {
        DagStepStatus::parse(&self.status)
    }

    /// Get the dependency step IDs.
    pub fn get_depends_on(&self) -> Vec<String> {
        json::parse_or_default(
            self.depends_on_step_ids.as_str(),
            JsonContext::DagStepExecutionField {
                dag_step_execution_id: &self.id,
                dag_execution_id: &self.dag_id,
                step_id: &self.step_id,
                field: "depends_on_step_ids",
            },
            "Invalid depends_on_step_ids JSON; treating as empty",
        )
    }

    /// Get the output paths.
    pub fn get_outputs(&self) -> Vec<String> {
        let Some(raw) = self.outputs.as_deref() else {
            return Vec::new();
        };

        json::parse_or_default(
            raw,
            JsonContext::DagStepExecutionField {
                dag_step_execution_id: &self.id,
                dag_execution_id: &self.dag_id,
                step_id: &self.step_id,
                field: "outputs",
            },
            "Invalid outputs JSON; treating as empty",
        )
    }

    /// Set the output paths.
    pub fn set_outputs(&mut self, outputs: &[String]) {
        self.outputs = Some(json::to_string_or_fallback(
            outputs,
            "[]",
            JsonContext::DagStepExecutionField {
                dag_step_execution_id: &self.id,
                dag_execution_id: &self.dag_id,
                step_id: &self.step_id,
                field: "outputs",
            },
            "Failed to serialize outputs; storing empty list",
        ));
        self.updated_at = crate::database::time::now_ms();
    }

    /// Check if this step has no dependencies (root step).
    pub fn is_root(&self) -> bool {
        self.get_depends_on().is_empty()
    }

    /// Mark the step as pending (ready to run).
    pub fn mark_pending(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = DagStepStatus::Pending.as_str().to_string();
        self.updated_at = now;
    }

    /// Mark the step as processing with a job ID.
    pub fn mark_processing(&mut self, job_id: &str) {
        let now = crate::database::time::now_ms();
        self.status = DagStepStatus::Processing.as_str().to_string();
        self.job_id = Some(job_id.to_string());
        self.updated_at = now;
    }

    /// Mark the step as completed with outputs.
    pub fn mark_completed(&mut self, outputs: &[String]) {
        let now = crate::database::time::now_ms();
        self.status = DagStepStatus::Completed.as_str().to_string();
        self.set_outputs(outputs);
        self.updated_at = now;
    }

    /// Mark the step as failed.
    pub fn mark_failed(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = DagStepStatus::Failed.as_str().to_string();
        self.updated_at = now;
    }

    /// Mark the step as cancelled.
    pub fn mark_cancelled(&mut self) {
        let now = crate::database::time::now_ms();
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
        assert_eq!(dag_exec.status, "PENDING");
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
        let stats = DagExecutionStats {
            blocked: 2,
            pending: 1,
            processing: 1,
            completed: 3,
            failed: 1,
            cancelled: 0,
        };

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
