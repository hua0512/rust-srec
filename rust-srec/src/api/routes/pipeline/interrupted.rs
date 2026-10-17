//! Partial temporary files left behind by interrupted pipeline jobs.
//!
//! Rows come from `staged_outputs` marked at startup recovery. The listed path
//! is always the recorded one, never a caller-supplied path, and a file is only
//! deleted when its name has exactly the temporary-output shape of the output
//! it was staged for.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::{
    Json,
    extract::{FromRef, Path as RoutePath, Query, State},
};
use chrono::{DateTime, Utc};
use tracing::warn;

use super::dag::validate_batch_ids;
use super::{BatchDagItemResult, BatchDeleteOutputsResponse};
use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::database::repositories::StagedOutputRepository;
use crate::database::time::ms_to_datetime;

/// Upper bound on listed leftovers; each one is statted per request.
const LIST_LIMIT: u32 = 500;
/// A hung network share must not hang the listing; later files go unsized.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Set while a stat pass runs. One stuck on a hung share keeps its blocking
/// thread until the file system answers, so later listings skip statting
/// rather than strand another thread each.
static INSPECTING: AtomicBool = AtomicBool::new(false);
const REMOVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Dependencies of the interrupted-output endpoints. `None` when the pipeline
/// runs without persistence, which records nothing to list.
#[derive(Clone)]
pub struct InterruptedOutputRouteState {
    staged_outputs: Option<Arc<dyn StagedOutputRepository>>,
}

impl FromRef<AppState> for InterruptedOutputRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            staged_outputs: state.pipeline_manager.staged_output_repository(),
        }
    }
}

impl InterruptedOutputRouteState {
    fn repository(&self, id: &str) -> ApiResult<&Arc<dyn StagedOutputRepository>> {
        self.staged_outputs
            .as_ref()
            .ok_or_else(|| ApiError::not_found(format!("Interrupted output '{id}' not found")))
    }
}

/// Query parameters for listing interrupted outputs.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct InterruptedOutputListParams {
    /// Only list files left by this job.
    pub job_id: Option<String>,
}

/// A partial temporary file an interrupted job may have left behind.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InterruptedOutputResponse {
    pub id: String,
    /// The temporary file.
    pub path: String,
    /// The output it would have been published as.
    pub output_path: String,
    pub job_id: String,
    pub created_at: DateTime<Utc>,
    /// When startup found the job interrupted.
    pub interrupted_at: DateTime<Utc>,
    /// File size, when the file system answered in time.
    pub size_bytes: Option<u64>,
    /// Whether the file is present; `null` when its folder is unavailable, the
    /// file system did not answer in time, or an earlier check is still
    /// waiting on it.
    pub exists: Option<bool>,
}

/// Interrupted outputs, newest interruption first.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct InterruptedOutputListResponse {
    pub outputs: Vec<InterruptedOutputResponse>,
    /// Sum of the sizes that could be read.
    pub total_size_bytes: u64,
}

/// Query parameters for settling one interrupted output.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct DeleteInterruptedOutputParams {
    /// Forget the entry but leave the file on disk.
    #[serde(default)]
    pub keep_file: bool,
}

/// Result of settling one interrupted output.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DeleteInterruptedOutputResponse {
    pub id: String,
    /// Whether a file was removed; `false` when it was kept or already gone.
    pub file_deleted: bool,
}

/// Request to settle several interrupted outputs.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct BatchDeleteInterruptedOutputsRequest {
    /// Unique interrupted output IDs.
    pub ids: Vec<String>,
    /// Forget the entries but leave the files on disk.
    #[serde(default)]
    pub keep_files: bool,
}

#[derive(Debug, Clone, Copy)]
enum FileState {
    Present(u64),
    /// The file is gone but its folder is reachable.
    Missing,
    /// The folder is missing (an unmounted drive, for example), the path is not
    /// a regular file, or the file system returned an error.
    Unavailable,
}

