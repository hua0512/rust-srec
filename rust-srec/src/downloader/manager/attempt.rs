//! Download attempt lifecycle implementation.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use futures::FutureExt;
use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;
use tokio::time::timeout_at;
use tracing::{debug, error, info, warn};

use crate::Result;
use crate::downloader::SegmentInfo;
use crate::downloader::engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadProgress,
    DownloadStatus, EngineType, SegmentEvent,
};
use crate::downloader::output_root_gate::OutputRootGate;
use crate::downloader::queue::SlotGuard;
use crate::downloader::resilience::EngineKey;
use crate::utils::task_supervisor::DrainedTasks;

use super::{
    ActiveDownload, AttemptPhase, DownloadManager, DownloadManagerEvent, DownloadProgressEvent,
    DownloadStopCause, DownloadTerminalEvent, PendingConfigUpdate, resolve_segment_path,
};

/// Completion signal shared by stop callers and the attempt finalizer.
pub(super) struct AttemptCompletion {
    outcome: Mutex<Option<std::result::Result<(), String>>>,
    notify: Notify,
}

impl AttemptCompletion {
    fn new() -> Self {
        Self {
            outcome: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    fn finish(&self, outcome: std::result::Result<(), String>) {
        let mut current = self.outcome.lock();
        if current.is_some() {
            return;
        }
        *current = Some(outcome);
        drop(current);
        self.notify.notify_waiters();
    }

    pub(super) async fn wait(&self) -> std::result::Result<(), String> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(outcome) = self.outcome.lock().clone() {
                return outcome;
            }
            notified.await;
        }
    }
}

/// Owns recording-attempt tasks and linearizes admission against shutdown.
pub(super) struct AttemptSupervisor {
    accepting: AtomicBool,
    tasks: Mutex<JoinSet<AttemptTaskReport>>,
    running: Arc<DashMap<String, ()>>,
    failures: Mutex<Vec<String>>,
}

struct AttemptTaskReport {
    download_id: String,
    outcome: std::result::Result<(), String>,
}

pub(super) struct AttemptShutdownReport {
    pub(super) failures: Vec<String>,
    pub(super) deadline_exceeded_download_ids: Vec<String>,
}

