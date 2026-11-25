//! Pipeline Manager implementation.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use super::coordination::{
    PairedSegmentOutputs, PipelineCommand, PipelineCoordinationEvent, PipelineCoordinator,
    SessionOutputs, SourceType,
};
use super::dag_scheduler::{
    BeforeRootJobsHook, DagCompletionInfo, DagCreationResult, DagExecutionMetadata, DagRetryError,
    DagRunContext, DagScheduler, PublicationRollback,
};
use super::job_queue::{Job, JobLogEntry, JobQueue, JobQueueConfig, QueueDepthStatus};
use super::ports::{
    PipelineNotification, PipelineSessionPort, PipelineSessionSummary, SessionPipelineConfigPort,
};
use super::processors::{
    AssBurnInProcessor, AudioExtractProcessor, BaiduPcsProcessor, CompressionProcessor,
    CopyMoveProcessor, DanmakuFactoryProcessor, DeleteProcessor, ExecuteCommandProcessor,
    MetadataProcessor, Processor, ProcessorType, RcloneProcessor, RemuxProcessor,
    ThumbnailProcessor,
};
use super::progress::JobProgressSnapshot;
use super::throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
use super::upload_events::UploadStatusBroadcaster;
use super::worker_pool::{
    STALLED_OPERATION_THRESHOLD, StalledOperation, WorkerPool, WorkerPoolConfig, WorkerType,
};
use crate::Error;
use crate::Result;
use crate::database::models::JobStatus;
use crate::database::models::job::{
    DagExecutionStatus, DagPipelineDefinition, DagStep, PipelineStep,
};
use crate::database::models::{DagSegmentSource, JobFilters, Pagination};
use crate::database::repositories::{
    DagRepository, JobPresetRepository, JobRepository, PipelinePresetRepository,
    StagedOutputRepository, UploadRecordRepository,
};
use crate::domain::proxy::ProxyRouteResolver;

mod dag;
mod drain;
mod events;
mod inputs;
mod jobs;
mod recovery;
mod runtime;

pub use drain::{SessionDrain, SessionPipelineOutstanding, StreamerDrain};

#[derive(Debug, Clone)]
struct SegmentDagContext {
    session_id: String,
    streamer_id: String,
    segment_index: u32,
    source: SourceType,
    created_at: std::time::Instant,
}

#[derive(Debug, Clone)]
struct PairedDagContext {
    session_id: String,
    streamer_id: String,
    segment_index: u32,
    created_at: std::time::Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipelineRuntimeState {
    NotStarted,
    Running,
    Stopped,
}

struct PipelineRuntime {
    state: PipelineRuntimeState,
    tasks: JoinSet<&'static str>,
}

impl PipelineRuntime {
    fn new() -> Self {
        Self {
            state: PipelineRuntimeState::NotStarted,
            tasks: JoinSet::new(),
        }
    }
}

const SESSION_COMPLETE_TTL_SECS: u64 = 48 * 60 * 60;
const SESSION_COMPLETE_CLEANUP_INTERVAL_SECS: u64 = 10 * 60;
const DAG_COMPLETION_DEDUP_TTL_SECS: u64 = 60 * 60;
/// How often failed step jobs with a due automatic retry are re-queued.
const RETRY_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Replaces [`RETRY_SWEEP_INTERVAL`] when set.
static RETRY_SWEEP_INTERVAL_OVERRIDE: parking_lot::RwLock<Option<std::time::Duration>> =
    parking_lot::RwLock::new(None);

/// Sweep for due automatic retries every `interval` instead of every 15
/// seconds, so end-to-end tests can observe a retry without waiting for the
/// production cadence. Applies to pipeline managers started afterwards.
#[doc(hidden)]
pub fn override_retry_sweep_interval(interval: std::time::Duration) {
    *RETRY_SWEEP_INTERVAL_OVERRIDE.write() = Some(interval);
}

fn retry_sweep_interval() -> std::time::Duration {
    RETRY_SWEEP_INTERVAL_OVERRIDE
        .read()
        .unwrap_or(RETRY_SWEEP_INTERVAL)
}

/// Configuration for the Pipeline Manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineManagerConfig {
    /// Job queue configuration.
    pub job_queue: JobQueueConfig,
    /// CPU worker pool configuration.
    pub cpu_pool: WorkerPoolConfig,
    /// IO worker pool configuration.
    pub io_pool: WorkerPoolConfig,
    /// Worker count of the upload pool. The pool otherwise uses `io_pool`'s
    /// settings, including its job timeout.
    #[serde(default = "default_max_concurrent_uploads")]
    pub max_concurrent_uploads: usize,

