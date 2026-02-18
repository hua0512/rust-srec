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
    PairedSegmentCoordinator, PairedSegmentOutputs, SessionCompleteCoordinator, SessionOutputs,
    SourceType,
};
use super::dag_scheduler::{
    DagCompletionInfo, DagCreationResult, DagExecutionMetadata, DagScheduler,
};
use super::job_queue::{Job, JobLogEntry, JobQueue, JobQueueConfig, JobStatus, QueueDepthStatus};
use super::processors::{
    AssBurnInProcessor, AudioExtractProcessor, CompressionProcessor, CopyMoveProcessor,
    DanmakuFactoryProcessor, DeleteProcessor, ExecuteCommandProcessor, MetadataProcessor,
    Processor, RcloneProcessor, RemuxProcessor, TdlUploadProcessor, ThumbnailProcessor,
};
use super::progress::JobProgressSnapshot;
use super::purge::{JobPurgeService, PurgeConfig};
use super::throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
use super::worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
use crate::Error;
use crate::Result;
use crate::config::ConfigService;
use crate::database::models::job::{DagPipelineDefinition, DagStep, PipelineStep};
use crate::database::models::{
    JobFilters, MediaFileType, MediaOutputDbModel, Pagination, TitleEntry,
};
use crate::database::repositories::config::{ConfigRepository, SqlxConfigRepository};
use crate::database::repositories::streamer::{SqlxStreamerRepository, StreamerRepository};
use crate::database::repositories::{
    DagRepository, JobPresetRepository, JobRepository, PipelinePresetRepository, SessionRepository,
};
use crate::downloader::DownloadManagerEvent;
use crate::utils::filename::sanitize_filename;

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

#[derive(Debug, Clone)]
struct SessionCompletePipelineEntry {
    last_seen: std::time::Instant,
    definition: DagPipelineDefinition,
}

#[derive(Debug, Clone)]
struct PairedSegmentPipelineEntry {
    last_seen: std::time::Instant,
    definition: DagPipelineDefinition,
}

