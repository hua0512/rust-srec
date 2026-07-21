use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::{
    ServiceContainer, autoscale_concurrency_limit, broadcast_error_is_recoverable,
    has_transient_error_state, should_end_stream_on_danmu_stream_closed,
    should_record_recovery_from_progress,
};
use crate::config::{ConfigService, ConfigUpdateEvent};
use crate::danmu::{DanmuEvent, DanmuService};
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{
    DownloadManager, DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent,
};
use crate::monitor::StreamMonitor;
use crate::pipeline::PipelineManager;
use crate::services::runtime_coordinator::RuntimeCoordinator;
use crate::session::SessionLifecycle;
use crate::streamer::StreamerManager;

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;
type RuntimeStreamMonitor = StreamMonitor<
    SqlxStreamerRepository,
    SqlxFilterRepository,
    SqlxSessionRepository,
    SqlxConfigRepository,
>;

impl ServiceContainer {
    /// Set up config event subscriptions between services.
    pub(super) fn setup_config_event_subscriptions(&self) {
        let handler = ConfigEventHandler {
            streamer_manager: self.streamer_manager.clone(),
            config_service: self.config_service.clone(),
            download_manager: self.download_manager.clone(),
            pipeline_manager: self.pipeline_manager.clone(),
            runtime_coordinator: self.runtime_coordinator.clone(),
            gpu_health_monitor: self.gpu_health_monitor.get().cloned(),
        };
        let receiver = self.event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor.spawn(
            "config event handler",
            handler.run(receiver, cancellation_token),
        );
    }

    /// Set up download event subscriptions to pipeline manager.
    pub(super) fn setup_download_event_subscriptions(&self) {
        let receiver = self.download_manager.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        const DOWNLOAD_EVENT_QUEUE_CAPACITY: usize = 8192;
        let (event_tx, event_rx) =
            mpsc::channel::<DownloadManagerEvent>(DOWNLOAD_EVENT_QUEUE_CAPACITY);

        self.task_supervisor.spawn(
            "discarded segment cleanup",
            run_discarded_segment_cleanup(
                self.discarded_segment_keys.clone(),
                cancellation_token.clone(),
            ),
        );

        self.task_supervisor.spawn(
            "download event drain",
            run_download_event_drain(receiver, event_tx, cancellation_token.clone()),
        );

        let processor = DownloadEventProcessor {
            pipeline_manager: self.pipeline_manager.clone(),
            stream_monitor: self.stream_monitor.clone(),
            streamer_manager: self.streamer_manager.clone(),
            danmu_service: self.danmu_service.clone(),
            config_service: self.config_service.clone(),
            session_lifecycle: self.session_lifecycle.clone(),
            discarded_segment_keys: self.discarded_segment_keys.clone(),
        };
        self.task_supervisor.spawn(
            "download event processor",
            processor.run(event_rx, cancellation_token),
        );
    }

