//! Per-stream download startup coordination.

use std::sync::Arc;

use dashmap::DashMap;
use pipeline_common::expand_path_template;
use tracing::{debug, info, warn};

use crate::danmu::CollectionSpec;
use crate::database::repositories::SessionRepository;
use crate::domain::{Priority, StreamerState};
use crate::downloader::{DownloadConfig, DownloadProtocol};
use crate::utils::filename::sanitize_filename_for_template;

use super::RuntimeCoordinator;

/// Owned payload carrying the per-streamer data needed by
/// [`run_live_download_pipeline`]. Mirrors the relevant fields of
/// [`crate::monitor::MonitorEvent::StreamerLive`] but is decoupled from
/// the enum so the spawned task can capture exactly what it needs.
pub(super) struct StreamerLivePayload {
    pub(super) runtime_instance: Option<String>,
    pub(super) credential_binding: Option<crate::credentials::CredentialBinding>,
    pub(super) streamer_id: String,
    pub(super) session_id: String,
    pub(super) streamer_name: String,
    pub(super) title: String,
    pub(super) streams: Vec<crate::monitor::StreamInfo>,
    pub(super) streamer_url: String,
    pub(super) media_headers: Option<std::collections::HashMap<String, String>>,
    pub(super) media_extras: Option<std::collections::HashMap<String, String>>,
}

/// How a pipeline ended, for the caller that owns its retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PipelineExit {
    Finished,
    /// The bound account changed or became unavailable between extraction and
    /// engine start. Fresh bound media is needed; the cached bundle is unusable.
    CredentialsChanged,
}

/// Removes the per-streamer reservation from
/// `RuntimeCoordinator::pending_pipelines` when the pipeline exits,
/// on every path (early return, panic, completion).
struct PipelineReservationGuard<'a> {
    map: &'a DashMap<String, ()>,
    streamer_id: &'a str,
}

impl Drop for PipelineReservationGuard<'_> {
    fn drop(&mut self) {
        self.map.remove(self.streamer_id);
    }
}