    /// Throttle controller configuration.
    #[serde(default)]
    pub throttle: ThrottleConfig,
    /// Timeout in seconds for the `execute` processor.
    ///
    /// This is enforced inside the processor (in addition to worker pool timeouts).
    #[serde(default = "default_execute_timeout_secs")]
    pub execute_timeout_secs: u64,
}

fn default_max_concurrent_uploads() -> usize {
    3
}

fn default_execute_timeout_secs() -> u64 {
    3600
}

impl Default for PipelineManagerConfig {
    fn default() -> Self {
        Self {
            job_queue: JobQueueConfig::default(),
            cpu_pool: WorkerPoolConfig {
                max_workers: 2,
                ..Default::default()
            },
            io_pool: WorkerPoolConfig {
                max_workers: 4,
                ..Default::default()
            },
            max_concurrent_uploads: default_max_concurrent_uploads(),

            throttle: ThrottleConfig::default(),
            execute_timeout_secs: default_execute_timeout_secs(),
        }
    }
}

/// Events emitted by the Pipeline Manager.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// Job enqueued.
    JobEnqueued {
        job_id: String,
        job_type: String,
        streamer_id: String,
    },
    /// Job started processing.
    JobStarted {
        job_id: String,
        job_type: String,
        streamer_id: String,
    },
    /// Job completed successfully.
    JobCompleted {
        job_id: String,
        job_type: String,
        duration_secs: f64,
    },
    /// Job failed terminally.
    JobFailed {
        job_id: String,
        job_type: String,
        error: String,
    },
    /// Job failed an intermediate attempt and was scheduled for automatic retry.
    JobRetryScheduled { job_id: String },
    /// Queue depth reached the configured warning threshold.
    QueueWarning { depth: usize, threshold: usize },
    /// Queue depth reached the configured critical threshold.
    QueueCritical { depth: usize, threshold: usize },
}

/// The Pipeline Manager service.
pub struct PipelineManager {
    /// Job queue.
    job_queue: Arc<JobQueue>,
    /// CPU worker pool.
    cpu_pool: WorkerPool,
    /// IO worker pool.
    io_pool: WorkerPool,
    /// Pool for [`ProcessorType::Upload`] processors.
    upload_pool: WorkerPool,
    /// Processors.
    processors: Vec<Arc<dyn Processor>>,
    /// Event broadcaster.
    event_tx: broadcast::Sender<PipelineEvent>,
    /// Narrow port for session and streamer metadata/persistence (optional).
    session_port: Option<Arc<dyn PipelineSessionPort>>,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Runtime loops spawned by `start`. `stop` takes and joins this set so
    /// none of them can retain repository access after it returns.
    runtime: parking_lot::Mutex<PipelineRuntime>,
    /// Throttle controller for download backpressure management.
    throttle_controller: Option<Arc<ThrottleController>>,
    /// Download limit adjuster for throttle controller integration.
    download_adjuster: Option<Arc<dyn DownloadLimitAdjuster>>,
    /// Job preset repository for resolving named pipeline steps.
    preset_repo: Option<Arc<dyn JobPresetRepository>>,
    /// Pipeline preset repository for resolving workflow steps.
    pipeline_preset_repo: Option<Arc<dyn PipelinePresetRepository>>,
    /// Narrow port for resolving effective pipeline rules for a streamer.
    config_port: Option<Arc<dyn SessionPipelineConfigPort>>,
    /// Last observed queue depth status (edge-trigger warnings).
    last_queue_status: AtomicU8,

    /// Single coordinator reducer for all pipeline trigger/readiness policy.
    pipeline_coordinator: PipelineCoordinator,