    /// Feed terminal download events into `SessionLifecycle` so every
    /// terminal download outcome closes the session row and emits
    /// `SessionTransition::Ended`, and feed those transitions into the
    /// pipeline manager so the session-complete DAG fires at the right
    /// moment (per `cause.should_run_session_complete_pipeline()`).
    pub(super) fn setup_session_lifecycle_subscriptions(&self) {
        // Take both required receivers before spawning either consumer, and
        // restore them on mismatch: spawning the download-terminal handler
        // without the transition coordinator (or vice versa) would leave
        // `SessionLifecycle::publish_transition` or
        // `DownloadEventPublisher::publish` feeding a channel nobody drains.
        let (mut download_rx, mut transition_rx) = {
            let mut download_slot = self.download_terminal_receiver.lock();
            let mut transition_slot = self.session_transition_receiver.lock();
            match (download_slot.take(), transition_slot.take()) {
                (Some(download_rx), Some(transition_rx)) => (download_rx, transition_rx),
                (download_rx, transition_rx) => {
                    *download_slot = download_rx;
                    *transition_slot = transition_rx;
                    warn!("Session lifecycle consumers are already running");
                    return;
                }
            }
        };
        let lifecycle = self.session_lifecycle.clone();
        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor
            .spawn_critical("session download events", async move {
                loop {
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            debug!("SessionLifecycle download-event handler shutting down");
                            return Ok::<(), String>(());
                        }
                        result = download_rx.recv() => {
                            match result {
                                Some(event) => {
                                    lifecycle.on_download_terminal(&event).await.map_err(|error| {
                                        format!(
                                            "failed to process terminal event for session '{}': {error}",
                                            event.session_id()
                                        )
                                    })?;
                                }
                                None => {
                                    if cancellation_token.is_cancelled() {
                                        return Ok(());
                                    }
                                    return Err(
                                        "required download terminal event channel closed".to_string()
                                    );
                                }
                            }
                        }
                    }
                }
            });

        let runtime_coordinator = self.runtime_coordinator.clone();
        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor
            .spawn_critical("session transition coordinator", async move {
                loop {
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            debug!("Session transition coordinator shutting down");
                            return Ok::<(), String>(());
                        }
                        result = transition_rx.recv() => {
                            match result {
                                Some(transition) => {
                                    runtime_coordinator
                                        .handle_session_transition(transition)
                                        .await;
                                }
                                None => {
                                    if cancellation_token.is_cancelled() {
                                        return Ok(());
                                    }
                                    return Err(
                                        "required session transition channel closed".to_string()
                                    );
                                }
                            }
                        }
                    }
                }
            });
    }

    /// Set up monitor event subscriptions to download manager and danmu service.
    pub(super) fn setup_monitor_event_subscriptions(&self) {
        let runtime_coordinator = self.runtime_coordinator.clone();
        let Some(mut receiver) = self.monitor_event_receiver.lock().take() else {
            warn!("Required monitor event consumer is already running");
            return;
        };
        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor
            .spawn_critical("monitor event handler", async move {
                loop {
                    tokio::select! {
                        _ = cancellation_token.cancelled() => {
                            debug!("Monitor event handler shutting down");
                            return Ok::<(), String>(());
                        }
                        delivery = receiver.recv() => {
                            match delivery {
                                Some(crate::monitor::MonitorEventDelivery {
                                    event,
                                    acknowledgement,
                                }) => {
                                    runtime_coordinator.handle_monitor_event(event, false).await;
                                    if acknowledgement.send(()).is_err() {
                                        debug!("Monitor outbox acknowledgement receiver was dropped");
                                    }
                                }
                                None => {
                                    if cancellation_token.is_cancelled() {
                                        return Ok(());
                                    }
                                    return Err("required monitor event channel closed".to_string());
                                }
                            }
                        }
                    }
                }
            });
    }

    /// Set up danmu event subscriptions for segment coordination.
    pub(super) fn setup_danmu_event_subscriptions(&self) {
        let receiver = self.danmu_service.subscribe();
        let handler = DanmuEventHandler {
            pipeline_manager: self.pipeline_manager.clone(),
            download_manager: self.download_manager.clone(),
            streamer_manager: self.streamer_manager.clone(),
            config_service: self.config_service.clone(),
            stream_monitor: self.stream_monitor.clone(),
            discarded_segment_keys: self.discarded_segment_keys.clone(),
        };
        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor.spawn(
            "danmu event handler",
            handler.run(receiver, cancellation_token),
        );
    }

    /// Set up notification service event subscriptions.
    pub(super) fn setup_notification_event_subscriptions(&self) {
        let notification_service = self.notification_service.clone();
        let monitor_rx = self.monitor_event_broadcaster.subscribe();
        let download_rx = self.download_manager.subscribe();
        let pipeline_rx = self.pipeline_manager.subscribe();
        let session_rx = self.session_lifecycle.subscribe();

        notification_service.start_event_listeners(
            monitor_rx,
            download_rx,
            pipeline_rx,
            session_rx,
        );
        info!("Notification service event listeners started");
    }
}

