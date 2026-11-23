//! Job `state` JSON serialization and repository metadata backfill.

use std::sync::Arc;

use jiff::Timestamp;
use tracing::warn;

use super::{Job, JobExecutionInfo, JobQueue};
use crate::database::models::{JobDbModel, JobStatus, TitleEntry};
use crate::pipeline::manifest::PipelineInputManifest;
use crate::utils::json::{self, JsonContext};

impl JobQueue {
    /// Resolve missing metadata (streamer_name, session_title, session_start) for a job.
    ///
    /// This is useful for jobs recovered from the database where metadata
    /// is not persisted. It looks up the streamer and session repositories.
    /// Anything resolved is written back to the job row's `state` column so
    /// the values survive later deletion of the backing streamer/session rows
    /// (a retry re-reads the job from the DB via `db_model_to_job`).
    pub async fn resolve_job_metadata(&self, job: &mut Job) {
        let mut backfilled = false;

        // Only resolve if we have a streamer_id and name is missing
        if job.streamer_name.is_none()
            && !job.streamer_id.is_empty()
            && let Some(streamer_repo) = self.streamer_repo.get()
        {
            match streamer_repo.get_streamer(&job.streamer_id).await {
                Ok(streamer) => {
                    tracing::debug!(
                        job_id = %job.id,
                        streamer_id = %job.streamer_id,
                        streamer_name = %streamer.name,
                        "Resolved streamer_name from repository"
                    );
                    job.streamer_name = Some(streamer.name);
                    backfilled = true;
                }
                Err(e) => {
                    tracing::debug!(
                        job_id = %job.id,
                        streamer_id = %job.streamer_id,
                        error = %e,
                        "Failed to lookup streamer_name"
                    );
                }
            }
        }

        // Only resolve if we have a session_id and session metadata is missing
        if (job.session_title.is_none() || job.session_start.is_none())
            && !job.session_id.is_empty()
            && let Some(session_repo) = self.session_repo.get()
        {
            match session_repo.get_session(&job.session_id).await {
                Ok(session) => {
                    if job.session_start.is_none() {
                        job.session_start =
                            Some(crate::database::time::ms_to_datetime(session.start_time));
                        backfilled = true;
                    }

                    // Parse titles JSON and get the most recent one
                    if job.session_title.is_none()
                        && let Some(titles_json) = session.titles
                        && let Ok(entries) = serde_json::from_str::<Vec<TitleEntry>>(&titles_json)
                        && let Some(last_entry) = entries.last()
                    {
                        tracing::debug!(
                            job_id = %job.id,
                            session_id = %job.session_id,
                            session_title = %last_entry.title,
                            "Resolved session_title from repository"
                        );
                        job.session_title = Some(last_entry.title.clone());
                        backfilled = true;
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        job_id = %job.id,
                        session_id = %job.session_id,
                        error = %e,
                        "Failed to lookup session_title"
                    );
                }
            }
        }

        // Best-effort durability: without this write the streamer/session
        // rows must still exist at every future dequeue of this job.
        if backfilled {
            let state = job_state_json(job);
            if let Err(e) = self.job_repository.update_job_state(&job.id, &state).await {
                tracing::debug!(
                    job_id = %job.id,
                    error = %e,
                    "Failed to persist resolved job metadata"
                );
            }
        }
    }
}

/// Placeholder metadata carried in the job `state` JSON column.
#[derive(Debug, Clone, Default)]
pub(crate) struct JobStateMeta {
    pub(crate) streamer_name: Option<String>,
    pub(crate) session_title: Option<String>,
    pub(crate) platform: Option<String>,
    pub(crate) session_start: Option<Timestamp>,
    pub(crate) manifest: Option<Arc<PipelineInputManifest>>,
    /// Per-attempt timeout of a workflow step job; not a placeholder source.
    pub(crate) timeout_secs: Option<u64>,
    /// Whether this step job has an automatic DAG retry policy (`max_attempts > 1`).
    pub(crate) has_step_retry: bool,
    /// Epoch milliseconds when the latest retry re-queued this job.
    pub(crate) enqueued_at_ms: Option<i64>,
}