impl AttemptSupervisor {
    pub(super) fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            tasks: Mutex::new(JoinSet::new()),
            running: Arc::new(DashMap::new()),
            failures: Mutex::new(Vec::new()),
        }
    }

    fn spawn<F, A>(&self, download_id: String, on_admitted: A, task: F) -> bool
    where
        F: Future<Output = std::result::Result<(), String>> + Send + 'static,
        A: FnOnce(),
    {
        let mut tasks = self.tasks.lock();
        if !self.accepting.load(Ordering::Acquire) {
            return false;
        }

        while let Some(result) = tasks.try_join_next() {
            match result {
                Ok(report) => {
                    debug!(download_id = %report.download_id, "Recording attempt reaped");
                    if let Err(error) = report.outcome {
                        self.failures.lock().push(format!(
                            "download {} failed before shutdown: {error}",
                            report.download_id
                        ));
                    }
                }
                Err(error) => {
                    warn!(%error, "Recording attempt failed before reap");
                    self.failures.lock().push(error.to_string());
                }
            }
        }
        on_admitted();
        self.running.insert(download_id.clone(), ());
        let running = self.running.clone();
        let guard = RunningAttemptGuard {
            download_id: download_id.clone(),
            running,
        };
        tasks.spawn(async move {
            let _guard = guard;
            let outcome = task.await;
            AttemptTaskReport {
                download_id,
                outcome,
            }
        });
        true
    }

    pub(super) fn close_admission(&self) {
        let _tasks = self.tasks.lock();
        self.accepting.store(false, Ordering::Release);
    }

    pub(super) async fn join_until(&self, deadline: tokio::time::Instant) -> AttemptShutdownReport {
        self.close_admission();
        let mut tasks = DrainedTasks::take_from(&self.tasks);

        let mut failures = std::mem::take(&mut *self.failures.lock());
        let mut deadline_exceeded_download_ids = Vec::new();
        while !tasks.is_empty() {
            match timeout_at(deadline, tasks.join_next()).await {
                Ok(Some(Ok(report))) => {
                    debug!(download_id = %report.download_id, "Recording attempt joined");
                    if let Err(error) = report.outcome {
                        failures.push(format!(
                            "download {} failed during shutdown: {error}",
                            report.download_id
                        ));
                    }
                }
                Ok(Some(Err(error))) => {
                    warn!(%error, "Recording attempt task failed while joining");
                    failures.push(error.to_string());
                }
                Ok(None) => break,
                Err(_) => {
                    deadline_exceeded_download_ids = self
                        .running
                        .iter()
                        .map(|entry| entry.key().clone())
                        .collect::<Vec<_>>();
                    deadline_exceeded_download_ids.sort();
                    failures.push(format!(
                        "graceful attempt deadline exceeded; awaiting structured cleanup for downloads: {deadline_exceeded_download_ids:?}"
                    ));

                    // The manager requested cancellation before entering this
                    // join. Aborting the outer attempt here would detach engine
                    // children, including Mesio's non-abortable blocking writer.
                    // Continue joining so producer quiescence remains a real
                    // precondition for closing the database.
                    while let Some(result) = tasks.join_next().await {
                        match result {
                            Ok(report) => {
                                debug!(download_id = %report.download_id, "Recording attempt joined after graceful deadline");
                                if let Err(error) = report.outcome {
                                    failures.push(format!(
                                        "download {} failed during containment: {error}",
                                        report.download_id
                                    ));
                                }
                            }
                            Err(error) => {
                                warn!(%error, "Recording attempt task failed during containment");
                                failures.push(error.to_string());
                            }
                        }
                    }
                    break;
                }
            }
        }

        AttemptShutdownReport {
            failures,
            deadline_exceeded_download_ids,
        }
    }

    /// Aborts every running attempt and joins the aborted tasks.
    ///
    /// [`Self::join_until`] never aborts, so an attempt whose engine ignores
    /// `DownloadHandle::cancel` keeps it waiting. This is the escape hatch for
    /// a caller that must stop waiting: aborting drops the attempt future,
    /// which drops the engine future and kills the ffmpeg/streamlink child it
    /// spawned with `kill_on_drop`. Joining afterwards is what proves those
    /// drops ran. Attempts that do not settle by `deadline` stay owned here.
    ///
    /// Returns the download ids that were still running.
    pub(super) async fn abort_running(&self, deadline: tokio::time::Instant) -> Vec<String> {
        self.close_admission();

        let mut download_ids = self
            .running
            .iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        download_ids.sort();

        let mut tasks = DrainedTasks::take_from(&self.tasks);
        tasks.abort_all();
        while !tasks.is_empty() {
            match timeout_at(deadline, tasks.join_next()).await {
                Ok(Some(Ok(report))) => {
                    debug!(download_id = %report.download_id, "Recording attempt stopped after abort");
                }
                Ok(Some(Err(error))) if error.is_cancelled() => {}
                Ok(Some(Err(error))) => {
                    warn!(%error, "Recording attempt failed while being aborted");
                }
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        unfinished = tasks.len(),
                        "Aborted recording attempts did not settle before the reap deadline"
                    );
                    break;
                }
            }
        }

        download_ids
    }
}

struct RunningAttemptGuard {
    download_id: String,
    running: Arc<DashMap<String, ()>>,
}

impl Drop for RunningAttemptGuard {
    fn drop(&mut self) {
        self.running.remove(&self.download_id);
    }
}

struct AttemptFinalizer {
    download_id: String,
    active_downloads: Arc<DashMap<String, ActiveDownload>>,
    pending_updates: Arc<DashMap<String, PendingConfigUpdate>>,
    completion: Arc<AttemptCompletion>,
    outcome: Option<std::result::Result<(), String>>,
    active_released: bool,
}

impl AttemptFinalizer {
    fn set_outcome(&mut self, outcome: std::result::Result<(), String>) {
        self.outcome = Some(outcome);
    }

