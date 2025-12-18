//! Pipeline Manager implementation.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::dag_scheduler::{DagCreationResult, DagScheduler};
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
use crate::database::models::job::{DagPipelineDefinition, DagStep, PipelineStep};
use crate::database::models::{JobFilters, MediaFileType, MediaOutputDbModel, Pagination};
use crate::database::repositories::config::{ConfigRepository, SqlxConfigRepository};
use crate::database::repositories::streamer::{SqlxStreamerRepository, StreamerRepository};
use crate::database::repositories::{
    DagRepository, JobPresetRepository, JobRepository, PipelinePresetRepository, SessionRepository,
};
use crate::downloader::DownloadManagerEvent;
use crate::pipeline::job_queue::JobExecutionInfo;

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
    /// Pipeline preset repository for resolving workflow steps.
    pipeline_preset_repo: Option<Arc<dyn PipelinePresetRepository>>,
    /// Config service for resolving pipeline rules.
    config_service: Option<Arc<ConfigService<CR, SR>>>,
    /// Last observed queue depth status (edge-trigger warnings).
    last_queue_status: AtomicU8,
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
            purge_service: None,
            preset_repo: None,
            pipeline_preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
            dag_repository: None,
            job_repository: None,
            dag_scheduler: None,
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
                job_repository.clone(),
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
            pipeline_preset_repo: None,
            config_service: None,
            last_queue_status: AtomicU8::new(0),
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
            self.dag_scheduler = Some(Arc::new(DagScheduler::new(
                self.job_queue.clone(),
                dag_repository,
                job_repo.clone(),
            )));
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

        // Start worker pools with optional DAG scheduler
        self.cpu_pool.start_with_dag_scheduler(
            self.job_queue.clone(),
            cpu_processors,
            self.dag_scheduler.clone(),
        );
        self.io_pool.start_with_dag_scheduler(
            self.job_queue.clone(),
            io_processors,
            self.dag_scheduler.clone(),
        );

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
    ///
    /// DEPRECATED: Use `create_dag_pipeline` for new pipelines. DAG pipelines
    /// support fan-in, fan-out, and parallel execution. Sequential pipelines
    /// are being phased out.
    #[deprecated(
        since = "0.2.0",
        note = "Use create_dag_pipeline() instead. Sequential pipelines are deprecated in favor of DAG pipelines which support fan-in, fan-out, and parallel execution."
    )]
    pub async fn create_pipeline(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_paths: &[String],
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

        // Resolve all steps (expand workflows/presets)
        let resolved_steps_config = self.resolve_pipeline(steps).await?;
        let resolved_len = resolved_steps_config.len();

        if resolved_steps_config.is_empty() {
            return Err(crate::Error::Validation(
                "Pipeline resulted in 0 steps after expansion".to_string(),
            ));
        }

        // Get the first step from resolved list
        let first_resolved_step = &resolved_steps_config[0];
        let (first_processor, first_config) = match first_resolved_step {
            PipelineStep::Inline { processor, config } => (processor.clone(), Some(config.clone())),
            _ => unreachable!("We just converted everything to Inline"),
        };

        // Calculate pipeline chain for this job
        // We will store the full remaining steps list (fully resolved) in the job
        let remaining_steps = if resolved_len > 1 {
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

                    // These should never happen
                    PipelineStep::Preset { name: _ } => unreachable!(),
                    PipelineStep::Workflow { name: _ } => unreachable!(),
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
            input_paths.to_vec(),
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
        let mut exec_info = JobExecutionInfo::new().with_processor(first_processor.clone());

        exec_info.current_step = Some(1);
        exec_info.total_steps = Some(resolved_len as u32);

        first_job.execution_info = Some(exec_info);

        // Enqueue the first job
        let job_id = self.enqueue(first_job.clone()).await?;

        info!(
            "Created pipeline {} with {} steps for session {}",
            pipeline_id, resolved_len, session_id
        );

        // Use resolved_steps_config to show the actual expanded pipeline steps
        let string_steps: Vec<String> = resolved_steps_config
            .iter()
            .map(|s| match s {
                PipelineStep::Preset { name } => name.clone(),
                PipelineStep::Workflow { name } => name.clone(),
                PipelineStep::Inline { processor, .. } => processor.clone(),
            })
            .collect();

        Ok(PipelineCreationResult {
            pipeline_id,
            first_job_id: job_id,
            first_job_type: first_processor,
            total_steps: resolved_len,
            steps: string_steps,
        })
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

        // Delegate to DAG scheduler
        let result = dag_scheduler
            .create_dag_pipeline(
                resolved_dag,
                &input_paths,
                Some(streamer_id.to_string()),
                Some(session_id.to_string()),
            )
            .await?;

        info!(
            "Created DAG pipeline {} with {} steps ({} root jobs) for session {}",
            result.dag_id,
            result.total_steps,
            result.root_job_ids.len(),
            session_id
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
            let workflow_steps: Vec<(usize, String, Vec<String>)> = dag
                .steps
                .iter()
                .enumerate()
                .filter_map(|(idx, step)| {
                    if let PipelineStep::Workflow { name } = &step.step {
                        Some((idx, name.clone(), step.depends_on.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            if workflow_steps.is_empty() {
                break; // No more workflows to expand
            }

            // Process each workflow step
            for (_, workflow_name, _) in workflow_steps.iter().rev() {
                // Process in reverse to maintain index validity
                let workflow_step_idx = dag
                    .steps
                    .iter()
                    .position(|s| matches!(&s.step, PipelineStep::Workflow { name } if name == workflow_name))
                    .unwrap();

                let workflow_step = &dag.steps[workflow_step_idx];
                let workflow_step_id = workflow_step.id.clone();
                let workflow_step_deps = workflow_step.depends_on.clone();

                // Look up the workflow
                let workflow_dag = self.lookup_workflow(workflow_name).await?;

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

    /// Resolve all steps in a pipeline to Inline steps.
    /// This ensures that all presets are expanded at creation time,
    /// so that the JobQueue doesn't need to depend on the JobPresetRepository.
    pub async fn resolve_pipeline(&self, steps: Vec<PipelineStep>) -> Result<Vec<PipelineStep>> {
        let mut resolved_steps = Vec::new();
        for step in steps {
            match step {
                PipelineStep::Preset { name } => {
                    if let Some(repo) = &self.preset_repo {
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
                PipelineStep::Workflow { name } => {
                    // Expand workflow by looking up pipeline preset
                    if let Some(repo) = &self.pipeline_preset_repo {
                        match repo.get_pipeline_preset_by_name(&name).await {
                            Ok(Some(workflow)) => {
                                // Parse the steps from the workflow
                                let workflow_steps: Vec<PipelineStep> =
                                    serde_json::from_str(&workflow.steps).unwrap_or_default();
                                // Recursively resolve the workflow's steps
                                let expanded =
                                    Box::pin(self.resolve_pipeline(workflow_steps)).await?;
                                resolved_steps.extend(expanded);
                            }
                            Ok(None) => {
                                return Err(crate::Error::Validation(format!(
                                    "Workflow '{}' not found",
                                    name
                                )));
                            }
                            Err(e) => {
                                warn!("Failed to load workflow '{}': {}", name, e);
                            }
                        }
                    } else {
                        return Err(crate::Error::Validation(format!(
                            "No pipeline preset repository, cannot expand workflow '{}'",
                            name
                        )));
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
            PipelineStep::Preset { name } => {
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
            PipelineStep::Workflow { name } => {
                // TODO: Expand workflow steps
                debug!("Workflow step '{}' - expansion not yet implemented", name);
                Err(crate::Error::Validation(format!(
                    "Workflow expansion not yet implemented for '{}'",
                    name
                )))
            }
            PipelineStep::Inline { processor, config } => {
                debug!("Resolving inline pipeline step: {}", processor);
                Ok((processor.clone(), Some(config.clone())))
            }
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
                let pipeline_config = if let Some(config_service) = &self.config_service {
                    config_service
                        .get_config_for_streamer(&streamer_id)
                        .await
                        .map(|c| c.pipeline)
                        .ok()
                        .flatten()
                } else {
                    None
                };

                // Check for thumbnail step in DAG nodes
                // For direct DAG support, we check if any node is a thumbnail processor/workflow
                let pipeline_has_thumbnail = if let Some(dag) = &pipeline_config {
                    dag.steps.iter().any(|node| match &node.step {
                        PipelineStep::Inline { processor, .. } => processor == "thumbnail",
                        PipelineStep::Preset { name } => name.contains("thumbnail"),
                        PipelineStep::Workflow { name } => name.contains("thumbnail"),
                    })
                } else {
                    false
                };

                // Generate automatic thumbnail for first segment only if:
                // 1. This is the first segment (segment_index == 0)
                // 2. User's pipeline doesn't already include a thumbnail step
                if segment_index == 0 && !pipeline_has_thumbnail {
                    self.maybe_create_thumbnail_job(&streamer_id, &session_id, &segment_path)
                        .await;
                }

                // Create pipeline jobs if pipeline is configured
                if let Some(dag) = pipeline_config {
                    if let Err(e) = self
                        .create_dag_pipeline(&session_id, &streamer_id, vec![segment_path], dag)
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

        // Use thumbnail_native preset
        let step = PipelineStep::Preset {
            name: "thumbnail_native".to_string(),
        };

        // Create DAG definition
        let dag_step = DagStep::new("thumbnail", step);
        let dag_def = DagPipelineDefinition::new("Automatic Thumbnail", vec![dag_step]);

        if let Err(e) = self
            .create_dag_pipeline(session_id, streamer_id, vec![segment_path], dag_def)
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
    #[allow(deprecated)]
    async fn test_create_pipeline_requires_steps_when_none() {
        let manager: PipelineManager = PipelineManager::new();

        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", None)
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn test_create_pipeline_custom_steps() {
        let manager: PipelineManager = PipelineManager::new();

        let custom_steps = vec![
            PipelineStep::preset("remux"),
            PipelineStep::preset("upload"),
        ];
        let result = manager
            .create_pipeline(
                "session-1",
                "streamer-1",
                vec!["/input.flv".to_string()],
                Some(custom_steps),
            )
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
        // The remaining_steps are resolved to Inline format now
        let remaining = job
            .remaining_steps
            .as_ref()
            .expect("Should have remaining steps");
        assert_eq!(remaining.len(), 1);
        assert!(
            matches!(&remaining[0], PipelineStep::Inline { processor, .. } if processor == "upload")
        );
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn test_create_pipeline_single_step() {
        let manager: PipelineManager = PipelineManager::new();

        let single_step = vec![PipelineStep::preset("remux")];
        let result = manager
            .create_pipeline(
                "session-1",
                "streamer-1",
                vec!["/input.flv".to_string()],
                Some(single_step),
            )
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
    #[allow(deprecated)]
    async fn test_create_pipeline_empty_steps_error() {
        let manager: PipelineManager = PipelineManager::new();

        let empty_steps: Vec<PipelineStep> = vec![];
        let result = manager
            .create_pipeline(
                "session-1",
                "streamer-1",
                vec!["/input.flv".to_string()],
                Some(empty_steps),
            )
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
}
