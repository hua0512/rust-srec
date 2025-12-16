//! Pipeline Manager implementation.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::job_queue::{Job, JobLogEntry, JobQueue, JobQueueConfig, QueueDepthStatus};
use super::processors::{
    AudioExtractProcessor, CompressionProcessor, CopyMoveProcessor, DeleteProcessor,
    ExecuteCommandProcessor, MetadataProcessor, Processor, RcloneProcessor, RemuxProcessor,
    ThumbnailProcessor,
};
use super::progress::JobProgressSnapshot;
use super::purge::{JobPurgeService, PurgeConfig};
use super::throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
use super::worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
use crate::Result;
use crate::config::ConfigService;
use crate::database::models::job::PipelineStep;
use crate::database::models::{JobFilters, MediaFileType, MediaOutputDbModel, Pagination};
use crate::database::repositories::config::{ConfigRepository, SqlxConfigRepository};
use crate::database::repositories::streamer::{SqlxStreamerRepository, StreamerRepository};
use crate::database::repositories::{JobPresetRepository, JobRepository, SessionRepository};
use crate::downloader::DownloadManagerEvent;

/// Configuration for the Pipeline Manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineManagerConfig {
    /// Job queue configuration.
    pub job_queue: JobQueueConfig,
    /// CPU worker pool configuration.
    pub cpu_pool: WorkerPoolConfig,
    /// IO worker pool configuration.
    pub io_pool: WorkerPoolConfig,
    /// Whether to enable download throttling on backpressure.
    /// Deprecated: Use throttle.enabled instead.
    pub enable_throttling: bool,
    /// Throttle controller configuration.
    /// Requirements: 8.1, 8.2, 8.3, 8.4, 8.5
    #[serde(default)]
    pub throttle: ThrottleConfig,
    /// Job purge service configuration.
    /// Requirements: 7.1, 7.2, 7.3, 7.4, 7.5
    #[serde(default)]
    pub purge: PurgeConfig,
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
            enable_throttling: false,
            throttle: ThrottleConfig::default(),
            purge: PurgeConfig::default(),
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
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Throttle controller for download backpressure management.
    /// Requirements: 8.1, 8.2, 8.3, 8.4, 8.5
    throttle_controller: Option<Arc<ThrottleController>>,
    /// Download limit adjuster for throttle controller integration.
    download_adjuster: Option<Arc<dyn DownloadLimitAdjuster>>,
    /// Job purge service for automatic cleanup of old jobs.
    /// Requirements: 7.1, 7.2, 7.3, 7.4, 7.5
    purge_service: Option<Arc<JobPurgeService>>,
    /// Job preset repository for resolving named pipeline steps.
    preset_repo: Option<Arc<dyn JobPresetRepository>>,
    /// Config service for resolving pipeline rules.
    config_service: Option<Arc<ConfigService<CR, SR>>>,
    /// Last observed queue depth status (edge-trigger warnings).
    last_queue_status: AtomicU8,
}

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    /// Create a new Pipeline Manager.
    pub fn new() -> Self {
        Self::with_config(PipelineManagerConfig::default())
    }

    /// Create a new Pipeline Manager with custom configuration.
    pub fn with_config(config: PipelineManagerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_config(config.job_queue.clone()));

        // Create default processors
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(RcloneProcessor::new()),
            Arc::new(ExecuteCommandProcessor::new()),
            Arc::new(ThumbnailProcessor::new()),
            Arc::new(CopyMoveProcessor::new()),
            Arc::new(AudioExtractProcessor::new()),
            Arc::new(CompressionProcessor::new()),
            Arc::new(MetadataProcessor::new()),
            Arc::new(DeleteProcessor::new()),
        ];

        // Create throttle controller if enabled
        // Requirements: 8.1, 8.5
        let throttle_controller = if config.throttle.enabled || config.enable_throttling {
            let mut throttle_config = config.throttle.clone();
            // Support legacy enable_throttling flag
            if config.enable_throttling && !config.throttle.enabled {
                throttle_config.enabled = true;
            }
            Some(Arc::new(ThrottleController::new(throttle_config)))
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
            cancellation_token: CancellationToken::new(),
            throttle_controller,
            download_adjuster: None,
            // Purge service requires a job repository, so it's None for in-memory queue
            purge_service: None,
            preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
        }
    }

    /// Create a new Pipeline Manager with custom configuration and job repository.
    /// This enables database persistence and job recovery on startup.
    /// Requirements: 6.1, 6.3
    pub fn with_repository(
        config: PipelineManagerConfig,
        job_repository: Arc<dyn JobRepository>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_repository(
            config.job_queue.clone(),
            job_repository.clone(),
        ));

        // Create purge service if retention is enabled
        // Requirements: 7.1, 7.2, 7.3, 7.4, 7.5
        let purge_service = if config.purge.retention_days > 0 {
            Some(Arc::new(JobPurgeService::new(
                config.purge.clone(),
                job_repository,
            )))
        } else {
            None
        };

        // Create default processors
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(RcloneProcessor::new()),
            Arc::new(ExecuteCommandProcessor::new()),
            Arc::new(ThumbnailProcessor::new()),
            Arc::new(CopyMoveProcessor::new()),
            Arc::new(AudioExtractProcessor::new()),
            Arc::new(CompressionProcessor::new()),
            Arc::new(MetadataProcessor::new()),
            Arc::new(DeleteProcessor::new()),
        ];

        // Create throttle controller if enabled
        // Requirements: 8.1, 8.5
        let throttle_controller = if config.throttle.enabled || config.enable_throttling {
            let mut throttle_config = config.throttle.clone();
            // Support legacy enable_throttling flag
            if config.enable_throttling && !config.throttle.enabled {
                throttle_config.enabled = true;
            }
            Some(Arc::new(ThrottleController::new(throttle_config)))
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
            cancellation_token: CancellationToken::new(),
            throttle_controller,
            download_adjuster: None,
            purge_service,
            preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
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

    /// Set the config service.
    pub fn with_config_service(mut self, config_service: Arc<ConfigService<CR, SR>>) -> Self {
        self.config_service = Some(config_service);
        self
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
    /// Requirements: 7.1
    pub fn purge_service(&self) -> Option<&Arc<JobPurgeService>> {
        self.purge_service.as_ref()
    }

    /// Recover jobs from database on startup.
    /// Resets PROCESSING jobs to PENDING for re-execution.
    /// For sequential pipelines, no special handling is needed since only one job
    /// per pipeline exists at a time.
    /// Requirements: 6.3, 7.4
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
    /// Requirements: 8.1, 8.2, 8.3
    pub fn start(&self) {
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

        // Start worker pools
        self.cpu_pool.start(self.job_queue.clone(), cpu_processors);
        self.io_pool.start(self.job_queue.clone(), io_processors);

        // Start throttle controller monitoring if enabled and adjuster is set
        // Requirements: 8.1, 8.2, 8.3
        if let (Some(throttle_controller), Some(adjuster)) =
            (&self.throttle_controller, &self.download_adjuster)
        {
            if throttle_controller.is_enabled() {
                info!("Starting throttle controller monitoring");
                throttle_controller.clone().start_monitoring(
                    self.job_queue.clone(),
                    adjuster.clone(),
                    self.cancellation_token.clone(),
                );
            }
        }

        // Start purge service background task if enabled
        // Requirements: 7.1, 7.2, 7.3, 7.4, 7.5
        if let Some(purge_service) = &self.purge_service {
            info!("Starting job purge service");
            purge_service.start_background_task(self.cancellation_token.clone());
        }

        info!("Pipeline Manager started");
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

    /// Create a new pipeline with sequential job execution.
    /// Only the first job is created immediately; subsequent jobs are created
    /// atomically when each job completes.
    ///
    /// Returns the pipeline_id (which is the first job's ID) for tracking.
    ///
    /// Requirements: 6.1, 7.1, 7.5
    /// Create a new pipeline with sequential job execution.
    /// Only the first job is created immediately; subsequent jobs are created
    /// atomically when each job completes.
    ///
    /// Returns the pipeline_id (which is the first job's ID) for tracking.
    ///
    /// Requirements: 6.1, 7.1, 7.5
    pub async fn create_pipeline(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_path: &str,
        steps: Option<Vec<PipelineStep>>,
    ) -> Result<PipelineCreationResult> {
        // Require explicit pipeline steps
        let steps = match steps {
            Some(steps) => steps,
            None => {
                return Err(crate::Error::Validation(
                    "Pipeline must have at least one step".to_string(),
                ));
            }
        };

        if steps.is_empty() {
            return Err(crate::Error::Validation(
                "Pipeline must have at least one step".to_string(),
            ));
        }

        // Resolve all steps config/processor upfront
        let mut resolved_steps_config: Vec<PipelineStep> = Vec::with_capacity(steps.len());

        for step in &steps {
            let (processor, config) = self.resolve_step(step).await?;
            resolved_steps_config.push(PipelineStep::Inline {
                processor,
                config: config.unwrap_or(serde_json::Value::Null),
            });
        }

        // Get the first step from resolved list
        let first_resolved_step = &resolved_steps_config[0];
        let (first_processor, first_config) = match first_resolved_step {
            PipelineStep::Inline { processor, config } => (processor.clone(), Some(config.clone())),
            _ => unreachable!("We just converted everything to Inline"),
        };

        // Calculate pipeline chain for this job
        // We will store the full remaining steps list (fully resolved) in the job
        let remaining_steps = if resolved_steps_config.len() > 1 {
            Some(
                resolved_steps_config
                    .iter()
                    .skip(1)
                    .cloned()
                    .collect::<Vec<PipelineStep>>(),
            )
        } else {
            None
        };

        // Determine next_job_type for observability/legacy fields (it's the processor name of the next step)
        let next_job_type = if let Some(remaining) = &remaining_steps {
            if let Some(next) = remaining.first() {
                // It's already Inline, so we can just grab the processor
                match next {
                    PipelineStep::Inline { processor, .. } => Some(processor.clone()),
                    PipelineStep::Preset(name) => Some(name.clone()), // Should not happen
                }
            } else {
                None
            }
        } else {
            None
        };

        // Create the first job with pipeline chain information
        let first_job = Job::new_pipeline_step(
            first_processor.clone(),
            vec![input_path.to_string()],
            vec![], // Output path will be determined by the processor logic or previous output
            streamer_id,
            session_id,
            None, // pipeline_id will be set to this job's ID
            next_job_type,
            remaining_steps,
        );

        // Apply specific config if present
        let first_job = if let Some(cfg) = first_config {
            let mut j = first_job;
            j.config = Some(cfg.to_string());
            j
        } else {
            first_job
        };

        // The pipeline_id is the first job's ID
        let pipeline_id = first_job.id.clone();

        // Set the pipeline_id on the first job
        let mut first_job = first_job.with_pipeline_id(pipeline_id.clone());

        // Initialize execution info with step 1/N
        let mut exec_info = crate::pipeline::job_queue::JobExecutionInfo::new()
            .with_processor(first_processor.clone());

        exec_info.current_step = Some(1);
        exec_info.total_steps = Some(steps.len() as u32);

        first_job.execution_info = Some(exec_info);

        // Enqueue the first job
        let job_id = self.enqueue(first_job.clone()).await?;

        info!(
            "Created pipeline {} with {} steps for session {}",
            pipeline_id,
            steps.len(),
            session_id
        );

        // Convert steps to strings for result (legacy compat)
        let string_steps: Vec<String> = steps
            .iter()
            .map(|s| match s {
                PipelineStep::Preset(n) => n.clone(),
                PipelineStep::Inline { processor, .. } => processor.clone(),
            })
            .collect();

        Ok(PipelineCreationResult {
            pipeline_id,
            first_job_id: job_id,
            first_job_type: first_processor,
            total_steps: steps.len(),
            steps: string_steps,
        })
    }

    /// Resolve all steps in a pipeline to Inline steps.
    /// This ensures that all presets are expanded at creation time,
    /// so that the JobQueue doesn't need to depend on the JobPresetRepository.
    pub async fn resolve_pipeline(&self, steps: Vec<PipelineStep>) -> Result<Vec<PipelineStep>> {
        let mut resolved_steps = Vec::new();
        for step in steps {
            match step {
                PipelineStep::Preset(name) => {
                    if let Some(repo) = &self.preset_repo {
                        // Fix: use get_preset_by_name
                        match repo.get_preset_by_name(&name).await {
                            Ok(Some(preset)) => {
                                let config = if !preset.config.is_empty() {
                                    serde_json::from_str(&preset.config)
                                        .unwrap_or(serde_json::Value::Null)
                                } else {
                                    serde_json::Value::Null
                                };
                                resolved_steps.push(PipelineStep::Inline {
                                    processor: preset.processor,
                                    config,
                                });
                            }
                            Ok(None) => {
                                // Fallback: assume name is processor
                                resolved_steps.push(PipelineStep::Inline {
                                    processor: name,
                                    config: serde_json::Value::Null,
                                });
                            }
                            Err(e) => return Err(crate::Error::Database(e.to_string())),
                        }
                    } else {
                        // No repo, fallback
                        resolved_steps.push(PipelineStep::Inline {
                            processor: name,
                            config: serde_json::Value::Null,
                        });
                    }
                }
                PipelineStep::Inline { .. } => {
                    resolved_steps.push(step);
                }
            }
        }
        Ok(resolved_steps)
    }

    /// Resolve a PipelineStep to (processor_name, optional_config).
    pub async fn resolve_step(
        &self,
        step: &PipelineStep,
    ) -> Result<(String, Option<serde_json::Value>)> {
        match step {
            PipelineStep::Preset(name) => {
                debug!("Resolving pipeline step preset: {}", name);
                if let Some(repo) = &self.preset_repo {
                    match repo.get_preset_by_name(name).await {
                        Ok(Some(preset)) => {
                            debug!("Found preset '{}', config: {}", name, preset.config);
                            let config = if !preset.config.is_empty() {
                                serde_json::from_str(&preset.config)
                                    .unwrap_or(serde_json::Value::Null)
                            } else {
                                serde_json::Value::Null
                            };
                            Ok((preset.processor, Some(config)))
                        }
                        Ok(None) => {
                            debug!(
                                "Preset '{}' not found, falling back to processor name",
                                name
                            );
                            Ok((name.clone(), None))
                        }
                        Err(e) => Err(e.into()),
                    }
                } else {
                    debug!(
                        "No preset repo available, falling back to processor name: {}",
                        name
                    );
                    Ok((name.clone(), None))
                }
            }
            PipelineStep::Inline { processor, config } => {
                debug!("Resolving inline pipeline step: {}", processor);
                Ok((processor.clone(), Some(config.clone())))
            }
        }
    }

    /// Helper to just get processor name (for next_job_type field).
    async fn resolve_step_processor_name(&self, step: &PipelineStep) -> String {
        match self.resolve_step(step).await {
            Ok((proc, _)) => proc,
            Err(_) => "unknown".to_string(),
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

                // Get pipeline config for this streamer (if available)
                let pipeline_steps = if let Some(config_service) = &self.config_service {
                    config_service
                        .get_config_for_streamer(&streamer_id)
                        .await
                        .map(|c| c.pipeline)
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                // Check if user's pipeline contains a thumbnail step
                let pipeline_has_thumbnail = pipeline_steps.iter().any(|step| match step {
                    crate::database::models::job::PipelineStep::Inline { processor, .. } => {
                        processor == "thumbnail"
                    }
                    crate::database::models::job::PipelineStep::Preset(name) => {
                        name.contains("thumbnail")
                    }
                });

                // Generate automatic thumbnail for first segment only if:
                // 1. This is the first segment (segment_index == 0)
                // 2. User's pipeline doesn't already include a thumbnail step
                if segment_index == 0 && !pipeline_has_thumbnail {
                    self.maybe_create_thumbnail_job(&streamer_id, &session_id, &segment_path)
                        .await;
                }

                // Create pipeline jobs if pipeline is configured
                if !pipeline_steps.is_empty() {
                    debug!(
                        "Creating pipeline for {} (session: {}) with {} steps",
                        streamer_id,
                        session_id,
                        pipeline_steps.len()
                    );
                    if let Err(e) = self
                        .create_pipeline(
                            &session_id,
                            &streamer_id,
                            &segment_path,
                            Some(pipeline_steps),
                        )
                        .await
                    {
                        tracing::error!(
                            "Failed to create pipeline for session {}: {}",
                            session_id,
                            e
                        );
                    }
                } else {
                    debug!(
                        "No pipeline steps configured for {} (session: {}), skipping pipeline creation",
                        streamer_id, session_id
                    );
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
                // Could create post-processing jobs here
            }
            _ => {}
        }
    }

    /// Check if session already has a thumbnail by querying media outputs.
    async fn session_has_thumbnail(&self, session_id: &str) -> bool {
        if let Some(repo) = &self.session_repo {
            if let Ok(outputs) = repo.get_media_outputs_for_session(session_id).await {
                return outputs
                    .iter()
                    .any(|o| o.file_type == MediaFileType::Thumbnail.as_str());
            }
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

        // Generate output path: replace extension with .jpg
        let thumbnail_path = std::path::Path::new(segment_path)
            .with_extension("jpg")
            .to_string_lossy()
            .to_string();

        // Use thumbnail_native preset config (preserve_resolution: true)
        let config = r#"{"timestamp_secs":10,"preserve_resolution":true,"quality":1}"#;

        if let Err(e) = self
            .create_thumbnail_job(
                segment_path,
                &thumbnail_path,
                streamer_id,
                session_id,
                Some(config),
            )
            .await
        {
            tracing::error!(
                "Failed to create thumbnail job for session {}: {}",
                session_id,
                e
            );
        } else {
            debug!(
                "Created thumbnail job for first segment of session {}",
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
        self.config.enable_throttling && self.job_queue.is_critical()
    }

    // ========================================================================
    // Query and Management Methods (Requirements 1.1-1.5, 2.1-2.5, 3.1-3.3)
    // ========================================================================

    /// List jobs with filters and pagination.
    /// Delegates to JobQueue/JobRepository.
    /// Requirements: 1.1, 1.3, 1.4, 1.5
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

    /// List pipelines (grouped by pipeline_id) with pagination.
    /// Returns pipeline summaries and total count.
    pub async fn list_pipelines(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<crate::database::repositories::PipelineSummary>, u64)> {
        self.job_queue.list_pipelines(filters, pagination).await
    }

    /// Get a job by ID.
    /// Retrieves job from repository.
    /// Requirements: 1.2
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        self.job_queue.get_job(id).await
    }

    /// Retry a failed job.
    /// Delegates to JobQueue.
    /// Requirements: 2.1, 2.2
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
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
    /// Requirements: 2.3, 2.4, 2.5
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
    /// Requirements: 6.1
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
    /// Requirements: 3.1, 3.2, 3.3
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
}

/// Comprehensive pipeline statistics.
/// Requirements: 3.1, 3.2, 3.3
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
/// Requirements: 6.1, 7.1
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
    use crate::database::models::job::PipelineStep;
    use crate::pipeline::JobStatus;

    #[test]
    fn test_pipeline_manager_config_default() {
        let config = PipelineManagerConfig::default();
        assert_eq!(config.cpu_pool.max_workers, 2);
        assert_eq!(config.io_pool.max_workers, 4);
        assert!(!config.enable_throttling);
        // Verify throttle config defaults
        assert!(!config.throttle.enabled);
        assert_eq!(config.throttle.critical_threshold, 500);
        assert_eq!(config.throttle.warning_threshold, 100);
        // Verify purge config defaults (Requirements: 7.1, 8.1)
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

    #[tokio::test]
    async fn test_create_pipeline_requires_steps_when_none() {
        let manager: PipelineManager = PipelineManager::new();

        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", None)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_create_pipeline_custom_steps() {
        let manager: PipelineManager = PipelineManager::new();

        let custom_steps = vec![
            PipelineStep::Preset("remux".to_string()),
            PipelineStep::Preset("upload".to_string()),
        ];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(custom_steps))
            .await
            .unwrap();

        assert_eq!(result.total_steps, 2);
        assert_eq!(
            result.steps,
            vec!["remux".to_string(), "upload".to_string()]
        );
        assert_eq!(result.first_job_type, "remux");

        // Verify the first job has correct chain info
        let job = manager
            .get_job(&result.first_job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.next_job_type, Some("upload".to_string()));
        assert_eq!(
            job.remaining_steps,
            Some(vec![PipelineStep::Preset("upload".to_string())])
        );
    }

    #[tokio::test]
    async fn test_create_pipeline_single_step() {
        let manager: PipelineManager = PipelineManager::new();

        let single_step = vec![PipelineStep::Preset("remux".to_string())];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(single_step))
            .await
            .unwrap();

        assert_eq!(result.total_steps, 1);
        assert_eq!(result.first_job_type, "remux");

        // Verify the first job has no next job
        let job = manager
            .get_job(&result.first_job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.next_job_type, None);
        assert_eq!(job.remaining_steps, None);
    }

    #[tokio::test]
    async fn test_create_pipeline_empty_steps_error() {
        let manager: PipelineManager = PipelineManager::new();

        let empty_steps: Vec<PipelineStep> = vec![];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(empty_steps))
            .await;

        assert!(result.is_err());
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
    fn test_throttle_controller_enabled_with_legacy_flag() {
        // Test backward compatibility with enable_throttling flag
        let config = PipelineManagerConfig {
            enable_throttling: true,
            ..Default::default()
        };
        let manager: PipelineManager = PipelineManager::with_config(config);

        // Throttle controller should be Some when legacy flag is set
        assert!(manager.throttle_controller().is_some());
    }

    #[test]
    fn test_config_includes_throttle_defaults() {
        let config = PipelineManagerConfig::default();

        assert!(!config.throttle.enabled);
        assert_eq!(config.throttle.critical_threshold, 500);
        assert_eq!(config.throttle.warning_threshold, 100);
        assert!((config.throttle.reduction_factor - 0.5).abs() < f32::EPSILON);
    }

    // ========================================================================
    // Pipeline Job Chaining Tests
    // Requirements: 10.1, 10.2, 10.3, 10.5
    // ========================================================================

    /// Test that create_pipeline creates only the first job immediately.
    /// Subsequent jobs should only be created when each job completes.
    /// Requirements: 10.1
    #[tokio::test]
    async fn test_create_pipeline_creates_only_first_job() {
        let manager: PipelineManager = PipelineManager::new();

        // Create a pipeline with 3 steps
        let steps = vec![
            PipelineStep::Preset("remux".to_string()),
            PipelineStep::Preset("upload".to_string()),
            PipelineStep::Preset("thumbnail".to_string()),
        ];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(steps))
            .await
            .unwrap();

        // Verify only one job exists in the queue
        assert_eq!(manager.queue_depth(), 1);

        // Verify the job is the first step
        let job = manager
            .get_job(&result.first_job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.job_type, "remux");

        // List all jobs and verify only one exists
        let filters = JobFilters::default();
        let pagination = Pagination::new(100, 0);
        let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

        assert_eq!(total, 1);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, result.first_job_id);
        assert_eq!(jobs[0].job_type, "remux");
    }

    /// Test that multiple pipelines each create only their first job.
    /// Requirements: 10.1
    #[tokio::test]
    async fn test_multiple_pipelines_create_only_first_jobs() {
        let manager: PipelineManager = PipelineManager::new();

        // Create first pipeline
        let result1 = manager
            .create_pipeline(
                "session-1",
                "streamer-1",
                "/input1.flv",
                Some(vec![
                    PipelineStep::Preset("remux".to_string()),
                    PipelineStep::Preset("upload".to_string()),
                ]),
            )
            .await
            .unwrap();

        // Create second pipeline
        let result2 = manager
            .create_pipeline(
                "session-2",
                "streamer-2",
                "/input2.flv",
                Some(vec![
                    PipelineStep::Preset("remux".to_string()),
                    PipelineStep::Preset("thumbnail".to_string()),
                ]),
            )
            .await
            .unwrap();

        // Verify exactly 2 jobs exist (one per pipeline)
        assert_eq!(manager.queue_depth(), 2);

        // Verify both jobs are first steps
        let job1 = manager
            .get_job(&result1.first_job_id)
            .await
            .unwrap()
            .unwrap();
        let job2 = manager
            .get_job(&result2.first_job_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(job1.job_type, "remux");
        assert_eq!(job2.job_type, "remux");

        // Verify they have different pipeline IDs
        assert_ne!(result1.pipeline_id, result2.pipeline_id);
    }

    // ========================================================================
    // Pipeline Recovery Tests
    // Requirements: 10.5
    // ========================================================================

    /// Test that pipeline jobs preserve chain info for recovery.
    /// When a pipeline job is created, it should have all the info needed
    /// to continue the pipeline after recovery.
    /// Requirements: 10.5
    #[tokio::test]
    async fn test_pipeline_job_preserves_chain_info_for_recovery() {
        let manager: PipelineManager = PipelineManager::new();

        // Create a pipeline with 3 steps
        let steps = vec![
            PipelineStep::Preset("remux".to_string()),
            PipelineStep::Preset("upload".to_string()),
            PipelineStep::Preset("thumbnail".to_string()),
        ];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(steps))
            .await
            .unwrap();

        // Get the first job
        let job = manager
            .get_job(&result.first_job_id)
            .await
            .unwrap()
            .unwrap();

        // Verify the job has all chain info needed for recovery
        assert_eq!(job.pipeline_id, Some(result.pipeline_id.clone()));
        assert_eq!(job.next_job_type, Some("upload".to_string()));
        assert_eq!(
            job.remaining_steps,
            Some(vec![
                PipelineStep::Preset("upload".to_string()),
                PipelineStep::Preset("thumbnail".to_string())
            ])
        );

        // This info is preserved in the database and will be available after recovery
        // When the job completes, complete_with_next will use this info to create the next job
    }

    /// Test that pipeline recovery works with in-memory queue.
    /// This verifies the basic recovery mechanism without database.
    /// Requirements: 10.5
    #[tokio::test]
    async fn test_pipeline_recovery_without_database() {
        let manager: PipelineManager = PipelineManager::new();

        // Create a pipeline
        let result = manager
            .create_pipeline(
                "session-1",
                "streamer-1",
                "/input.flv",
                Some(vec![
                    PipelineStep::Preset("remux".to_string()),
                    PipelineStep::Preset("upload".to_string()),
                ]),
            )
            .await
            .unwrap();

        // Verify job exists
        let job = manager
            .get_job(&result.first_job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobStatus::Pending);

        // Without a database, recover_jobs returns 0 (no persistence)
        let recovered = manager.recover_jobs().await.unwrap();
        assert_eq!(recovered, 0);

        // But the in-memory job is still there
        let job_after = manager
            .get_job(&result.first_job_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job_after.status, JobStatus::Pending);
    }
}