    fn release_active(&mut self) {
        if self.active_released {
            return;
        }
        self.active_downloads.remove(&self.download_id);
        self.pending_updates.remove(&self.download_id);
        self.active_released = true;
    }
}

impl Drop for AttemptFinalizer {
    fn drop(&mut self) {
        self.release_active();
        self.completion
            .finish(self.outcome.take().unwrap_or_else(|| {
                Err("recording attempt was aborted before finalization".to_string())
            }));
    }
}

fn choose_attempt_terminal(
    phase: &Mutex<AttemptPhase>,
    mut natural: DownloadTerminalEvent,
    download_id: &str,
    streamer_id: &str,
    streamer_name: &str,
    session_id: &str,
) -> DownloadTerminalEvent {
    let stop_cause = {
        let mut phase = phase.lock();
        let stop_cause = phase.stop_cause();
        *phase = AttemptPhase::TerminalChosen;
        stop_cause
    };

    match (stop_cause, &mut natural) {
        (Some(DownloadStopCause::Shutdown), _) => DownloadTerminalEvent::Cancelled {
            download_id: download_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            session_id: session_id.to_string(),
            cause: DownloadStopCause::Shutdown,
        },
        (Some(cause), DownloadTerminalEvent::Completed { stop_cause, .. }) => {
            *stop_cause = Some(cause);
            natural
        }
        (Some(cause), _) => DownloadTerminalEvent::Cancelled {
            download_id: download_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            session_id: session_id.to_string(),
            cause,
        },
        (None, _) => natural,
    }
}