/// Owned service handles for the `config event handler` task, cloned out of
/// [`ServiceContainer`] by [`ServiceContainer::setup_config_event_subscriptions`]
/// so the spawned future is `'static`.
struct ConfigEventHandler {
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<RuntimeConfigService>,
    download_manager: Arc<DownloadManager>,
    pipeline_manager: Arc<PipelineManager>,
    runtime_coordinator: Arc<RuntimeCoordinator>,
    gpu_health_monitor: Option<Arc<crate::metrics::GpuHealthMonitor>>,
}

impl ConfigEventHandler {
    /// Receive loop for [`ConfigUpdateEvent`]s. A lagged receiver
    /// (recoverable per `broadcast_error_is_recoverable`) compensates for
    /// the skipped events by handling a synthetic `GlobalUpdated`, the
    /// broadest refresh; a closed channel or cancellation ends the task.
    async fn run(
        self,
        mut receiver: broadcast::Receiver<ConfigUpdateEvent>,
        cancellation_token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Config event handler shutting down");
                    break;
                }
                result = receiver.recv() => {
                    let event = match result {
                        Ok(event) => Some(event),
                        Err(error) => {
                            if broadcast_error_is_recoverable("config", error) {
                                Some(ConfigUpdateEvent::GlobalUpdated)
                            } else {
                                None
                            }
                        }
                    };
                    let Some(event) = event else {
                        break;
                    };
                    self.handle_event(event).await;
                }
            }
        }
    }

    async fn handle_event(&self, event: ConfigUpdateEvent) {
        match event {
            ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                // Ensure merged config cache is not stale after streamer/template/platform changes.
                self.config_service.invalidate_streamer(&streamer_id);

                // Refresh the cached effective offline_check_* on the
                // streamer metadata so the actor's StreamerConfig and
                // SessionLifecycle hysteresis backstop pick up any new
                // per-streamer override.
                self.runtime_coordinator
                    .refresh_metadata_offline_check(&streamer_id)
                    .await;

                // Config update event - handles name, URL, priority, template changes.
                // If the update includes a state transition to an inactive state
                // (e.g., user disables a streamer via API), we must still perform
                // best-effort cleanup to stop active downloads and danmu collection.
                debug!("Received streamer config update event: {}", streamer_id);

                // Align with ConfigUpdateEvent docs: handlers should check
                // `metadata.is_active()` to determine if cleanup is needed.
                match self.streamer_manager.get_streamer(&streamer_id) {
                    Some(metadata) if !metadata.is_active() => {
                        info!(
                            "Streamer {} is inactive after update (state: {}), initiating cleanup",
                            streamer_id, metadata.state
                        );
                        self.runtime_coordinator
                            .handle_streamer_disabled(&streamer_id)
                            .await;
                    }
                    Some(_) => {}
                    None => {
                        // Streamer not in memory (race with delete/hydration issues).
                        // Best-effort cleanup anyway.
                        warn!(
                            "Streamer {} not found after update, initiating best-effort cleanup",
                            streamer_id
                        );
                        self.runtime_coordinator
                            .handle_streamer_disabled(&streamer_id)
                            .await;
                    }
                }
            }
            ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                debug!("Received platform config update event: {}", platform_id);
                // Refresh effective offline_check_* on every streamer
                // bound to this platform. The cache invalidation runs
                // upstream; we just need to repopulate metadata.
                let affected: Vec<String> = self
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .filter(|m| m.platform_config_id == platform_id)
                    .map(|m| m.id)
                    .collect();
                for id in affected {
                    self.runtime_coordinator
                        .refresh_metadata_offline_check(&id)
                        .await;
                }
            }
            ConfigUpdateEvent::TemplateUpdated { template_id } => {
                debug!("Received template config update event: {}", template_id);
                let affected: Vec<String> = self
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .filter(|m| m.template_config_id.as_deref() == Some(template_id.as_str()))
                    .map(|m| m.id)
                    .collect();
                for id in affected {
                    self.runtime_coordinator
                        .refresh_metadata_offline_check(&id)
                        .await;
                }
            }
            ConfigUpdateEvent::GlobalUpdated => {
                debug!("Received global config update event");

                // Refresh effective offline_check_* on every streamer
                // since the global default may have changed (and any
                // streamer not overriding this layer inherits from it).
                let all_ids: Vec<String> = self
                    .streamer_manager
                    .get_all()
                    .into_iter()
                    .map(|m| m.id)
                    .collect();
                for id in all_ids {
                    self.runtime_coordinator
                        .refresh_metadata_offline_check(&id)
                        .await;
                }

                match self.config_service.get_global_config().await {
                    Ok(global) => {
                        let new_limit = (global.max_concurrent_downloads as i64).max(1) as usize;
                        let old_limit = self.download_manager.max_concurrent_downloads();

                        if new_limit != old_limit {
                            self.download_manager
                                .set_max_concurrent_downloads(new_limit);
                            info!(
                                "Updated download concurrency: max_concurrent_downloads {} -> {}",
                                old_limit, new_limit
                            );
                        }

                        // Apply the queue-wait freshness threshold. The
                        // setter clamps and returns the applied value.
                        let old_freshness = self.download_manager.queue_freshness_threshold_ms();
                        let new_freshness = self
                            .download_manager
                            .set_queue_freshness_threshold_ms(global.queue_freshness_threshold_ms);
                        if new_freshness != old_freshness {
                            info!(
                                "Updated queue-wait freshness threshold: {} ms -> {} ms",
                                old_freshness, new_freshness
                            );
                        }

                        // Hot-reload the GPU health probe cadence.
                        // No-op when the monitor wasn't registered (no
                        // GPU). `.max(0)` guards against a negative i64
                        // wrapping during the u64 cast (the API
                        // validator already rejects sub-second values,
                        // but the DB could be edited out-of-band);
                        // `set_interval` then clamps to its own minimum.
                        if let Some(monitor) = self.gpu_health_monitor.as_ref() {
                            monitor
                                .set_interval(global.gpu_health_probe_interval_secs.max(0) as u64);
                        }

                        // Wire CPU/IO pipeline job concurrency knobs (best-effort).
                        let cpu_jobs = autoscale_concurrency_limit(global.max_concurrent_cpu_jobs);
                        let io_jobs = autoscale_concurrency_limit(global.max_concurrent_io_jobs);
                        self.pipeline_manager
                            .set_worker_concurrency(cpu_jobs, io_jobs);
                    }
                    Err(e) => {
                        warn!(
                            "Failed to reload global config for download concurrency: {}",
                            e
                        );
                    }
                }
            }
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                // Best-effort: drop any stale cache entry (usually already removed).
                self.config_service.invalidate_streamer(&streamer_id);

                info!("Streamer {} deleted, initiating cleanup", streamer_id);
                // Reuse the same cleanup logic as disabled state
                self.runtime_coordinator
                    .handle_streamer_disabled(&streamer_id)
                    .await;
            }
            ConfigUpdateEvent::EngineUpdated { engine_id } => {
                debug!("Received engine config update event: {}", engine_id);
            }
            ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id,
                is_active,
            } => {
                debug!(
                    "Received streamer state change event: {} (active={})",
                    streamer_id, is_active
                );
                // If streamer became inactive (error, disabled, etc.), clean up
                if !is_active {
                    info!(
                        "Streamer {} became inactive, initiating cleanup",
                        streamer_id
                    );
                    self.runtime_coordinator
                        .handle_streamer_disabled(&streamer_id)
                        .await;
                }
            }
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                // Filters are evaluated by StreamMonitor on each check, but changing
                // them can affect OutOfSchedule smart-wake behavior. Invalidate merged
                // config and let the scheduler/actors re-check soon.
                self.config_service.invalidate_streamer(&streamer_id);
                debug!("Received streamer filters update event: {}", streamer_id);
            }
        }
    }
}

