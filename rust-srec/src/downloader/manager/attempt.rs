//! Download attempt lifecycle implementation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::Result;
use crate::downloader::SegmentInfo;
use crate::downloader::engine::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadProgress, DownloadStatus, EngineType,
    SegmentEvent,
};
use crate::downloader::output_root_gate::OutputRootGate;
use crate::downloader::queue::{Priority, SlotGuard};
use crate::downloader::resilience::EngineKey;

use super::{
    ActiveDownload, DownloadManager, DownloadManagerEvent, DownloadProgressEvent,
    DownloadTerminalEvent, resolve_segment_path,
};

impl DownloadManager {
    pub(super) async fn start_download_with_engine_and_slot(
        &self,
        config: DownloadConfig,
        engine: Arc<dyn DownloadEngine>,
        engine_type: EngineType,
        engine_key: EngineKey,
        slot: SlotGuard,
    ) -> Result<String> {
        let is_high_priority = matches!(slot.priority(), Priority::High);
        let active_slot = slot.into_active();
        Self::seed_session_segment_index(
            &self.session_segment_indices,
            &config.session_id,
            config.initial_segment_index,
        );

        // Generate download ID
        let download_id = uuid::Uuid::new_v4().to_string();

        // Create event channel for this download
        let (segment_tx, mut segment_rx) = mpsc::channel::<SegmentEvent>(32);

        // Create download handle
        let handle = Arc::new(DownloadHandle::new(
            download_id.clone(),
            engine_type,
            config.clone(),
            segment_tx,
        ));

        // Store active download
        let cdn_host = crate::utils::url::extract_host(&config.url).unwrap_or_default();
        self.active_downloads.insert(
            download_id.clone(),
            ActiveDownload {
                handle: handle.clone(),
                status: DownloadStatus::Starting,
                progress: DownloadProgress::default(),
                is_high_priority,
                output_path: None,
                current_segment_index: None,
                current_engine_segment_index: None,
                current_segment_path: None,
                current_segment_started_at: None,
                slot: Some(active_slot),
                retry_config_override: None,
            },
        );

        // Emit start event (broadcast send is synchronous, ignore if no receivers)
        self.events.publish(DownloadManagerEvent::Progress(
            DownloadProgressEvent::DownloadStarted {
                download_id: download_id.clone(),
                streamer_id: config.streamer_id.clone(),
                streamer_name: config.streamer_name.clone(),
                session_id: config.session_id.clone(),
                engine_type,
                cdn_host,
                download_url: config.url.clone(),
            },
        ));

        info!(
            "Starting download {} for streamer {} with engine {}",
            download_id, config.streamer_id, engine_type
        );

        // Start the engine
        let engine_clone = engine.clone();
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            if let Err(e) = engine_clone.start(handle_clone.clone()).await {
                error!("Engine start error: {}", e);
                let _ = handle_clone
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        kind: e.kind,
                        message: format!("Engine start error: {}", e),
                    })
                    .await;
            }
        });

        // Spawn task to handle segment events
        let download_id_clone = download_id.clone();
        let events = self.events.clone();
        let streamer_id = config.streamer_id.clone();
        let streamer_name = config.streamer_name.clone();
        let session_id = config.session_id.clone();
        let protocol = config.protocol;

        // Clone references for the spawned task
        let active_downloads = self.active_downloads.clone();
        let pending_updates = self.pending_updates.clone();
        let session_segment_indices = self.session_segment_indices.clone();
        let circuit_breakers_ref = self.circuit_breakers.get(&engine_key);
        // Handle into the segment event loop so runtime ENOSPC from the
        // engine stderr readers can reach `gate.record_failure` — the
        // mid-stream case where today's date dir already exists and
        // `prepare_output_dir` has nothing to detect.
        let output_root_gate_ref: Option<Arc<OutputRootGate>> =
            self.output_root_gate.get().cloned();

        tokio::spawn(async move {
            // Limit how often we broadcast progress updates (per download).
            // Engines may emit progress 1-10x/sec; broadcasting every tick can overwhelm
            // tokio::broadcast (clone-per-subscriber) and the WS clients.
            const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(250);
            let mut last_progress_emit = Instant::now() - PROGRESS_MIN_INTERVAL;

            // engine_segment_index -> session_segment_index for THIS download
            // attempt. Local to the spawn loop — populated as we observe
            // engine events, dropped when the loop exits. Trailing
            // `SegmentCompleted` events flushed by the engine after
            // `stop_download_with_reason` or after the dedicated cleanup
            // subscriber ran `clear_session_segment_index` still resolve to
            // the index allocated by their matching `SegmentStarted`, because
            // the map is alive for as long as we are draining the channel.
            //
            // A *new* engine_segment_index arriving after
            // `clear_session_segment_index` ran would allocate from a
            // recreated counter starting at 0, but that requires the engine
            // to start a fresh segment after the session has been declared
            // Ended — which is not realistic for stop-then-flush, and the
            // orphan is ignored by the pipeline coordinator's
            // `session_complete_triggered` gate.
            let mut engine_to_session: HashMap<u32, u32> = HashMap::new();
            // Danmu derives its sibling path from SegmentStarted, so completion must reuse
            // the same resolved representation.
            let mut engine_segment_paths: HashMap<u32, String> = HashMap::new();

            while let Some(event) = segment_rx.recv().await {
                match event {
                    SegmentEvent::SegmentCompleted(info) => {
                        let SegmentInfo {
                            path,
                            duration_secs,
                            size_bytes,
                            index,
                            started_at: info_started_at,
                            completed_at,
                            split_reason_code,
                            split_reason_details_json,
                            ..
                        } = info;
                        let segment_path = engine_segment_paths
                            .remove(&index)
                            .unwrap_or_else(|| resolve_segment_path(&path));
                        // Prefer started_at from SegmentInfo (shared between start/complete callbacks),
                        // fall back to the active_downloads lookup for backward compat.
                        let started_at = info_started_at.or_else(|| {
                            active_downloads
                                .get(&download_id_clone)
                                .and_then(|download| {
                                    if download.current_engine_segment_index == Some(index) {
                                        download.current_segment_started_at.as_ref().cloned()
                                    } else {
                                        None
                                    }
                                })
                        });
                        let segment_index = *engine_to_session.entry(index).or_insert_with(|| {
                            Self::allocate_next_session_segment_index(
                                &session_segment_indices,
                                &session_id,
                            )
                        });

                        // Broadcast send is synchronous, ignore if no receivers
                        events.publish(DownloadManagerEvent::Progress(
                            DownloadProgressEvent::SegmentCompleted {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                segment_path: segment_path.clone(),
                                segment_index,
                                started_at,
                                completed_at,
                                duration_secs,
                                size_bytes,
                                split_reason_code,
                                split_reason_details_json,
                            },
                        ));

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.output_path = Some(segment_path);
                            if download.current_engine_segment_index == Some(index) {
                                download.current_engine_segment_index = None;
                                download.current_segment_index = None;
                                download.current_segment_path = None;
                                download.current_segment_started_at = None;
                            }
                        }
                        debug!(
                            download_id = %download_id_clone,
                            path = %path.display(),
                            "Segment completed"
                        );
                    }
                    SegmentEvent::Progress(progress) => {
                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.progress = progress.clone();
                            download.status = DownloadStatus::Downloading;
                        }

                        // Broadcast progress event to WebSocket subscribers (throttled).
                        if last_progress_emit.elapsed() >= PROGRESS_MIN_INTERVAL {
                            last_progress_emit = Instant::now();
                            events.publish(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: download_id_clone.clone(),
                                    streamer_id: streamer_id.clone(),
                                    streamer_name: streamer_name.clone(),
                                    session_id: session_id.clone(),
                                    status: DownloadStatus::Downloading,
                                    progress,
                                },
                            ));
                        }
                    }
                    SegmentEvent::DownloadCompleted {
                        total_bytes,
                        total_duration_secs,
                        total_segments,
                        engine_signal,
                    } => {
                        circuit_breakers_ref.record_success();

                        // If progress is throttled, the latest tick might not have been broadcast.
                        // Emit one final progress update before sending the terminal event.
                        if let Some(download) = active_downloads.get(&download_id_clone) {
                            let final_progress = download.progress.clone();
                            events.publish(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: download_id_clone.clone(),
                                    streamer_id: streamer_id.clone(),
                                    streamer_name: streamer_name.clone(),
                                    session_id: session_id.clone(),
                                    status: DownloadStatus::Downloading,
                                    progress: final_progress,
                                },
                            ));
                        }

                        // remove download from active_downloads
                        // just before the event to avoid race condition
                        let output_path = if let Some((_, download)) =
                            active_downloads.remove(&download_id_clone)
                        {
                            download.output_path
                        } else {
                            None
                        };

                        pending_updates.remove(&download_id_clone);

                        // Dropping the active download removes its
                        // ActiveSlot, which releases the queue capacity
                        // and wakes the next waiter automatically.

                        events.publish(DownloadManagerEvent::Terminal(
                            DownloadTerminalEvent::Completed {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                total_bytes,
                                total_duration_secs,
                                total_segments,
                                file_path: output_path,
                                // Forwarded from the engine's SegmentEvent::
                                // DownloadCompleted unchanged. Lifecycle reads
                                // this to decide hysteresis vs direct Ended.
                                engine_signal,
                            },
                        ));

                        debug!(
                            download_id = %download_id_clone,
                            "Download completed"
                        );
                        break;
                    }
                    SegmentEvent::DiskFull { output_dir, detail } => {
                        // Out-of-band signal only — the engine will still
                        // emit its own DownloadFailed on exit. Feeding the
                        // gate here short-circuits other streamers under
                        // the same root before they reach the engine.
                        if let Some(gate) = output_root_gate_ref.as_ref() {
                            let synthetic_io_err =
                                std::io::Error::new(std::io::ErrorKind::StorageFull, detail);
                            gate.record_failure(&output_dir, &synthetic_io_err);
                        } else {
                            debug!(
                                "DiskFull event received but no output-root gate attached; ignoring"
                            );
                        }
                    }
                    SegmentEvent::DownloadFailed { kind, message } => {
                        if kind.affects_circuit_breaker() {
                            circuit_breakers_ref.record_failure();
                        }

                        let recoverable = kind.is_recoverable();

                        // Emit one final progress update (best-effort) before the failure event.
                        if let Some(download) = active_downloads.get(&download_id_clone) {
                            let final_progress = download.progress.clone();
                            events.publish(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: download_id_clone.clone(),
                                    streamer_id: streamer_id.clone(),
                                    streamer_name: streamer_name.clone(),
                                    session_id: session_id.clone(),
                                    status: DownloadStatus::Downloading,
                                    progress: final_progress,
                                },
                            ));
                        }

                        // remove download from active_downloads
                        // just before the event to avoid race condition
                        active_downloads.remove(&download_id_clone);
                        pending_updates.remove(&download_id_clone);

                        // Dropping the active download removes its
                        // ActiveSlot, which releases the queue capacity
                        // and wakes the next waiter automatically.

                        events.publish(DownloadManagerEvent::Terminal(
                            DownloadTerminalEvent::Failed {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                engine_type,
                                protocol,
                                kind,
                                error: message,
                                recoverable,
                            },
                        ));

                        break;
                    }
                    SegmentEvent::SegmentStarted {
                        path,
                        sequence,
                        started_at,
                    } => {
                        let segment_path = resolve_segment_path(&path);
                        engine_segment_paths.insert(sequence, segment_path.clone());
                        let segment_index =
                            *engine_to_session.entry(sequence).or_insert_with(|| {
                                Self::allocate_next_session_segment_index(
                                    &session_segment_indices,
                                    &session_id,
                                )
                            });

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.current_engine_segment_index = Some(sequence);
                            download.current_segment_index = Some(segment_index);
                            download.current_segment_path = Some(segment_path.clone());
                            download.current_segment_started_at = Some(started_at);
                        }

                        // Emit segment started event
                        events.publish(DownloadManagerEvent::Progress(
                            DownloadProgressEvent::SegmentStarted {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                segment_path: segment_path.clone(),
                                segment_index,
                                started_at,
                            },
                        ));

                        if let Some((_, pending_update)) =
                            pending_updates.remove(&download_id_clone)
                            && let Some(mut download) = active_downloads.get_mut(&download_id_clone)
                        {
                            DownloadManager::apply_pending_update_to_download(
                                &mut download,
                                pending_update,
                                &download_id_clone,
                                &streamer_id,
                                &events,
                            );
                        }

                        debug!(
                            download_id = %download_id_clone,
                            path = %path.display(),
                            engine_segment_index = sequence,
                            segment_index = segment_index,
                            "Segment started"
                        );
                    }
                }
            }
        });

        Ok(download_id)
    }
}