impl DownloadManager {
    pub(super) async fn start_download_with_engine_and_slot(
        &self,
        config: DownloadConfig,
        engine: Arc<dyn DownloadEngine>,
        engine_type: EngineType,
        engine_key: EngineKey,
        slot: SlotGuard,
    ) -> Result<String> {
        let active_slot = slot.into_active();
        Self::seed_session_segment_index(
            &self.session_segment_indices,
            &config.session_id,
            config.initial_segment_index,
        );

        let download_id = uuid::Uuid::new_v4().to_string();
        let (segment_tx, mut segment_rx) = mpsc::channel::<SegmentEvent>(32);
        let handle = Arc::new(DownloadHandle::new(
            download_id.clone(),
            engine_type,
            config.clone(),
            segment_tx,
        ));
        let phase = Arc::new(Mutex::new(AttemptPhase::Running));
        let completion = Arc::new(AttemptCompletion::new());

        let active_download = ActiveDownload {
            handle: handle.clone(),
            phase: phase.clone(),
            completion: completion.clone(),
            status: DownloadStatus::Starting,
            progress: DownloadProgress::default(),
            output_path: None,
            current_segment_index: None,
            current_engine_segment_index: None,
            current_segment_path: None,
            current_segment_started_at: None,
            slot: Some(active_slot),
            retry_config_override: None,
        };

        let streamer_id = config.streamer_id.clone();
        let streamer_name = config.streamer_name.clone();
        let session_id = config.session_id.clone();
        let protocol = config.protocol;
        let cdn_host = crate::utils::url::extract_host(&config.url).unwrap_or_default();

        let active_downloads = self.active_downloads.clone();
        let pending_updates = self.pending_updates.clone();
        let session_segment_indices = self.session_segment_indices.clone();
        let circuit_breakers_ref = self.circuit_breakers.get(&engine_key);
        let output_root_gate_ref: Option<Arc<OutputRootGate>> =
            self.output_root_gate.get().cloned();
        let events = self.events.clone();

        let attempt_download_id = download_id.clone();
        let attempt_active_downloads = active_downloads.clone();
        let attempt_pending_updates = pending_updates.clone();
        let attempt_completion = completion.clone();
        let terminal_events = events.clone();
        let terminal_streamer_id = streamer_id.clone();
        let terminal_streamer_name = streamer_name.clone();
        let terminal_session_id = session_id.clone();
        let terminal_phase = phase.clone();

        let engine_handle = handle.clone();
        let engine_future = async move {
            let outcome = AssertUnwindSafe(engine.run(engine_handle.clone()))
                .catch_unwind()
                .await;
            let (kind, message) = match outcome {
                Ok(Ok(())) => (
                    DownloadFailureKind::Other,
                    "engine returned without a terminal event".to_string(),
                ),
                Ok(Err(error)) => {
                    error!(%error, "Download engine failed");
                    (error.kind, error.message)
                }
                Err(payload) => {
                    let message = if let Some(message) = payload.downcast_ref::<&str>() {
                        (*message).to_string()
                    } else if let Some(message) = payload.downcast_ref::<String>() {
                        message.clone()
                    } else {
                        "engine panicked with a non-string payload".to_string()
                    };
                    error!(%message, "Download engine panicked");
                    (DownloadFailureKind::Other, message)
                }
            };

            // Every production engine queues its terminal event before run()
            // returns. This fallback is therefore observed only when an engine
            // violates that contract or fails before it can publish a terminal.
            if let Err(send_error) = engine_handle
                .event_tx
                .send(SegmentEvent::DownloadFailed { kind, message })
                .await
            {
                debug!(%send_error, "Engine fallback terminal was not needed");
            }
        };

        let translator_download_id = download_id.clone();
        let translator_streamer_id = streamer_id.clone();
        let translator_streamer_name = streamer_name.clone();
        let translator_session_id = session_id.clone();
        let translator_events = events.clone();
        let translator_active_downloads = active_downloads.clone();
        let translator_pending_updates = pending_updates.clone();
        let translator_phase = phase.clone();
        let translator_handle = handle.clone();
        let translator_future = async move {
            const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(250);
            let mut last_progress_emit = Instant::now()
                .checked_sub(PROGRESS_MIN_INTERVAL)
                .unwrap_or_else(Instant::now);
            let mut engine_to_session: HashMap<u32, u32> = HashMap::new();
            let mut engine_segment_paths: HashMap<u32, String> = HashMap::new();

            let natural_terminal = loop {
                let Some(event) = segment_rx.recv().await else {
                    break DownloadTerminalEvent::Failed {
                        download_id: translator_download_id.clone(),
                        streamer_id: translator_streamer_id.clone(),
                        streamer_name: translator_streamer_name.clone(),
                        session_id: translator_session_id.clone(),
                        engine_type,
                        protocol,
                        kind: DownloadFailureKind::Other,
                        error: "download engine event channel closed without a terminal event"
                            .to_string(),
                        recoverable: true,
                    };
                };

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
                        let started_at = info_started_at.or_else(|| {
                            translator_active_downloads
                                .get(&translator_download_id)
                                .and_then(|download| {
                                    if download.current_engine_segment_index == Some(index) {
                                        download.current_segment_started_at
                                    } else {
                                        None
                                    }
                                })
                        });
                        let segment_index = *engine_to_session.entry(index).or_insert_with(|| {
                            Self::allocate_next_session_segment_index(
                                &session_segment_indices,
                                &translator_session_id,
                            )
                        });

                        let completed_event = DownloadManagerEvent::Progress(
                            DownloadProgressEvent::SegmentCompleted {
                                download_id: translator_download_id.clone(),
                                streamer_id: translator_streamer_id.clone(),
                                streamer_name: translator_streamer_name.clone(),
                                session_id: translator_session_id.clone(),
                                segment_path: segment_path.clone(),
                                segment_index,
                                started_at,
                                completed_at,
                                duration_secs,
                                size_bytes,
                                split_reason_code,
                                split_reason_details_json,
                            },
                        );
                        if let Err(delivery_error) =
                            translator_events.publish_and_wait(completed_event).await
                        {
                            error!(
                                download_id = %translator_download_id,
                                %delivery_error,
                                "Required segment completion could not be applied"
                            );
                            translator_handle.cancel();
                            break DownloadTerminalEvent::Failed {
                                download_id: translator_download_id.clone(),
                                streamer_id: translator_streamer_id.clone(),
                                streamer_name: translator_streamer_name.clone(),
                                session_id: translator_session_id.clone(),
                                engine_type,
                                protocol,
                                kind: DownloadFailureKind::Other,
                                error: format!(
                                    "required segment completion could not be applied: {delivery_error}"
                                ),
                                recoverable: true,
                            };
                        }

                        if let Some(mut download) =
                            translator_active_downloads.get_mut(&translator_download_id)
                        {
                            download.output_path = Some(segment_path);
                            if download.current_engine_segment_index == Some(index) {
                                download.current_engine_segment_index = None;
                                download.current_segment_index = None;
                                download.current_segment_path = None;
                                download.current_segment_started_at = None;
                            }
                        }
                        debug!(
                            download_id = %translator_download_id,
                            path = %path.display(),
                            "Segment completed"
                        );
                    }
                    SegmentEvent::Progress(progress) => {
                        if last_progress_emit.elapsed() < PROGRESS_MIN_INTERVAL {
                            if let Some(mut download) =
                                translator_active_downloads.get_mut(&translator_download_id)
                            {
                                download.progress = progress;
                                if !translator_phase.lock().is_stop_requested() {
                                    download.status = DownloadStatus::Downloading;
                                }
                            }
                        } else {
                            last_progress_emit = Instant::now();
                            if let Some(mut download) =
                                translator_active_downloads.get_mut(&translator_download_id)
                            {
                                download.progress = progress.clone();
                                if !translator_phase.lock().is_stop_requested() {
                                    download.status = DownloadStatus::Downloading;
                                }
                            }

                            translator_events.publish(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: translator_download_id.clone(),
                                    streamer_id: translator_streamer_id.clone(),
                                    streamer_name: translator_streamer_name.clone(),
                                    session_id: translator_session_id.clone(),
                                    status: if translator_phase.lock().is_stop_requested() {
                                        DownloadStatus::Cancelled
                                    } else {
                                        DownloadStatus::Downloading
                                    },
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
                        if let Some(download) =
                            translator_active_downloads.get(&translator_download_id)
                        {
                            translator_events.publish(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: translator_download_id.clone(),
                                    streamer_id: translator_streamer_id.clone(),
                                    streamer_name: translator_streamer_name.clone(),
                                    session_id: translator_session_id.clone(),
                                    status: if translator_phase.lock().is_stop_requested() {
                                        DownloadStatus::Cancelled
                                    } else {
                                        DownloadStatus::Downloading
                                    },
                                    progress: download.progress.clone(),
                                },
                            ));
                        }

                        let output_path = translator_active_downloads
                            .get(&translator_download_id)
                            .and_then(|download| download.output_path.clone());
                        break DownloadTerminalEvent::Completed {
                            download_id: translator_download_id.clone(),
                            streamer_id: translator_streamer_id.clone(),
                            streamer_name: translator_streamer_name.clone(),
                            session_id: translator_session_id.clone(),
                            total_bytes,
                            total_duration_secs,
                            total_segments,
                            file_path: output_path,
                            engine_signal,
                            stop_cause: None,
                        };
                    }
                    SegmentEvent::DiskFull { output_dir, detail } => {
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
                        if let Some(download) =
                            translator_active_downloads.get(&translator_download_id)
                        {
                            translator_events.publish(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: translator_download_id.clone(),
                                    streamer_id: translator_streamer_id.clone(),
                                    streamer_name: translator_streamer_name.clone(),
                                    session_id: translator_session_id.clone(),
                                    status: if translator_phase.lock().is_stop_requested() {
                                        DownloadStatus::Cancelled
                                    } else {
                                        DownloadStatus::Downloading
                                    },
                                    progress: download.progress.clone(),
                                },
                            ));
                        }

                        let recoverable = kind.is_recoverable();
                        break DownloadTerminalEvent::Failed {
                            download_id: translator_download_id.clone(),
                            streamer_id: translator_streamer_id.clone(),
                            streamer_name: translator_streamer_name.clone(),
                            session_id: translator_session_id.clone(),
                            engine_type,
                            protocol,
                            kind,
                            error: message,
                            recoverable,
                        };
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
                                    &translator_session_id,
                                )
                            });

                        if let Some(mut download) =
                            translator_active_downloads.get_mut(&translator_download_id)
                        {
                            download.current_engine_segment_index = Some(sequence);
                            download.current_segment_index = Some(segment_index);
                            download.current_segment_path = Some(segment_path.clone());
                            download.current_segment_started_at = Some(started_at);
                        }

                        let started_event =
                            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
                                download_id: translator_download_id.clone(),
                                streamer_id: translator_streamer_id.clone(),
                                streamer_name: translator_streamer_name.clone(),
                                session_id: translator_session_id.clone(),
                                segment_path,
                                segment_index,
                                started_at,
                            });
                        if let Err(delivery_error) =
                            translator_events.publish_and_wait(started_event).await
                        {
                            error!(
                                download_id = %translator_download_id,
                                %delivery_error,
                                "Required segment start could not be applied"
                            );
                            translator_handle.cancel();
                            break DownloadTerminalEvent::Failed {
                                download_id: translator_download_id.clone(),
                                streamer_id: translator_streamer_id.clone(),
                                streamer_name: translator_streamer_name.clone(),
                                session_id: translator_session_id.clone(),
                                engine_type,
                                protocol,
                                kind: DownloadFailureKind::Other,
                                error: format!(
                                    "required segment start could not be applied: {delivery_error}"
                                ),
                                recoverable: true,
                            };
                        }

                        if let Some((_, pending_update)) =
                            translator_pending_updates.remove(&translator_download_id)
                            && let Some(mut download) =
                                translator_active_downloads.get_mut(&translator_download_id)
                        {
                            DownloadManager::apply_pending_update_to_download(
                                &mut download,
                                pending_update,
                                &translator_download_id,
                                &translator_streamer_id,
                                &translator_events,
                            );
                        }

                        debug!(
                            download_id = %translator_download_id,
                            path = %path.display(),
                            engine_segment_index = sequence,
                            segment_index,
                            "Segment started"
                        );
                    }
                }
            };

            let terminal = choose_attempt_terminal(
                &translator_phase,
                natural_terminal,
                &translator_download_id,
                &translator_streamer_id,
                &translator_streamer_name,
                &translator_session_id,
            );
            match &terminal {
                DownloadTerminalEvent::Completed { .. } => circuit_breakers_ref.record_success(),
                DownloadTerminalEvent::Failed { kind, .. } if kind.affects_circuit_breaker() => {
                    circuit_breakers_ref.record_failure();
                }
                DownloadTerminalEvent::Failed { .. }
                | DownloadTerminalEvent::Cancelled { .. }
                | DownloadTerminalEvent::Rejected { .. } => {}
            }
            terminal
        };

        let finalizer = AttemptFinalizer {
            download_id: attempt_download_id.clone(),
            active_downloads: attempt_active_downloads,
            pending_updates: attempt_pending_updates,
            completion: attempt_completion,
            outcome: None,
            active_released: false,
        };
        let attempt_cancel_handle = handle.clone();
        let attempt_task = async move {
            let mut finalizer = finalizer;
            let mut engine_task = Box::pin(AssertUnwindSafe(engine_future).catch_unwind());
            let mut translator_task = Box::pin(AssertUnwindSafe(translator_future).catch_unwind());

            // The attempt owns both siblings directly. A translator panic
            // cancels the engine before awaiting its cleanup; an engine wrapper
            // panic publishes a fallback failure so the translator can finish.
            let (engine_result, translator_result) = tokio::select! {
                translator_result = &mut translator_task => {
                    if translator_result.is_err() {
                        attempt_cancel_handle.cancel();
                    }
                    let engine_result = engine_task.await;
                    (engine_result, translator_result)
                }
                engine_result = &mut engine_task => {
                    if let Err(payload) = &engine_result {
                        attempt_cancel_handle.cancel();
                        let message = if let Some(message) = payload.downcast_ref::<&str>() {
                            (*message).to_string()
                        } else if let Some(message) = payload.downcast_ref::<String>() {
                            message.clone()
                        } else {
                            "engine wrapper panicked with a non-string payload".to_string()
                        };
                        if let Err(send_error) = attempt_cancel_handle
                            .event_tx
                            .send(SegmentEvent::DownloadFailed {
                                kind: DownloadFailureKind::Other,
                                message,
                            })
                            .await
                        {
                            debug!(%send_error, "Engine wrapper panic fallback was not delivered");
                        }
                    }
                    let translator_result = translator_task.await;
                    (engine_result, translator_result)
                }
            };
            let mut lifecycle_errors = Vec::new();
            if let Err(payload) = engine_result {
                let error = if let Some(message) = payload.downcast_ref::<&str>() {
                    (*message).to_string()
                } else if let Some(message) = payload.downcast_ref::<String>() {
                    message.clone()
                } else {
                    "engine wrapper panicked with a non-string payload".to_string()
                };
                error!(
                    download_id = %attempt_download_id,
                    %error,
                    "Engine task failed"
                );
                lifecycle_errors.push(format!("engine task failed: {error}"));
            }

            let terminal = match translator_result {
                Ok(terminal) => terminal,
                Err(payload) => {
                    let error = if let Some(message) = payload.downcast_ref::<&str>() {
                        (*message).to_string()
                    } else if let Some(message) = payload.downcast_ref::<String>() {
                        message.clone()
                    } else {
                        "download event translator panicked with a non-string payload".to_string()
                    };
                    error!(
                        download_id = %attempt_download_id,
                        %error,
                        "Download event translator failed"
                    );
                    let message = format!("download event translator failed: {error}");
                    lifecycle_errors.push(message.clone());
                    choose_attempt_terminal(
                        &terminal_phase,
                        DownloadTerminalEvent::Failed {
                            download_id: attempt_download_id.clone(),
                            streamer_id: terminal_streamer_id.clone(),
                            streamer_name: terminal_streamer_name.clone(),
                            session_id: terminal_session_id.clone(),
                            engine_type,
                            protocol,
                            kind: DownloadFailureKind::Other,
                            error: message,
                            recoverable: true,
                        },
                        &attempt_download_id,
                        &terminal_streamer_id,
                        &terminal_streamer_name,
                        &terminal_session_id,
                    )
                }
            };

            let terminal_event = DownloadManagerEvent::Terminal(terminal);
            if let Err(delivery_error) = terminal_events.coordinate_and_wait(&terminal_event).await
            {
                error!(
                    download_id = %attempt_download_id,
                    %delivery_error,
                    "Required terminal outcome could not be applied"
                );
                lifecycle_errors.push(format!(
                    "required terminal outcome could not be applied: {delivery_error}"
                ));
            }

            finalizer.release_active();
            terminal_events.observe(terminal_event);
            let outcome = if lifecycle_errors.is_empty() {
                Ok(())
            } else {
                Err(lifecycle_errors.join("; "))
            };
            finalizer.set_outcome(outcome.clone());
            debug!(download_id = %attempt_download_id, "Recording attempt finished");
            outcome
        };

        let admitted_active_downloads = self.active_downloads.clone();
        let admitted_download_id = download_id.clone();
        let started_events = self.events.clone();
        let started_download_id = download_id.clone();
        let started_streamer_id = streamer_id.clone();
        let started_streamer_name = streamer_name.clone();
        let started_session_id = session_id.clone();
        let started_url = config.url.clone();
        let admitted = self.attempts.spawn(
            download_id.clone(),
            move || {
                admitted_active_downloads.insert(admitted_download_id, active_download);
                started_events.publish(DownloadManagerEvent::Progress(
                    DownloadProgressEvent::DownloadStarted {
                        download_id: started_download_id,
                        streamer_id: started_streamer_id,
                        streamer_name: started_streamer_name,
                        session_id: started_session_id,
                        engine_type,
                        cdn_host,
                        download_url: started_url,
                    },
                ));
            },
            attempt_task,
        );

        if !admitted {
            return Err(crate::Error::Other(
                "download manager shutting down".to_string(),
            ));
        }

        info!(
            download_id,
            streamer_id,
            engine = %engine_type,
            "Starting download"
        );
        Ok(download_id)
    }
}