/// Prevent unbounded growth of `ServiceContainer::discarded_segment_keys`
/// if the paired `DanmuEvent::SegmentCompleted` never arrives to consume
/// an entry (best-effort cleanup).
async fn run_discarded_segment_cleanup(
    cleanup_keys: Arc<DashMap<(String, String), Instant>>,
    cleanup_token: CancellationToken,
) {
    const CLEANUP_INTERVAL_SECS: u64 = 600;
    const MAX_AGE_SECS: u64 = 3600;
    let mut interval = tokio::time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));
    loop {
        tokio::select! {
            _ = cleanup_token.cancelled() => break,
            _ = interval.tick() => {
                cleanup_keys.retain(|_, inserted_at| inserted_at.elapsed() < Duration::from_secs(MAX_AGE_SECS));
            }
        }
    }
}

/// Fast path: drain the download broadcast channel into the bounded
/// `event_tx` queue quickly so slow per-event processing in
/// [`DownloadEventProcessor::run`] cannot lag the broadcast receiver and
/// drop critical session events under backpressure. Progress ticks that
/// carry no recovery signal (per `should_record_recovery_from_progress`)
/// are dropped here before they consume queue capacity.
async fn run_download_event_drain(
    mut receiver: broadcast::Receiver<DownloadManagerEvent>,
    event_tx: mpsc::Sender<DownloadManagerEvent>,
    drain_token: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = drain_token.cancelled() => {
                debug!("Download event drain shutting down");
                break;
            }
            result = receiver.recv() => {
                match result {
                    Ok(download_event) => {
                        if let DownloadManagerEvent::Progress(DownloadProgressEvent::Progress { progress, .. }) = &download_event
                            && !should_record_recovery_from_progress(progress)
                        {
                            continue;
                        }
                        if event_tx.send(download_event).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Download event handler lagged {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("Download event channel closed");
                        break;
                    }
                }
            }
        }
    }
}