impl JobStateMeta {
    /// True when at least one placeholder field carries a value. The manifest
    /// is not a placeholder source: DAG recovery reads it from the DAG row.
    pub(crate) fn has_any(&self) -> bool {
        self.streamer_name.is_some()
            || self.session_title.is_some()
            || self.platform.is_some()
            || self.session_start.is_some()
    }
}

/// Serialize a job's placeholder metadata into the `state` column JSON.
///
/// Single source of the state shape, together with its inverse
/// [`parse_job_state`]: used by [`job_to_db_model`] at persist time, by
/// [`JobQueue::resolve_job_metadata`] when writing back-filled values, and
/// by `DagScheduler::create_step_job`.
pub(crate) fn job_state_json(job: &Job) -> String {
    if job.streamer_name.is_some()
        || job.session_title.is_some()
        || job.platform.is_some()
        || job.session_start.is_some()
        || job.manifest.is_some()
        || job.timeout_secs.is_some()
        || job.has_step_retry
        || job.enqueued_at_ms.is_some()
    {
        let mut state = serde_json::json!({
            "streamer_name": job.streamer_name.clone(),
            "session_title": job.session_title.clone(),
            "platform": job.platform.clone(),
            "session_start_ms": job.session_start.as_ref().map(|dt| dt.as_millisecond()),
        });
        if let Some(timeout_secs) = job.timeout_secs {
            state["timeout_secs"] = serde_json::json!(timeout_secs);
        }
        if job.has_step_retry {
            state["has_step_retry"] = serde_json::json!(true);
        }
        if let Some(enqueued_at_ms) = job.enqueued_at_ms {
            state["enqueued_at_ms"] = serde_json::json!(enqueued_at_ms);
        }
        if let Some(manifest) = job.manifest.as_deref() {
            state["manifest"] = match serde_json::to_value(manifest) {
                Ok(value) => value,
                Err(error) => {
                    warn!(job_id = %job.id, %error, "Failed to serialize job manifest; storing null");
                    serde_json::Value::Null
                }
            };
        }
        state.to_string()
    } else {
        "{}".to_string()
    }
}

/// Parse the placeholder metadata out of a job `state` JSON string.
///
/// Inverse of [`job_state_json`]; malformed JSON or missing keys yield
/// all-`None`. Used by [`db_model_to_job`] and
/// `DagScheduler::recover_placeholder_metadata`.
pub(crate) fn parse_job_state(state: &str) -> JobStateMeta {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(state) else {
        return JobStateMeta::default();
    };
    let Some(obj) = value.as_object() else {
        return JobStateMeta::default();
    };

    let get_str = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
    };

    JobStateMeta {
        streamer_name: get_str("streamer_name"),
        session_title: get_str("session_title"),
        platform: get_str("platform"),
        session_start: obj
            .get("session_start_ms")
            .and_then(|v| v.as_i64())
            .map(crate::database::time::ms_to_datetime),
        manifest: obj
            .get("manifest")
            .filter(|value| !value.is_null())
            .and_then(|value| {
                serde_json::from_value::<PipelineInputManifest>(value.clone())
                    .inspect_err(|error| warn!(%error, "Invalid manifest in job state; ignoring"))
                    .ok()
            })
            .map(Arc::new),
        timeout_secs: obj.get("timeout_secs").and_then(|v| v.as_u64()),
        has_step_retry: obj
            .get("has_step_retry")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        enqueued_at_ms: obj.get("enqueued_at_ms").and_then(|v| v.as_i64()),
    }
}