    /// DAG execution -> segment context mapping (for per-segment DAG completion accounting).
    ///
    /// `Arc`-shared: `run_segment_pipeline` moves a handle into the `before_root_jobs` hook,
    /// and `DashMap::clone` copies entries instead of sharing them, so a bare clone would
    /// record the context in a map that dies with the closure — leaving
    /// `handle_dag_completion` with nothing to look up.
    dag_segment_contexts: Arc<DashMap<String, SegmentDagContext>>,
    /// DAG execution -> paired-segment DAG context mapping (for gating session-complete).
    ///
    /// `Arc`-shared for the same reason as `dag_segment_contexts`, via
    /// `run_paired_segment_pipeline`.
    paired_dag_contexts: Arc<DashMap<String, PairedDagContext>>,
    /// DAG execution IDs already processed by `handle_dag_completion` (best-effort dedupe).
    handled_dag_completions: DashMap<String, std::time::Instant>,

    /// DAG repository for DAG pipeline persistence.
    dag_repository: Option<Arc<dyn DagRepository>>,
    /// Job repository reference (needed for DAG scheduler).
    job_repository: Arc<dyn JobRepository>,
    /// DAG scheduler for orchestrating DAG pipeline execution.
    dag_scheduler: Option<Arc<DagScheduler>>,
}

/// Dependencies that every production pipeline runtime requires.
pub(crate) struct PipelineRuntimeDependencies {
    pub(crate) job_repository: Arc<dyn JobRepository>,
    pub(crate) session_port: Arc<dyn PipelineSessionPort>,
    pub(crate) config_port: Arc<dyn SessionPipelineConfigPort>,
    pub(crate) preset_repository: Arc<dyn JobPresetRepository>,
    pub(crate) pipeline_preset_repository: Arc<dyn PipelinePresetRepository>,
    pub(crate) dag_repository: Arc<dyn DagRepository>,
    pub(crate) upload_record_repository: Arc<dyn UploadRecordRepository>,
    pub(crate) staged_output_repository: Arc<dyn StagedOutputRepository>,
    pub(crate) upload_broadcaster: UploadStatusBroadcaster,
    pub(crate) download_limit_adjuster: Arc<dyn DownloadLimitAdjuster>,
    /// Resolves the proxy routes of upload steps.
    pub(crate) routes: Arc<dyn ProxyRouteResolver>,
}

impl PipelineManager {
    pub(crate) fn output_files_gate(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.job_queue.output_files_gate.clone()
    }

    /// Adjust CPU, IO and upload worker pool concurrency at runtime.
    ///
    /// Notes:
    /// - New admissions use the limit immediately; already-admitted jobs finish normally.
    /// - It cannot increase beyond each pool's `max_workers()` without restarting the manager.
    pub fn set_worker_concurrency(&self, cpu_jobs: usize, io_jobs: usize, uploads: usize) {
        for (pool, requested) in [
            (&self.cpu_pool, cpu_jobs),
            (&self.io_pool, io_jobs),
            (&self.upload_pool, uploads),
        ] {
            let requested = requested.max(1);
            let previous = pool.desired_max_workers();
            let applied = pool.set_desired_max_workers(requested);
            if applied != requested {
                tracing::warn!(
                    pool = %pool.worker_type(),
                    requested,
                    applied,
                    max_workers = pool.max_workers(),
                    "Worker pool concurrency was clamped; restart is required to increase max_workers"
                );
            } else if applied != previous {
                tracing::info!(
                    pool = %pool.worker_type(),
                    applied,
                    "Updated pipeline worker pool concurrency"
                );
            }
        }
    }

    /// Create a persistent pipeline manager with startup job recovery.
    pub fn with_repository(
        config: PipelineManagerConfig,
        job_repository: Arc<dyn JobRepository>,
    ) -> Self {
        Self::with_routes(config, job_repository, None)
    }

    /// [`Self::with_repository`] whose upload processors resolve step
    /// routes through `routes`.
    fn with_routes(
        config: PipelineManagerConfig,
        job_repository: Arc<dyn JobRepository>,
        routes: Option<Arc<dyn ProxyRouteResolver>>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_repository(
            config.job_queue.clone(),
            job_repository.clone(),
        ));

        let execute_timeout_secs = config.execute_timeout_secs;