/// Per-streamer download pipeline.
///
/// Runs as a `TaskSupervisor::spawn` task, started by
/// `RuntimeCoordinator::handle_monitor_event` per `StreamerLive` event.
/// Walks the split startup flow:
///
/// 1. **Dedup / pre-checks** — bail if the streamer is already
///    downloading, no longer active, disabled, or has no streams.
/// 2. **Preflight** — engine resolution, circuit breaker, output-root
///    write gate, `prepare_output_dir`. Failures emit
///    `DownloadRejected` events directly (the manager handles that)
///    and the pipeline exits without consuming a queue slot.
/// 3. **Acquire slot** — parks on the priority-aware download queue,
///    emitting `DownloadQueued` if it had to wait. Honours the
///    per-session [`tokio_util::sync::CancellationToken`] so a `StreamerOffline`
///    arriving mid-wait aborts cleanly with no engine startup.
/// 4. **Freshness re-check** — when the wait was non-trivial
///    (`waited_ms > queue_freshness_threshold_ms()`), refetches the
///    live state via `StreamMonitor::check_streamer`; on
///    Offline / Filtered, drops the slot and exits without starting
///    the engine; checker errors retain cached media. Below the threshold, only does a cheap
///    state re-check via the streamer manager.
/// 5. **Start engine** — calls `start_with_slot`, which moves the
///    slot into the active downloads map and emits `DownloadStarted`.
/// 6. **Danmu** — gated on download success, so danmu collection
///    never opens a platform connection for a stream that's still
///    queued or got aborted.
pub(super) async fn run_live_download_pipeline(
    coordinator: Arc<RuntimeCoordinator>,
    payload: StreamerLivePayload,
    // `true` when called for a session that just resumed out of
    // hysteresis. The resume-download subscriber synthesises a
    // `MonitorEvent::StreamerLive` from the lifecycle's
    // `SessionTransition::Started { from_hysteresis: true, .. }` and routes
    // it through the same pipeline as a fresh-live event; this flag
    // tells the short-queue-wait branch to trust the lifecycle signal
    // instead of re-reading the streamer-manager cache.
    from_hysteresis_resume: bool,
) -> PipelineExit {
    use crate::downloader::{AcquireRequest, PreflightRequest, Priority as QueuePriority};

    let RuntimeCoordinator {
        download_manager,
        streamer_manager,
        config_service,
        danmu_service,
        session_repository,
        session_cancels,
        pending_pipelines,
        ..
    } = &*coordinator;

    let StreamerLivePayload {
        runtime_instance,
        mut credential_binding,
        streamer_id,
        session_id,
        streamer_name,
        title,
        mut streams,
        streamer_url,
        mut media_headers,
        mut media_extras,
    } = payload;

    // Per-streamer reservation. The earliest atomic point we can grab
    // — before any await, before preflight, before queue acquire —
    // covers the window where two concurrent `StreamerLive` events
    // could otherwise both pass `has_active_download` and both
    // proceed to `start_with_slot`. Hysteresis-resume synthetic
    // events for the same streamer also funnel through here. The
    // queue's session_id dedup catches the rarer case of duplicate
    // session_ids; this catches the common case of duplicate
    // streamer_ids racing.
    if pending_pipelines.insert(streamer_id.clone(), ()).is_some() {
        debug!(
            "Skipping StreamerLive for {} — pipeline already in flight",
            streamer_id
        );
        return PipelineExit::Finished;
    }
    let _pipeline_guard = PipelineReservationGuard {
        map: pending_pipelines,
        streamer_id: &streamer_id,
    };

    // Per-session cancellation token. The registration handle clears
    // itself on exit, but only if it still owns the same token; this
    // keeps cleanup local to the cancellation registry instead of
    // spreading token lifetime rules through the pipeline.
    let cancel_handle = session_cancels.register(&session_id);
    let cancel = cancel_handle.token();

    // Dedup and pre-checks.
    if download_manager.has_active_download(&streamer_id) {
        debug!("Download already active for {}", streamer_id);
        let active = download_manager.get_active_downloads();
        for conflict in active.iter().filter(|d| d.streamer_id == streamer_id) {
            tracing::warn!(
                "CONFLICTING DOWNLOAD: ID={}, Status={:?}, Started={:?}",
                conflict.id,
                conflict.status,
                conflict.started_at
            );
        }
        return PipelineExit::Finished;
    }

    let streamer_metadata = streamer_manager.get_streamer(&streamer_id);
    if let Some(metadata) = &streamer_metadata {
        if !metadata.is_active() {
            info!(
                "Ignoring StreamerLive for inactive streamer {} (state: {})",
                streamer_id, metadata.state
            );
            return PipelineExit::Finished;
        }
        if metadata.is_disabled() {
            // Returning silently here would strand the pipeline: the session
            // lifecycle has already committed this session to Recording (via
            // `start_or_resume` or `resume_from_hysteresis`), and with no
            // download there is never a DownloadStarted/DownloadEnded to move
            // the streamer actor out of Live — the `(Live, Live)` arm of
            // `HysteresisState::should_emit` then suppresses every future
            // check and recording stays dead for the rest of the broadcast.
            // Emit the same Rejected terminal `preflight` uses so the session
            // lifecycle closes the session (`TerminalCause::Rejected` is an
            // authoritative end) and the actor re-checks once the backoff
            // expires.
            let retry_after_secs = metadata
                .remaining_backoff_std()
                .map_or(0, |d| d.as_secs())
                .saturating_add(2);
            info!(
                streamer_id = %streamer_id,
                streamer_name = %streamer_name,
                disabled_until = ?metadata.disabled_until,
                retry_after_secs,
                "Ignoring StreamerLive while temporarily disabled"
            );
            if let Err(error) = download_manager
                .emit_rejected(
                    streamer_id.clone(),
                    streamer_name.clone(),
                    session_id.clone(),
                    "streamer temporarily disabled (error backoff)".to_string(),
                    Some(retry_after_secs),
                    crate::downloader::DownloadRejectedKind::StreamerBackoff,
                )
                .await
            {
                warn!(%error, "Failed to publish download rejection");
            }
            return PipelineExit::Finished;
        }
    }

    let is_high_priority = streamer_metadata
        .as_ref()
        .is_some_and(|s| s.priority == Priority::High);
    // Load merged config for this streamer.
    let resolved_config = tokio::select! {
        biased;
        _ = cancel.cancelled() => return PipelineExit::Finished,
        config = config_service.get_config_for_streamer(&streamer_id) => config,
    };
    let merged_config = match resolved_config {
        Ok(config) => config,
        Err(e) => {
            warn!(
                "Failed to load config for streamer {}, using defaults: {}",
                streamer_id, e
            );
            Arc::new(crate::config::MergedConfig::builder().build())
        }
    };
    let managed = credential_binding.is_some() || merged_config.credential_policy.is_some();
    let replayed = runtime_instance.as_deref() != Some(crate::monitor::runtime_instance_id());
    let mut credential_snapshot: Option<Arc<crate::credentials::CredentialSnapshot>> = None;
    if streams.is_empty() && !managed && !replayed {
        warn!(streamer_id, "No streams available for download startup");
        return PipelineExit::Finished;
    }
    if let Some(binding) = credential_binding.as_ref() {
        match coordinator
            .stream_monitor
            .session_credential_binding(&session_id)
            .await
        {
            Ok(Some(current))
                if current.epoch == binding.epoch && current.identity == binding.identity => {}
            // An active session without a committed binding (a legacy session
            // whose scope became managed, resumed from hysteresis) has no newer
            // binding to protect. Managed startup re-extracts and commits the
            // first epoch below; that commit also rechecks session liveness.
            Ok(None) if coordinator.session_lifecycle.is_session_active(&session_id) => {}
            _ => {
                debug!(session_id, "Ignoring stale credential handoff");
                return PipelineExit::Finished;
            }
        }
    }

    // Sanitize names for filename usage.
    let sanitized_streamer = sanitize_filename_for_template(&streamer_name);
    let sanitized_title = sanitize_filename_for_template(&title);
    let platform = streamer_metadata
        .as_ref()
        .map_or("unknown", |s| s.platform());

    let dir = merged_config
        .output_folder
        .replace("{streamer}", &sanitized_streamer)
        .replace("{title}", &sanitized_title)
        .replace("{session_id}", &session_id)
        .replace("{platform}", platform);
    let output_dir = expand_path_template(&dir);

    // Preflight.
    let preflight_req = PreflightRequest {
        streamer_id: streamer_id.clone(),
        streamer_name: streamer_name.clone(),
        session_id: session_id.clone(),
        output_dir: output_dir.clone().into(),
        engine_id: Some(merged_config.download_engine.clone()),
        engines_override: merged_config.engines_override.clone(),
    };
    let preflight = tokio::select! {
        biased;
        _ = cancel.cancelled() => return PipelineExit::Finished,
        engine = download_manager.preflight(preflight_req) => engine,
    };
    let engine = match preflight {
        Ok(e) => e,
        Err(e) => {
            warn!("Preflight failed for streamer {}: {}", streamer_id, e);
            return PipelineExit::Finished; // Manager has already emitted DownloadRejected if applicable.
        }
    };
    let engine_type = engine.engine_type;

    // Honour cancellation that fired between preflight and slot acquire.
    if cancel.is_cancelled() {
        debug!("Streamer {} cancelled before slot acquire", streamer_id);
        return PipelineExit::Finished;
    }

    // Acquire slot.
    let acquire_req = AcquireRequest {
        session_id: session_id.clone(),
        streamer_id: streamer_id.clone(),
        streamer_name: streamer_name.clone(),
        engine_type,
        priority: if is_high_priority {
            QueuePriority::High
        } else {
            QueuePriority::Normal
        },
    };
    let slot = match download_manager
        .acquire_slot(acquire_req, cancel.clone())
        .await
    {
        Ok(slot) => slot,
        Err(e) => {
            // Cancelled / duplicate session / shutdown are benign
            // exits. If a visible queued event fired, the manager has
            // already emitted the matching `DownloadDequeued`.
            debug!(
                "acquire_slot returned without a slot for streamer {}: {}",
                streamer_id, e
            );
            return PipelineExit::Finished;
        }
    };

    let waited_ms = slot.waited_ms();
    let recovered = if replayed {
        None
    } else {
        coordinator.take_recovered_media(&session_id, credential_binding.as_ref())
    };

    // Freshness re-check.
    if let Some(recovered) = recovered {
        // A successful credential diagnostic for this session just extracted
        // with the committed binding; its bundle is this attempt's URL renewal.
        debug!(
            session_id,
            "Starting with media from the credential diagnostic"
        );
        streams = recovered.streams;
        media_headers = recovered.media_headers;
        media_extras = recovered.media_extras;
        credential_binding = Some(recovered.binding);
        credential_snapshot = Some(recovered.snapshot);
    } else if managed || replayed || waited_ms > download_manager.queue_freshness_threshold_ms() {
        debug!(
            streamer_id = %streamer_id,
            waited_ms,
            "Queue wait exceeded freshness threshold; refetching live state"
        );
        // Re-fetch via the monitor's deduped, rate-limited check.
        let metadata_for_check = streamer_manager.get_streamer(&streamer_id);
        if let Some(meta) = metadata_for_check {
            let fresh = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                    return PipelineExit::Finished;
                }
                fresh = coordinator.check_startup_freshness(&meta) => fresh,
            };
            match fresh {
                Ok(crate::monitor::LiveStatus::Live {
                    credential_binding: fresh_binding,
                    credential_snapshot: fresh_snapshot,
                    streams: fresh_streams,
                    media_headers: fresh_headers,
                    media_extras: fresh_extras,
                    ..
                }) => {
                    if fresh_streams.is_empty() {
                        debug!(
                            streamer_id = %streamer_id,
                            "Refetch returned Live with no streams; aborting"
                        );
                        download_manager.emit_dequeued_for_slot(
                            &slot,
                            &streamer_id,
                            &streamer_name,
                        );
                        return PipelineExit::Finished;
                    }
                    // Replace BOTH the URLs and the associated
                    // headers/extras. On platforms whose signed
                    // URLs rotate together with required headers
                    // (e.g. Host overrides, signed referer),
                    // keeping the old headers with new URLs would
                    // 403 just as reliably as keeping the old
                    // URLs.
                    streams = fresh_streams;
                    media_headers = fresh_headers;
                    media_extras = fresh_extras.map(|extras| *extras);
                    credential_binding = fresh_binding.map(|binding| *binding);
                    credential_snapshot = fresh_snapshot;
                }
                Ok(_) => {
                    debug!(
                        streamer_id = %streamer_id,
                        "Streamer no longer live after queue wait; aborting"
                    );
                    download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                    return PipelineExit::Finished;
                }
                Err(e) => {
                    if managed || replayed {
                        warn!(streamer_id, error = %e, "Managed download startup awaits fresh credentials and media");
                        download_manager.emit_dequeued_for_slot(
                            &slot,
                            &streamer_id,
                            &streamer_name,
                        );
                        return if matches!(
                            e,
                            crate::Error::CredentialProfile(
                                crate::credentials::ProfileError::SourceChanged
                            )
                        ) {
                            PipelineExit::CredentialsChanged
                        } else {
                            PipelineExit::Finished
                        };
                    }
                    warn!(
                        streamer_id = %streamer_id,
                        error = %e,
                        "Refetch failed; falling back to cached URLs"
                    );
                }
            }
        } else {
            debug!(
                streamer_id = %streamer_id,
                "Streamer metadata vanished during queue wait; aborting"
            );
            download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            return PipelineExit::Finished;
        }
    } else if !from_hysteresis_resume {
        // Cheap re-check for the short-wait case: is the streamer
        // still in a state that permits a fresh recording? `Live`
        // specifically, NOT just `is_active()` — `OutOfSchedule`
        // counts as active in the metadata sense (the streamer is
        // still being monitored) but recording is not allowed.
        // Without this tighter check, a schedule window could close
        // mid-wait and we'd start an out-of-schedule recording.
        //
        // Skipped on hysteresis resume: the lifecycle writes
        // `state=LIVE` before broadcasting, but `StreamMonitor::handle_live`
        // reloads the streamer-manager cache only after the broadcast
        // returns, so this check can read a stale `NotLive`. Out-of-schedule
        // streamers never reach this code path because
        // `monitor::service::handle_live` runs only for `LiveStatus::Live`
        // (filtered events take a different branch), and any window that
        // closes mid-recording is caught later by the
        // `MonitorEvent::StateChanged { OutOfSchedule }` handler.
        let meta = streamer_manager.get_streamer(&streamer_id);
        let permits_start = meta
            .as_ref()
            .is_some_and(|m| m.state == StreamerState::Live && !m.is_disabled());
        if !permits_start {
            debug!(
                streamer_id = %streamer_id,
                state = ?meta.as_ref().map(|m| m.state),
                "Streamer no longer in LIVE state after short queue wait; aborting"
            );
            download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            return PipelineExit::Finished;
        }
    }

    if cancel.is_cancelled() {
        debug!(
            "Streamer {} cancelled between freshness check and engine start",
            streamer_id
        );
        download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
        return PipelineExit::Finished;
    }

    let mut committed_binding = None;
    if let Some(binding) = credential_binding {
        if credential_snapshot.is_none() {
            warn!(
                session_id,
                "Managed download startup has no credential snapshot"
            );
            download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            return PipelineExit::Finished;
        }
        match coordinator
            .session_lifecycle
            .commit_credential_binding(&session_id, &streamer_id, binding)
            .await
        {
            Ok(binding) => committed_binding = Some(binding),
            Err(error) => {
                debug!(session_id, %error, "Credential binding changed before download startup");
                download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                return credentials_exit(&error);
            }
        }
    }

    // ── Build full DownloadConfig with possibly-refreshed URLs ──
    let best_stream = &streams[0];
    let stream_format = best_stream.stream_format.as_str();
    let media_format = best_stream.media_format.as_str();
    let next_index = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            return PipelineExit::Finished;
        }
        index = session_repository.next_session_segment_index(&session_id) => index,
    };
    let initial_segment_index = match next_index {
        Ok(index) => index,
        Err(e) => {
            warn!(
                session_id = %session_id,
                streamer_id = %streamer_id,
                error = %e,
                "Failed to load next persisted session segment index; starting from zero"
            );
            0
        }
    };

    // Last read of `media_headers`; move the map out rather than clone it.
    let mut headers = media_headers.unwrap_or_default();
    if let Some(extras) = best_stream.extras.as_ref() {
        if let Some(extra_headers) = extras.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in extra_headers {
                if let Some(v) = v.as_str() {
                    headers.retain(|existing, _| !existing.eq_ignore_ascii_case(k));
                    headers.insert(k.clone(), v.to_string());
                }
            }
        }
        if let Some(host_header) = extras.get("host_header").and_then(|v| v.as_str()) {
            headers.retain(|existing, _| !existing.eq_ignore_ascii_case("host"));
            headers.insert("Host".to_string(), host_header.to_string());
        }
    }
    let extractor_cookie_updates: Vec<String> = if managed {
        headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("cookie"))
            .map(|(_, value)| value.clone())
            .collect()
    } else {
        Vec::new()
    };
    if managed {
        headers.retain(|key, _| !key.eq_ignore_ascii_case("cookie"));
    }
    if !headers.is_empty() {
        debug!(
            "Using {} merged headers for download: {:?}",
            headers.len(),
            headers.keys().collect::<Vec<_>>()
        );
    }

    let mut config = DownloadConfig::new(
        best_stream.url.clone(),
        output_dir,
        streamer_id.clone(),
        streamer_name.clone(),
        session_id.clone(),
    )
    .with_initial_segment_index(initial_segment_index)
    .with_filename_template(
        merged_config
            .output_filename_template
            .replace("{streamer}", &sanitized_streamer)
            .replace("{title}", &sanitized_title)
            .replace("{platform}", platform),
    )
    .with_output_format(&merged_config.output_file_format)
    .with_protocol(DownloadProtocol::from_format_label(stream_format))
    .with_max_segment_duration(merged_config.max_download_duration_secs as u64)
    .with_max_segment_size(merged_config.max_part_size_bytes as u64)
    .with_engines_override(merged_config.engines_override.clone());
    config.managed_credentials = managed;
    config.credential_binding = committed_binding;

    let cookies = credential_snapshot
        .as_ref()
        .map(|snapshot| {
            crate::credentials::merge_cookie_updates(
                &snapshot.material.cookies,
                extractor_cookie_updates.iter().map(String::as_str),
            )
        })
        .or_else(|| merged_config.cookies.clone());
    if let Some(ref cookies) = cookies {
        debug!(
            "Applying cookies from merged config to download (length: {} chars)",
            cookies.len()
        );
        config = config.with_cookies(cookies);
    }

    let proxy_config = &merged_config.proxy_config;
    if proxy_config.enabled {
        if let Some(effective_proxy_url) = proxy_config.effective_url() {
            // Both the configured and effective URL may contain credentials.
            debug!(
                has_auth = proxy_config.password.is_some(),
                "Applying explicit proxy from merged config to download"
            );
            config = config.with_proxy(effective_proxy_url);
        } else if proxy_config.use_system_proxy {
            debug!("Enabling system proxy for download");
            config = config.with_system_proxy(true);
        }
    }

    for (key, value) in headers {
        config = config.with_header(key, value);
    }

    info!(
        streamer_id,
        session_id,
        stream_format,
        media_format,
        queue_wait_ms = waited_ms,
        initial_segment_index,
        "Starting download with extracted media"
    );
    let danmu_statistics = merged_config.danmu_statistics.clone();

    // Maintenance may defer admission after a slot has been granted. Keep the
    // session cancellation connected until the manager actually starts the engine.
    let mut exit = PipelineExit::Finished;
    let started = match download_manager
        .start_with_slot_cancellable(slot, config, engine, &cancel)
        .await
    {
        Ok(Some(download_id)) => {
            info!(
                "Started download {} for streamer {} (priority: {})",
                download_id,
                streamer_id,
                if is_high_priority { "high" } else { "normal" }
            );
            true
        }
        Ok(None) => {
            debug!(%streamer_id, "Download startup cancelled while waiting for admission");
            false
        }
        Err(crate::Error::SchedulerFeedbackBusy { .. }) => {
            info!(%streamer_id, "Recording admission deferred until scheduler feedback capacity recovers");
            false
        }
        Err(e) => {
            warn!(
                "Failed to start download for streamer {}: {}",
                streamer_id, e
            );
            exit = credentials_exit(&e);
            false
        }
    };

    // Danmu.
    // Gated on the download having a real id. If `start_with_slot`
    // failed, the slot is already released by SlotGuard's drop and
    // there's no engine to interleave danmu with — opening a danmu
    // socket for a stream we're not recording would leak a platform
    // connection.
    // A later attempt of the same session (engine retry, credential recovery)
    // keeps the running collector and hands it this attempt's account.
    if started
        && !cancel.is_cancelled()
        && merged_config.record_danmu
        && danmu_service.is_collecting(&session_id)
    {
        if !danmu_service
            .update_authentication(&session_id, cookies, media_extras)
            .await
        {
            debug!(%session_id, "Danmu collection ended before its account could be updated");
        }
    } else if started && !cancel.is_cancelled() && merged_config.record_danmu {
        match danmu_service
            .start_collection_cancellable(
                CollectionSpec {
                    session_id: session_id.clone(),
                    streamer_id: streamer_id.clone(),
                    streamer_url,
                    cookies,
                    extras: media_extras,
                    statistics: danmu_statistics,
                },
                &cancel,
            )
            .await
        {
            Ok(Some(handle)) => {
                info!(
                    "Started danmu collection for session {} (streamer: {})",
                    handle.session_id(),
                    streamer_id
                );
            }
            Ok(None) => debug!(%session_id, "Danmu startup cancelled and contained"),
            Err(e) => {
                warn!(
                    "Failed to start danmu collection for streamer {}: {}",
                    streamer_id, e
                );
            }
        }
    }
    exit
}

fn credentials_exit(error: &crate::Error) -> PipelineExit {
    match error {
        crate::Error::CredentialProfile(crate::credentials::ProfileError::SourceChanged)
        | crate::Error::CredentialUnavailable(_) => PipelineExit::CredentialsChanged,
        _ => PipelineExit::Finished,
    }
}