const SESSION_COMPLETE_TTL_SECS: u64 = 48 * 60 * 60;
const SESSION_COMPLETE_CLEANUP_INTERVAL_SECS: u64 = 10 * 60;
// When session-complete triggering is gated on DB `sessions.end_time`, it's possible for
// the "all outputs complete" condition to become true before the DB end_time is persisted.
// A periodic retry prevents the session-complete pipeline from getting stuck waiting for
// an event that may never re-run `try_trigger_session_complete`.
const SESSION_COMPLETE_RETRY_INTERVAL_SECS: u64 = 5;
const PAIRED_SEGMENT_TTL_SECS: u64 = 6 * 60 * 60;
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
    /// Job purge service configuration.
    #[serde(default)]
    pub purge: PurgeConfig,

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
            purge: PurgeConfig::default(),
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
    /// Job purge service for automatic cleanup of old jobs.
    purge_service: Option<Arc<JobPurgeService>>,
    /// Job preset repository for resolving named pipeline steps.
    preset_repo: Option<Arc<dyn JobPresetRepository>>,
    /// Pipeline preset repository for resolving workflow steps.
    pipeline_preset_repo: Option<Arc<dyn PipelinePresetRepository>>,
    /// Config service for resolving pipeline rules.
    config_service: Option<Arc<ConfigService<CR, SR>>>,
    /// Last observed queue depth status (edge-trigger warnings).
    last_queue_status: AtomicU8,

    /// Session-complete pipeline coordinator.
    session_complete_coordinator: Arc<SessionCompleteCoordinator>,
    /// Session -> session-complete pipeline definition (captured at runtime).
    session_complete_pipelines: DashMap<String, SessionCompletePipelineEntry>,

    /// Paired segment (video+danmu) coordinator.
    paired_segment_coordinator: Arc<PairedSegmentCoordinator>,
    /// Session -> paired-segment pipeline definition (captured at runtime).
    paired_segment_pipelines: DashMap<String, PairedSegmentPipelineEntry>,

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
            purge_service: None,
            preset_repo: None,
            pipeline_preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
            session_complete_coordinator: Arc::new(SessionCompleteCoordinator::new()),
            session_complete_pipelines: DashMap::new(),
            paired_segment_coordinator: Arc::new(PairedSegmentCoordinator::new()),
            paired_segment_pipelines: DashMap::new(),
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

        // Create purge service if retention is enabled
        let purge_service = if config.purge.retention_days > 0 {
            Some(Arc::new(JobPurgeService::new(
                config.purge.clone(),
                job_repository.clone(),
            )))
        } else {
            None
        };

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
            purge_service,
            preset_repo: None,
            pipeline_preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
            session_complete_coordinator: Arc::new(SessionCompleteCoordinator::new()),
            session_complete_pipelines: DashMap::new(),
            paired_segment_coordinator: Arc::new(PairedSegmentCoordinator::new()),
            paired_segment_pipelines: DashMap::new(),
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

    /// Get a reference to the purge service, if enabled.
    pub fn purge_service(&self) -> Option<&Arc<JobPurgeService>> {
        self.purge_service.as_ref()
    }

    /// Recover jobs from database on startup.
    /// Resets PROCESSING jobs to PENDING for re-execution.
    /// For sequential pipelines, no special handling is needed since only one job
    /// per pipeline exists at a time.
    pub async fn recover_jobs(&self) -> Result<usize> {
        info!("Recovering jobs from database...");
        let recovered = self.job_queue.recover_jobs().await?;
        if recovered > 0 {
            info!("Recovered {} jobs from database", recovered);
        } else {
            debug!("No jobs to recover from database");
        }
        Ok(recovered)
    }

    /// Start the pipeline manager.
    pub fn start(self: Arc<Self>) {
        info!("Starting Pipeline Manager");

        // Get CPU and IO processors
        let cpu_processors: Vec<Arc<dyn Processor>> = self
            .processors
            .iter()
            .filter(|p| p.processor_type() == super::processors::ProcessorType::Cpu)
            .cloned()
            .collect();

        info!(
            "Starting CPU pool with processors: {:?}",
            cpu_processors.iter().map(|p| p.name()).collect::<Vec<_>>()
        );

        let io_processors: Vec<Arc<dyn Processor>> = self
            .processors
            .iter()
            .filter(|p| p.processor_type() == super::processors::ProcessorType::Io)
            .cloned()
            .collect();

        info!(
            "Starting IO pool with processors: {:?}",
            io_processors.iter().map(|p| p.name()).collect::<Vec<_>>()
        );

        // Use a bounded channel for DAG completion notifications to avoid unbounded memory growth
        // if completions outpace handling (apply backpressure instead).
        let (dag_notify_tx, mut dag_notify_rx) = mpsc::channel::<DagCompletionInfo>(1024);
        let manager = self.clone();
        tokio::spawn(async move {
            while let Some(completion) = dag_notify_rx.recv().await {
                manager.handle_dag_completion(completion).await;
            }
        });

        let cleanup_manager = self.clone();
        let cleanup_token = self.cancellation_token.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(SESSION_COMPLETE_CLEANUP_INTERVAL_SECS);
            loop {
                tokio::select! {
                    _ = cleanup_token.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {
                        cleanup_manager.session_complete_coordinator.cleanup_stale(SESSION_COMPLETE_TTL_SECS);
                        cleanup_manager.paired_segment_coordinator.cleanup_stale(PAIRED_SEGMENT_TTL_SECS);

                        let now = std::time::Instant::now();
                        cleanup_manager.session_complete_pipelines.retain(|session_id, entry| {
                            if now.duration_since(entry.last_seen).as_secs() > SESSION_COMPLETE_TTL_SECS {
                                warn!(session_id = %session_id, "Removing stale session_complete_pipeline entry");
                                false
                            } else {
                                true
                            }
                        });

                        cleanup_manager.paired_segment_pipelines.retain(|session_id, entry| {
                            if now.duration_since(entry.last_seen).as_secs() > PAIRED_SEGMENT_TTL_SECS {
                                warn!(session_id = %session_id, "Removing stale paired_segment_pipeline entry");
                                false
                            } else {
                                true
                            }
                        });

                        cleanup_manager.dag_segment_contexts.retain(|dag_id, ctx| {
                            if now.duration_since(ctx.created_at).as_secs() > SESSION_COMPLETE_TTL_SECS {
                                warn!(dag_id = %dag_id, session_id = %ctx.session_id, "Removing stale per-segment DAG context");
                                false
                            } else {
                                true
                            }
                        });

                        cleanup_manager.paired_dag_contexts.retain(|dag_id, ctx| {
                            if now.duration_since(ctx.created_at).as_secs() > SESSION_COMPLETE_TTL_SECS {
                                warn!(
                                    dag_id = %dag_id,
                                    session_id = %ctx.session_id,
                                    streamer_id = %ctx.streamer_id,
                                    segment_index = %ctx.segment_index,
                                    "Removing stale paired-segment DAG context"
                                );
                                false
                            } else {
                                true
                            }
                        });

                        cleanup_manager.handled_dag_completions.retain(|_, ts| {
                            now.duration_since(*ts).as_secs() <= DAG_COMPLETION_DEDUP_TTL_SECS
                        });
                    }
                }
            }
        });

        // Periodically retry session-complete triggering for sessions that are already ready
        // (outputs collected + segment/paired DAGs done) but are still waiting for DB end_time.
        let retry_manager = self.clone();
        let retry_token = self.cancellation_token.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(SESSION_COMPLETE_RETRY_INTERVAL_SECS);
            loop {
                tokio::select! {
                    _ = retry_token.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {
                        let ready_sessions: Vec<String> = retry_manager
                            .session_complete_pipelines
                            .iter()
                            .filter_map(|entry| {
                                let session_id = entry.key().clone();
                                retry_manager
                                    .session_complete_coordinator
                                    .is_ready_nonempty(&session_id)
                                    .then_some(session_id)
                            })
                            .collect();

                        for session_id in ready_sessions {
                            retry_manager.try_trigger_session_complete(&session_id).await;
                        }
                    }
                }
            }
        });

        // Start worker pools with optional DAG scheduler
        self.cpu_pool.start_with_dag_scheduler(
            self.job_queue.clone(),
            cpu_processors,
            self.dag_scheduler.clone(),
            Some(dag_notify_tx.clone()),
        );
        self.io_pool.start_with_dag_scheduler(
            self.job_queue.clone(),
            io_processors,
            self.dag_scheduler.clone(),
            Some(dag_notify_tx),
        );

        // Start throttle controller monitoring if enabled and adjuster is set
        if let Some(throttle_controller) = &self.throttle_controller
            && let Some(adjuster) = &self.download_adjuster
            && throttle_controller.is_enabled()
        {
            info!("Starting throttle controller monitoring");
            throttle_controller.clone().start_monitoring(
                self.job_queue.clone(),
                adjuster.clone(),
                self.cancellation_token.clone(),
            );
        }

        // Start purge service background task if enabled
        if let Some(purge_service) = &self.purge_service {
            info!("Starting job purge service");
            purge_service.start_background_task(self.cancellation_token.clone());
        }

        info!("Pipeline Manager started");
    }

    async fn handle_dag_completion(&self, completion: DagCompletionInfo) {
        let dag_id = completion.dag_id.clone();

        debug!(
            dag_id = %completion.dag_id,
            streamer_id = ?completion.streamer_id,
            session_id = ?completion.session_id,
            succeeded = completion.succeeded,
            leaf_outputs = %completion.leaf_outputs.len(),
            "DAG completion received"
        );

        if let Some(session_id) = completion.session_id.as_deref()
            && let Some(mut entry) = self.session_complete_pipelines.get_mut(session_id)
        {
            entry.last_seen = std::time::Instant::now();
        }
        if let Some(session_id) = completion.session_id.as_deref()
            && let Some(mut entry) = self.paired_segment_pipelines.get_mut(session_id)
        {
            entry.last_seen = std::time::Instant::now();
        }

        if self
            .handled_dag_completions
            .insert(dag_id.clone(), std::time::Instant::now())
            .is_some()
        {
            trace!(dag_id = %dag_id, "Ignoring duplicate DAG completion");
            return;
        }

        if let Some((_, ctx)) = self.dag_segment_contexts.remove(&dag_id) {
            if let Some(session_id) = completion.session_id.as_deref()
                && session_id != ctx.session_id
            {
                warn!(
                    dag_id = %completion.dag_id,
                    completion_session_id = %session_id,
                    context_session_id = %ctx.session_id,
                    "DAG completion session_id mismatch"
                );
            }

            if completion.succeeded {
                let leaf_outputs: Vec<PathBuf> = completion
                    .leaf_outputs
                    .into_iter()
                    .map(PathBuf::from)
                    .collect();

                let paired_enabled = self.paired_segment_pipelines.contains_key(&ctx.session_id);
                debug!(
                    dag_id = %dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    source = ?ctx.source,
                    leaf_outputs = %leaf_outputs.len(),
                    paired_enabled = %paired_enabled,
                    "Processing successful DAG completion with segment context"
                );
                if paired_enabled {
                    self.session_complete_coordinator.on_dag_complete(
                        &ctx.session_id,
                        ctx.segment_index,
                        leaf_outputs.clone(),
                        ctx.source,
                    );
                    let paired = match ctx.source {
                        SourceType::Video => self.paired_segment_coordinator.on_video_ready(
                            &ctx.session_id,
                            &ctx.streamer_id,
                            ctx.segment_index,
                            leaf_outputs,
                        ),
                        SourceType::Danmu => self.paired_segment_coordinator.on_danmu_ready(
                            &ctx.session_id,
                            &ctx.streamer_id,
                            ctx.segment_index,
                            leaf_outputs,
                        ),
                    };

                    if let Some(ready) = paired {
                        self.try_trigger_paired_segment(ready).await;
                    }
                } else {
                    self.session_complete_coordinator.on_dag_complete(
                        &ctx.session_id,
                        ctx.segment_index,
                        leaf_outputs,
                        ctx.source,
                    );
                }
            } else {
                warn!(
                    dag_id = %dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    source = ?ctx.source,
                    "DAG failed for segment context"
                );
                self.session_complete_coordinator
                    .on_dag_failed(&ctx.session_id, ctx.source);
            }

            self.try_trigger_session_complete(&ctx.session_id).await;
            return;
        }

        if let Some((_, ctx)) = self.paired_dag_contexts.remove(&dag_id) {
            if let Some(session_id) = completion.session_id.as_deref()
                && session_id != ctx.session_id
            {
                warn!(
                    dag_id = %completion.dag_id,
                    completion_session_id = %session_id,
                    context_session_id = %ctx.session_id,
                    "Paired DAG completion session_id mismatch"
                );
            }

            if completion.succeeded {
                trace!(
                    dag_id = %completion.dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    "Paired-segment DAG completed"
                );
                self.session_complete_coordinator
                    .on_paired_dag_complete(&ctx.session_id);
            } else {
                trace!(
                    dag_id = %completion.dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    "Paired-segment DAG failed"
                );
                self.session_complete_coordinator
                    .on_paired_dag_failed(&ctx.session_id);
            }

            self.try_trigger_session_complete(&ctx.session_id).await;
            return;
        }

        if self.handle_dag_completion_without_context(completion).await {
            return;
        }

        let _ = self.handled_dag_completions.remove(&dag_id);
        debug!(
            dag_id = %dag_id,
            "Ignoring DAG completion without segment context"
        );
    }

    async fn handle_dag_completion_without_context(&self, completion: DagCompletionInfo) -> bool {
        let Some(session_id) = completion.session_id.as_deref() else {
            return false;
        };

        let tracking_session_complete = self.session_complete_pipelines.contains_key(session_id);
        let paired_enabled = self.paired_segment_pipelines.contains_key(session_id);
        if !tracking_session_complete && !paired_enabled {
            return false;
        }

        let Some(repo) = &self.dag_repository else {
            trace!(
                dag_id = %completion.dag_id,
                session_id = %session_id,
                "DAG repository not configured; cannot recover completion context"
            );
            return false;
        };

        let dag = match repo.get_dag(&completion.dag_id).await {
            Ok(dag) => dag,
            Err(e) => {
                warn!(
                    dag_id = %completion.dag_id,
                    error = %e,
                    "Failed to load DAG execution for completion recovery"
                );
                return false;
            }
        };

        if let Some(db_session_id) = dag.session_id.as_deref()
            && db_session_id != session_id
        {
            warn!(
                dag_id = %completion.dag_id,
                completion_session_id = %session_id,
                db_session_id = %db_session_id,
                "DAG completion session_id mismatch (db vs completion)"
            );
        }

        let Some(segment_source) = dag.segment_source.as_deref() else {
            trace!(
                dag_id = %completion.dag_id,
                session_id = %session_id,
                "DAG completion has no segment metadata; ignoring"
            );
            return false;
        };

        match segment_source {
            "video" | "danmu" => {
                let Some(raw_index) = dag.segment_index else {
                    warn!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        segment_source = %segment_source,
                        "DAG completion missing segment_index metadata"
                    );
                    return false;
                };
                let Ok(segment_index) = u32::try_from(raw_index) else {
                    warn!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        segment_source = %segment_source,
                        segment_index = %raw_index,
                        "DAG completion has invalid segment_index metadata"
                    );
                    return false;
                };

                let source = match segment_source {
                    "video" => SourceType::Video,
                    "danmu" => SourceType::Danmu,
                    _ => unreachable!("match guards ensure only video/danmu"),
                };

                if completion.succeeded {
                    let leaf_outputs: Vec<PathBuf> = completion
                        .leaf_outputs
                        .into_iter()
                        .map(PathBuf::from)
                        .collect();

                    if paired_enabled {
                        if tracking_session_complete {
                            self.session_complete_coordinator.on_dag_complete(
                                session_id,
                                segment_index,
                                leaf_outputs.clone(),
                                source,
                            );
                        }

                        let streamer_id = dag
                            .streamer_id
                            .as_deref()
                            .or(completion.streamer_id.as_deref());

                        if let Some(streamer_id) = streamer_id {
                            let paired = match source {
                                SourceType::Video => {
                                    self.paired_segment_coordinator.on_video_ready(
                                        session_id,
                                        streamer_id,
                                        segment_index,
                                        leaf_outputs,
                                    )
                                }
                                SourceType::Danmu => {
                                    self.paired_segment_coordinator.on_danmu_ready(
                                        session_id,
                                        streamer_id,
                                        segment_index,
                                        leaf_outputs,
                                    )
                                }
                            };

                            if let Some(ready) = paired {
                                self.try_trigger_paired_segment(ready).await;
                            }
                        } else {
                            warn!(
                                dag_id = %completion.dag_id,
                                session_id = %session_id,
                                segment_index = %segment_index,
                                segment_source = %segment_source,
                                "Missing streamer_id for paired-segment coordination"
                            );
                        }
                    } else if tracking_session_complete {
                        self.session_complete_coordinator.on_dag_complete(
                            session_id,
                            segment_index,
                            leaf_outputs,
                            source,
                        );
                    }
                } else if tracking_session_complete {
                    self.session_complete_coordinator
                        .on_dag_failed(session_id, source);
                }

                if tracking_session_complete {
                    self.try_trigger_session_complete(session_id).await;
                }

                true
            }
            "paired" => {
                if !tracking_session_complete {
                    return false;
                }

                if completion.succeeded {
                    trace!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        "Paired-segment DAG completed (recovered context)"
                    );
                    self.session_complete_coordinator
                        .on_paired_dag_complete(session_id);
                } else {
                    trace!(
                        dag_id = %completion.dag_id,
                        session_id = %session_id,
                        "Paired-segment DAG failed (recovered context)"
                    );
                    self.session_complete_coordinator
                        .on_paired_dag_failed(session_id);
                }

                self.try_trigger_session_complete(session_id).await;
                true
            }
            other => {
                trace!(
                    dag_id = %completion.dag_id,
                    session_id = %session_id,
                    segment_source = %other,
                    "Unknown DAG segment_source; ignoring"
                );
                false
            }
        }
    }

    async fn try_trigger_session_complete(&self, session_id: &str) {
        // `SessionCompleteCoordinator::try_trigger` consumes/removes session state.
        // When we need to gate triggering on DB session end_time, we must not remove until that
        // condition is satisfied.
        if !self
            .session_complete_coordinator
            .is_ready_nonempty(session_id)
        {
            return;
        }

        if !self.session_has_ended(session_id).await {
            return;
        }

        let Some(outputs) = self.session_complete_coordinator.try_trigger(session_id) else {
            return;
        };

        debug!(
            session_id = %session_id,
            video_outputs = %outputs.video_outputs.len(),
            danmu_outputs = %outputs.danmu_outputs.len(),
            "try_trigger returned outputs, attempting to retrieve pipeline definition"
        );

        let pipeline_keys: Vec<_> = self
            .session_complete_pipelines
            .iter()
            .map(|e| e.key().clone())
            .collect();
        debug!(
            session_id = %session_id,
            tracked_sessions = ?pipeline_keys,
            "Current session_complete_pipelines keys before remove"
        );

        let Some((_, pipeline_entry)) = self.session_complete_pipelines.remove(session_id) else {
            warn!(
                session_id = %session_id,
                "Session became ready but no session_complete_pipeline definition was captured"
            );
            return;
        };

        debug!(
            session_id = %session_id,
            pipeline_name = %pipeline_entry.definition.name,
            pipeline_steps = %pipeline_entry.definition.steps.len(),
            "Pipeline definition retrieved, calling run_session_complete_pipeline"
        );

        self.run_session_complete_pipeline(outputs, pipeline_entry.definition)
            .await;
    }

    async fn session_has_ended(&self, session_id: &str) -> bool {
        let Some(repo) = &self.session_repo else {
            // If persistence is not configured, do not block session-complete forever.
            warn!(
                session_id = %session_id,
                "Session repository not configured; triggering session-complete without end_time gate"
            );
            return true;
        };

        match repo.get_session(session_id).await {
            Ok(session) => {
                if session.end_time.is_some() {
                    true
                } else {
                    debug!(
                        session_id = %session_id,
                        "Session not ended yet; delaying session-complete pipeline trigger"
                    );
                    false
                }
            }
            Err(e) => {
                warn!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to query session end_time; delaying session-complete pipeline trigger"
                );
                false
            }
        }
    }

    async fn run_session_complete_pipeline(
        &self,
        outputs: SessionOutputs,
        pipeline_def: DagPipelineDefinition,
    ) {
        debug!(
            session_id = %outputs.session_id,
            streamer_id = %outputs.streamer_id,
            pipeline_name = %pipeline_def.name,
            pipeline_steps = %pipeline_def.steps.len(),
            "Entered run_session_complete_pipeline"
        );

        // Skip if pipeline has no steps configured
        if pipeline_def.is_empty() {
            debug!(
                session_id = %outputs.session_id,
                "Skipping session-complete pipeline: no steps configured"
            );
            return;
        }

        #[derive(Serialize)]
        struct SessionCompleteManifest {
            session_id: String,
            streamer_id: String,
            video_inputs: Vec<String>,
            danmu_inputs: Vec<String>,
        }

        let video_paths = outputs.get_sorted_video_outputs();
        let danmu_paths = outputs.get_sorted_danmu_outputs();

        let mut input_paths: Vec<String> = Vec::new();

        if let Some(base_dir) = video_paths
            .first()
            .or_else(|| danmu_paths.first())
            .and_then(|p| p.parent())
        {
            let manifest = SessionCompleteManifest {
                session_id: outputs.session_id.clone(),
                streamer_id: outputs.streamer_id.clone(),
                video_inputs: video_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
                danmu_inputs: danmu_paths
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            };

            let manifest_name = format!(
                "session_{}_inputs.json",
                sanitize_filename(&outputs.session_id)
            );
            let manifest_path = base_dir.join(manifest_name);

            match serde_json::to_vec_pretty(&manifest) {
                Ok(json) => {
                    if let Err(e) = tokio::fs::write(&manifest_path, json).await {
                        warn!(
                            session_id = %outputs.session_id,
                            path = %manifest_path.display(),
                            error = %e,
                            "Failed to write session input manifest (continuing without manifest)"
                        );
                    } else {
                        input_paths.push(manifest_path.to_string_lossy().to_string());
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %outputs.session_id,
                        error = %e,
                        "Failed to serialize session input manifest (continuing without manifest)"
                    );
                }
            }
        }

        input_paths.extend(
            video_paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );
        input_paths.extend(
            danmu_paths
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );

        info!(
            session_id = %outputs.session_id,
            streamer_id = %outputs.streamer_id,
            inputs = %input_paths.len(),
            "Triggering session-complete pipeline"
        );

        if let Err(e) = self
            .create_dag_pipeline(
                &outputs.session_id,
                &outputs.streamer_id,
                input_paths,
                pipeline_def,
            )
            .await
        {
            tracing::error!(
                "Failed to create session-complete pipeline for session {}: {}",
                outputs.session_id,
                e
            );
        }
    }

    async fn try_trigger_paired_segment(&self, outputs: PairedSegmentOutputs) {
        let Some(entry) = self.paired_segment_pipelines.get(&outputs.session_id) else {
            warn!(
                session_id = %outputs.session_id,
                segment_index = %outputs.segment_index,
                "Paired segment became ready but no paired_segment_pipeline definition was captured"
            );
            return;
        };

        let pipeline_def = entry.definition.clone();
        drop(entry);

        self.run_paired_segment_pipeline(outputs, pipeline_def)
            .await;
    }

    async fn run_paired_segment_pipeline(
        &self,
        outputs: PairedSegmentOutputs,
        pipeline_def: DagPipelineDefinition,
    ) {
        // Skip if pipeline has no steps configured
        if pipeline_def.is_empty() {
            debug!(
                session_id = %outputs.session_id,
                segment_index = %outputs.segment_index,
                "Skipping paired-segment pipeline: no steps configured"
            );
            return;
        }

        #[derive(Serialize)]
        struct PairedSegmentManifest {
            session_id: String,
            streamer_id: String,
            segment_index: u32,
            video_inputs: Vec<String>,
            danmu_inputs: Vec<String>,
        }

        let video_inputs: Vec<String> = outputs
            .video_outputs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let danmu_inputs: Vec<String> = outputs
            .danmu_outputs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        let mut input_paths: Vec<String> = Vec::new();

        if let Some(base_dir) = outputs
            .video_outputs
            .first()
            .or_else(|| outputs.danmu_outputs.first())
            .and_then(|p| p.parent())
        {
            let manifest = PairedSegmentManifest {
                session_id: outputs.session_id.clone(),
                streamer_id: outputs.streamer_id.clone(),
                segment_index: outputs.segment_index,
                video_inputs,
                danmu_inputs,
            };

            let manifest_name = format!(
                "segment_{}_{}_inputs.json",
                sanitize_filename(&outputs.session_id),
                outputs.segment_index
            );
            let manifest_path = base_dir.join(manifest_name);

            match serde_json::to_vec_pretty(&manifest) {
                Ok(json) => {
                    if let Err(e) = tokio::fs::write(&manifest_path, json).await {
                        warn!(
                            session_id = %outputs.session_id,
                            segment_index = %outputs.segment_index,
                            path = %manifest_path.display(),
                            error = %e,
                            "Failed to write paired-segment input manifest (continuing without manifest)"
                        );
                    } else {
                        input_paths.push(manifest_path.to_string_lossy().to_string());
                    }
                }
                Err(e) => {
                    warn!(
                        session_id = %outputs.session_id,
                        segment_index = %outputs.segment_index,
                        error = %e,
                        "Failed to serialize paired-segment input manifest (continuing without manifest)"
                    );
                }
            }
        }

        input_paths.extend(
            outputs
                .video_outputs
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );
        input_paths.extend(
            outputs
                .danmu_outputs
                .into_iter()
                .map(|p| p.to_string_lossy().to_string()),
        );

        info!(
            session_id = %outputs.session_id,
            streamer_id = %outputs.streamer_id,
            segment_index = %outputs.segment_index,
            inputs = %input_paths.len(),
            "Triggering paired-segment pipeline"
        );

        let tracking_session_complete = self
            .session_complete_pipelines
            .contains_key(&outputs.session_id);
        if tracking_session_complete {
            self.session_complete_coordinator
                .on_paired_dag_started(&outputs.session_id);
        }

        let before_root_jobs = if tracking_session_complete {
            let contexts = self.paired_dag_contexts.clone();
            let ctx = PairedDagContext {
                session_id: outputs.session_id.clone(),
                streamer_id: outputs.streamer_id.clone(),
                segment_index: outputs.segment_index,
                created_at: std::time::Instant::now(),
            };
            Some(Box::new(move |dag_id: &str| {
                debug!(
                    dag_id = %dag_id,
                    session_id = %ctx.session_id,
                    streamer_id = %ctx.streamer_id,
                    segment_index = %ctx.segment_index,
                    "Tracking paired-segment DAG context"
                );
                contexts.insert(dag_id.to_string(), ctx);
            }) as BeforeRootJobsHook)
        } else {
            None
        };

        if let Err(e) = self
            .create_dag_pipeline_internal(
                &outputs.session_id,
                &outputs.streamer_id,
                input_paths,
                pipeline_def,
                before_root_jobs,
                Some(DagExecutionMetadata {
                    segment_index: Some(outputs.segment_index),
                    segment_source: Some("paired".to_string()),
                }),
            )
            .await
        {
            tracing::error!(
                "Failed to create paired-segment pipeline for session {} segment {}: {}",
                outputs.session_id,
                outputs.segment_index,
                e
            );
            if tracking_session_complete {
                self.session_complete_coordinator
                    .on_paired_dag_failed(&outputs.session_id);
            }
        }
    }

    /// Stop the pipeline manager.
    pub async fn stop(&self) {
        info!("Stopping Pipeline Manager");
        self.cancellation_token.cancel();

        // Stop worker pools
        self.cpu_pool.stop().await;
        self.io_pool.stop().await;

        info!("Pipeline Manager stopped");
    }

    /// Subscribe to pipeline events.
    pub fn subscribe(&self) -> broadcast::Receiver<PipelineEvent> {
        self.event_tx.subscribe()
    }

    /// Enqueue a job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();
        let streamer_id = job.streamer_id.clone();

        self.job_queue.enqueue(job).await?;

        // Emit event
        let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
            job_id: job_id.clone(),
            job_type,
            streamer_id,
        });

        // Check queue depth
        self.check_queue_depth();

        Ok(job_id)
    }

    /// Create a remux job for a downloaded segment.
    pub async fn create_remux_job(
        &self,
        input_path: &str,
        output_path: &str,
        streamer_id: &str,
        session_id: &str,
    ) -> Result<String> {
        let job = Job::new(
            "remux",
            vec![input_path.to_string()],
            vec![output_path.to_string()],
            streamer_id,
            session_id,
        );
        self.enqueue(job).await
    }

    /// Create an rclone job.
    pub async fn create_rclone_job(
        &self,
        input_path: &str,
        destination: &str,
        streamer_id: &str,
        session_id: &str,
    ) -> Result<String> {
        let job = Job::new(
            "rclone",
            vec![input_path.to_string()],
            vec![destination.to_string()],
            streamer_id,
            session_id,
        );
        self.enqueue(job).await
    }

    /// Create a thumbnail job.
    pub async fn create_thumbnail_job(
        &self,
        input_path: &str,
        output_path: &str,
        streamer_id: &str,
        session_id: &str,
        config: Option<&str>,
    ) -> Result<String> {
        let mut job = Job::new(
            "thumbnail",
            vec![input_path.to_string()],
            vec![output_path.to_string()],
            streamer_id,
            session_id,
        );
        if let Some(cfg) = config {
            job = job.with_config(cfg);
        }
        self.enqueue(job).await
    }

    /// Look up the streamer name from the repository.
    async fn lookup_streamer_name(&self, streamer_id: &str) -> Option<String> {
        let repo = self.streamer_repo.as_ref()?;

        match repo.get_streamer(streamer_id).await {
            Ok(streamer) => Some(streamer.name),
            Err(e) => {
                debug!(
                    streamer_id = %streamer_id,
                    error = %e,
                    "Failed to look up streamer name"
                );
                None
            }
        }
    }

    /// Look up the platform name (e.g. "Twitch") from the streamer's platform config.
    async fn lookup_platform_name(&self, streamer_id: &str) -> Option<String> {
        let streamer_repo = self.streamer_repo.as_ref()?;
        let config_service = self.config_service.as_ref()?;

        let platform_id = match streamer_repo.get_streamer(streamer_id).await {
            Ok(streamer) => streamer.platform_config_id,
            Err(e) => {
                debug!(
                    streamer_id = %streamer_id,
                    error = %e,
                    "Failed to look up streamer platform_config_id"
                );
                return None;
            }
        };

        match config_service.get_platform_config(&platform_id).await {
            Ok(platform) => Some(platform.platform_name),
            Err(e) => {
                debug!(
                    streamer_id = %streamer_id,
                    platform_id = %platform_id,
                    error = %e,
                    "Failed to look up platform name"
                );
                None
            }
        }
    }

    /// Look up the session title from the repository.
    /// Returns the most recent title from the titles JSON array.
    async fn lookup_session_title(&self, session_id: &str) -> Option<String> {
        let repo = self.session_repo.as_ref()?;

        match repo.get_session(session_id).await {
            Ok(session) => {
                // Parse the titles JSON array and get the most recent title
                if let Some(titles_json) = session.titles
                    && let Ok(entries) = serde_json::from_str::<Vec<TitleEntry>>(&titles_json)
                {
                    // Return the last (most recent) title
                    return entries.last().map(|e| e.title.clone());
                }

                None
            }
            Err(e) => {
                debug!(
                    session_id = %session_id,
                    error = %e,
                    "Failed to look up session title"
                );
                None
            }
        }
    }

    /// Create a DAG pipeline with fan-in/fan-out support.
    ///
    /// Unlike sequential pipelines, DAG pipelines support:
    /// - Fan-out: One step can trigger multiple downstream steps
    /// - Fan-in: Multiple steps can merge their outputs before a downstream step
    /// - Fail-fast: Any step failure cancels all pending/running jobs in the DAG
    ///
    /// Returns the DAG ID and root job IDs for tracking.
    pub async fn create_dag_pipeline(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_paths: Vec<String>,
        dag_definition: DagPipelineDefinition,
    ) -> Result<DagCreationResult> {
        self.create_dag_pipeline_internal(
            session_id,
            streamer_id,
            input_paths,
            dag_definition,
            None,
            None,
        )
        .await
    }

    /// Cancel a DAG execution.
    ///
    /// This also notifies the paired/session coordinators so session-complete orchestration
    /// can't get stuck waiting for a cancelled DAG to finish.
    pub async fn cancel_dag(&self, dag_id: &str) -> Result<u64> {
        let dag_scheduler = self.dag_scheduler.as_ref().ok_or_else(|| {
            crate::Error::Validation(
                "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
            )
        })?;

        let update = dag_scheduler.cancel_dag_with_completion(dag_id).await?;
        if let Some(completion) = update.completion {
            self.handle_dag_completion(completion).await;
        }

        Ok(update.cancelled_count)
    }

    async fn create_dag_pipeline_internal(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_paths: Vec<String>,
        dag_definition: DagPipelineDefinition,
        before_root_jobs: Option<BeforeRootJobsHook>,
        metadata: Option<DagExecutionMetadata>,
    ) -> Result<DagCreationResult> {
        let dag_scheduler = self.dag_scheduler.as_ref().ok_or_else(|| {
            crate::Error::Validation(
                "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
            )
        })?;

        // First, expand any workflow steps in the DAG
        let expanded_dag = self.expand_workflows_in_dag(dag_definition).await?;

        // Resolve all steps in the DAG before creation (Presets -> Inline)
        let mut resolved_dag = expanded_dag;
        for dag_step in &mut resolved_dag.steps {
            let resolved = self.resolve_dag_step(&dag_step.step).await?;
            dag_step.step = resolved;
        }

        // Look up metadata for placeholder support
        let streamer_name = self.lookup_streamer_name(streamer_id).await;
        let session_title = self.lookup_session_title(session_id).await;
        let platform = self.lookup_platform_name(streamer_id).await;

        // Delegate to DAG scheduler
        let result = dag_scheduler
            .create_dag_pipeline_with_hook(
                resolved_dag,
                &input_paths,
                Some(streamer_id.to_string()),
                Some(session_id.to_string()),
                streamer_name.clone(),
                session_title.clone(),
                platform.clone(),
                metadata,
                before_root_jobs,
            )
            .await?;

        info!(
            "Created DAG pipeline {} with {} steps ({} root jobs) for session {}, streamer {}, streamer name {}, session title {}",
            result.dag_id,
            result.total_steps,
            result.root_job_ids.len(),
            session_id,
            streamer_id,
            streamer_name.unwrap_or_default(),
            session_title.unwrap_or_default(),
        );

        // Emit events for root jobs
        for job_id in &result.root_job_ids {
            let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
                job_id: job_id.clone(),
                job_type: "dag_step".to_string(),
                streamer_id: streamer_id.to_string(),
            });
        }

        // Check queue depth
        self.check_queue_depth();

        Ok(result)
    }

    /// Expand workflow steps in a DAG definition.
    ///
    /// For each step that is a `Workflow`, this method:
    /// 1. Looks up the workflow by name from the pipeline preset repository
    /// 2. Gets the workflow's `dag_definition` (its internal DAG structure)
    /// 3. Expands the workflow's steps into the parent DAG with prefixed IDs
    /// 4. Wires up dependencies correctly:
    ///    - Workflow's root steps inherit the original workflow step's `depends_on`
    ///    - Steps that depended on the workflow step now depend on the workflow's leaf steps
    ///
    /// This process is applied until no workflow steps remain (handles nested workflows).
    async fn expand_workflows_in_dag(
        &self,
        mut dag: DagPipelineDefinition,
    ) -> Result<DagPipelineDefinition> {
        use std::collections::HashSet;

        // Keep expanding until no workflow steps remain (handles nested workflows)
        let mut iteration = 0;
        const MAX_ITERATIONS: usize = 10; // Prevent infinite loops from circular workflow references

        loop {
            iteration += 1;
            if iteration > MAX_ITERATIONS {
                return Err(crate::Error::Validation(
                    "Maximum workflow expansion depth exceeded. Check for circular workflow references.".to_string(),
                ));
            }

            // Find workflow steps that need expansion
            let workflow_steps: Vec<(usize, String)> = dag
                .steps
                .iter()
                .enumerate()
                .filter_map(|(idx, step)| {
                    if let PipelineStep::Workflow { name } = &step.step {
                        Some((idx, name.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            if workflow_steps.is_empty() {
                break; // No more workflows to expand
            }

            // Process each workflow step
            for (workflow_step_idx, workflow_name) in workflow_steps.into_iter().rev() {
                // Process in reverse to maintain index validity.
                let workflow_step = &dag.steps[workflow_step_idx];
                let workflow_step_id = workflow_step.id.clone();
                let workflow_step_deps = workflow_step.depends_on.clone();

                // Look up the workflow
                let workflow_dag = self.lookup_workflow(&workflow_name).await?;

                // Find workflow's root and leaf steps
                let root_step_ids: HashSet<String> = workflow_dag
                    .root_steps()
                    .iter()
                    .map(|s| s.id.clone())
                    .collect();
                let leaf_step_ids: HashSet<String> = workflow_dag
                    .leaf_steps()
                    .iter()
                    .map(|s| s.id.clone())
                    .collect();

                // Create a prefix to avoid ID collisions
                let prefix = format!("{}__", workflow_step_id);

                // Build expanded steps with prefixed IDs
                let expanded_steps: Vec<DagStep> = workflow_dag
                    .steps
                    .iter()
                    .map(|s| {
                        let new_id = format!("{}{}", prefix, s.id);
                        let new_deps: Vec<String> = if root_step_ids.contains(&s.id) {
                            // Root steps inherit the original workflow step's dependencies
                            workflow_step_deps.clone()
                        } else {
                            // Internal steps get prefixed dependencies
                            s.depends_on
                                .iter()
                                .map(|d| format!("{}{}", prefix, d))
                                .collect()
                        };
                        DagStep {
                            id: new_id,
                            step: s.step.clone(),
                            depends_on: new_deps,
                        }
                    })
                    .collect();

                // Find steps that depend on the workflow step and update their dependencies
                let prefixed_leaf_ids: Vec<String> = leaf_step_ids
                    .iter()
                    .map(|id| format!("{}{}", prefix, id))
                    .collect();

                for step in &mut dag.steps {
                    if step.depends_on.contains(&workflow_step_id) {
                        // Remove the workflow step ID and add the workflow's leaf step IDs
                        step.depends_on.retain(|d| d != &workflow_step_id);
                        step.depends_on.extend(prefixed_leaf_ids.clone());
                    }
                }

                // Remove the workflow step and insert the expanded steps
                dag.steps.remove(workflow_step_idx);
                dag.steps
                    .splice(workflow_step_idx..workflow_step_idx, expanded_steps);
            }
        }

        Ok(dag)
    }

    /// Look up a workflow by name and return its DAG definition.
    async fn lookup_workflow(&self, name: &str) -> Result<DagPipelineDefinition> {
        let repo = self.pipeline_preset_repo.as_ref().ok_or_else(|| {
            crate::Error::Validation(format!(
                "No pipeline preset repository, cannot expand workflow '{}'",
                name
            ))
        })?;

        let workflow = repo
            .get_pipeline_preset_by_name(name)
            .await
            .map_err(|e| crate::Error::Database(e.to_string()))?
            .ok_or_else(|| crate::Error::Validation(format!("Workflow '{}' not found", name)))?;

        // Get the DAG definition from the workflow
        let dag_def = workflow.get_dag_definition().ok_or_else(|| {
            crate::Error::Validation(format!(
                "Workflow '{}' does not have a DAG definition. Only DAG-based workflows can be embedded.",
                name
            ))
        })?;

        Ok(dag_def)
    }

    /// Resolve a DAG step's PipelineStep to an Inline step.
    async fn resolve_dag_step(&self, step: &PipelineStep) -> Result<PipelineStep> {
        match step {
            PipelineStep::Preset { name } => {
                if let Some(repo) = &self.preset_repo {
                    match repo.get_preset_by_name(name).await {
                        Ok(Some(preset)) => {
                            let config = if !preset.config.is_empty() {
                                serde_json::from_str(&preset.config)
                                    .unwrap_or(serde_json::Value::Null)
                            } else {
                                serde_json::Value::Null
                            };
                            Ok(PipelineStep::Inline {
                                processor: preset.processor,
                                config,
                            })
                        }
                        Ok(None) => {
                            // Fallback: assume name is processor
                            Ok(PipelineStep::Inline {
                                processor: name.clone(),
                                config: serde_json::Value::Null,
                            })
                        }
                        Err(e) => Err(crate::Error::Database(e.to_string())),
                    }
                } else {
                    // No repo, fallback
                    Ok(PipelineStep::Inline {
                        processor: name.clone(),
                        config: serde_json::Value::Null,
                    })
                }
            }
            PipelineStep::Workflow { name } => {
                // Workflows should be expanded before DAG creation
                Err(crate::Error::Validation(format!(
                    "Workflow '{}' should be resolved before DAG creation. \
                     Expand workflows into individual DAG steps.",
                    name
                )))
            }
            PipelineStep::Inline { .. } => Ok(step.clone()),
        }
    }

    /// Handle download manager events.
    pub async fn handle_download_event(&self, event: DownloadManagerEvent) {
        match event {
            DownloadManagerEvent::SegmentCompleted {
                streamer_id,
                session_id,
                segment_path,
                segment_index,
                size_bytes,
                ..
            } => {
                debug!(
                    "Segment completed for {} (session: {}): {}",
                    streamer_id, session_id, segment_path
                );
                // Persist segment to database
                self.persist_segment(&session_id, &segment_path, size_bytes)
                    .await;

                let merged_config = if let Some(config_service) = &self.config_service {
                    config_service
                        .get_config_for_streamer(&streamer_id)
                        .await
                        .ok()
                } else {
                    None
                };

                let pipeline_config = merged_config.as_ref().and_then(|c| c.pipeline.clone());
                let session_complete_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.session_complete_pipeline.clone());
                let paired_segment_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.paired_segment_pipeline.clone());
                let danmu_enabled = merged_config
                    .as_ref()
                    .map(|c| c.record_danmu)
                    .unwrap_or(false);

                if let Some(def) = &session_complete_pipeline {
                    self.session_complete_pipelines
                        .entry(session_id.clone())
                        .and_modify(|e| e.last_seen = std::time::Instant::now())
                        .or_insert_with(|| SessionCompletePipelineEntry {
                            last_seen: std::time::Instant::now(),
                            definition: def.clone(),
                        });
                    self.session_complete_coordinator.init_session(
                        &session_id,
                        &streamer_id,
                        danmu_enabled,
                    );
                }

                if danmu_enabled && let Some(def) = &paired_segment_pipeline {
                    self.paired_segment_pipelines
                        .entry(session_id.clone())
                        .and_modify(|e| {
                            e.last_seen = std::time::Instant::now();
                            e.definition = def.clone();
                        })
                        .or_insert_with(|| PairedSegmentPipelineEntry {
                            last_seen: std::time::Instant::now(),
                            definition: def.clone(),
                        });
                }

                // Check for thumbnail step in DAG nodes
                // For direct DAG support, we check if any node is a thumbnail processor/workflow
                let pipeline_has_thumbnail = if let Some(dag) = &pipeline_config {
                    dag.steps.iter().any(|node| match &node.step {
                        PipelineStep::Inline { processor, .. } => processor == "thumbnail",
                        // Match exact preset names or those with thumbnail_ prefix
                        PipelineStep::Preset { name } => {
                            name == "thumbnail"
                                || name.starts_with("thumbnail_")
                                || name == "thumbnail_native"
                                || name == "thumbnail_hd"
                        }
                        // Match exact workflow names or those with thumbnail prefix
                        PipelineStep::Workflow { name } => {
                            name == "thumbnail" || name.starts_with("thumbnail_")
                        }
                    })
                } else {
                    false
                };

                // Check if auto_thumbnail is enabled in global settings (defaults to true)
                let auto_thumbnail_enabled = merged_config
                    .as_ref()
                    .map(|c| c.auto_thumbnail)
                    .unwrap_or(true);

                // Generate automatic thumbnail for first segment only if:
                // 1. This is the first segment (segment_index == 0)
                // 2. User's pipeline doesn't already include a thumbnail step
                // 3. Auto thumbnail generation is enabled in global settings
                if segment_index == 0 && !pipeline_has_thumbnail && auto_thumbnail_enabled {
                    self.maybe_create_thumbnail_job(&streamer_id, &session_id, &segment_path)
                        .await;
                }

                // Create pipeline jobs if pipeline is configured and has steps
                if let Some(dag) = pipeline_config.filter(|d| !d.is_empty()) {
                    let tracking_session_complete = session_complete_pipeline.is_some();
                    let tracking_paired = danmu_enabled && paired_segment_pipeline.is_some();
                    if tracking_session_complete {
                        self.session_complete_coordinator
                            .on_dag_started(&session_id, SourceType::Video);
                    }

                    let before_root_jobs = if tracking_session_complete || tracking_paired {
                        let contexts = self.dag_segment_contexts.clone();
                        let ctx = SegmentDagContext {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                            segment_index,
                            source: SourceType::Video,
                            created_at: std::time::Instant::now(),
                        };
                        Some(Box::new(move |dag_id: &str| {
                            debug!(
                                dag_id = %dag_id,
                                session_id = %ctx.session_id,
                                streamer_id = %ctx.streamer_id,
                                segment_index = %ctx.segment_index,
                                source = ?ctx.source,
                                "Tracking per-segment DAG context"
                            );
                            contexts.insert(dag_id.to_string(), ctx);
                        }) as Box<dyn FnOnce(&str) + Send>)
                    } else {
                        None
                    };

                    match self
                        .create_dag_pipeline_internal(
                            &session_id,
                            &streamer_id,
                            vec![segment_path.clone()],
                            dag,
                            before_root_jobs,
                            Some(DagExecutionMetadata {
                                segment_index: Some(segment_index),
                                segment_source: Some("video".to_string()),
                            }),
                        )
                        .await
                    {
                        Ok(DagCreationResult { .. }) => {}
                        Err(e) => {
                            tracing::error!(
                                "Failed to create pipeline for session {}: {}",
                                session_id,
                                e
                            );
                            if tracking_session_complete {
                                self.session_complete_coordinator
                                    .on_dag_failed(&session_id, SourceType::Video);
                            }
                        }
                    }
                } else {
                    debug!(
                        "No pipeline steps configured for {} (session: {}), skipping pipeline creation",
                        streamer_id, session_id
                    );
                    if session_complete_pipeline.is_some() {
                        self.session_complete_coordinator.on_raw_segment(
                            &session_id,
                            segment_index,
                            PathBuf::from(&segment_path),
                            SourceType::Video,
                        );
                    }

                    if danmu_enabled
                        && paired_segment_pipeline.is_some()
                        && let Some(ready) = self.paired_segment_coordinator.on_video_ready(
                            &session_id,
                            &streamer_id,
                            segment_index,
                            vec![PathBuf::from(&segment_path)],
                        )
                    {
                        self.try_trigger_paired_segment(ready).await;
                    }
                }

                if session_complete_pipeline.is_some() {
                    self.try_trigger_session_complete(&session_id).await;
                }
            }
            DownloadManagerEvent::DownloadCompleted {
                streamer_id,
                session_id,
                ..
            } => {
                info!(
                    "Download completed for streamer {} session {}",
                    streamer_id, session_id
                );

                if !self.session_complete_pipelines.contains_key(&session_id)
                    && let Some(config_service) = &self.config_service
                    && let Ok(config) = config_service.get_config_for_streamer(&streamer_id).await
                    && let Some(def) = config.session_complete_pipeline.clone()
                {
                    self.session_complete_pipelines.insert(
                        session_id.clone(),
                        SessionCompletePipelineEntry {
                            last_seen: std::time::Instant::now(),
                            definition: def,
                        },
                    );
                    self.session_complete_coordinator.init_session(
                        &session_id,
                        &streamer_id,
                        config.record_danmu,
                    );
                }

                if self.session_complete_pipelines.contains_key(&session_id) {
                    if let Some(mut entry) = self.session_complete_pipelines.get_mut(&session_id) {
                        entry.last_seen = std::time::Instant::now();
                    }
                    self.session_complete_coordinator
                        .on_video_complete(&session_id);
                    self.try_trigger_session_complete(&session_id).await;
                }
            }
            DownloadManagerEvent::DownloadCancelled {
                streamer_id,
                session_id,
                cause,
                ..
            } => {
                info!(
                    streamer_id = %streamer_id,
                    session_id = %session_id,
                    cause = %cause.as_str(),
                    "Download cancelled"
                );

                // Do not treat DownloadCancelled as stream completion for session-complete purposes.
                //
                // On mesio (and other engines), cancellation is a *stop request*; the final segment
                // may still be flushing and `SegmentCompleted`/`DownloadCompleted` can arrive later.
                // Marking video complete here can trigger the session-complete pipeline before the
                // final video output is recorded, resulting in missing `.flv` inputs/outputs.
                if let Some(mut entry) = self.session_complete_pipelines.get_mut(&session_id) {
                    entry.last_seen = std::time::Instant::now();
                }
                debug!(
                    streamer_id = %streamer_id,
                    session_id = %session_id,
                    cause = %cause.as_str(),
                    "Download cancellation observed; waiting for DownloadCompleted before marking video complete"
                );
            }
            _ => {}
        }
    }

    /// Handle danmu service events.
    ///
    /// Processes `DanmuEvent::SegmentCompleted` events by:
    /// 1. Persisting the danmu segment to the database as a media output
    /// 2. Creating pipeline jobs if a pipeline is configured for the streamer
    pub async fn handle_danmu_event(&self, event: crate::danmu::DanmuEvent) {
        use crate::danmu::DanmuControlEvent;
        use crate::danmu::DanmuEvent;
        use crate::database::models::TitleEntry;

        match event {
            DanmuEvent::CollectionStarted {
                session_id,
                streamer_id,
            } => {
                if let Some(config_service) = &self.config_service
                    && let Ok(config) = config_service.get_config_for_streamer(&streamer_id).await
                {
                    if let Some(def) = config.session_complete_pipeline.clone() {
                        self.session_complete_pipelines
                            .entry(session_id.clone())
                            .and_modify(|e| {
                                e.last_seen = std::time::Instant::now();
                                e.definition = def.clone();
                            })
                            .or_insert_with(|| SessionCompletePipelineEntry {
                                last_seen: std::time::Instant::now(),
                                definition: def,
                            });
                        self.session_complete_coordinator.init_session(
                            &session_id,
                            &streamer_id,
                            config.record_danmu,
                        );
                        // If danmu collection actually started, require a corresponding
                        // completion signal before triggering session-complete.
                        self.session_complete_coordinator
                            .on_danmu_started(&session_id);
                    }

                    if config.record_danmu
                        && let Some(def) = config.paired_segment_pipeline.clone()
                    {
                        self.paired_segment_pipelines
                            .entry(session_id.clone())
                            .and_modify(|e| {
                                e.last_seen = std::time::Instant::now();
                                e.definition = def.clone();
                            })
                            .or_insert_with(|| PairedSegmentPipelineEntry {
                                last_seen: std::time::Instant::now(),
                                definition: def,
                            });
                    }
                }
            }
            DanmuEvent::Control {
                session_id,
                streamer_id,
                control,
                ..
            } => {
                // Bump activity timestamp for any tracked pipelines to prevent premature cleanup.
                if let Some(mut entry) = self.session_complete_pipelines.get_mut(&session_id) {
                    entry.last_seen = std::time::Instant::now();
                }
                if let Some(mut entry) = self.paired_segment_pipelines.get_mut(&session_id) {
                    entry.last_seen = std::time::Instant::now();
                }

                // Apply title changes immediately so session titles stay accurate even when
                // the monitor polling interval is long.
                if let DanmuControlEvent::RoomInfoChanged {
                    title: Some(title), ..
                } = &control
                {
                    let Some(repo) = &self.session_repo else {
                        return;
                    };
                    match repo.get_session(&session_id).await {
                        Ok(session) => {
                            let now = chrono::Utc::now();
                            let mut titles: Vec<TitleEntry> = match session.titles.as_deref() {
                                Some(json) => serde_json::from_str(json).unwrap_or_default(),
                                None => Vec::new(),
                            };

                            let needs_update =
                                titles.last().map(|t| t.title != *title).unwrap_or(true);
                            if needs_update {
                                titles.push(TitleEntry {
                                    ts: now.timestamp_millis(),
                                    title: title.clone(),
                                });
                                match serde_json::to_string(&titles) {
                                    Ok(updated) => {
                                        if let Err(e) =
                                            repo.update_session_titles(&session_id, &updated).await
                                        {
                                            warn!(
                                                streamer_id = %streamer_id,
                                                session_id = %session_id,
                                                error = %e,
                                                "Failed to persist session title update from danmu control event"
                                            );
                                        }
                                    }
                                    Err(e) => warn!(
                                        streamer_id = %streamer_id,
                                        session_id = %session_id,
                                        error = %e,
                                        "Failed to serialize session titles for danmu control title update"
                                    ),
                                }
                            }
                        }
                        Err(e) => warn!(
                            streamer_id = %streamer_id,
                            session_id = %session_id,
                            error = %e,
                            "Failed to load session for danmu control title update"
                        ),
                    }
                }
            }
            DanmuEvent::CollectionStopped { session_id, .. } => {
                // Only process if we're tracking this session for session-complete coordination.
                // If no session_complete_pipeline is configured, init_session was never called
                // and we don't need to track danmu completion.
                let tracking_session_complete = {
                    if let Some(mut entry) = self.session_complete_pipelines.get_mut(&session_id) {
                        entry.last_seen = std::time::Instant::now();
                        true
                    } else {
                        false
                    }
                };

                if tracking_session_complete {
                    self.session_complete_coordinator
                        .on_danmu_complete(&session_id);
                    self.try_trigger_session_complete(&session_id).await;
                }
            }
            DanmuEvent::SegmentCompleted {
                streamer_id,
                session_id,
                segment_id,
                output_path,
                message_count,
            } => {
                let segment_path = output_path.to_string_lossy().to_string();

                debug!(
                    "Danmu segment completed for {} (session: {}): {} ({} messages)",
                    streamer_id, session_id, segment_path, message_count
                );

                // Check if the danmu file still exists before processing.
                // The file may have been deleted if the corresponding video segment was too small.
                if !output_path.exists() {
                    debug!("Danmu segment file no longer exists: {}", segment_path);
                    return;
                }

                let Some(segment_index) = parse_segment_index_from_danmu(&segment_id, &output_path)
                else {
                    warn!(
                        session_id = %session_id,
                        segment_id = %segment_id,
                        path = %output_path.display(),
                        "Failed to parse danmu segment_index; skipping danmu pipeline coordination for this segment"
                    );
                    return;
                };

                // Persist danmu segment to database as a media output
                self.persist_danmu_segment(&session_id, &segment_path, message_count)
                    .await;

                let merged_config = if let Some(config_service) = &self.config_service {
                    config_service
                        .get_config_for_streamer(&streamer_id)
                        .await
                        .ok()
                } else {
                    None
                };

                let pipeline_config = merged_config.as_ref().and_then(|c| c.pipeline.clone());
                let session_complete_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.session_complete_pipeline.clone());
                let paired_segment_pipeline = merged_config
                    .as_ref()
                    .and_then(|c| c.paired_segment_pipeline.clone());
                let danmu_enabled = merged_config
                    .as_ref()
                    .map(|c| c.record_danmu)
                    .unwrap_or(false);

                if let Some(def) = &session_complete_pipeline {
                    self.session_complete_pipelines
                        .entry(session_id.clone())
                        .and_modify(|e| e.last_seen = std::time::Instant::now())
                        .or_insert_with(|| SessionCompletePipelineEntry {
                            last_seen: std::time::Instant::now(),
                            definition: def.clone(),
                        });
                    self.session_complete_coordinator.init_session(
                        &session_id,
                        &streamer_id,
                        danmu_enabled,
                    );
                }

                if danmu_enabled && let Some(def) = &paired_segment_pipeline {
                    self.paired_segment_pipelines
                        .entry(session_id.clone())
                        .and_modify(|e| {
                            e.last_seen = std::time::Instant::now();
                            e.definition = def.clone();
                        })
                        .or_insert_with(|| PairedSegmentPipelineEntry {
                            last_seen: std::time::Instant::now(),
                            definition: def.clone(),
                        });
                }

                // Create pipeline jobs if pipeline is configured and has steps
                if let Some(dag) = pipeline_config.filter(|d| !d.is_empty()) {
                    let tracking_session_complete = session_complete_pipeline.is_some();
                    let tracking_paired = danmu_enabled && paired_segment_pipeline.is_some();
                    if tracking_session_complete {
                        self.session_complete_coordinator
                            .on_dag_started(&session_id, SourceType::Danmu);
                    }

                    let before_root_jobs = if tracking_session_complete || tracking_paired {
                        let contexts = self.dag_segment_contexts.clone();
                        let ctx = SegmentDagContext {
                            session_id: session_id.clone(),
                            streamer_id: streamer_id.clone(),
                            segment_index,
                            source: SourceType::Danmu,
                            created_at: std::time::Instant::now(),
                        };
                        Some(Box::new(move |dag_id: &str| {
                            debug!(
                                dag_id = %dag_id,
                                session_id = %ctx.session_id,
                                streamer_id = %ctx.streamer_id,
                                segment_index = %ctx.segment_index,
                                source = ?ctx.source,
                                "Tracking per-segment DAG context"
                            );
                            contexts.insert(dag_id.to_string(), ctx);
                        }) as Box<dyn FnOnce(&str) + Send>)
                    } else {
                        None
                    };

                    match self
                        .create_dag_pipeline_internal(
                            &session_id,
                            &streamer_id,
                            vec![segment_path.clone()],
                            dag,
                            before_root_jobs,
                            Some(DagExecutionMetadata {
                                segment_index: Some(segment_index),
                                segment_source: Some("danmu".to_string()),
                            }),
                        )
                        .await
                    {
                        Ok(DagCreationResult { .. }) => {}
                        Err(e) => {
                            tracing::error!(
                                "Failed to create pipeline for danmu segment (session {}): {}",
                                session_id,
                                e
                            );
                            if tracking_session_complete {
                                self.session_complete_coordinator
                                    .on_dag_failed(&session_id, SourceType::Danmu);
                            }
                        }
                    }
                } else {
                    debug!(
                        "No pipeline steps configured for {} (session: {}), skipping danmu pipeline creation",
                        streamer_id, session_id
                    );
                    if session_complete_pipeline.is_some() {
                        self.session_complete_coordinator.on_raw_segment(
                            &session_id,
                            segment_index,
                            output_path.clone(),
                            SourceType::Danmu,
                        );
                    }

                    if danmu_enabled
                        && paired_segment_pipeline.is_some()
                        && let Some(ready) = self.paired_segment_coordinator.on_danmu_ready(
                            &session_id,
                            &streamer_id,
                            segment_index,
                            vec![output_path.clone()],
                        )
                    {
                        self.try_trigger_paired_segment(ready).await;
                    }
                }

                if session_complete_pipeline.is_some() {
                    self.try_trigger_session_complete(&session_id).await;
                }
            }
            _ => {}
        }
    }

    /// Check if session already has a thumbnail by querying media outputs.
    async fn session_has_thumbnail(&self, session_id: &str) -> bool {
        if let Some(repo) = &self.session_repo
            && let Ok(outputs) = repo.get_media_outputs_for_session(session_id).await
        {
            return outputs
                .iter()
                .any(|o| o.file_type == MediaFileType::Thumbnail.as_str());
        }
        false
    }

    /// Create a thumbnail job for the first segment if session doesn't already have one.
    async fn maybe_create_thumbnail_job(
        &self,
        streamer_id: &str,
        session_id: &str,
        segment_path: &str,
    ) {
        // Check if session already has a thumbnail (reuses existing query)
        if self.session_has_thumbnail(session_id).await {
            debug!("Session {} already has a thumbnail, skipping", session_id);
            return;
        }

        // Use thumbnail_native preset
        let step = PipelineStep::Preset {
            name: "thumbnail_native".to_string(),
        };

        // Create DAG definition
        let dag_step = DagStep::new("thumbnail", step);
        let dag_def = DagPipelineDefinition::new("Automatic Thumbnail", vec![dag_step]);

        if let Err(e) = self
            .create_dag_pipeline(
                session_id,
                streamer_id,
                vec![segment_path.to_string()],
                dag_def,
            )
            .await
        {
            tracing::error!(
                "Failed to create automatic thumbnail pipeline for session {}: {}",
                session_id,
                e
            );
        } else {
            debug!(
                "Created automatic thumbnail pipeline for first segment of session {}",
                session_id
            );
        }
    }

    /// Listen for download events and create jobs.
    pub fn listen_for_downloads(&self, mut rx: mpsc::Receiver<DownloadManagerEvent>) {
        let _job_queue = self.job_queue.clone();
        let _event_tx = self.event_tx.clone();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download event listener shutting down");
                        break;
                    }
                    event = rx.recv() => {
                        match event {
                            Some(DownloadManagerEvent::DownloadCompleted {
                                streamer_id,
                                session_id,
                                ..
                            }) => {
                                info!(
                                    "Creating post-processing jobs for {} / {}",
                                    streamer_id, session_id
                                );
                                // Jobs would be created based on pipeline configuration
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                }
            }
        });
    }

    /// Check queue depth and emit warnings.
    fn check_queue_depth(&self) {
        let depth = self.job_queue.depth();
        let status = self.job_queue.depth_status();

        let status_code = match status {
            QueueDepthStatus::Normal => 0,
            QueueDepthStatus::Warning => 1,
            QueueDepthStatus::Critical => 2,
        };

        let prev = self.last_queue_status.load(Ordering::Relaxed);
        if prev == status_code {
            return;
        }
        self.last_queue_status.store(status_code, Ordering::Relaxed);

        match status {
            QueueDepthStatus::Critical => {
                warn!("Queue depth critical: {} jobs", depth);
                let _ = self.event_tx.send(PipelineEvent::QueueCritical { depth });
            }
            QueueDepthStatus::Warning => {
                warn!("Queue depth warning: {} jobs", depth);
                let _ = self.event_tx.send(PipelineEvent::QueueWarning { depth });
            }
            QueueDepthStatus::Normal => {}
        }
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.job_queue.depth()
    }

    /// Get the queue depth status.
    pub fn queue_status(&self) -> QueueDepthStatus {
        self.job_queue.depth_status()
    }

    /// Check if throttling should be enabled.
    pub fn should_throttle(&self) -> bool {
        self.config.throttle.enabled && self.job_queue.is_critical()
    }

    // ========================================================================
    // Query and Management Methods
    // ========================================================================

    /// List jobs with filters and pagination.
    /// Delegates to JobQueue/JobRepository.
    pub async fn list_jobs(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<Job>, u64)> {
        self.job_queue.list_jobs(filters, pagination).await
    }

    /// List jobs with filters and pagination, without running a total `COUNT(*)`.
    pub async fn list_jobs_page(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<Job>> {
        self.job_queue.list_jobs_page(filters, pagination).await
    }

    /// List job execution logs (paged).
    pub async fn list_job_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobLogEntry>, u64)> {
        self.job_queue.list_job_logs(job_id, pagination).await
    }

    /// Get latest execution progress snapshot for a job (if available).
    pub async fn get_job_progress(&self, job_id: &str) -> Result<Option<JobProgressSnapshot>> {
        self.job_queue.get_job_progress(job_id).await
    }

    /// Get a job by ID.
    /// Retrieves job from repository.
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        self.job_queue.get_job(id).await
    }

    /// Retry a failed job.
    /// Delegates to JobQueue.
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        // If this is a DAG step job, retrying the underlying job is not enough: the parent DAG
        // must be reset to a non-terminal state and the step execution must be marked active
        // again so downstream steps can be scheduled when the job completes.
        let job_snapshot = self
            .job_queue
            .get_job(id)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))?;

        if job_snapshot.status != JobStatus::Failed && job_snapshot.status != JobStatus::Interrupted
        {
            return Err(Error::InvalidStateTransition {
                from: job_snapshot.status.as_str().to_string(),
                to: "PENDING".to_string(),
            });
        }

        if let Some(step_exec_id) = job_snapshot.dag_step_execution_id.as_deref() {
            let Some(dag_scheduler) = &self.dag_scheduler else {
                return Err(Error::Validation(
                    "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
                ));
            };

            let dag_id = match job_snapshot.pipeline_id.as_deref() {
                Some(existing_dag_id) => existing_dag_id.to_string(),
                None => dag_scheduler.get_step_execution(step_exec_id).await?.dag_id,
            };

            let dag = dag_scheduler.get_dag_status(&dag_id).await?;
            if matches!(
                dag.get_status(),
                Some(crate::database::models::DagExecutionStatus::Failed)
                    | Some(crate::database::models::DagExecutionStatus::Interrupted)
            ) {
                dag_scheduler.reset_dag_for_retry(&dag_id).await?;
            }
        }

        let job = self.job_queue.retry_job(id).await?;

        // Emit event for the retried job
        let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
            job_id: job.id.clone(),
            job_type: job.job_type.clone(),
            streamer_id: job.streamer_id.clone(),
        });

        // Check queue depth after retry
        self.check_queue_depth();

        Ok(job)
    }

    /// Cancel a job.
    /// For Pending jobs: removes from queue and marks as Interrupted.
    /// For Processing jobs: signals cancellation and marks as Interrupted.
    /// Returns error for Completed/Failed jobs.
    /// Delegates to JobQueue.
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
        let cancelled_job = self.job_queue.cancel_job(id).await?;

        // Emit JobFailed event for cancelled jobs (pipeline interrupted)
        let _ = self.event_tx.send(PipelineEvent::JobFailed {
            job_id: cancelled_job.id.clone(),
            job_type: cancelled_job.job_type.clone(),
            error: "Job cancelled".to_string(),
        });

        Ok(())
    }

    /// Delete a job.
    /// Only allows deleting jobs in terminal states (Completed, Failed, Interrupted).
    /// Removes from database and cache.
    /// Delegates to JobQueue.
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        self.job_queue.delete_job(id).await
    }

    /// Cancel all jobs in a pipeline.
    /// Cancels all pending and processing jobs that belong to the specified pipeline.
    /// Returns the number of jobs cancelled.
    pub async fn cancel_pipeline(&self, pipeline_id: &str) -> Result<usize> {
        let cancelled_jobs = self.job_queue.cancel_pipeline(pipeline_id).await?;

        // Emit events for each cancelled job
        for job in &cancelled_jobs {
            let _ = self.event_tx.send(PipelineEvent::JobFailed {
                job_id: job.id.clone(),
                job_type: job.job_type.clone(),
                error: "Pipeline cancelled".to_string(),
            });
        }

        Ok(cancelled_jobs.len())
    }

    /// List available job presets.
    pub async fn list_presets(&self) -> Result<Vec<crate::database::models::JobPreset>> {
        if let Some(repo) = &self.preset_repo {
            repo.list_presets().await
        } else {
            Ok(vec![])
        }
    }

    /// List job presets filtered by category.
    pub async fn list_presets_by_category(
        &self,
        category: Option<&str>,
    ) -> Result<Vec<crate::database::models::JobPreset>> {
        if let Some(repo) = &self.preset_repo {
            repo.list_presets_by_category(category).await
        } else {
            Ok(vec![])
        }
    }

    /// List job presets with filtering, searching, and pagination.
    pub async fn list_presets_filtered(
        &self,
        filters: &crate::database::repositories::JobPresetFilters,
        pagination: &crate::database::models::Pagination,
    ) -> Result<(Vec<crate::database::models::JobPreset>, u64)> {
        if let Some(repo) = &self.preset_repo {
            repo.list_presets_filtered(filters, pagination).await
        } else {
            Ok((vec![], 0))
        }
    }

    /// List all unique preset categories.
    pub async fn list_preset_categories(&self) -> Result<Vec<String>> {
        if let Some(repo) = &self.preset_repo {
            repo.list_categories().await
        } else {
            Ok(vec![])
        }
    }

    /// Get a job preset by ID.
    pub async fn get_preset(&self, id: &str) -> Result<Option<crate::database::models::JobPreset>> {
        if let Some(repo) = &self.preset_repo {
            repo.get_preset(id).await
        } else {
            Ok(None)
        }
    }

    /// Check if a preset name exists (optionally excluding a specific ID).
    pub async fn name_exists(&self, name: &str, exclude_id: Option<&str>) -> Result<bool> {
        if let Some(repo) = &self.preset_repo {
            repo.name_exists(name, exclude_id).await
        } else {
            Ok(false)
        }
    }

    /// Create a new job preset.
    pub async fn create_preset(&self, preset: &crate::database::models::JobPreset) -> Result<()> {
        if let Some(repo) = &self.preset_repo {
            repo.create_preset(preset).await
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Update an existing job preset.
    pub async fn update_preset(&self, preset: &crate::database::models::JobPreset) -> Result<()> {
        if let Some(repo) = &self.preset_repo {
            repo.update_preset(preset).await
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Delete a job preset.
    pub async fn delete_preset(&self, id: &str) -> Result<()> {
        if let Some(repo) = &self.preset_repo {
            repo.delete_preset(id).await
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Clone an existing job preset with a new name.
    ///
    /// Creates a copy of the preset with a new ID and name.
    /// The new name must be unique.
    pub async fn clone_preset(
        &self,
        source_id: &str,
        new_name: String,
    ) -> Result<crate::database::models::JobPreset> {
        if let Some(repo) = &self.preset_repo {
            // Get the source preset
            let source =
                repo.get_preset(source_id)
                    .await?
                    .ok_or_else(|| crate::Error::NotFound {
                        entity_type: "Preset".to_string(),
                        id: source_id.to_string(),
                    })?;

            // Check if the new name already exists
            if repo.name_exists(&new_name, None).await? {
                return Err(crate::Error::Validation(format!(
                    "A preset with name '{}' already exists",
                    new_name
                )));
            }

            // Create the cloned preset with a new ID
            let cloned = crate::database::models::JobPreset {
                id: uuid::Uuid::new_v4().to_string(),
                name: new_name,
                description: source.description.map(|d| format!("Copy of: {}", d)),
                category: source.category,
                processor: source.processor,
                config: source.config,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };

            repo.create_preset(&cloned).await?;
            Ok(cloned)
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Get comprehensive pipeline statistics.
    /// Returns counts by status (pending, processing, completed, failed)
    /// and average processing time.
    pub async fn get_stats(&self) -> Result<PipelineStats> {
        let job_stats = self.job_queue.get_stats().await?;

        Ok(PipelineStats {
            pending: job_stats.pending,
            processing: job_stats.processing,
            completed: job_stats.completed,
            failed: job_stats.failed,
            interrupted: job_stats.interrupted,
            avg_processing_time_secs: job_stats.avg_processing_time_secs,
            queue_depth: self.queue_depth(),
            queue_status: self.queue_status(),
        })
    }

    /// Persist a downloaded segment to the database.
    async fn persist_segment(&self, session_id: &str, path: &str, size_bytes: u64) {
        if let Some(repo) = &self.session_repo {
            let output = MediaOutputDbModel::new(
                session_id,
                path,
                MediaFileType::Video, // Assuming video segments for now
                size_bytes as i64,
            );

            if let Err(e) = repo.create_media_output(&output).await {
                tracing::error!(
                    "Failed to persist segment for session {}: {}",
                    session_id,
                    e
                );
            } else {
                debug!("Persisted segment for session {}", session_id);
            }
        }
    }

    /// Persist a danmu segment to the database.
    async fn persist_danmu_segment(&self, session_id: &str, path: &str, message_count: u64) {
        if let Some(repo) = &self.session_repo {
            // Get actual file size from disk
            let size_bytes = match tokio::fs::metadata(path).await {
                Ok(metadata) => metadata.len() as i64,
                Err(e) => {
                    tracing::warn!(
                        "Failed to get file size for danmu segment {}: {}, using 0",
                        path,
                        e
                    );
                    0
                }
            };

            let output =
                MediaOutputDbModel::new(session_id, path, MediaFileType::DanmuXml, size_bytes);

            if let Err(e) = repo.create_media_output(&output).await {
                tracing::error!(
                    "Failed to persist danmu segment for session {}: {}",
                    session_id,
                    e
                );
            } else {
                debug!(
                    "Persisted danmu segment for session {} ({} messages, {} bytes)",
                    session_id, message_count, size_bytes
                );
            }
        }
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
    /// Number of interrupted jobs.
    pub interrupted: u64,
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
mod tests {
    use super::*;
    use crate::database::models::{
        DagExecutionDbModel, DagStepExecutionDbModel, DanmuStatisticsDbModel, JobDbModel,
        JobExecutionLogDbModel, LiveSessionDbModel, OutputFilters, PipelinePreset, SessionFilters,
    };
    use crate::database::repositories::{PipelinePresetFilters, PipelinePresetRepository};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct TestSessionRepository {
        end_time: Mutex<Option<i64>>,
    }

    impl TestSessionRepository {
        fn new(end_time: Option<i64>) -> Self {
            Self {
                end_time: Mutex::new(end_time),
            }
        }
    }

    #[async_trait]
    impl SessionRepository for TestSessionRepository {
        async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel> {
            Ok(LiveSessionDbModel {
                id: id.to_string(),
                streamer_id: "streamer-1".to_string(),
                start_time: chrono::Utc::now().timestamp_millis(),
                end_time: *self.end_time.lock().expect("lock poisoned"),
                titles: Some("[]".to_string()),
                danmu_statistics_id: None,
                total_size_bytes: 0,
            })
        }

        async fn get_active_session_for_streamer(
            &self,
            _streamer_id: &str,
        ) -> Result<Option<LiveSessionDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_sessions_for_streamer(
            &self,
            _streamer_id: &str,
            _limit: i32,
        ) -> Result<Vec<LiveSessionDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn create_session(&self, _session: &LiveSessionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn end_session(&self, _id: &str, _end_time: i64) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn resume_session(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_session_titles(&self, _id: &str, _titles: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_session(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_sessions_batch(&self, _ids: &[String]) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn list_sessions_filtered(
            &self,
            _filters: &SessionFilters,
            _pagination: &Pagination,
        ) -> Result<(Vec<LiveSessionDbModel>, u64)> {
            unimplemented!("not needed for these tests")
        }

        async fn get_media_output(&self, _id: &str) -> Result<MediaOutputDbModel> {
            unimplemented!("not needed for these tests")
        }

        async fn get_media_outputs_for_session(
            &self,
            _session_id: &str,
        ) -> Result<Vec<MediaOutputDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn create_media_output(&self, _output: &MediaOutputDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_media_output(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_output_count(&self, _session_id: &str) -> Result<u32> {
            unimplemented!("not needed for these tests")
        }

        async fn list_outputs_filtered(
            &self,
            _filters: &OutputFilters,
            _pagination: &Pagination,
        ) -> Result<(Vec<MediaOutputDbModel>, u64)> {
            unimplemented!("not needed for these tests")
        }

        async fn get_danmu_statistics(
            &self,
            _session_id: &str,
        ) -> Result<Option<DanmuStatisticsDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn create_danmu_statistics(&self, _stats: &DanmuStatisticsDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_danmu_statistics(&self, _stats: &DanmuStatisticsDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn upsert_danmu_statistics(
            &self,
            _session_id: &str,
            _total_danmus: i64,
            _danmu_rate_timeseries: Option<&str>,
            _top_talkers: Option<&str>,
            _word_frequency: Option<&str>,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }
    }

    struct TestDagRepository {
        dags: Mutex<HashMap<String, DagExecutionDbModel>>,
    }

    impl TestDagRepository {
        fn new() -> Self {
            Self {
                dags: Mutex::new(HashMap::new()),
            }
        }

        fn insert(&self, dag: DagExecutionDbModel) {
            self.dags
                .lock()
                .expect("lock poisoned")
                .insert(dag.id.clone(), dag);
        }
    }

    struct TestJobRepository {
        jobs: Mutex<HashMap<String, JobDbModel>>,
    }

    impl TestJobRepository {
        fn new() -> Self {
            Self {
                jobs: Mutex::new(HashMap::new()),
            }
        }

        fn insert(&self, job: JobDbModel) {
            self.jobs
                .lock()
                .expect("lock poisoned")
                .insert(job.id.clone(), job);
        }
    }

    #[async_trait]
    impl crate::database::repositories::JobRepository for TestJobRepository {
        async fn get_job(&self, id: &str) -> Result<JobDbModel> {
            self.jobs
                .lock()
                .expect("lock poisoned")
                .get(id)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("Job", id))
        }

        async fn list_pending_jobs(&self, _job_type: &str) -> Result<Vec<JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_jobs_by_status(&self, _status: &str) -> Result<Vec<JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_recent_jobs(&self, _limit: i32) -> Result<Vec<JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn create_job(&self, job: &JobDbModel) -> Result<()> {
            self.insert(job.clone());
            Ok(())
        }

        async fn update_job_status(&self, _id: &str, _status: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn mark_job_failed(&self, _id: &str, _error: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn mark_job_interrupted(&self, _id: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_job_for_retry(&self, id: &str) -> Result<()> {
            let now = chrono::Utc::now().timestamp_millis();
            let mut jobs = self.jobs.lock().expect("lock poisoned");
            let job = jobs
                .get_mut(id)
                .ok_or_else(|| crate::Error::not_found("Job", id))?;

            match job.status.as_str() {
                "FAILED" | "INTERRUPTED" => {
                    job.status = "PENDING".to_string();
                    job.started_at = None;
                    job.completed_at = None;
                    job.error = None;
                    job.retry_count += 1;
                    job.updated_at = now;
                    Ok(())
                }
                other => Err(crate::Error::InvalidStateTransition {
                    from: other.to_ascii_uppercase(),
                    to: "PENDING".to_string(),
                }),
            }
        }

        async fn count_pending_jobs(&self, _job_types: Option<&[String]>) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn upsert_job_execution_progress(
            &self,
            _progress: &crate::database::models::JobExecutionProgressDbModel,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job_execution_progress(
            &self,
            _job_id: &str,
        ) -> Result<Option<crate::database::models::JobExecutionProgressDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn claim_next_pending_job(
            &self,
            _job_types: Option<&[String]>,
        ) -> Result<Option<JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job_execution_info(&self, _id: &str) -> Result<Option<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_execution_info(&self, _id: &str, _execution_info: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_state(&self, _id: &str, _state: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job(&self, _job: &JobDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_if_status(
            &self,
            _job: &JobDbModel,
            _expected_status: &str,
        ) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_interrupted_jobs(&self) -> Result<i32> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_processing_jobs(&self) -> Result<i32> {
            unimplemented!("not needed for these tests")
        }

        async fn cleanup_old_jobs(&self, _retention_days: i32) -> Result<i32> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_job(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn purge_jobs_older_than(&self, _days: u32, _batch_size: u32) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn get_purgeable_jobs(&self, _days: u32, _limit: u32) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn add_execution_log(&self, _log: &JobExecutionLogDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn add_execution_logs(&self, _logs: &[JobExecutionLogDbModel]) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_execution_logs(&self, _job_id: &str) -> Result<Vec<JobExecutionLogDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_execution_logs(
            &self,
            _job_id: &str,
            _pagination: &crate::database::models::Pagination,
        ) -> Result<(Vec<JobExecutionLogDbModel>, u64)> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_execution_logs_for_job(&self, _job_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn list_jobs_filtered(
            &self,
            _filters: &crate::database::models::JobFilters,
            _pagination: &crate::database::models::Pagination,
        ) -> Result<(Vec<JobDbModel>, u64)> {
            unimplemented!("not needed for these tests")
        }

        async fn list_jobs_page_filtered(
            &self,
            _filters: &crate::database::models::JobFilters,
            _pagination: &crate::database::models::Pagination,
        ) -> Result<Vec<JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job_counts_by_status(&self) -> Result<crate::database::models::JobCounts> {
            unimplemented!("not needed for these tests")
        }

        async fn get_avg_processing_time(&self) -> Result<Option<f64>> {
            unimplemented!("not needed for these tests")
        }

        async fn cancel_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn get_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<Vec<JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestDagRepositoryForRetry {
        dags: Mutex<HashMap<String, DagExecutionDbModel>>,
        reset_calls: AtomicUsize,
    }

    impl TestDagRepositoryForRetry {
        fn new() -> Self {
            Self {
                dags: Mutex::new(HashMap::new()),
                reset_calls: AtomicUsize::new(0),
            }
        }

        fn insert(&self, dag: DagExecutionDbModel) {
            self.dags
                .lock()
                .expect("lock poisoned")
                .insert(dag.id.clone(), dag);
        }

        fn reset_calls(&self) -> usize {
            self.reset_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DagRepository for TestDagRepositoryForRetry {
        async fn create_dag(&self, _dag: &DagExecutionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
            self.dags
                .lock()
                .expect("lock poisoned")
                .get(id)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("DAG execution", id))
        }

        async fn update_dag_status(
            &self,
            _id: &str,
            _status: &str,
            _error: Option<&str>,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn increment_dag_completed(&self, _dag_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn increment_dag_failed(&self, _dag_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn list_dags(
            &self,
            _status: Option<&str>,
            _session_id: Option<&str>,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<DagExecutionDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn count_dags(
            &self,
            _status: Option<&str>,
            _session_id: Option<&str>,
        ) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_dag(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn create_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn create_steps(&self, _steps: &[DagStepExecutionDbModel]) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_step(&self, _id: &str) -> Result<DagStepExecutionDbModel> {
            unimplemented!("not needed for these tests")
        }

        async fn get_step_by_dag_and_step_id(
            &self,
            _dag_id: &str,
            _step_id: &str,
        ) -> Result<DagStepExecutionDbModel> {
            unimplemented!("not needed for these tests")
        }

        async fn get_steps_by_dag(&self, _dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
            Ok(Vec::new())
        }

        async fn update_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_step_status(&self, _id: &str, _status: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_step_status_with_job(
            &self,
            _id: &str,
            _status: &str,
            _job_id: &str,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn complete_step_and_check_dependents(
            &self,
            _step_id: &str,
            _outputs: &[String],
        ) -> Result<Vec<crate::database::models::ReadyStep>> {
            unimplemented!("not needed for these tests")
        }

        async fn fail_dag_and_cancel_steps(
            &self,
            _dag_id: &str,
            _error: &str,
        ) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()> {
            self.reset_calls.fetch_add(1, Ordering::SeqCst);
            let mut dags = self.dags.lock().expect("lock poisoned");
            let dag = dags
                .get_mut(dag_id)
                .ok_or_else(|| crate::Error::not_found("DAG execution", dag_id))?;
            dag.status = crate::database::models::DagExecutionStatus::Processing
                .as_str()
                .to_string();
            dag.completed_at = None;
            dag.error = None;
            Ok(())
        }

        async fn get_dependency_outputs(
            &self,
            _dag_id: &str,
            _step_ids: &[String],
        ) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn check_all_dependencies_complete(
            &self,
            _dag_id: &str,
            _step_id: &str,
        ) -> Result<bool> {
            unimplemented!("not needed for these tests")
        }

        async fn get_dag_stats(
            &self,
            _dag_id: &str,
        ) -> Result<crate::database::models::DagExecutionStats> {
            unimplemented!("not needed for these tests")
        }

        async fn get_processing_job_ids(&self, _dag_id: &str) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn get_pending_root_steps(
            &self,
            _dag_id: &str,
        ) -> Result<Vec<DagStepExecutionDbModel>> {
            unimplemented!("not needed for these tests")
        }
    }

    #[async_trait]
    impl DagRepository for TestDagRepository {
        async fn create_dag(&self, _dag: &DagExecutionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
            self.dags
                .lock()
                .expect("lock poisoned")
                .get(id)
                .cloned()
                .ok_or_else(|| crate::Error::not_found("DAG execution", id))
        }

        async fn update_dag_status(
            &self,
            _id: &str,
            _status: &str,
            _error: Option<&str>,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn increment_dag_completed(&self, _dag_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn increment_dag_failed(&self, _dag_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn list_dags(
            &self,
            _status: Option<&str>,
            _session_id: Option<&str>,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<DagExecutionDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn count_dags(
            &self,
            _status: Option<&str>,
            _session_id: Option<&str>,
        ) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_dag(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn create_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn create_steps(&self, _steps: &[DagStepExecutionDbModel]) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_step(&self, _id: &str) -> Result<DagStepExecutionDbModel> {
            unimplemented!("not needed for these tests")
        }

        async fn get_step_by_dag_and_step_id(
            &self,
            _dag_id: &str,
            _step_id: &str,
        ) -> Result<DagStepExecutionDbModel> {
            unimplemented!("not needed for these tests")
        }

        async fn get_steps_by_dag(&self, _dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn update_step(&self, _step: &DagStepExecutionDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_step_status(&self, _id: &str, _status: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_step_status_with_job(
            &self,
            _id: &str,
            _status: &str,
            _job_id: &str,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn complete_step_and_check_dependents(
            &self,
            _step_id: &str,
            _outputs: &[String],
        ) -> Result<Vec<crate::database::models::ReadyStep>> {
            unimplemented!("not needed for these tests")
        }

        async fn fail_dag_and_cancel_steps(
            &self,
            _dag_id: &str,
            _error: &str,
        ) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_dag_for_retry(&self, _dag_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_dependency_outputs(
            &self,
            _dag_id: &str,
            _step_ids: &[String],
        ) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn check_all_dependencies_complete(
            &self,
            _dag_id: &str,
            _step_id: &str,
        ) -> Result<bool> {
            unimplemented!("not needed for these tests")
        }

        async fn get_dag_stats(
            &self,
            _dag_id: &str,
        ) -> Result<crate::database::models::DagExecutionStats> {
            unimplemented!("not needed for these tests")
        }

        async fn get_processing_job_ids(&self, _dag_id: &str) -> Result<Vec<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn get_pending_root_steps(
            &self,
            _dag_id: &str,
        ) -> Result<Vec<DagStepExecutionDbModel>> {
            unimplemented!("not needed for these tests")
        }
    }

    #[test]
    fn test_pipeline_manager_config_default() {
        let config = PipelineManagerConfig::default();
        assert_eq!(config.cpu_pool.max_workers, 2);
        assert_eq!(config.io_pool.max_workers, 4);
        assert_eq!(config.execute_timeout_secs, 3600);
        // Verify throttle config defaults
        assert!(!config.throttle.enabled);
        assert_eq!(config.throttle.critical_threshold, 500);
        assert_eq!(config.throttle.warning_threshold, 100);
        // Verify purge config defaults
        assert_eq!(config.purge.retention_days, 30);
        assert_eq!(config.purge.batch_size, 100);
        assert!(config.purge.time_window.is_some());
    }

    #[test]
    fn test_pipeline_manager_creation() {
        let manager: PipelineManager = PipelineManager::new();
        assert_eq!(manager.queue_depth(), 0);
        assert_eq!(manager.queue_status(), QueueDepthStatus::Normal);
    }

    #[tokio::test]
    async fn test_retry_job_resets_failed_dag_when_job_is_dag_step() {
        let job_repo = Arc::new(TestJobRepository::new());
        let dag_repo = Arc::new(TestDagRepositoryForRetry::new());

        let dag_def = crate::database::models::DagPipelineDefinition::new(
            "test",
            vec![crate::database::models::DagStep::new(
                "step-a",
                crate::database::models::PipelineStep::Inline {
                    processor: "remux".to_string(),
                    config: serde_json::json!({}),
                },
            )],
        );

        let mut dag = DagExecutionDbModel::new(&dag_def, None, None);
        dag.status = crate::database::models::DagExecutionStatus::Failed
            .as_str()
            .to_string();
        let dag_id = dag.id.clone();
        dag_repo.insert(dag);

        let mut job = JobDbModel::new_pipeline_step(
            "remux",
            serde_json::to_string(&vec!["/in.flv".to_string()]).unwrap(),
            "[]",
            0,
            None,
            None,
        );
        job.pipeline_id = Some(dag_id.clone());
        job.dag_step_execution_id = Some("step-exec-1".to_string());
        job.status = "FAILED".to_string();
        job.error = Some("boom".to_string());
        job.completed_at = Some(chrono::Utc::now().timestamp_millis());
        let job_id = job.id.clone();
        job_repo.insert(job);

        let mut config = PipelineManagerConfig::default();
        config.purge.retention_days = 0;

        let manager: PipelineManager = PipelineManager::with_repository(config, job_repo)
            .with_dag_repository(dag_repo.clone());

        let retried = manager.retry_job(&job_id).await.unwrap();
        assert_eq!(
            retried.status,
            crate::pipeline::job_queue::JobStatus::Pending
        );
        assert_eq!(retried.retry_count, 1);
        assert!(retried.error.is_none());

        assert_eq!(dag_repo.reset_calls(), 1);
    }

    #[test]
    fn test_set_worker_concurrency_clamps_to_max_workers() {
        let manager: PipelineManager = PipelineManager::new();

        // Defaults are 2/4, so requests above should clamp.
        manager.set_worker_concurrency(10, 20);
        assert_eq!(manager.cpu_pool.desired_max_workers(), 2);
        assert_eq!(manager.io_pool.desired_max_workers(), 4);

        // Requests below max should apply.
        manager.set_worker_concurrency(1, 3);
        assert_eq!(manager.cpu_pool.desired_max_workers(), 1);
        assert_eq!(manager.io_pool.desired_max_workers(), 3);
    }

    struct TestPipelinePresetRepository {
        preset: PipelinePreset,
    }

    #[async_trait]
    impl PipelinePresetRepository for TestPipelinePresetRepository {
        async fn list_pipeline_presets(&self) -> Result<Vec<PipelinePreset>> {
            Ok(vec![])
        }

        async fn list_pipeline_presets_filtered(
            &self,
            _filters: &PipelinePresetFilters,
            _pagination: &Pagination,
        ) -> Result<(Vec<PipelinePreset>, u64)> {
            Ok((vec![], 0))
        }

        async fn get_pipeline_preset(&self, _id: &str) -> Result<Option<PipelinePreset>> {
            Ok(None)
        }

        async fn get_pipeline_preset_by_name(&self, name: &str) -> Result<Option<PipelinePreset>> {
            if name == self.preset.name {
                Ok(Some(self.preset.clone()))
            } else {
                Ok(None)
            }
        }

        async fn create_pipeline_preset(&self, _preset: &PipelinePreset) -> Result<()> {
            Ok(())
        }

        async fn update_pipeline_preset(&self, _preset: &PipelinePreset) -> Result<()> {
            Ok(())
        }

        async fn delete_pipeline_preset(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_expand_workflows_with_duplicate_names() {
        let workflow_dag = DagPipelineDefinition::new(
            "wf",
            vec![
                DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({}))),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::inline("noop", serde_json::json!({})),
                    vec!["A".to_string()],
                ),
            ],
        );
        let repo = Arc::new(TestPipelinePresetRepository {
            preset: PipelinePreset::new("wf", workflow_dag),
        });

        let manager: PipelineManager = PipelineManager::new().with_pipeline_preset_repository(repo);

        let parent = DagPipelineDefinition::new(
            "parent",
            vec![
                DagStep {
                    id: "W1".to_string(),
                    step: PipelineStep::Workflow {
                        name: "wf".to_string(),
                    },
                    depends_on: vec![],
                },
                DagStep {
                    id: "W2".to_string(),
                    step: PipelineStep::Workflow {
                        name: "wf".to_string(),
                    },
                    depends_on: vec!["W1".to_string()],
                },
                DagStep::with_dependencies(
                    "Z",
                    PipelineStep::inline("noop", serde_json::json!({})),
                    vec!["W2".to_string()],
                ),
            ],
        );

        let expanded = manager.expand_workflows_in_dag(parent).await.unwrap();

        let mut deps_by_id: HashMap<String, Vec<String>> = HashMap::new();
        for step in &expanded.steps {
            deps_by_id.insert(step.id.clone(), step.depends_on.clone());
        }

        assert!(!deps_by_id.contains_key("W1"));
        assert!(!deps_by_id.contains_key("W2"));

        assert_eq!(deps_by_id.get("W1__A").unwrap(), &Vec::<String>::new());
        assert_eq!(deps_by_id.get("W1__B").unwrap(), &vec!["W1__A".to_string()]);

        // W2 depends on the *leaf* of W1 after expansion.
        assert_eq!(deps_by_id.get("W2__A").unwrap(), &vec!["W1__B".to_string()]);
        assert_eq!(deps_by_id.get("W2__B").unwrap(), &vec!["W2__A".to_string()]);

        // Z depends on the *leaf* of W2 after expansion.
        assert_eq!(deps_by_id.get("Z").unwrap(), &vec!["W2__B".to_string()]);
    }

    #[tokio::test]
    async fn test_enqueue_job() {
        let manager: PipelineManager = PipelineManager::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec!["/output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let job_id = manager.enqueue(job).await.unwrap();

        assert!(!job_id.is_empty());
        assert_eq!(manager.queue_depth(), 1);
    }

    #[tokio::test]
    async fn test_create_remux_job() {
        let manager: PipelineManager = PipelineManager::new();

        let job_id = manager
            .create_remux_job("/input.flv", "/output.mp4", "streamer-1", "session-1")
            .await
            .unwrap();

        assert!(!job_id.is_empty());
    }

    #[tokio::test]
    async fn test_list_jobs() {
        use crate::database::models::{JobFilters, Pagination};

        let manager: PipelineManager = PipelineManager::new();

        // Enqueue some jobs
        let job1 = Job::new(
            "remux",
            vec!["/input1.flv".to_string()],
            vec!["/output1.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let job2 = Job::new(
            "upload",
            vec!["/input2.flv".to_string()],
            vec!["/output2.mp4".to_string()],
            "streamer-2",
            "session-2",
        );
        manager.enqueue(job1).await.unwrap();
        manager.enqueue(job2).await.unwrap();

        // List all jobs
        let filters = JobFilters::default();
        let pagination = Pagination::new(10, 0);
        let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

        assert_eq!(total, 2);
        assert_eq!(jobs.len(), 2);
    }

    #[tokio::test]
    async fn test_list_jobs_with_filter() {
        use crate::database::models::{JobFilters, Pagination};

        let manager: PipelineManager = PipelineManager::new();

        // Enqueue jobs for different streamers
        let job1 = Job::new(
            "remux",
            vec!["/input1.flv".to_string()],
            vec!["/output1.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let job2 = Job::new(
            "upload",
            vec!["/input2.flv".to_string()],
            vec!["/output2.mp4".to_string()],
            "streamer-2",
            "session-2",
        );
        manager.enqueue(job1).await.unwrap();
        manager.enqueue(job2).await.unwrap();

        // Filter by streamer_id
        let filters = JobFilters::new().with_streamer_id("streamer-1");
        let pagination = Pagination::new(10, 0);
        let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

        assert_eq!(total, 1);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].streamer_id, "streamer-1");
    }

    #[tokio::test]
    async fn test_get_job() {
        let manager: PipelineManager = PipelineManager::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec!["/output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        manager.enqueue(job).await.unwrap();

        // Get existing job
        let retrieved = manager.get_job(&job_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, job_id);

        // Get non-existing job
        let not_found = manager.get_job("non-existent-id").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_get_stats() {
        let manager: PipelineManager = PipelineManager::new();

        // Enqueue some jobs
        let job1 = Job::new(
            "remux",
            vec!["/input1.flv".to_string()],
            vec!["/output1.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let job2 = Job::new(
            "upload",
            vec!["/input2.flv".to_string()],
            vec!["/output2.mp4".to_string()],
            "streamer-2",
            "session-2",
        );
        manager.enqueue(job1).await.unwrap();
        manager.enqueue(job2).await.unwrap();

        let stats = manager.get_stats().await.unwrap();

        assert_eq!(stats.pending, 2);
        assert_eq!(stats.processing, 0);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.queue_depth, 2);
        assert_eq!(stats.queue_status, QueueDepthStatus::Normal);
    }

    #[tokio::test]
    async fn test_cancel_pending_job() {
        use crate::pipeline::JobStatus;

        let manager: PipelineManager = PipelineManager::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec!["/output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        manager.enqueue(job).await.unwrap();

        // Cancel the pending job
        manager.cancel_job(&job_id).await.unwrap();

        // Verify job is now interrupted
        let cancelled = manager.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, JobStatus::Interrupted);
    }

    #[test]
    fn test_throttle_controller_disabled_by_default() {
        let manager: PipelineManager = PipelineManager::new();

        // Throttle controller should be None when disabled
        assert!(manager.throttle_controller().is_none());
        assert!(!manager.is_throttled());
        assert!(manager.subscribe_throttle_events().is_none());
    }

    #[test]
    fn test_throttle_controller_enabled_with_config() {
        let config = PipelineManagerConfig {
            throttle: ThrottleConfig {
                enabled: true,
                critical_threshold: 100,
                warning_threshold: 50,
                ..Default::default()
            },
            ..Default::default()
        };
        let manager: PipelineManager = PipelineManager::with_config(config);

        // Throttle controller should be Some when enabled
        assert!(manager.throttle_controller().is_some());
        assert!(!manager.is_throttled());
        assert!(manager.subscribe_throttle_events().is_some());
    }

    #[test]
    fn test_config_includes_throttle_defaults() {
        let config = PipelineManagerConfig::default();

        assert!(!config.throttle.enabled);
        assert_eq!(config.throttle.critical_threshold, 500);
        assert_eq!(config.throttle.warning_threshold, 100);
        assert!((config.throttle.reduction_factor - 0.5).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_create_dag_pipeline_requires_dag_scheduler() {
        use crate::database::models::job::{DagPipelineDefinition, DagStep, PipelineStep};

        let manager: PipelineManager = PipelineManager::new();

        // Create a simple DAG definition
        let dag_def = DagPipelineDefinition::new(
            "Test Pipeline",
            vec![DagStep::new("remux", PipelineStep::preset("remux"))],
        );

        // Without a DAG scheduler configured, this should fail
        let result = manager
            .create_dag_pipeline(
                "session-1",
                "streamer-1",
                vec!["/input.flv".to_string()],
                dag_def,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("DAG scheduler not configured"));
    }

    #[tokio::test]
    async fn test_session_complete_waits_for_session_end_time() {
        let session_repo = Arc::new(TestSessionRepository::new(None));
        let manager: PipelineManager = PipelineManager::new().with_session_repository(session_repo);

        let session_id = "session-1".to_string();
        let streamer_id = "streamer-1".to_string();

        // Capture a (empty) session-complete pipeline definition so we can assert gating behavior
        // without needing a DAG scheduler configured.
        manager.session_complete_pipelines.insert(
            session_id.clone(),
            SessionCompletePipelineEntry {
                last_seen: std::time::Instant::now(),
                definition: DagPipelineDefinition::new("empty", vec![]),
            },
        );

        manager
            .session_complete_coordinator
            .init_session(&session_id, &streamer_id, false);
        manager.session_complete_coordinator.on_raw_segment(
            &session_id,
            0,
            PathBuf::from("/seg0.mp4"),
            SourceType::Video,
        );
        manager
            .session_complete_coordinator
            .on_video_complete(&session_id);

        assert!(
            manager
                .session_complete_coordinator
                .is_ready_nonempty(&session_id)
        );

        // Should not trigger (and therefore not consume coordinator state) while end_time is NULL.
        manager.try_trigger_session_complete(&session_id).await;
        assert_eq!(manager.queue_depth(), 0);
        assert_eq!(
            manager.session_complete_coordinator.active_session_count(),
            1
        );
        assert!(manager.session_complete_pipelines.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_session_complete_triggers_after_session_end_time() {
        let session_repo = Arc::new(TestSessionRepository::new(Some(
            chrono::Utc::now().timestamp_millis(),
        )));
        let manager: PipelineManager = PipelineManager::new().with_session_repository(session_repo);

        let session_id = "session-1".to_string();
        let streamer_id = "streamer-1".to_string();

        manager.session_complete_pipelines.insert(
            session_id.clone(),
            SessionCompletePipelineEntry {
                last_seen: std::time::Instant::now(),
                definition: DagPipelineDefinition::new("empty", vec![]),
            },
        );

        manager
            .session_complete_coordinator
            .init_session(&session_id, &streamer_id, false);
        manager.session_complete_coordinator.on_raw_segment(
            &session_id,
            0,
            PathBuf::from("/seg0.mp4"),
            SourceType::Video,
        );
        manager
            .session_complete_coordinator
            .on_video_complete(&session_id);

        manager.try_trigger_session_complete(&session_id).await;
        assert_eq!(manager.queue_depth(), 0);
        assert_eq!(
            manager.session_complete_coordinator.active_session_count(),
            0
        );
        assert!(!manager.session_complete_pipelines.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_session_complete_waits_for_paired_dags() {
        let session_repo = Arc::new(TestSessionRepository::new(Some(
            chrono::Utc::now().timestamp_millis(),
        )));
        let manager: PipelineManager = PipelineManager::new().with_session_repository(session_repo);

        let session_id = "session-1".to_string();
        let streamer_id = "streamer-1".to_string();

        manager.session_complete_pipelines.insert(
            session_id.clone(),
            SessionCompletePipelineEntry {
                last_seen: std::time::Instant::now(),
                definition: DagPipelineDefinition::new("empty", vec![]),
            },
        );

        manager
            .session_complete_coordinator
            .init_session(&session_id, &streamer_id, false);
        manager.session_complete_coordinator.on_raw_segment(
            &session_id,
            0,
            PathBuf::from("/seg0.mp4"),
            SourceType::Video,
        );
        manager
            .session_complete_coordinator
            .on_video_complete(&session_id);

        manager
            .session_complete_coordinator
            .on_paired_dag_started(&session_id);

        manager.try_trigger_session_complete(&session_id).await;
        assert_eq!(
            manager.session_complete_coordinator.active_session_count(),
            1
        );
        assert!(manager.session_complete_pipelines.contains_key(&session_id));

        manager
            .session_complete_coordinator
            .on_paired_dag_complete(&session_id);
        manager.try_trigger_session_complete(&session_id).await;
        assert_eq!(
            manager.session_complete_coordinator.active_session_count(),
            0
        );
        assert!(!manager.session_complete_pipelines.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_session_complete_recovers_segment_dag_completion_without_context() {
        let session_repo = Arc::new(TestSessionRepository::new(Some(
            chrono::Utc::now().timestamp_millis(),
        )));
        let dag_repo = Arc::new(TestDagRepository::new());
        let manager: PipelineManager = PipelineManager::new()
            .with_session_repository(session_repo)
            .with_dag_repository(dag_repo.clone());

        let session_id = "session-1".to_string();
        let streamer_id = "streamer-1".to_string();

        manager.session_complete_pipelines.insert(
            session_id.clone(),
            SessionCompletePipelineEntry {
                last_seen: std::time::Instant::now(),
                definition: DagPipelineDefinition::new("empty", vec![]),
            },
        );

        manager
            .session_complete_coordinator
            .init_session(&session_id, &streamer_id, false);
        manager
            .session_complete_coordinator
            .on_dag_started(&session_id, SourceType::Video);
        manager
            .session_complete_coordinator
            .on_video_complete(&session_id);

        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );
        let mut dag = DagExecutionDbModel::new(
            &dag_def,
            Some(streamer_id.clone()),
            Some(session_id.clone()),
        );
        dag.segment_index = Some(0);
        dag.segment_source = Some("video".to_string());
        let dag_id = dag.id.clone();
        dag_repo.insert(dag);

        manager
            .handle_dag_completion(DagCompletionInfo {
                dag_id,
                streamer_id: Some(streamer_id),
                session_id: Some(session_id.clone()),
                succeeded: true,
                leaf_outputs: vec!["/out.mp4".to_string()],
            })
            .await;

        assert_eq!(
            manager.session_complete_coordinator.active_session_count(),
            0
        );
        assert!(!manager.session_complete_pipelines.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_session_complete_recovers_paired_dag_completion_without_context() {
        let session_repo = Arc::new(TestSessionRepository::new(Some(
            chrono::Utc::now().timestamp_millis(),
        )));
        let dag_repo = Arc::new(TestDagRepository::new());
        let manager: PipelineManager = PipelineManager::new()
            .with_session_repository(session_repo)
            .with_dag_repository(dag_repo.clone());

        let session_id = "session-1".to_string();
        let streamer_id = "streamer-1".to_string();

        manager.session_complete_pipelines.insert(
            session_id.clone(),
            SessionCompletePipelineEntry {
                last_seen: std::time::Instant::now(),
                definition: DagPipelineDefinition::new("empty", vec![]),
            },
        );

        manager
            .session_complete_coordinator
            .init_session(&session_id, &streamer_id, false);
        manager.session_complete_coordinator.on_raw_segment(
            &session_id,
            0,
            PathBuf::from("/seg0.mp4"),
            SourceType::Video,
        );
        manager
            .session_complete_coordinator
            .on_video_complete(&session_id);
        manager
            .session_complete_coordinator
            .on_paired_dag_started(&session_id);

        let dag_def = DagPipelineDefinition::new(
            "paired-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );
        let mut dag = DagExecutionDbModel::new(
            &dag_def,
            Some(streamer_id.clone()),
            Some(session_id.clone()),
        );
        dag.segment_index = Some(0);
        dag.segment_source = Some("paired".to_string());
        let dag_id = dag.id.clone();
        dag_repo.insert(dag);

        manager
            .handle_dag_completion(DagCompletionInfo {
                dag_id,
                streamer_id: Some(streamer_id),
                session_id: Some(session_id.clone()),
                succeeded: true,
                leaf_outputs: Vec::new(),
            })
            .await;

        assert_eq!(
            manager.session_complete_coordinator.active_session_count(),
            0
        );
        assert!(!manager.session_complete_pipelines.contains_key(&session_id));
    }

    #[tokio::test]
    async fn test_paired_segment_recovers_segment_dag_completion_without_context() {
        let dag_repo = Arc::new(TestDagRepository::new());
        let manager: PipelineManager = PipelineManager::new().with_dag_repository(dag_repo.clone());

        let session_id = "session-1".to_string();
        let streamer_id = "streamer-1".to_string();

        manager.paired_segment_pipelines.insert(
            session_id.clone(),
            PairedSegmentPipelineEntry {
                last_seen: std::time::Instant::now(),
                definition: DagPipelineDefinition::new("empty", vec![]),
            },
        );

        let dag_def = DagPipelineDefinition::new(
            "segment-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );

        let mut video_dag = DagExecutionDbModel::new(
            &dag_def,
            Some(streamer_id.clone()),
            Some(session_id.clone()),
        );
        video_dag.segment_index = Some(0);
        video_dag.segment_source = Some("video".to_string());
        let video_dag_id = video_dag.id.clone();
        dag_repo.insert(video_dag);

        manager
            .handle_dag_completion(DagCompletionInfo {
                dag_id: video_dag_id,
                streamer_id: Some(streamer_id.clone()),
                session_id: Some(session_id.clone()),
                succeeded: true,
                leaf_outputs: vec!["/v.mp4".to_string()],
            })
            .await;
        assert_eq!(manager.paired_segment_coordinator.active_pair_count(), 1);

        let mut danmu_dag = DagExecutionDbModel::new(
            &dag_def,
            Some(streamer_id.clone()),
            Some(session_id.clone()),
        );
        danmu_dag.segment_index = Some(0);
        danmu_dag.segment_source = Some("danmu".to_string());
        let danmu_dag_id = danmu_dag.id.clone();
        dag_repo.insert(danmu_dag);

        manager
            .handle_dag_completion(DagCompletionInfo {
                dag_id: danmu_dag_id,
                streamer_id: Some(streamer_id),
                session_id: Some(session_id.clone()),
                succeeded: true,
                leaf_outputs: vec!["/d.ass".to_string()],
            })
            .await;
        assert_eq!(manager.paired_segment_coordinator.active_pair_count(), 0);
        assert!(manager.paired_segment_pipelines.contains_key(&session_id));
    }
}