/// Stat `paths` on one blocking thread. A hung share then holds one thread, and
/// the files after it are reported as unknown once the deadline passes.
async fn inspect(paths: Vec<PathBuf>) -> Vec<Option<FileState>> {
    inspect_unless_running(paths, &INSPECTING).await
}

/// [`inspect`], except that every state is unknown while an earlier pass
/// flagged in `running` has not returned.
async fn inspect_unless_running(
    paths: Vec<PathBuf>,
    running: &'static AtomicBool,
) -> Vec<Option<FileState>> {
    if running.swap(true, Ordering::AcqRel) {
        return vec![None; paths.len()];
    }
    // Clears the flag when the pass returns, or when it never runs because the
    // runtime is shutting down.
    let pass = RunningPass(running);
    let results = Arc::new(parking_lot::Mutex::new(vec![None; paths.len()]));
    let shared = results.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _pass = pass;
        for (index, path) in paths.iter().enumerate() {
            let state = match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() => FileState::Present(metadata.len()),
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && path.parent().is_some_and(Path::is_dir) =>
                {
                    FileState::Missing
                }
                _ => FileState::Unavailable,
            };
            shared.lock()[index] = Some(state);
        }
    });
    match tokio::time::timeout(INSPECT_TIMEOUT, task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!(%error, "Inspecting interrupted outputs failed"),
        Err(_) => warn!(
            timeout_secs = INSPECT_TIMEOUT.as_secs(),
            "The file system did not answer while listing interrupted outputs"
        ),
    }
    results.lock().clone()
}