/// Owned service handles for the `download event processor` task, cloned
/// out of [`ServiceContainer`] by
/// [`ServiceContainer::setup_download_event_subscriptions`] so the spawned
/// future is `'static`. Consumes the queue fed by
/// [`run_download_event_drain`].
struct DownloadEventProcessor {
    pipeline_manager: Arc<PipelineManager>,
    stream_monitor: Arc<RuntimeStreamMonitor>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    danmu_service: Arc<DanmuService>,
    config_service: Arc<RuntimeConfigService>,
    session_lifecycle: Arc<SessionLifecycle>,
    discarded_segment_keys: Arc<DashMap<(String, String), Instant>>,
}

impl DownloadEventProcessor {
    async fn run(
        self,
        mut event_rx: mpsc::Receiver<DownloadManagerEvent>,
        process_token: CancellationToken,
    ) {
        while let Some(download_event) = event_rx.recv().await {
            if process_token.is_cancelled() {
                debug!("Download event processor shutting down");
                break;
            }
            self.handle_event(download_event).await;
        }
    }

    async fn handle_event(&self, download_event: DownloadManagerEvent) {
        // Handle download failure for streamer error tracking. Danmu
        // collection is stopped separately by
        // `RuntimeCoordinator::handle_session_transition` so Failed
        // and Cancelled share one code path.
        if let DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
            ref streamer_id,
            ref error,
            ..
        }) = download_event
            && let Some(metadata) = self.streamer_manager.get_streamer(streamer_id)
        {
            if let Err(e) = self.stream_monitor.handle_error(&metadata, error).await {
                warn!("Failed to record download error for {}: {}", streamer_id, e);
            } else {
                debug!("Recorded download error for {}: {}", streamer_id, error);
            }
        }

        if let DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
            ref streamer_id,
            ref progress,
            ..
        }) = download_event
            && should_record_recovery_from_progress(progress)
            && let Some(metadata) = self.streamer_manager.get_streamer(streamer_id)
            && metadata.is_active()
            && has_transient_error_state(&metadata)
            && let Err(e) = self
                .streamer_manager
                .record_success(streamer_id, true)
                .await
        {
            warn!(
                streamer_id = %streamer_id,
                error = %e,
                "failed to clear transient streamer error state after sustained download progress"
            );
        }

        // Handle danmu segmentation
        match &download_event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
                session_id,
                streamer_id,
                segment_path,
                segment_index,
                started_at,
                ..
            }) => {
                if let Some(metadata) = self.streamer_manager.get_streamer(streamer_id)
                    && !metadata.is_disabled()
                    && metadata.last_error.is_some()
                    && let Err(e) = self.streamer_manager.clear_last_error(streamer_id).await
                {
                    warn!(
                        streamer_id = %streamer_id,
                        error = %e,
                        "failed to clear streamer last_error on segment start"
                    );
                }

                if let Some(handle) = self.danmu_service.get_handle(session_id) {
                    let path = std::path::Path::new(segment_path);
                    let segment_id = segment_index.to_string();

                    // Start danmu segment
                    // We change extension to .xml for danmu file
                    let mut danmu_path = path.to_path_buf();
                    danmu_path.set_extension("xml");

                    if let Err(e) = handle
                        .start_segment(&segment_id, danmu_path, started_at.to_owned())
                        .await
                    {
                        warn!("Failed to start danmu segment: {}", e);
                    }
                }
            }
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                session_id,
                streamer_id,
                segment_path,
                segment_index,
                size_bytes,
                ..
            }) => {
                self.session_lifecycle.on_segment_completed(streamer_id);

                // Decide discard *before* ending danmu segment so we can suppress the
                // imminent DanmuEvent::SegmentCompleted (avoids pipeline race with deletion).
                let mut discard = false;
                let effective_size_bytes = tokio::fs::metadata(segment_path)
                    .await
                    .map_or(*size_bytes, |m| m.len());

                // Resolve config to check min_size.
                match self
                    .config_service
                    .get_config_for_streamer(streamer_id)
                    .await
                {
                    Ok(config) => {
                        let min = u64::try_from(config.min_segment_size_bytes)
                            .ok()
                            .filter(|v| *v > 0);
                        if let Some(min) = min
                            && effective_size_bytes < min
                        {
                            info!(
                                "Segment {} is too small ({} bytes < min {}), discarding",
                                segment_path, effective_size_bytes, min
                            );
                            discard = true;
                            self.discarded_segment_keys.insert(
                                (session_id.clone(), segment_index.to_string()),
                                Instant::now(),
                            );
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to resolve config for streamer {} during segment completion: {}",
                            streamer_id, e
                        );
                    }
                }

                // Always finish the danmu segment first (Flush/Close XML).
                if let Some(handle) = self.danmu_service.get_handle(session_id) {
                    let segment_id = segment_index.to_string();

                    if let Err(e) = handle.end_segment(&segment_id).await {
                        warn!("Failed to end danmu segment: {}", e);
                    }
                }

                if discard {
                    let path = std::path::Path::new(segment_path);
                    match tokio::fs::remove_file(path).await {
                        Ok(()) => debug!("Deleted small segment: {}", segment_path),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            warn!("Failed to delete small segment {}: {}", segment_path, e)
                        }
                    }

                    let mut danmu_path = path.to_path_buf();
                    danmu_path.set_extension("xml");
                    match tokio::fs::remove_file(&danmu_path).await {
                        Ok(()) => {
                            debug!("Deleted small segment danmu: {}", danmu_path.display())
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => warn!(
                            "Failed to delete small segment danmu {}: {}",
                            danmu_path.display(),
                            e
                        ),
                    }
                    // Discarded segments never reach the pipeline manager.
                    return;
                }

                if let Some(metadata) = self.streamer_manager.get_streamer(streamer_id)
                    && metadata.is_active()
                    && has_transient_error_state(&metadata)
                    && let Err(e) = self
                        .streamer_manager
                        .record_success(streamer_id, true)
                        .await
                {
                    warn!(
                        streamer_id = %streamer_id,
                        error = %e,
                        "failed to clear transient streamer error state after segment completion"
                    );
                }
            }
            _ => {}
        }

        // Forward to pipeline manager. Last use of the event in
        // this call — move it instead of cloning (Progress events
        // carry ids/strings and fire on every progress tick of
        // every active download).
        self.pipeline_manager
            .handle_download_event(download_event)
            .await;
    }
}

