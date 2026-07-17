use super::*;

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    /// Start the pipeline manager.
    pub fn start(self: Arc<Self>) {
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
        let manager = self.clone();
        tokio::spawn(async move {
            while let Some(completion) = dag_notify_rx.recv().await {
                manager.handle_dag_completion(completion).await;
            }
        });

        let coordinator = self.pipeline_coordinator.clone();
        let coordinator_token = self.cancellation_token.clone();
        tokio::spawn(async move {
            coordinator.start(coordinator_token).await;
        });

        let cleanup_manager = self.clone();
        let cleanup_token = self.cancellation_token.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(SESSION_COMPLETE_CLEANUP_INTERVAL_SECS);
            loop {
                tokio::select! {
                    _ = cleanup_token.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {
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

        info!("Pipeline Manager started");
    }
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
}