struct RunningPass(&'static AtomicBool);

impl Drop for RunningPass {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// List partial temporary files left by interrupted jobs.
///
/// A file confirmed gone from a reachable folder (removed by hand, or cleaned up
/// after the row was written) is forgotten here rather than listed.
#[utoipa::path(
    get,
    path = "/api/pipeline/interrupted-outputs",
    tag = "pipeline",
    params(InterruptedOutputListParams),
    responses(
        (status = 200, description = "Partial files left by interrupted jobs", body = InterruptedOutputListResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_interrupted_outputs(
    State(state): State<InterruptedOutputRouteState>,
    Query(params): Query<InterruptedOutputListParams>,
) -> ApiResult<Json<InterruptedOutputListResponse>> {
    let Some(repository) = &state.staged_outputs else {
        return Ok(Json(InterruptedOutputListResponse {
            outputs: Vec::new(),
            total_size_bytes: 0,
        }));
    };
    let rows = repository
        .list_interrupted(params.job_id.as_deref(), LIST_LIMIT)
        .await
        .map_err(ApiError::from)?;
    let states = inspect(rows.iter().map(|row| PathBuf::from(&row.path)).collect()).await;

    let mut outputs = Vec::with_capacity(rows.len());
    let mut total_size_bytes = 0u64;
    for (row, state) in rows.into_iter().zip(states) {
        let (size_bytes, exists) = match state {
            Some(FileState::Present(size)) => (Some(size), Some(true)),
            Some(FileState::Missing) => match repository.forget_interrupted(&row.id).await {
                Ok(_) => continue,
                Err(error) => {
                    warn!(id = %row.id, %error, "Failed to forget a vanished interrupted output");
                    (None, Some(false))
                }
            },
            Some(FileState::Unavailable) | None => (None, None),
        };
        total_size_bytes += size_bytes.unwrap_or(0);
        outputs.push(InterruptedOutputResponse {
            id: row.id,
            path: row.path,
            output_path: row.output_path,
            job_id: row.job_id,
            created_at: ms_to_datetime(row.created_at),
            interrupted_at: ms_to_datetime(row.interrupted_at.unwrap_or(row.created_at)),
            size_bytes,
            exists,
        });
    }

    Ok(Json(InterruptedOutputListResponse {
        outputs,
        total_size_bytes,
    }))
}

/// Delete or keep one interrupted output, then forget its entry. The file is
/// removed first, so a failure leaves the entry to retry or keep.
async fn settle(state: &InterruptedOutputRouteState, id: &str, keep_file: bool) -> ApiResult<bool> {
    let repository = state.repository(id)?;
    let row = repository
        .get_interrupted(id)
        .await
        .map_err(ApiError::from)?;

    let mut file_deleted = false;
    if !keep_file {
        let path = PathBuf::from(&row.path);
        if !crate::pipeline::is_temp_path_for(&path, Path::new(&row.output_path)) {
            return Err(ApiError::validation(
                "Refusing to delete a file that is not a staged temporary output; keep it instead",
            ));
        }
        let removal = tokio::task::spawn_blocking({
            let path = path.clone();
            move || crate::pipeline::remove_leftover(&path)
        });
        file_deleted = match tokio::time::timeout(REMOVE_TIMEOUT, removal).await {
            Ok(Ok(Ok(deleted))) => deleted,
            Ok(Ok(Err(error))) if error.kind() == std::io::ErrorKind::InvalidInput => {
                return Err(ApiError::validation(
                    "Refusing to delete a path that is not a regular file; keep it instead",
                ));
            }
            Ok(Ok(Err(error))) => {
                return Err(ApiError::from(crate::Error::io_path(
                    "remove_file",
                    &path,
                    error,
                )));
            }
            Ok(Err(error)) => {
                return Err(ApiError::internal(format!("File removal failed: {error}")));
            }
            Err(_) => {
                return Err(ApiError::service_unavailable(
                    "The file system did not respond; try again later",
                ));
            }
        };
    }

    repository
        .forget_interrupted(id)
        .await
        .map_err(ApiError::from)?;
    Ok(file_deleted)
}

/// Delete one interrupted output's file, or keep it, and forget the entry.
#[utoipa::path(
    delete,
    path = "/api/pipeline/interrupted-outputs/{id}",
    tag = "pipeline",
    params(
        ("id" = String, Path, description = "Interrupted output ID"),
        DeleteInterruptedOutputParams
    ),
    responses(
        (status = 200, description = "Interrupted output settled", body = DeleteInterruptedOutputResponse),
        (status = 404, description = "Interrupted output not found", body = crate::api::error::ApiErrorResponse),
        (status = 422, description = "The recorded path is not a staged temporary file", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_interrupted_output(
    State(state): State<InterruptedOutputRouteState>,
    RoutePath(id): RoutePath<String>,
    Query(params): Query<DeleteInterruptedOutputParams>,
) -> ApiResult<Json<DeleteInterruptedOutputResponse>> {
    let file_deleted = settle(&state, &id, params.keep_file).await?;
    Ok(Json(DeleteInterruptedOutputResponse { id, file_deleted }))
}

/// Settle several interrupted outputs, reporting a result per ID. Entries are
/// settled one by one and never rolled back.
#[utoipa::path(
    post,
    path = "/api/pipeline/interrupted-outputs/batch-delete",
    tag = "pipeline",
    request_body = BatchDeleteInterruptedOutputsRequest,
    responses(
        (status = 200, description = "Per-entry results", body = BatchDeleteOutputsResponse),
        (status = 422, description = "Invalid batch request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn batch_delete_interrupted_outputs(
    State(state): State<InterruptedOutputRouteState>,
    Json(request): Json<BatchDeleteInterruptedOutputsRequest>,
) -> ApiResult<Json<BatchDeleteOutputsResponse>> {
    validate_batch_ids(&request.ids, "interrupted output")?;

    let requested = request.ids.len();
    let mut results = Vec::with_capacity(requested);
    for id in request.ids {
        match settle(&state, &id, request.keep_files).await {
            Ok(_) => results.push(BatchDagItemResult {
                id,
                success: true,
                code: None,
                error: None,
            }),
            Err(error) => {
                warn!(id = %id, error_code = %error.code, "Settling an interrupted output failed");
                results.push(BatchDagItemResult {
                    id,
                    success: false,
                    code: Some(error.code),
                    error: Some(error.message),
                });
            }
        }
    }

    let succeeded = results.iter().filter(|result| result.success).count();
    Ok(Json(BatchDeleteOutputsResponse {
        requested,
        succeeded,
        failed: requested - succeeded,
        results,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::{NewStagedOutput, SqlxStagedOutputRepository};
    use crate::database::{init_pool_with_size, run_migrations};

    async fn state() -> (InterruptedOutputRouteState, Arc<SqlxStagedOutputRepository>) {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repository = Arc::new(SqlxStagedOutputRepository::new(pool.clone(), pool));
        (
            InterruptedOutputRouteState {
                staged_outputs: Some(repository.clone()),
            },
            repository,
        )
    }

    /// Record `path` as an interrupted staging of `output` and return its ID.
    async fn interrupted(
        repository: &SqlxStagedOutputRepository,
        path: &Path,
        output: &Path,
    ) -> String {
        let (path, output) = (path.to_string_lossy(), output.to_string_lossy());
        repository
            .record(
                NewStagedOutput {
                    path: &path,
                    output_path: &output,
                    job_id: "job-1",
                    attempt_id: "attempt-1",
                },
                1_000,
            )
            .await
            .unwrap();
        repository.mark_interrupted(2_000).await.unwrap();
        repository
            .list_interrupted(None, 100)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.path == path)
            .unwrap()
            .id
    }

    async fn remaining(repository: &SqlxStagedOutputRepository) -> usize {
        repository.list_interrupted(None, 100).await.unwrap().len()
    }

    #[tokio::test]
    async fn deleting_removes_only_a_temporary_file_and_its_entry() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let temp = crate::pipeline::temp_path_for(&output);
        std::fs::write(&temp, b"partial").unwrap();
        let id = interrupted(&repository, &temp, &output).await;

        let response = delete_interrupted_output(
            State(state),
            RoutePath(id.clone()),
            Query(DeleteInterruptedOutputParams::default()),
        )
        .await
        .unwrap();

        assert!(response.file_deleted);
        assert!(!temp.exists());
        assert_eq!(remaining(&repository).await, 0);
    }

    #[tokio::test]
    async fn deleting_refuses_a_path_without_the_temporary_shape() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let recording = directory.path().join("video.flv");
        std::fs::write(&recording, b"keep").unwrap();
        let id = interrupted(&repository, &recording, &output).await;

        let error = delete_interrupted_output(
            State(state.clone()),
            RoutePath(id.clone()),
            Query(DeleteInterruptedOutputParams::default()),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(recording.exists());
        assert_eq!(remaining(&repository).await, 1);

        // Keeping the file is still allowed and only forgets the entry.
        let kept = delete_interrupted_output(
            State(state),
            RoutePath(id),
            Query(DeleteInterruptedOutputParams { keep_file: true }),
        )
        .await
        .unwrap();
        assert!(!kept.file_deleted);
        assert!(recording.exists());
        assert_eq!(remaining(&repository).await, 0);
    }

    #[tokio::test]
    async fn a_file_that_is_already_gone_counts_as_settled() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let temp = crate::pipeline::temp_path_for(&output);
        let id = interrupted(&repository, &temp, &output).await;

        let response = delete_interrupted_output(
            State(state),
            RoutePath(id),
            Query(DeleteInterruptedOutputParams::default()),
        )
        .await
        .unwrap();

        assert!(!response.file_deleted);
        assert_eq!(remaining(&repository).await, 0);
    }

    #[tokio::test]
    async fn keeping_a_file_forgets_only_the_entry() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let temp = crate::pipeline::temp_path_for(&output);
        std::fs::write(&temp, b"partial").unwrap();
        let id = interrupted(&repository, &temp, &output).await;

        let response = delete_interrupted_output(
            State(state),
            RoutePath(id),
            Query(DeleteInterruptedOutputParams { keep_file: true }),
        )
        .await
        .unwrap();

        assert!(!response.file_deleted);
        assert!(temp.exists());
        assert_eq!(remaining(&repository).await, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn deleting_refuses_a_symlink_with_the_temporary_name() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let target = directory.path().join("recording.mp4");
        std::fs::write(&target, b"keep").unwrap();
        let temp = crate::pipeline::temp_path_for(&output);
        std::os::unix::fs::symlink(&target, &temp).unwrap();
        let id = interrupted(&repository, &temp, &output).await;

        let error = delete_interrupted_output(
            State(state),
            RoutePath(id),
            Query(DeleteInterruptedOutputParams::default()),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert!(temp.exists() && target.exists());
        assert_eq!(remaining(&repository).await, 1);
    }

    #[tokio::test]
    async fn listing_reports_sizes_and_forgets_vanished_files() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let present = crate::pipeline::temp_path_for(&output);
        std::fs::write(&present, vec![0u8; 7]).unwrap();
        let vanished = crate::pipeline::temp_path_for(&output);
        let unmounted = directory.path().join("unplugged").join("clip.tmp-x.mp4");
        let present_id = interrupted(&repository, &present, &output).await;
        interrupted(&repository, &vanished, &output).await;
        let unmounted_id = interrupted(
            &repository,
            &unmounted,
            &directory.path().join("unplugged").join("clip.mp4"),
        )
        .await;

        let listed = list_interrupted_outputs(
            State(state.clone()),
            Query(InterruptedOutputListParams::default()),
        )
        .await
        .unwrap();

        let mut found: Vec<_> = listed
            .outputs
            .iter()
            .map(|output| (output.id.clone(), output.size_bytes, output.exists))
            .collect();
        found.sort();
        let mut expected = vec![
            (present_id, Some(7), Some(true)),
            (unmounted_id, None, None),
        ];
        expected.sort();
        assert_eq!(found, expected);
        assert_eq!(listed.total_size_bytes, 7);
        // The vanished file's entry is gone; the unreachable folder's stays.
        assert_eq!(remaining(&repository).await, 2);

        let other_job = list_interrupted_outputs(
            State(state),
            Query(InterruptedOutputListParams {
                job_id: Some("job-2".into()),
            }),
        )
        .await
        .unwrap();
        assert!(other_job.outputs.is_empty());
    }

    #[tokio::test]
    async fn listing_skips_statting_while_an_earlier_pass_is_still_running() {
        static RUNNING: AtomicBool = AtomicBool::new(false);
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("video.tmp-x.mp4");
        std::fs::write(&file, b"partial").unwrap();

        // A pass stuck on a hung share still holds the flag.
        RUNNING.store(true, Ordering::SeqCst);
        let skipped = inspect_unless_running(vec![file.clone()], &RUNNING).await;
        assert!(matches!(skipped.as_slice(), [None]));
        assert!(
            RUNNING.load(Ordering::SeqCst),
            "a skipped pass must not clear the flag"
        );

        RUNNING.store(false, Ordering::SeqCst);
        let states = inspect_unless_running(vec![file], &RUNNING).await;
        assert!(matches!(states.as_slice(), [Some(FileState::Present(7))]));
        assert!(!RUNNING.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn batch_settling_reports_each_entry() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let temp = crate::pipeline::temp_path_for(&output);
        std::fs::write(&temp, b"partial").unwrap();
        let id = interrupted(&repository, &temp, &output).await;

        let response = batch_delete_interrupted_outputs(
            State(state.clone()),
            Json(BatchDeleteInterruptedOutputsRequest {
                ids: vec![id, "missing".into()],
                keep_files: false,
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            (response.requested, response.succeeded, response.failed),
            (2, 1, 1)
        );
        assert_eq!(response.results[1].code.as_deref(), Some("NOT_FOUND"));
        assert!(!temp.exists());

        let invalid = batch_delete_interrupted_outputs(
            State(state),
            Json(BatchDeleteInterruptedOutputsRequest {
                ids: Vec::new(),
                keep_files: false,
            }),
        )
        .await;
        assert!(invalid.is_err());
    }
}