/// Owned service handles for the `danmu event handler` task, cloned out of
/// [`ServiceContainer`] by [`ServiceContainer::setup_danmu_event_subscriptions`]
/// so the spawned future is `'static`.
struct DanmuEventHandler {
    pipeline_manager: Arc<PipelineManager>,
    download_manager: Arc<DownloadManager>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<RuntimeConfigService>,
    stream_monitor: Arc<RuntimeStreamMonitor>,
    discarded_segment_keys: Arc<DashMap<(String, String), Instant>>,
}

impl DanmuEventHandler {
    async fn run(
        self,
        mut receiver: broadcast::Receiver<DanmuEvent>,
        cancellation_token: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Danmu event handler shutting down");
                    break;
                }
                result = receiver.recv() => {
                    match result {
                        Ok(event) => self.handle_event(event).await,
                        Err(error) => {
                            if !broadcast_error_is_recoverable("danmu", error) {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn handle_event(&self, event: DanmuEvent) {
        match &event {
            DanmuEvent::CollectionStarted {
                session_id,
                streamer_id,
            } => {
                info!(
                    "Danmu collection started for session {} (streamer: {})",
                    session_id, streamer_id
                );
                self.pipeline_manager
                    .handle_danmu_event(event.clone())
                    .await;
            }
            DanmuEvent::CollectionStopped {
                session_id,
                statistics,
            } => {
                info!(
                    "Danmu collection stopped for session {}: {} messages",
                    session_id, statistics.total_count
                );
                self.pipeline_manager
                    .handle_danmu_event(event.clone())
                    .await;
            }
            DanmuEvent::SegmentStarted {
                session_id,
                segment_id,
                output_path,
                start_time,
                ..
            } => {
                debug!(
                    "Danmu segment started: session={}, segment={}, path={:?}, start_time={}",
                    session_id, segment_id, output_path, start_time
                );
            }
            DanmuEvent::SegmentCompleted {
                session_id,
                segment_id,
                output_path,
                message_count,
                ..
            } => {
                info!(
                    "Danmu segment completed: session={}, segment={}, messages={}",
                    session_id, segment_id, message_count
                );
                if self
                    .discarded_segment_keys
                    .remove(&(session_id.clone(), segment_id.clone()))
                    .is_some()
                {
                    match tokio::fs::remove_file(output_path).await {
                        Ok(()) => {
                            debug!("Deleted discarded danmu segment: {}", output_path.display())
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => warn!(
                            "Failed to delete discarded danmu segment {}: {}",
                            output_path.display(),
                            e
                        ),
                    }
                    debug!(
                        "Skipping danmu segment {} for session {} (paired video discarded)",
                        segment_id, session_id
                    );
                    return;
                }
                // Forward to pipeline manager for processing
                self.pipeline_manager
                    .handle_danmu_event(event.clone())
                    .await;
            }
            DanmuEvent::Control {
                session_id,
                streamer_id,
                platform,
                control,
            } => {
                warn!(
                    "Danmu control event for session {} (streamer={} platform={}): {:?}",
                    session_id, streamer_id, platform, control
                );

                // Forward to pipeline manager (e.g., title updates).
                self.pipeline_manager
                    .handle_danmu_event(event.clone())
                    .await;

                if matches!(
                    control,
                    crate::danmu::DanmuControlEvent::StreamClosed { .. }
                ) {
                    self.handle_stream_closed(session_id, streamer_id, platform)
                        .await;
                }
            }
            DanmuEvent::Reconnecting {
                session_id,
                attempt,
            } => {
                warn!(
                    "Danmu reconnecting for session {}: attempt {}",
                    session_id, attempt
                );
            }
            DanmuEvent::ReconnectFailed { session_id, error } => {
                warn!(
                    "Danmu reconnect failed for session {}: {}",
                    session_id, error
                );
            }
            DanmuEvent::Error { session_id, error } => {
                warn!("Danmu error for session {}: {}", session_id, error);
            }
        }
    }

    /// Treat a danmu stream-closed control event as authoritative
    /// end-of-stream (unless the platform config opts out via
    /// `should_end_stream_on_danmu_stream_closed`):
    /// - stop downloads promptly
    /// - end session and bypass resume hysteresis
    async fn handle_stream_closed(&self, session_id: &str, streamer_id: &str, platform: &str) {
        let should_end_stream = match self
            .config_service
            .get_platform_config_by_name(platform)
            .await
        {
            Ok(platform_config) => should_end_stream_on_danmu_stream_closed(
                platform_config.platform_specific_config.as_deref(),
            ),
            Err(e) => {
                warn!(
                    "Failed to load platform config for '{}' while handling danmu stream closed: {}",
                    platform, e
                );
                true
            }
        };

        if !should_end_stream {
            info!(
                session_id = %session_id,
                streamer_id = %streamer_id,
                platform = %platform,
                "Ignoring danmu stream-closed signal due to platform config"
            );
            return;
        }

        debug!(
            session_id = %session_id,
            streamer_id = %streamer_id,
            "Danmu stream closed; forcing end-of-stream handling"
        );

        if let Some(download_info) = self.download_manager.get_download_by_streamer(streamer_id) {
            match self
                .download_manager
                .stop_download_with_reason(
                    &download_info.id,
                    crate::downloader::DownloadStopCause::DanmuStreamClosed,
                )
                .await
            {
                Ok(()) => info!(
                    session_id = %session_id,
                    streamer_id = %streamer_id,
                    download_id = %download_info.id,
                    "Stopped download after danmu stream closed"
                ),
                Err(e) => warn!(
                    "Failed to stop download {} after danmu stream closed (streamer={}): {}",
                    download_info.id, streamer_id, e
                ),
            }
        } else {
            debug!(
                session_id = %session_id,
                streamer_id = %streamer_id,
                "No active download found to stop after danmu stream closed"
            );
        }

        // The danmu observer lets the lifecycle perform the
        // terminal write. `handle_offline_with_session` routes
        // to `lifecycle.on_offline_detected`, which writes
        // `end_time` atomically. That DB write is the fence
        // that makes the next `LiveDetected` create a fresh
        // session.
        //
        // Pass `Some(DanmuStreamClosed)` so the lifecycle
        // promotes the cause to
        // `TerminalCause::DefinitiveOffline { signal }` —
        // preserves "danmu triggered this end" in the audit
        // log instead of the generic `StreamerOffline`.
        if let Some(streamer) = self.streamer_manager.get_streamer(streamer_id) {
            if let Err(e) = self
                .stream_monitor
                .handle_offline_with_session(
                    &streamer,
                    Some(session_id.to_owned()),
                    Some(crate::session::state::OfflineSignal::DanmuStreamClosed),
                )
                .await
            {
                warn!(
                    "Failed to mark streamer offline after danmu stream closed (streamer={} session={}): {}",
                    streamer_id, session_id, e
                );
            }
        } else {
            warn!(
                "Streamer metadata not found for stream-closed danmu control (streamer={} session={})",
                streamer_id, session_id
            );
        }
    }
}
