use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::danmu::DanmuEvent;
use crate::downloader::{DownloadManagerEvent, DownloadProgressEvent, DownloadTerminalEvent};
use crate::monitor::MonitorEvent;

use super::{
    ServiceContainer, autoscale_concurrency_limit, broadcast_error_is_recoverable,
    has_transient_error_state, should_end_stream_on_danmu_stream_closed,
    should_record_recovery_from_progress,
};

impl ServiceContainer {
    /// Set up config event subscriptions between services.
    pub(super) fn setup_config_event_subscriptions(&self) {
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let download_manager = self.download_manager.clone();
        let pipeline_manager = self.pipeline_manager.clone();
        let danmu_service = self.danmu_service.clone();
        let session_lifecycle = self.session_lifecycle.clone();
        let gpu_health_monitor = self.gpu_health_monitor.get().cloned();
        let mut receiver = self.event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        // Spawn a task to handle config update events
        tokio::spawn(async move {
            use crate::config::ConfigUpdateEvent;

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

                        match event {
                                    ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                                        // Ensure merged config cache is not stale after streamer/template/platform changes.
                                        config_service.invalidate_streamer(&streamer_id);

                                        // Refresh the cached effective offline_check_* on the
                                        // streamer metadata so the actor's StreamerConfig and
                                        // SessionLifecycle hysteresis backstop pick up any new
                                        // per-streamer override.
                                        Self::refresh_metadata_offline_check(
                                            &streamer_manager,
                                            &config_service,
                                            &streamer_id,
                                        )
                                        .await;

                                        // Config update event - handles name, URL, priority, template changes.
                                        // If the update includes a state transition to an inactive state
                                        // (e.g., user disables a streamer via API), we must still perform
                                        // best-effort cleanup to stop active downloads and danmu collection.
                                        debug!(
                                            "Received streamer config update event: {}",
                                            streamer_id
                                        );

                                        // Align with ConfigUpdateEvent docs: handlers should check
                                        // `metadata.is_active()` to determine if cleanup is needed.
                                        match streamer_manager.get_streamer(&streamer_id) {
                                            Some(metadata) if !metadata.is_active() => {
                                                info!(
                                                    "Streamer {} is inactive after update (state: {}), initiating cleanup",
                                                    streamer_id, metadata.state
                                                );
                                                Self::handle_streamer_disabled(
                                                    &download_manager,
                                                    &danmu_service,
                                                    &session_lifecycle,
                                                    &streamer_manager,
                                                    &streamer_id,
                                                )
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
                                                Self::handle_streamer_disabled(
                                                    &download_manager,
                                                    &danmu_service,
                                                    &session_lifecycle,
                                                    &streamer_manager,
                                                    &streamer_id,
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                    ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                                        debug!(
                                            "Received platform config update event: {}",
                                            platform_id
                                        );
                                        // Refresh effective offline_check_* on every streamer
                                        // bound to this platform. The cache invalidation runs
                                        // upstream; we just need to repopulate metadata.
                                        let affected: Vec<String> = streamer_manager
                                            .get_all()
                                            .into_iter()
                                            .filter(|m| m.platform_config_id == platform_id)
                                            .map(|m| m.id)
                                            .collect();
                                        for id in affected {
                                            Self::refresh_metadata_offline_check(
                                                &streamer_manager,
                                                &config_service,
                                                &id,
                                            )
                                            .await;
                                        }
                                    }
                                    ConfigUpdateEvent::TemplateUpdated { template_id } => {
                                        debug!(
                                            "Received template config update event: {}",
                                            template_id
                                        );
                                        let affected: Vec<String> = streamer_manager
                                            .get_all()
                                            .into_iter()
                                            .filter(|m| {
                                                m.template_config_id.as_deref()
                                                    == Some(template_id.as_str())
                                            })
                                            .map(|m| m.id)
                                            .collect();
                                        for id in affected {
                                            Self::refresh_metadata_offline_check(
                                                &streamer_manager,
                                                &config_service,
                                                &id,
                                            )
                                            .await;
                                        }
                                    }
                                    ConfigUpdateEvent::GlobalUpdated => {
                                        debug!("Received global config update event");

                                        // Refresh effective offline_check_* on every streamer
                                        // since the global default may have changed (and any
                                        // streamer not overriding this layer inherits from it).
                                        let all_ids: Vec<String> = streamer_manager
                                            .get_all()
                                            .into_iter()
                                            .map(|m| m.id)
                                            .collect();
                                        for id in all_ids {
                                            Self::refresh_metadata_offline_check(
                                                &streamer_manager,
                                                &config_service,
                                                &id,
                                            )
                                            .await;
                                        }

                                        match config_service.get_global_config().await {
                                            Ok(global) => {
                                                let new_limit =
                                                    (global.max_concurrent_downloads as i64)
                                                        .max(1)
                                                        as usize;
                                                let old_limit =
                                                    download_manager.max_concurrent_downloads();

                                                if new_limit != old_limit {
                                                    download_manager
                                                        .set_max_concurrent_downloads(new_limit);
                                                    info!(
                                                        "Updated download concurrency: max_concurrent_downloads {} -> {}",
                                                        old_limit, new_limit
                                                    );
                                                }

                                                // Apply the queue-wait freshness threshold. The
                                                // setter clamps and returns the applied value.
                                                let old_freshness =
                                                    download_manager.queue_freshness_threshold_ms();
                                                let new_freshness = download_manager
                                                    .set_queue_freshness_threshold_ms(
                                                        global.queue_freshness_threshold_ms,
                                                    );
                                                if new_freshness != old_freshness {
                                                    info!(
                                                        "Updated queue-wait freshness threshold: {} ms -> {} ms",
                                                        old_freshness, new_freshness
                                                    );
                                                }

                                                // Hot-reload the GPU health probe cadence (#555).
                                                // No-op when the monitor wasn't registered (no
                                                // GPU). `.max(0)` guards against a negative i64
                                                // wrapping during the u64 cast (the API
                                                // validator already rejects sub-second values,
                                                // but the DB could be edited out-of-band);
                                                // `set_interval` then clamps to its own minimum.
                                                if let Some(monitor) = gpu_health_monitor.as_ref() {
                                                    monitor.set_interval(
                                                        global.gpu_health_probe_interval_secs.max(0)
                                                            as u64,
                                                    );
                                                }

                                                // Wire CPU/IO pipeline job concurrency knobs (best-effort).
                                                let cpu_jobs = autoscale_concurrency_limit(
                                                    global.max_concurrent_cpu_jobs,
                                                );
                                                let io_jobs =
                                                    autoscale_concurrency_limit(
                                                        global.max_concurrent_io_jobs,
                                                    );
                                                pipeline_manager
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
                                        config_service.invalidate_streamer(&streamer_id);

                                        info!(
                                            "Streamer {} deleted, initiating cleanup",
                                            streamer_id
                                        );
                                        // Reuse the same cleanup logic as disabled state
                                        Self::handle_streamer_disabled(
                                            &download_manager,
                                            &danmu_service,
                                            &session_lifecycle,
                                            &streamer_manager,
                                            &streamer_id,
                                        ).await;
                                    }
                                    ConfigUpdateEvent::EngineUpdated { engine_id } => {
                                        debug!(
                                            "Received engine config update event: {}",
                                            engine_id
                                        );
                                    }
                                    ConfigUpdateEvent::StreamerStateSyncedFromDb { streamer_id, is_active } => {
                                        debug!(
                                            "Received streamer state change event: {} (active={})",
                                            streamer_id, is_active
                                        );
                                        // If streamer became inactive (error, disabled, etc.), clean up
                                        if !is_active {
                                            info!("Streamer {} became inactive, initiating cleanup", streamer_id);
                                            Self::handle_streamer_disabled(
                                                &download_manager,
                                                &danmu_service,
                                                &session_lifecycle,
                                                &streamer_manager,
                                                &streamer_id,
                                            ).await;
                                        }
                                    }
                                    ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                                        // Filters are evaluated by StreamMonitor on each check, but changing
                                        // them can affect OutOfSchedule smart-wake behavior. Invalidate merged
                                        // config and let the scheduler/actors re-check soon.
                                        config_service.invalidate_streamer(&streamer_id);
                                        debug!(
                                            "Received streamer filters update event: {}",
                                            streamer_id
                                        );
                                    }
                        }
                    }
                }
            }
        });
    }

