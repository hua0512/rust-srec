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
    pub status: String,
    /// Associated streamer ID.
    pub streamer_id: Option<String>,
    /// Associated session ID.
    pub session_id: Option<String>,
    /// Segment index when this DAG is tied to a specific session segment.
    pub segment_index: Option<i64>,
    /// Coordination discriminator ("video", "danmu", "paired", or "session_complete").
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
    /// JSON `PipelineInputManifest` pairing video and danmu inputs per segment.
    /// Set for paired-segment and session-complete DAGs; NULL otherwise.
    pub input_manifest: Option<String>,
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
            input_manifest: None,
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

    pub fn mark_cancelled(&mut self) {
        let now = crate::database::time::now_ms();
        self.status = DagExecutionStatus::Cancelled.as_str().to_string();
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
}

/// Display fields for an execution list, without its input manifest or error payload.
#[derive(Debug, Clone, PartialEq)]
pub struct DagExecutionListSummary {
    pub id: String,
    pub name: Option<String>,
    pub display_status: DagExecutionStatus,
    pub streamer_id: Option<String>,
    pub session_id: Option<String>,
    pub total_steps: i32,
    pub completed_steps: i32,
    pub failed_steps: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Read only the `name` field of a DAG definition.
///
/// List views need the name alone; deserialising every step and its
/// processor config through `get_dag_definition` is wasted work there.
pub(crate) fn dag_definition_name(definition: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct DagDefinitionName {
        name: String,
    }

    serde_json::from_str::<DagDefinitionName>(definition)
        .ok()
        .map(|definition| definition.name)
}

/// A workflow's progress as a percentage (0-100) of its `total_steps`.
///
/// Each completed step counts fully, and each of `running_percents`, the
/// reported percents of running steps clamped to 0-100, counts as that share
/// of a step. Every other step counts nothing, including a running step that
/// has not reported a percent or reported NaN, so a failed or cancelled
/// workflow shows how much of it completed. A workflow without steps reports 0.
pub fn dag_progress_percent(
    total_steps: i32,
    completed_steps: i32,
    running_percents: impl IntoIterator<Item = f32>,
) -> f64 {
    if total_steps <= 0 {
        return 0.0;
    }
    // `clamp` passes NaN through, and `f64::min` would then turn the NaN sum
    // into 100.
    let running_steps: f64 = running_percents
        .into_iter()
        .filter(|percent| !percent.is_nan())
        .map(|percent| f64::from(percent.clamp(0.0, 100.0)) / 100.0)
        .sum();
    (100.0 * (f64::from(completed_steps) + running_steps) / f64::from(total_steps)).min(100.0)
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

    /// Whether this step is still running `job_id` in a DAG that has not
    /// settled, so a scheduled retry of that job will run. `dag_status` is the
    /// stored status of the step's DAG.
    pub fn awaits_job(&self, job_id: &str, dag_status: Option<DagExecutionStatus>) -> bool {
        self.get_status() == Some(DagStepStatus::Processing)
            && self.job_id.as_deref() == Some(job_id)
            && !dag_status.is_some_and(|status| status.is_terminal())
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

/// A step execution together with the status and timing of its job.
#[derive(Debug, Clone, FromRow)]
pub struct DagStepWithJob {
    #[sqlx(flatten)]
    pub step: DagStepExecutionDbModel,
    /// Stored status of the step's job; `None` while the step has no job.
    pub job_status: Option<String>,
    /// Unix epoch milliseconds (UTC) when the job's latest attempt started.
    pub job_started_at: Option<i64>,
    /// Unix epoch milliseconds (UTC) of the job's scheduled automatic retry.
    /// It stays set until the retry is due even when the DAG settles first, so
    /// it only means a retry will run while
    /// [`DagStepExecutionDbModel::awaits_job`] holds.
    pub job_retry_after: Option<i64>,
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

/// DAG execution counts by display status, plus the average wall-clock
/// duration of completed DAGs.
///
/// A stored PENDING or PROCESSING DAG is displayed as PROCESSING only while one
/// of its jobs is running; otherwise it is waiting for a worker and is
/// displayed as PENDING.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DagDisplayCounts {
    pub pending: u64,
    pub processing: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    /// Average time from creation to completion of completed DAGs, in seconds.
    pub avg_duration_secs: Option<f64>,
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

    #[test]
    fn dag_progress_percent_counts_completed_steps_and_running_progress() {
        assert_eq!(dag_progress_percent(4, 1, []), 25.0);
        assert_eq!(dag_progress_percent(4, 1, [50.0]), 37.5);
        assert_eq!(dag_progress_percent(4, 1, [-20.0, 150.0]), 50.0);
        assert_eq!(dag_progress_percent(4, 1, [f32::NAN]), 25.0);
        assert_eq!(dag_progress_percent(0, 0, []), 0.0);
        assert_eq!(dag_progress_percent(2, 2, [100.0]), 100.0);
    }
}
