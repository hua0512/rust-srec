//! Pipeline Manager implementation.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use super::coordination::{
    PairedSegmentOutputs, PipelineCommand, PipelineCoordinationEvent, PipelineCoordinator,
    SessionOutputs, SourceType,
};
use super::dag_scheduler::{
    DagCompletionInfo, DagCreationResult, DagExecutionMetadata, DagScheduler,
};
use super::job_queue::{Job, JobLogEntry, JobQueue, JobQueueConfig, QueueDepthStatus};
use super::processors::{
    AssBurnInProcessor, AudioExtractProcessor, CompressionProcessor, CopyMoveProcessor,
    DanmakuFactoryProcessor, DeleteProcessor, ExecuteCommandProcessor, MetadataProcessor,
    Processor, RcloneProcessor, RemuxProcessor, TdlUploadProcessor, ThumbnailProcessor,
};
use super::progress::JobProgressSnapshot;
use super::throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
use super::worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
use crate::Error;
use crate::Result;
use crate::config::ConfigService;
use crate::database::models::JobStatus;
use crate::database::models::job::{
    DagExecutionStatus, DagPipelineDefinition, DagStep, PipelineStep,
};
use crate::database::models::{
    JobFilters, MediaFileType, MediaOutputDbModel, Pagination, SessionFilters,
    SessionSegmentLifecycle, SessionSegmentSplitReason, TitleEntry,
};
use crate::database::repositories::config::{ConfigRepository, SqlxConfigRepository};
use crate::database::repositories::streamer::{SqlxStreamerRepository, StreamerRepository};
use crate::database::repositories::{
    DagRepository, JobPresetRepository, JobRepository, PipelinePresetRepository, SessionRepository,
};
use crate::downloader::{DownloadManagerEvent, DownloadProgressEvent};
use crate::utils::filename::sanitize_filename;

mod dag;
mod events;
mod jobs;
mod recovery;
mod runtime;

type BeforeRootJobsHook = Box<dyn FnOnce(&str) + Send>;

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

const SESSION_COMPLETE_TTL_SECS: u64 = 48 * 60 * 60;
const SESSION_COMPLETE_CLEANUP_INTERVAL_SECS: u64 = 10 * 60;
const DAG_COMPLETION_DEDUP_TTL_SECS: u64 = 60 * 60;

fn parse_trailing_u32(value: &str) -> Option<u32> {
    let bytes = value.as_bytes();
    let end = bytes.len();
    let mut start = end;

    while start > 0 && bytes[start - 1].is_ascii_digit() {
        start -= 1;
    }

    if start == end {
        return None;
    }

    // Safe: the slice only spans ASCII digits, which are always valid UTF-8 boundaries.
    value.get(start..end)?.parse::<u32>().ok()
}

fn parse_segment_index_from_segment_id(segment_id: &str) -> Option<u32> {
    if let Some(value) = parse_trailing_u32(segment_id) {
        return Some(value);
    }

    let stem = Path::new(segment_id)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(segment_id);
    parse_trailing_u32(stem)
}

fn parse_segment_index_from_danmu(segment_id: &str, output_path: &Path) -> Option<u32> {
    if let Ok(idx) = segment_id.parse::<u32>() {
        return Some(idx);
    }

    if let Some(stem) = output_path.file_stem().and_then(|s| s.to_str())
        && let Some(idx) = parse_trailing_u32(stem)
    {
        return Some(idx);
    }

    parse_segment_index_from_segment_id(segment_id)
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

    /// Throttle controller configuration.
    #[serde(default)]
    pub throttle: ThrottleConfig,
    /// Timeout in seconds for the `execute` processor.
    ///
    /// This is enforced inside the processor (in addition to worker pool timeouts).
    #[serde(default = "default_execute_timeout_secs")]
    pub execute_timeout_secs: u64,
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
    JobStarted { job_id: String, job_type: String },
    /// Job completed successfully.
    JobCompleted {
        job_id: String,
        job_type: String,
        duration_secs: f64,
    },
    /// Job failed.
    JobFailed {
        job_id: String,
        job_type: String,
        error: String,
    },
    /// Queue depth warning.
    QueueWarning { depth: usize },
    /// Queue depth critical.
    QueueCritical { depth: usize },
}