/// Convert a Job to JobDbModel.
pub(crate) fn job_to_db_model(job: &Job) -> JobDbModel {
    let inputs_json = json::to_string_or_fallback(
        &job.inputs,
        "[]",
        JsonContext::JobField {
            job_id: &job.id,
            field: "inputs",
        },
        "Failed to serialize job inputs; storing empty list",
    );
    let outputs_json = json::to_string_or_fallback(
        &job.outputs,
        "[]",
        JsonContext::JobField {
            job_id: &job.id,
            field: "outputs",
        },
        "Failed to serialize job outputs; storing empty list",
    );

    let state = job_state_json(job);

    // Serialize execution_info to JSON
    let execution_info_json = job.execution_info.as_ref().and_then(|info| {
        json::to_string_option_or_warn(
            info,
            JsonContext::JobField {
                job_id: &job.id,
                field: "execution_info",
            },
            "Failed to serialize job execution_info; omitting",
        )
    });

    JobDbModel {
        id: job.id.clone(),
        job_type: job.job_type.clone(),
        status: job.status.as_str().to_string(),
        config: job.config.clone().unwrap_or_else(|| "{}".to_string()),
        state,
        created_at: job.created_at.as_millisecond(),
        updated_at: crate::database::time::now_ms(),
        input: Some(inputs_json),
        outputs: Some(outputs_json),
        priority: job.priority,
        streamer_id: Some(job.streamer_id.clone()),
        session_id: Some(job.session_id.clone()),
        started_at: job.started_at.map(|dt| dt.as_millisecond()),
        completed_at: job.completed_at.map(|dt| dt.as_millisecond()),
        error: job.error.clone(),
        retry_count: job.retry_count,
        pipeline_id: job.pipeline_id.clone(),
        execution_info: execution_info_json,
        duration_secs: job.duration_secs,
        queue_wait_secs: job.queue_wait_secs,
        dag_step_execution_id: job.dag_step_execution_id.clone(),
        retry_after: job.retry_after.map(|at| at.as_millisecond()),
    }
}

/// Convert a JobDbModel to Job.
pub(crate) fn db_model_to_job(db_job: &JobDbModel) -> Job {
    let status = db_job.get_status().unwrap_or(JobStatus::Pending);

    let created_at = crate::database::time::ms_to_datetime(db_job.created_at);

    let started_at = db_job.started_at.map(crate::database::time::ms_to_datetime);

    let completed_at = db_job
        .completed_at
        .map(crate::database::time::ms_to_datetime);

    // Parse inputs JSON array
    // If it fails (legacy data), treat as single path wrapped in vec
    let input_str = db_job.input.clone().unwrap_or_default();
    let inputs = if input_str.starts_with('[') {
        serde_json::from_str::<Vec<String>>(&input_str).unwrap_or_else(|_| {
            if input_str.is_empty() {
                vec![]
            } else {
                vec![input_str]
            }
        })
    } else if input_str.is_empty() {
        vec![]
    } else {
        vec![input_str]
    };

    // Parse outputs JSON array
    let output_str = db_job.outputs.clone().unwrap_or_default();
    let outputs = if output_str.starts_with('[') {
        serde_json::from_str::<Vec<String>>(&output_str).unwrap_or_else(|_| vec![])
    } else if output_str.is_empty() {
        vec![]
    } else {
        vec![output_str]
    };

    let JobStateMeta {
        streamer_name,
        session_title,
        platform,
        session_start,
        manifest,
        timeout_secs,
        has_step_retry,
        enqueued_at_ms,
    } = parse_job_state(&db_job.state);

    Job {
        id: db_job.id.clone(),
        job_type: db_job.job_type.clone(),
        inputs,
        outputs,
        priority: db_job.priority,
        status,
        streamer_id: db_job.streamer_id.clone().unwrap_or_default(),
        session_id: db_job.session_id.clone().unwrap_or_default(),
        config: if db_job.config == "{}" {
            None
        } else {
            Some(db_job.config.clone())
        },
        created_at,
        started_at,
        completed_at,
        error: db_job.error.clone(),
        retry_count: db_job.retry_count,
        retry_after: db_job
            .retry_after
            .map(crate::database::time::ms_to_datetime),
        timeout_secs,
        has_step_retry,
        enqueued_at_ms,
        pipeline_id: db_job.pipeline_id.clone(),
        // Parse execution_info JSON
        execution_info: db_job
            .execution_info
            .as_ref()
            .and_then(|s| serde_json::from_str::<JobExecutionInfo>(s).ok()),
        duration_secs: db_job.duration_secs,
        queue_wait_secs: db_job.queue_wait_secs,
        dag_step_execution_id: db_job.dag_step_execution_id.clone(),
        streamer_name,
        session_title,
        platform,
        session_start,
        manifest,
    }
}
