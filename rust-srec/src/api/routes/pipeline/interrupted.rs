//! Partial temporary files left behind by interrupted pipeline jobs.
//!
//! Rows come from `staged_outputs` marked at startup recovery. The listed path
//! is always the recorded one, never a caller-supplied path, and a file is only
//! deleted when its name has exactly the temporary-output shape of the output
//! it was staged for.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
const REMOVE_TIMEOUT: Duration = Duration::from_secs(10);

/// A blocking file-system call that outlives its deadline keeps its thread
/// until a hung share answers. These stop later requests from stranding
/// another thread each meanwhile.
struct FileSystemCalls {
    /// Set while a stat pass runs; later listings skip statting.
    inspecting: AtomicBool,
    /// Removals that timed out and have not returned; later removals fail at
    /// once.
    stuck_removals: AtomicUsize,
}

impl FileSystemCalls {
    const fn new() -> Self {
        Self {
            inspecting: AtomicBool::new(false),
            stuck_removals: AtomicUsize::new(0),
        }
    }
}

static FILE_SYSTEM_CALLS: FileSystemCalls = FileSystemCalls::new();

/// Dependencies of the interrupted-output endpoints. `None` when the pipeline
/// runs without persistence, which records nothing to list.
#[derive(Clone)]
pub struct InterruptedOutputRouteState {
    staged_outputs: Option<Arc<dyn StagedOutputRepository>>,
    file_system: &'static FileSystemCalls,
}

impl FromRef<AppState> for InterruptedOutputRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            staged_outputs: state.pipeline_manager.staged_output_repository(),
            file_system: &FILE_SYSTEM_CALLS,
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
/// the files after it are reported as unknown once the deadline passes. Every
/// state is unknown while an earlier pass flagged in `running` has not
/// returned.
async fn inspect(paths: Vec<PathBuf>, running: &'static AtomicBool) -> Vec<Option<FileState>> {
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

/// Run `remove` on a blocking thread for at most `deadline`. A removal that
/// times out stays counted in `stuck` until its thread returns, and later
/// removals fail at once meanwhile, so retries and batches on a hung share do
/// not strand a thread each.
async fn remove_unless_stuck(
    stuck: &'static AtomicUsize,
    deadline: Duration,
    remove: impl FnOnce() -> std::io::Result<bool> + Send + 'static,
) -> ApiResult<std::io::Result<bool>> {
    if stuck.load(Ordering::Acquire) > 0 {
        return Err(ApiError::service_unavailable(
            "An earlier deletion is still waiting on the file system; try again later",
        ));
    }
    let mut removal = tokio::task::spawn_blocking(remove);
    match tokio::time::timeout(deadline, &mut removal).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(ApiError::internal(format!("File removal failed: {error}"))),
        Err(_) => {
            stuck.fetch_add(1, Ordering::AcqRel);
            tokio::spawn(async move {
                // The entry stays listed; if the file is gone by the time the
                // share answers, the next listing forgets it.
                match removal.await {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => warn!(%error, "A timed-out file removal failed"),
                    Err(error) => warn!(%error, "A timed-out file removal panicked"),
                }
                stuck.fetch_sub(1, Ordering::AcqRel);
            });
            Err(ApiError::service_unavailable(
                "The file system did not respond; try again later",
            ))
        }
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
    let states = inspect(
        rows.iter().map(|row| PathBuf::from(&row.path)).collect(),
        &state.file_system.inspecting,
    )
    .await;

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
        let removal = remove_unless_stuck(&state.file_system.stuck_removals, REMOVE_TIMEOUT, {
            let path = path.clone();
            move || crate::pipeline::remove_leftover(&path)
        })
        .await?;
        file_deleted = match removal {
            Ok(deleted) => deleted,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                return Err(ApiError::validation(
                    "Refusing to delete a path that is not a regular file; keep it instead",
                ));
            }
            Err(error) => {
                return Err(ApiError::from(crate::Error::io_path(
                    "remove_file",
                    &path,
                    error,
                )));
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
        (status = 422, description = "The recorded path is not a staged temporary file", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "The file system did not respond, or an earlier deletion is still waiting on it", body = crate::api::error::ApiErrorResponse)
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
                // Each test gets its own guards.
                file_system: Box::leak(Box::new(FileSystemCalls::new())),
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
    async fn listing_skips_statting_while_an_earlier_pass_is_still_running() {
        static RUNNING: AtomicBool = AtomicBool::new(false);
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("video.tmp-x.mp4");
        std::fs::write(&file, b"partial").unwrap();

        // A pass stuck on a hung share still holds the flag.
        RUNNING.store(true, Ordering::SeqCst);
        let skipped = inspect(vec![file.clone()], &RUNNING).await;
        assert!(matches!(skipped.as_slice(), [None]));
        assert!(
            RUNNING.load(Ordering::SeqCst),
            "a skipped pass must not clear the flag"
        );

        RUNNING.store(false, Ordering::SeqCst);
        let states = inspect(vec![file], &RUNNING).await;
        assert!(matches!(states.as_slice(), [Some(FileState::Present(7))]));
        assert!(!RUNNING.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_removal_that_times_out_blocks_later_ones_until_it_returns() {
        static STUCK: AtomicUsize = AtomicUsize::new(0);
        let (release, released) = std::sync::mpsc::channel::<()>();

        // The share stops answering: this removal outlives its deadline.
        let error = remove_unless_stuck(&STUCK, Duration::from_millis(50), move || {
            released.recv().ok();
            Ok(true)
        })
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(STUCK.load(Ordering::SeqCst), 1);

        // Later removals fail at once instead of starting another thread.
        let started = Arc::new(AtomicBool::new(false));
        let error = remove_unless_stuck(&STUCK, Duration::from_secs(5), {
            let started = started.clone();
            move || {
                started.store(true, Ordering::SeqCst);
                Ok(true)
            }
        })
        .await
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(!started.load(Ordering::SeqCst));

        // Once the share answers, removals run again.
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while STUCK.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let removed = remove_unless_stuck(&STUCK, Duration::from_secs(5), || Ok(true))
            .await
            .unwrap()
            .unwrap();
        assert!(removed);
    }

    #[tokio::test]
    async fn a_stuck_removal_still_allows_keeping_files() {
        let (state, repository) = state().await;
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("video.mp4");
        let temp = crate::pipeline::temp_path_for(&output);
        std::fs::write(&temp, b"partial").unwrap();
        let id = interrupted(&repository, &temp, &output).await;
        state.file_system.stuck_removals.store(1, Ordering::SeqCst);

        let response = batch_delete_interrupted_outputs(
            State(state.clone()),
            Json(BatchDeleteInterruptedOutputsRequest {
                ids: vec![id.clone()],
                keep_files: false,
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.failed, 1);
        assert_eq!(
            response.results[0].code.as_deref(),
            Some("SERVICE_UNAVAILABLE")
        );
        assert!(temp.exists());
        assert_eq!(remaining(&repository).await, 1);

        // Keeping a file does not touch the file system.
        let kept = delete_interrupted_output(
            State(state),
            RoutePath(id),
            Query(DeleteInterruptedOutputParams { keep_file: true }),
        )
        .await
        .unwrap();
        assert!(!kept.file_deleted);
        assert_eq!(remaining(&repository).await, 0);
    }
}