/// The Pipeline Manager service.
pub struct PipelineManager<
    CR: ConfigRepository + Send + Sync + 'static = SqlxConfigRepository,
    SR: StreamerRepository + Send + Sync + 'static = SqlxStreamerRepository,
> {
    /// Configuration.
    config: PipelineManagerConfig,
    /// Job queue.
    job_queue: Arc<JobQueue>,
    /// CPU worker pool.
    cpu_pool: WorkerPool,
    /// IO worker pool.
    io_pool: WorkerPool,
    /// Processors.
    processors: Vec<Arc<dyn Processor>>,
    /// Event broadcaster.
    event_tx: broadcast::Sender<PipelineEvent>,
    /// Session repository for persistence (optional).
    session_repo: Option<Arc<dyn SessionRepository>>,
    /// Streamer repository for metadata lookup (optional).
    streamer_repo: Option<Arc<SR>>,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Throttle controller for download backpressure management.
    throttle_controller: Option<Arc<ThrottleController>>,
    /// Download limit adjuster for throttle controller integration.
    download_adjuster: Option<Arc<dyn DownloadLimitAdjuster>>,
    /// Job preset repository for resolving named pipeline steps.
    preset_repo: Option<Arc<dyn JobPresetRepository>>,
    /// Pipeline preset repository for resolving workflow steps.
    pipeline_preset_repo: Option<Arc<dyn PipelinePresetRepository>>,
    /// Config service for resolving pipeline rules.
    config_service: Option<Arc<ConfigService<CR, SR>>>,
    /// Last observed queue depth status (edge-trigger warnings).
    last_queue_status: AtomicU8,

    /// Single coordinator actor/reducer for all pipeline trigger/readiness policy.
    pipeline_coordinator: PipelineCoordinator,

    /// DAG execution -> segment context mapping (for per-segment DAG completion accounting).
    dag_segment_contexts: DashMap<String, SegmentDagContext>,
    /// DAG execution -> paired-segment DAG context mapping (for gating session-complete).
    paired_dag_contexts: DashMap<String, PairedDagContext>,
    /// DAG execution IDs already processed by `handle_dag_completion` (best-effort dedupe).
    handled_dag_completions: DashMap<String, std::time::Instant>,

    /// DAG repository for DAG pipeline persistence.
    dag_repository: Option<Arc<dyn DagRepository>>,
    /// Job repository reference (needed for DAG scheduler).
    job_repository: Option<Arc<dyn JobRepository>>,
    /// DAG scheduler for orchestrating DAG pipeline execution.
    dag_scheduler: Option<Arc<DagScheduler>>,
}

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    /// Adjust CPU/IO worker pool concurrency at runtime.
    ///
    /// Notes:
    /// - This updates the *desired* concurrency only; it cannot increase beyond each pool's
    ///   `max_workers()` without restarting the pipeline manager.
    pub fn set_worker_concurrency(&self, cpu_jobs: usize, io_jobs: usize) {
        let cpu_jobs = cpu_jobs.max(1);
        let io_jobs = io_jobs.max(1);

        let cpu_max = self.cpu_pool.max_workers();
        let io_max = self.io_pool.max_workers();

        let applied_cpu = self.cpu_pool.set_desired_max_workers(cpu_jobs);
        let applied_io = self.io_pool.set_desired_max_workers(io_jobs);

        if applied_cpu != cpu_jobs {
            tracing::warn!(
                requested = cpu_jobs,
                applied = applied_cpu,
                max_workers = cpu_max,
                "CPU worker pool concurrency was clamped; restart is required to increase max_workers"
            );
        }
        if applied_io != io_jobs {
            tracing::warn!(
                requested = io_jobs,
                applied = applied_io,
                max_workers = io_max,
                "IO worker pool concurrency was clamped; restart is required to increase max_workers"
            );
        }

        tracing::info!(
            cpu_requested = cpu_jobs,
            cpu_applied = applied_cpu,
            io_requested = io_jobs,
            io_applied = applied_io,
            "Updated pipeline worker pool concurrency"
        );
    }

    /// Create a new Pipeline Manager.
    pub fn new() -> Self {
        Self::with_config(PipelineManagerConfig::default())
    }

    /// Create a new Pipeline Manager with custom configuration.
    pub fn with_config(config: PipelineManagerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_config(config.job_queue.clone()));

        let execute_timeout_secs = config.execute_timeout_secs;

        // Create default processors
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(DanmakuFactoryProcessor::new()),
            Arc::new(AssBurnInProcessor::new()),
            Arc::new(RcloneProcessor::new()),
            Arc::new(TdlUploadProcessor::new()),
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
            config,
            job_queue,
            processors,
            event_tx,
            session_repo: None,
            streamer_repo: None,
            cancellation_token: CancellationToken::new(),
            throttle_controller,
            download_adjuster: None,
            preset_repo: None,
            pipeline_preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
            pipeline_coordinator: PipelineCoordinator::new(),
            dag_segment_contexts: DashMap::new(),
            paired_dag_contexts: DashMap::new(),
            handled_dag_completions: DashMap::new(),
            dag_repository: None,
            job_repository: None,
            dag_scheduler: None,
        }
    }

    /// Create a new Pipeline Manager with custom configuration and job repository.
    /// This enables database persistence and job recovery on startup.
    pub fn with_repository(
        config: PipelineManagerConfig,
        job_repository: Arc<dyn JobRepository>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_repository(
            config.job_queue.clone(),
            job_repository.clone(),
        ));

        let execute_timeout_secs = config.execute_timeout_secs;

        // Create default processors
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(DanmakuFactoryProcessor::new()),
            Arc::new(AssBurnInProcessor::new()),
            Arc::new(RcloneProcessor::new()),
            Arc::new(TdlUploadProcessor::new()),
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
            config,
            job_queue,
            processors,
            event_tx,
            session_repo: None,
            streamer_repo: None,
            cancellation_token: CancellationToken::new(),
            throttle_controller,
            download_adjuster: None,
            preset_repo: None,
            pipeline_preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
            pipeline_coordinator: PipelineCoordinator::new(),
            dag_segment_contexts: DashMap::new(),
            paired_dag_contexts: DashMap::new(),
            handled_dag_completions: DashMap::new(),
            dag_repository: None,
            job_repository: Some(job_repository),
            dag_scheduler: None,
        }
    }

    /// Set the session repository for persistence.
    pub fn with_session_repository(
        mut self,
        session_repository: Arc<dyn SessionRepository>,
    ) -> Self {
        self.session_repo = Some(session_repository.clone());
        // Also set session repo on job queue
        self.job_queue.set_session_repo(session_repository);
        self
    }

    /// Set the streamer repository for metadata lookup.
    pub fn with_streamer_repository(mut self, streamer_repository: Arc<SR>) -> Self {
        // Also set streamer repo on job queue for metadata resolution during dequeue
        self.job_queue
            .set_streamer_repo(streamer_repository.clone() as Arc<dyn StreamerRepository>);
        self.streamer_repo = Some(streamer_repository);
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

    /// Set the config service.
    pub fn with_config_service(mut self, config_service: Arc<ConfigService<CR, SR>>) -> Self {
        self.config_service = Some(config_service);
        self
    }

    /// Set the DAG repository for DAG pipeline persistence.
    /// This also creates the DAG scheduler if job_repository is already set.
    pub fn with_dag_repository(mut self, dag_repository: Arc<dyn DagRepository>) -> Self {
        self.dag_repository = Some(dag_repository.clone());

        // Create DAG scheduler if we have both repositories
        if let Some(job_repo) = &self.job_repository {
            let scheduler =
                DagScheduler::new(self.job_queue.clone(), dag_repository, job_repo.clone());

            self.dag_scheduler = Some(Arc::new(scheduler));
        }

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

impl Default for PipelineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