    /// Set up download event subscriptions to pipeline manager.
    pub(super) fn setup_download_event_subscriptions(&self) {
        let pipeline_manager = self.pipeline_manager.clone();
        let stream_monitor = self.stream_monitor.clone();
        let streamer_manager = self.streamer_manager.clone();
        let danmu_service = self.danmu_service.clone();
        let config_service = self.config_service.clone();
        let discarded_segment_keys = self.discarded_segment_keys.clone();
        let mut receiver = self.download_manager.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        const DOWNLOAD_EVENT_QUEUE_CAPACITY: usize = 8192;
        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::channel::<DownloadManagerEvent>(DOWNLOAD_EVENT_QUEUE_CAPACITY);

        // Prevent unbounded growth if danmu events are missed (best-effort cleanup).
        let cleanup_token = cancellation_token.clone();
        let cleanup_keys = discarded_segment_keys.clone();
        tokio::spawn(async move {
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
        });

        // Fast path: drain broadcast channel quickly so we don't drop critical session events under backpressure.
        let drain_token = cancellation_token.clone();
        tokio::spawn(async move {
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
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Download event handler lagged {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("Download event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });

        let process_token = cancellation_token.clone();
        tokio::spawn(async move {
            while let Some(download_event) = event_rx.recv().await {
                if process_token.is_cancelled() {
                    debug!("Download event processor shutting down");
                    break;
                }

                // Handle download failure for streamer error tracking. Danmu
                // collection is stopped separately by the SessionTransition
                // subscriber in `setup_session_lifecycle_subscriptions` so
                // Failed and Cancelled share one code path.
                if let DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
                    ref streamer_id,
                    ref error,
                    ..
                }) = download_event
                    && let Some(metadata) = streamer_manager.get_streamer(streamer_id)
                {
                    if let Err(e) = stream_monitor.handle_error(&metadata, error).await {
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
                    && let Some(metadata) = streamer_manager.get_streamer(streamer_id)
                    && metadata.is_active()
                    && has_transient_error_state(&metadata)
                    && let Err(e) = streamer_manager.record_success(streamer_id, true).await
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
                        if let Some(metadata) = streamer_manager.get_streamer(streamer_id)
                            && !metadata.is_disabled()
                            && metadata.last_error.is_some()
                            && let Err(e) = streamer_manager.clear_last_error(streamer_id).await
                        {
                            warn!(
                                streamer_id = %streamer_id,
                                error = %e,
                                "failed to clear streamer last_error on segment start"
                            );
                        }

                        if let Some(handle) = danmu_service.get_handle(session_id) {
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
                        // Decide discard *before* ending danmu segment so we can suppress the
                        // imminent DanmuEvent::SegmentCompleted (avoids pipeline race with deletion).
                        let mut discard = false;
                        let effective_size_bytes = tokio::fs::metadata(segment_path)
                            .await
                            .map(|m| m.len())
                            .unwrap_or(*size_bytes);

                        // Resolve config to check min_size.
                        match config_service.get_config_for_streamer(streamer_id).await {
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
                                    discarded_segment_keys.insert(
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
                        if let Some(handle) = danmu_service.get_handle(session_id) {
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
                            continue;
                        }

                        if let Some(metadata) = streamer_manager.get_streamer(streamer_id)
                            && metadata.is_active()
                            && has_transient_error_state(&metadata)
                            && let Err(e) = streamer_manager.record_success(streamer_id, true).await
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
                // this iteration — move it instead of cloning (Progress
                // events carry ids/strings and fire on every progress tick
                // of every active download).
                pipeline_manager.handle_download_event(download_event).await;
            }
        });
    }

    /// Feed terminal download events into `SessionLifecycle` so every
    /// terminal download outcome closes the session row and emits
    /// `SessionTransition::Ended`, and feed those transitions into the
    /// pipeline manager so the session-complete DAG fires at the right
    /// moment (per `cause.should_run_session_complete_pipeline()`).
    pub(super) fn setup_session_lifecycle_subscriptions(&self) {
        // Download terminals → SessionLifecycle.
        let lifecycle = self.session_lifecycle.clone();
        let mut download_rx = self.download_manager.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("SessionLifecycle download-event handler shutting down");
                        break;
                    }
                    result = download_rx.recv() => {
                        match result {
                            Ok(DownloadManagerEvent::Terminal(event)) => {
                                if let Err(e) = lifecycle.on_download_terminal(&event).await {
                                    warn!(
                                        session_id = %event.session_id(),
                                        streamer_id = %event.streamer_id(),
                                        error = %e,
                                        "SessionLifecycle failed to process terminal download event",
                                    );
                                }
                            }
                            Ok(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::SegmentCompleted { streamer_id, .. },
                            )) => {
                                // Successful segment → reset the classifier's
                                // consecutive-failure counter for this streamer
                                // (preserves Bilibili-style mid-stream RST
                                // reconnects from being classified as offline).
                                lifecycle.on_segment_completed(&streamer_id);
                            }
                            Ok(DownloadManagerEvent::Progress(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("SessionLifecycle download-event handler lagged {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("SessionLifecycle download-event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });

        // SessionTransition → PipelineManager.
        let pipeline_manager = self.pipeline_manager.clone();
        let mut transition_rx = self.session_lifecycle.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Pipeline session-transition handler shutting down");
                        break;
                    }
                    result = transition_rx.recv() => {
                        match result {
                            Ok(transition) => {
                                pipeline_manager.handle_session_transition(transition).await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Pipeline session-transition handler lagged {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("Pipeline session-transition channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });

        // SessionTransition::Ended → DownloadManager::clear_session_segment_index.
        //
        // The handler is a synchronous DashMap remove; the subscriber does no
        // async work after `recv`, so its broadcast cursor cannot Lag from
        // upstream load. Each subscriber on `session_lifecycle` has an
        // independent cursor, so a slow neighbour (the pipeline-manager
        // subscriber above, which awaits `handle_session_transition`) cannot
        // drop cleanups for `DownloadManager::session_segment_indices`. Without
        // this isolation, a `Lagged` on the pipeline subscriber would leak
        // session-scoped segment-index counters until process restart.
        let download_manager = self.download_manager.clone();
        let mut transition_rx = self.session_lifecycle.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download session-cleanup handler shutting down");
                        break;
                    }
                    result = transition_rx.recv() => {
                        match result {
                            Ok(transition) => {
                                if let crate::session::SessionTransition::Ended { session_id, .. } = transition {
                                    download_manager.clear_session_segment_index(&session_id);
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    lagged = n,
                                    "Download session-cleanup handler lagged; DownloadManager::session_segment_indices entries for the missed sessions will persist until process restart"
                                );
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("Download session-cleanup channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });

        // SessionTransition → stop danmu collection on Failed.
        //
        // Cancelled does not emit SessionTransition (the engine may still
        // flush a Completed, so `SessionLifecycle` leaves the session in
        // Recording); when the actor's cancellation path eventually pushes
        // Offline through the monitor, danmu is stopped by the existing
        // MonitorEvent::StreamerOffline branch. Completed cleanly ends the
        // download and the platform-side danmu close signal stops the
        // stream. Only Failed needs an explicit stop here because the
        // engine has given up without a flush and the monitor may not
        // observe offline for a full polling cycle.
        let danmu_service = self.danmu_service.clone();
        let mut transition_rx = self.session_lifecycle.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Danmu session-transition handler shutting down");
                        break;
                    }
                    result = transition_rx.recv() => {
                        match result {
                            Ok(crate::session::SessionTransition::Ended {
                                session_id,
                                cause: crate::session::TerminalCause::Failed { .. },
                                ..
                            }) => {
                                if !danmu_service.is_collecting(&session_id) {
                                    continue;
                                }
                                match danmu_service.stop_collection(&session_id).await {
                                    Ok(stats) => {
                                        info!(
                                            "Stopped danmu collection after download failure for session {}: {} messages",
                                            session_id, stats.total_count
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to stop danmu collection for session {}: {}",
                                            session_id, e
                                        );
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Danmu session-transition handler lagged {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("Danmu session-transition channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Subscribe to `SessionTransition::Started { from_hysteresis: true, .. }`
    /// and (re)start the download for the resumed session.
    ///
    /// The lifecycle's `resume_from_hysteresis` short-circuits before
    /// `start_or_resume`, so the `MonitorEvent::StreamerLive` outbox
    /// event that drives `handle_monitor_event::StreamerLive` is never
    /// emitted on a resume — leaving the streamer "Live" in memory but
    /// with no actual download running. Without this subscriber, every
    /// FLV TCP-close that resumes within the hysteresis window stops
    /// recording for the rest of the broadcast (kinetic / 2026-05-02
    /// 02:28 → 03:51 was a 1.5-hour silent gap).
    ///
    /// We synthesise a `MonitorEvent::StreamerLive` from the
    /// `DownloadStartPayload` carried on the `Started` transition and
    /// dispatch through the existing handler — same code path as a
    /// fresh-session start. The handler's `has_active_download` guard
    /// makes this idempotent against any race with a real
    /// `MonitorEvent::StreamerLive` outbox event.
    ///
    /// Defence against the resume-vs-Ended race: before dispatching, we
    /// re-check `is_session_active`. If the hysteresis-timer / direct
    /// authoritative-end fired between the lifecycle's broadcast and
    /// our dispatch, we skip — the session is already over.
    pub(super) fn setup_resume_download_subscriber(&self) {
        let download_manager = self.download_manager.clone();
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let danmu_service = self.danmu_service.clone();
        let stream_monitor = self.stream_monitor.clone();
        let session_repository = self.session_repository.clone();
        let session_cancels = self.session_cancels.clone();
        let pending_pipelines = self.pending_pipelines.clone();
        let session_lifecycle = self.session_lifecycle.clone();
        let mut transition_rx = self.session_lifecycle.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Resume-download subscriber shutting down");
                        break;
                    }
                    result = transition_rx.recv() => {
                        match result {
                            Ok(crate::session::SessionTransition::Started {
                                from_hysteresis: true,
                                download_start: Some(payload),
                                session_id,
                                streamer_id,
                                streamer_name,
                                title,
                                category,
                                started_at,
                                ..
                            }) => {
                                if !session_lifecycle.is_session_active(&session_id) {
                                    debug!(
                                        session_id,
                                        streamer_id,
                                        "resume-download subscriber: session no longer active, skipping"
                                    );
                                    continue;
                                }
                                info!(
                                    streamer_id,
                                    session_id,
                                    streamer_name,
                                    "Session resumed from hysteresis — restarting download"
                                );
                                let synthetic = MonitorEvent::StreamerLive {
                                    streamer_id,
                                    session_id,
                                    streamer_name,
                                    streamer_url: payload.streamer_url,
                                    title,
                                    category,
                                    streams: payload.streams,
                                    media_headers: payload.media_headers,
                                    media_extras: payload.media_extras,
                                    timestamp: started_at,
                                };
                                Self::handle_monitor_event(
                                    &download_manager,
                                    &streamer_manager,
                                    &config_service,
                                    &danmu_service,
                                    &stream_monitor,
                                    &session_repository,
                                    &session_cancels,
                                    &pending_pipelines,
                                    synthetic,
                                    /* from_hysteresis_resume */ true,
                                )
                                .await;
                            }
                            Ok(_) => {
                                // Other transitions (Started fresh, Ending, Resumed,
                                // Ended) are handled by other subscribers / paths.
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "resume-download subscriber lagged {} events",
                                    n
                                );
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("resume-download transition channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Set up monitor event subscriptions to download manager and danmu service.
    pub(super) fn setup_monitor_event_subscriptions(&self) {
        let download_manager = self.download_manager.clone();
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let danmu_service = self.danmu_service.clone();
        let stream_monitor = self.stream_monitor.clone();
        let session_repository = self.session_repository.clone();
        let session_cancels = self.session_cancels.clone();
        let pending_pipelines = self.pending_pipelines.clone();
        let mut receiver = self.monitor_event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Monitor event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                Self::handle_monitor_event(
                                    &download_manager,
                                    &streamer_manager,
                                    &config_service,
                                    &danmu_service,
                                    &stream_monitor,
                                    &session_repository,
                                    &session_cancels,
                                    &pending_pipelines,
                                    event,
                                    /* from_hysteresis_resume */ false,
                                ).await;
                            }
                            Err(error) => {
                                if !broadcast_error_is_recoverable("monitor", error) {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    /// Set up danmu event subscriptions for segment coordination.
    pub(super) fn setup_danmu_event_subscriptions(&self) {
        let mut receiver = self.danmu_service.subscribe();
        let pipeline_manager = self.pipeline_manager.clone();
        let download_manager = self.download_manager.clone();
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let stream_monitor = self.stream_monitor.clone();
        let session_lifecycle = self.session_lifecycle.clone();
        let discarded_segment_keys = self.discarded_segment_keys.clone();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Danmu event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                match &event {
                                    DanmuEvent::CollectionStarted { session_id, streamer_id } => {
                                        info!(
                                            "Danmu collection started for session {} (streamer: {})",
                                            session_id, streamer_id
                                        );
                                        pipeline_manager.handle_danmu_event(event.clone()).await;
                                    }
                                    DanmuEvent::CollectionStopped { session_id, statistics } => {
                                        info!(
                                            "Danmu collection stopped for session {}: {} messages",
                                            session_id, statistics.total_count
                                        );
                                        pipeline_manager.handle_danmu_event(event.clone()).await;
                                    }
                                    DanmuEvent::SegmentStarted { session_id, segment_id, output_path, start_time, .. } => {
                                        debug!(
                                            "Danmu segment started: session={}, segment={}, path={:?}, start_time={}",
                                            session_id, segment_id, output_path, start_time
                                        );
                                    }
                                    DanmuEvent::SegmentCompleted { session_id, segment_id, output_path, message_count, .. } => {
                                        info!(
                                            "Danmu segment completed: session={}, segment={}, messages={}",
                                            session_id, segment_id, message_count
                                        );
                                        if discarded_segment_keys
                                            .remove(&(session_id.clone(), segment_id.clone()))
                                            .is_some()
                                        {
                                            match tokio::fs::remove_file(output_path).await {
                                                Ok(()) => debug!(
                                                    "Deleted discarded danmu segment: {}",
                                                    output_path.display()
                                                ),
                                                Err(e)
                                                    if e.kind() == std::io::ErrorKind::NotFound => {}
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
                                            continue;
                                        }
                                        // Forward to pipeline manager for processing
                                        pipeline_manager.handle_danmu_event(event.clone()).await;
                                    }
                                    DanmuEvent::Control { session_id, streamer_id, platform, control } => {
                                        warn!(
                                            "Danmu control event for session {} (streamer={} platform={}): {:?}",
                                            session_id, streamer_id, platform, control
                                        );

                                        // Forward to pipeline manager (e.g., title updates).
                                        pipeline_manager.handle_danmu_event(event.clone()).await;

                                        // Treat stream-closed as authoritative end-of-stream:
                                        // - stop downloads promptly
                                        // - end session and bypass resume hysteresis
                                        if matches!(control, crate::danmu::DanmuControlEvent::StreamClosed { .. }) {
                                            let should_end_stream = match config_service
                                                .get_platform_config_by_name(platform)
                                                .await
                                            {
                                                Ok(platform_config) => {
                                                    should_end_stream_on_danmu_stream_closed(
                                                        platform_config
                                                            .platform_specific_config
                                                            .as_deref(),
                                                    )
                                                }
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
                                                continue;
                                            }

                                            debug!(
                                                session_id = %session_id,
                                                streamer_id = %streamer_id,
                                                "Danmu stream closed; forcing end-of-stream handling"
                                            );

                                            if let Some(download_info) =
                                                download_manager.get_download_by_streamer(streamer_id)
                                            {
                                                match download_manager
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
                                            let _ = &session_lifecycle;
                                            if let Some(streamer) = streamer_manager.get_streamer(streamer_id) {
                                                if let Err(e) = stream_monitor
                                                    .handle_offline_with_session(
                                                        &streamer,
                                                        Some(session_id.clone()),
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
                                    DanmuEvent::Reconnecting { session_id, attempt } => {
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
                                        warn!(
                                            "Danmu error for session {}: {}",
                                            session_id, error
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                if !broadcast_error_is_recoverable("danmu", error) {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });
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