        // Create default processors. Without `routes` the upload processors
        // keep the server's proxy environment.
        let mut rclone = RcloneProcessor::new();
        let mut baidupcs = BaiduPcsProcessor::new();
        if let Some(routes) = routes {
            rclone = rclone.with_routes(routes.clone());
            baidupcs = baidupcs.with_routes(routes);
        }
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(DanmakuFactoryProcessor::new()),
            Arc::new(AssBurnInProcessor::new()),
            Arc::new(rclone),
            Arc::new(baidupcs),
            Arc::new(ExecuteCommandProcessor::new().with_timeout(execute_timeout_secs)),
            Arc::new(ThumbnailProcessor::new()),
            Arc::new(CopyMoveProcessor::new()),
            Arc::new(AudioExtractProcessor::new()),
            Arc::new(CompressionProcessor::new()),
            Arc::new(MetadataProcessor::new()),
            Arc::new(DeleteProcessor::new()),
        ];

        // Create throttle controller if enabled
        let throttle_controller = if config.throttle.enabled {
            Some(Arc::new(ThrottleController::new(config.throttle.clone())))
        } else {
            None
        };

        Self {
            cpu_pool: WorkerPool::with_config(WorkerType::Cpu, config.cpu_pool.clone()),
            io_pool: WorkerPool::with_config(WorkerType::Io, config.io_pool.clone()),
            upload_pool: WorkerPool::with_config(
                WorkerType::Upload,
                WorkerPoolConfig {
                    max_workers: config.max_concurrent_uploads.max(1),
                    ..config.io_pool.clone()
                },
            ),
            job_queue,
            processors,
            event_tx,
            session_port: None,
            cancellation_token: CancellationToken::new(),
            runtime: parking_lot::Mutex::new(PipelineRuntime::new()),
            throttle_controller,
            download_adjuster: None,
            preset_repo: None,
            pipeline_preset_repo: None,
            config_port: None,
            last_queue_status: AtomicU8::new(0),
            pipeline_coordinator: PipelineCoordinator::new(),
            dag_segment_contexts: Arc::new(DashMap::new()),
            paired_dag_contexts: Arc::new(DashMap::new()),
            handled_dag_completions: DashMap::new(),
            dag_repository: None,
            job_repository,
            dag_scheduler: None,
        }
    }

    /// Create the fully wired production pipeline runtime.
    pub(crate) fn for_runtime(
        config: PipelineManagerConfig,
        dependencies: PipelineRuntimeDependencies,
    ) -> Self {
        let PipelineRuntimeDependencies {
            job_repository,
            session_port,
            config_port,
            preset_repository,
            pipeline_preset_repository,
            dag_repository,
            upload_record_repository,
            staged_output_repository,
            upload_broadcaster,
            download_limit_adjuster,
            routes,
        } = dependencies;

        Self::with_routes(config, job_repository, Some(routes))
            .with_session_port(session_port)
            .with_config_port(config_port)
            .with_preset_repository(preset_repository)
            .with_pipeline_preset_repository(pipeline_preset_repository)
            .with_dag_repository(dag_repository)
            .with_upload_record_repository(upload_record_repository)
            .with_staged_output_repository(staged_output_repository)
            .with_upload_broadcaster(upload_broadcaster)
            .with_download_adjuster(download_limit_adjuster)
    }

    /// Set the narrow session/streamer metadata port.
    pub fn with_session_port(mut self, session_port: Arc<dyn PipelineSessionPort>) -> Self {
        self.job_queue.set_session_port(session_port.clone());
        self.session_port = Some(session_port);
        self
    }

    /// Set the upload record repository for persisting per-file upload results.
    pub fn with_upload_record_repository(
        self,
        upload_record_repository: Arc<dyn UploadRecordRepository>,
    ) -> Self {
        self.job_queue.set_upload_repo(upload_record_repository);
        self
    }

    /// Set the repository that records staged temporary files.
    pub fn with_staged_output_repository(
        self,
        staged_output_repository: Arc<dyn StagedOutputRepository>,
    ) -> Self {
        self.job_queue
            .set_staged_output_repo(staged_output_repository);
        self
    }

    /// Temporary files interrupted attempts left behind, when persistence is wired.
    pub(crate) fn staged_output_repository(&self) -> Option<Arc<dyn StagedOutputRepository>> {
        self.job_queue.staged_output_repo()
    }

    /// Install the live upload-status broadcaster on the job queue.
    pub fn with_upload_broadcaster(self, broadcaster: UploadStatusBroadcaster) -> Self {
        self.job_queue.set_upload_broadcaster(broadcaster);
        self
    }

    /// Set the download limit adjuster for throttle controller integration.
    /// This connects the throttle controller to the download manager.
    pub fn with_download_adjuster(mut self, adjuster: Arc<dyn DownloadLimitAdjuster>) -> Self {
        self.download_adjuster = Some(adjuster);
        self
    }

    /// Set the job preset repository.
    pub fn with_preset_repository(mut self, preset_repo: Arc<dyn JobPresetRepository>) -> Self {
        self.preset_repo = Some(preset_repo);
        self
    }

    /// Set the pipeline preset repository (for workflow expansion).
    pub fn with_pipeline_preset_repository(
        mut self,
        pipeline_preset_repo: Arc<dyn PipelinePresetRepository>,
    ) -> Self {
        self.pipeline_preset_repo = Some(pipeline_preset_repo);
        self
    }

    /// Set the narrow session pipeline configuration port.
    pub fn with_config_port(mut self, config_port: Arc<dyn SessionPipelineConfigPort>) -> Self {
        self.config_port = Some(config_port);
        self
    }

    /// Set the DAG repository for DAG pipeline persistence.
    /// This also creates the DAG scheduler.
    pub fn with_dag_repository(mut self, dag_repository: Arc<dyn DagRepository>) -> Self {
        self.dag_repository = Some(dag_repository.clone());
        self.dag_scheduler = Some(Arc::new(DagScheduler::new(
            self.job_queue.clone(),
            dag_repository,
            self.job_repository.clone(),
        )));
        self
    }

    /// Get a reference to the DAG scheduler, if available.
    pub fn dag_scheduler(&self) -> Option<&Arc<DagScheduler>> {
        self.dag_scheduler.as_ref()
    }

    /// Get a reference to the throttle controller, if enabled.
    pub fn throttle_controller(&self) -> Option<&Arc<ThrottleController>> {
        self.throttle_controller.as_ref()
    }

    /// Subscribe to throttle events.
    /// Returns None if throttling is not enabled.
    pub fn subscribe_throttle_events(&self) -> Option<broadcast::Receiver<ThrottleEvent>> {
        self.throttle_controller.as_ref().map(|tc| tc.subscribe())
    }

    /// Check if throttling is currently active.
    pub fn is_throttled(&self) -> bool {
        self.throttle_controller
            .as_ref()
            .map(|tc| tc.is_throttled())
            .unwrap_or(false)
    }
}

