use super::*;

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    /// Start the pipeline manager.
    pub fn start(self: Arc<Self>) {
        let mut runtime = self.runtime.lock();
        match runtime.state {
            PipelineRuntimeState::NotStarted => {
                runtime.state = PipelineRuntimeState::Running;
            }
            PipelineRuntimeState::Running => {
                warn!("Pipeline Manager is already started");
                return;
            }
            PipelineRuntimeState::Stopped => {
                warn!("Pipeline Manager cannot restart after shutdown");
                return;
            }
        }

        info!("Starting Pipeline Manager");

        // Get CPU and IO processors
        let cpu_processors: Vec<Arc<dyn Processor>> = self
            .processors
            .iter()
            .filter(|p| p.processor_type() == crate::pipeline::ProcessorType::Cpu)
            .cloned()
            .collect();

        info!(
            "Starting CPU pool with processors: {:?}",
            cpu_processors.iter().map(|p| p.name()).collect::<Vec<_>>()
        );

        let io_processors: Vec<Arc<dyn Processor>> = self
            .processors
            .iter()
            .filter(|p| p.processor_type() == crate::pipeline::ProcessorType::Io)
            .cloned()
            .collect();

        info!(
            "Starting IO pool with processors: {:?}",
            io_processors.iter().map(|p| p.name()).collect::<Vec<_>>()
        );

        // Use a bounded channel for DAG completion notifications to avoid unbounded memory growth
        // if completions outpace handling (apply backpressure instead).
        let (dag_notify_tx, mut dag_notify_rx) = mpsc::channel::<DagCompletionInfo>(1024);
        let manager = Arc::downgrade(&self);
        runtime.tasks.spawn(async move {
            while let Some(completion) = dag_notify_rx.recv().await {
                let Some(manager) = manager.upgrade() else {
                    break;
                };
                manager.handle_dag_completion(completion).await;
            }
            "DAG completion handler"
        });

        let coordinator = self.pipeline_coordinator.clone();
        let coordinator_token = self.cancellation_token.clone();
        runtime.tasks.spawn(async move {
            coordinator.start(coordinator_token).await;
            "pipeline coordinator"
        });

        let cleanup_manager = Arc::downgrade(&self);
        let cleanup_token = self.cancellation_token.clone();
        runtime.tasks.spawn(async move {
            let interval = std::time::Duration::from_secs(SESSION_COMPLETE_CLEANUP_INTERVAL_SECS);
            loop {
                tokio::select! {
                    _ = cleanup_token.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {
                        let Some(cleanup_manager) = cleanup_manager.upgrade() else {
                            break;
                        };
                        let now = std::time::Instant::now();
                        cleanup_manager
                            .pipeline_coordinator
                            .cleanup_stale(SESSION_COMPLETE_TTL_SECS)
                            .await;

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
            "pipeline stale-state cleanup"
        });

        // Start worker pools with optional DAG scheduler
        self.cpu_pool.start_with_dag_scheduler(
            self.job_queue.clone(),
            cpu_processors,
            self.dag_scheduler.clone(),
            Some(dag_notify_tx.clone()),
            Some(self.event_tx.clone()),
        );
        self.io_pool.start_with_dag_scheduler(
            self.job_queue.clone(),
            io_processors,
            self.dag_scheduler.clone(),
            Some(dag_notify_tx),
            Some(self.event_tx.clone()),
        );

        // Start throttle controller monitoring if enabled and adjuster is set
        if let Some(throttle_controller) = &self.throttle_controller
            && let Some(adjuster) = &self.download_adjuster
            && throttle_controller.is_enabled()
        {
            info!("Starting throttle controller monitoring");
            let throttle_controller = throttle_controller.clone();
            let job_queue = self.job_queue.clone();
            let adjuster = adjuster.clone();
            let cancellation_token = self.cancellation_token.clone();
            runtime.tasks.spawn(async move {
                throttle_controller
                    .run_monitoring(job_queue, adjuster, cancellation_token)
                    .await;
                "pipeline throttle controller"
            });
        }

        info!("Pipeline Manager started");
    }
    pub async fn stop(&self) {
        info!("Stopping Pipeline Manager");
        self.cancellation_token.cancel();

        let mut runtime_tasks = {
            let mut runtime = self.runtime.lock();
            runtime.state = PipelineRuntimeState::Stopped;
            std::mem::take(&mut runtime.tasks)
        };

        // Stop worker pools
        self.cpu_pool.stop().await;
        self.io_pool.stop().await;

        // Workers are the only producers of DAG completion and progress
        // updates. Once they have joined, the consumers can drain/stop without
        // racing a late repository write.
        self.job_queue.stop_progress_aggregator().await;

        while let Some(result) = runtime_tasks.join_next().await {
            match result {
                Ok(task) => debug!(task, "Pipeline runtime task stopped"),
                Err(error) => warn!(%error, "Pipeline runtime task failed while stopping"),
            }
        }

        info!("Pipeline Manager stopped");
    }

    /// Abort the worker pools and runtime tasks instead of joining them.
    ///
    /// [`Self::stop`] waits for in-flight jobs through
    /// `WorkerPool::stop`, which is bounded only by
    /// `WorkerPoolConfig::job_timeout_secs`. Callers that must stop waiting use
    /// this so each job future is dropped and any process a processor spawned
    /// with `kill_on_drop` is killed. Tasks that do not settle by `deadline`
    /// are left aborted but unjoined.
    ///
    /// Returns the number of aborted tasks.
    pub(crate) async fn abort(&self, deadline: tokio::time::Instant) -> usize {
        warn!("Aborting Pipeline Manager");
        self.cancellation_token.cancel();

        let mut runtime_tasks = {
            let mut runtime = self.runtime.lock();
            runtime.state = PipelineRuntimeState::Stopped;
            std::mem::take(&mut runtime.tasks)
        };

        let mut aborted = self.cpu_pool.abort(deadline).await + self.io_pool.abort(deadline).await;

        aborted += runtime_tasks.len();
        runtime_tasks.abort_all();
        while !runtime_tasks.is_empty() {
            match tokio::time::timeout_at(deadline, runtime_tasks.join_next()).await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        unfinished = runtime_tasks.len(),
                        "Aborted pipeline runtime tasks did not settle before the reap deadline"
                    );
                    break;
                }
            }
        }

        aborted
    }

    /// Subscribe to pipeline events.
    pub fn subscribe(&self) -> broadcast::Receiver<PipelineEvent> {
        self.event_tx.subscribe()
    }
}