/// Comprehensive pipeline statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStats {
    /// Number of pending jobs.
    pub pending: u64,
    /// Number of processing jobs.
    pub processing: u64,
    /// Number of completed jobs.
    pub completed: u64,
    /// Number of failed jobs.
    pub failed: u64,
    pub cancelled: u64,
    /// Average processing time in seconds for completed jobs.
    pub avg_processing_time_secs: Option<f64>,
    /// Current queue depth.
    pub queue_depth: usize,
    /// Current queue status.
    pub queue_status: QueueDepthStatus,
}

/// Result of creating a new pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineCreationResult {
    /// Pipeline ID (same as first job's ID).
    pub pipeline_id: String,
    /// ID of the first job in the pipeline.
    pub first_job_id: String,
    /// Type of the first job.
    pub first_job_type: String,
    /// Total number of steps in the pipeline.
    pub total_steps: usize,
    /// List of all steps in the pipeline.
    pub steps: Vec<String>,
}

/// Outcome of checking a workflow definition the way DAG creation would,
/// without creating anything. See `PipelineManager::analyze_dag_definition`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DagAnalysis {
    /// Problems creation would refuse: an unknown workflow or processor, a
    /// structural fault, or a step that removes files a sibling still reads.
    pub errors: Vec<String>,
    /// Hazards creation tolerates and logs: a step that deletes its sources
    /// through a processor option while a sibling still reads them.
    pub warnings: Vec<String>,
}

/// Outcome of retrying the failed and cancelled branches of a workflow.
#[derive(Debug)]
pub struct DagRetryResult {
    pub dag_id: String,
    pub retried_steps: usize,
    pub job_ids: Vec<String>,
    pub message: String,
}

#[cfg(test)]
mod tests;
