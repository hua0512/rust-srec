//! Download attempt lifecycle implementation.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::FutureExt;
use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

use crate::Result;
use crate::downloader::SegmentInfo;
use crate::downloader::engine::utils::sanitize_engine_message;
use crate::downloader::engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadProgress,
    DownloadProtocol, DownloadStatus, EngineType, IoErrorKindSer, SegmentEvent,
};
use crate::downloader::output_root_gate::OutputRootGate;
use crate::downloader::queue::SlotGuard;
use crate::downloader::resilience::{CircuitBreaker, EngineKey};
use crate::scheduler::feedback::FeedbackPermit;
use crate::utils::failure_samples::FailureSamples;
use crate::utils::task_supervisor::{DrainedTasks, panic_payload_message};

use super::coordination::DownloadEventPublisher;
use super::{
    ActiveDownload, AttemptPhase, CredentialDiagnostic, DownloadManager, DownloadManagerEvent,
    DownloadProgressEvent, DownloadStopCause, DownloadTerminalEvent, EngineEndSignal,
    resolve_segment_path,
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
    failures: Mutex<FailureSamples>,
}

struct AttemptTaskReport {
    download_id: String,
    outcome: std::result::Result<(), String>,
}

pub(super) struct AttemptShutdownReport {
    /// Lifecycle errors from attempts that had already ended when
    /// `join_until` closed admission, drained from `AttemptSupervisor::failures`.
    ///
    /// An attempt can end this way at any point in a long run while the process
    /// keeps recording, so these say nothing about whether shutdown itself
    /// finalized cleanly. Callers report them and continue. Details are bounded;
    /// an additional entry reports the total and omitted sample count.
    pub(super) runtime_failures: Vec<String>,
    /// Failures of the shutdown phase itself: an attempt task that could not be
    /// joined, or one whose outcome was `Err` while shutdown was draining it.
    /// Each means `join_until` could not prove the attempt finalized. Fatal.
    pub(super) failures: Vec<String>,
    /// Soft-budget overruns that were still contained to completion. Never
    /// fatal — the attempts below were all joined, just later than budgeted.
    pub(super) overruns: Vec<String>,
    pub(super) deadline_exceeded_download_ids: Vec<String>,
}

impl AttemptSupervisor {
    pub(super) fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            tasks: Mutex::new(JoinSet::new()),
            running: Arc::new(DashMap::new()),
            failures: Mutex::new(FailureSamples::default()),
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

        // Attempts reaped lazily by `spawn` ended earlier in the run and
        // describe those recordings, not this shutdown, so they stay out of
        // `failures`.
        let runtime_failures = std::mem::take(&mut *self.failures.lock()).into_report();
        let mut failures = Vec::new();
        let mut overruns = Vec::new();
        let mut deadline_exceeded_download_ids = Vec::new();
        // The manager requested cancellation before entering this join.
        // Aborting the outer attempt past the deadline would detach engine
        // children, including Mesio's non-abortable blocking writer, so the
        // join continues and producer quiescence remains a real precondition
        // for closing the database.
        tasks
            .join_with_grace(
                deadline,
                |_| {
                    deadline_exceeded_download_ids = self
                        .running
                        .iter()
                        .map(|entry| entry.key().clone())
                        .collect::<Vec<_>>();
                    deadline_exceeded_download_ids.sort();
                    overruns.push(format!(
                        "graceful attempt deadline exceeded; awaiting structured cleanup for downloads: {deadline_exceeded_download_ids:?}"
                    ));
                },
                |result, contained| match result {
                    Ok(report) => {
                        if contained {
                            debug!(download_id = %report.download_id, "Recording attempt joined after graceful deadline");
                        } else {
                            debug!(download_id = %report.download_id, "Recording attempt joined");
                        }
                        if let Err(error) = report.outcome {
                            let phase = if contained { "containment" } else { "shutdown" };
                            failures.push(format!(
                                "download {} failed during {phase}: {error}",
                                report.download_id
                            ));
                        }
                    }
                    Err(error) => {
                        if contained {
                            warn!(%error, "Recording attempt task failed during containment");
                        } else {
                            warn!(%error, "Recording attempt task failed while joining");
                        }
                        failures.push(error.to_string());
                    }
                },
            )
            .await;

        AttemptShutdownReport {
            runtime_failures,
            failures,
            overruns,
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

        let unfinished = DrainedTasks::take_from(&self.tasks)
            .abort_and_reap(deadline)
            .await;
        if unfinished > 0 {
            warn!(
                unfinished,
                "Aborted recording attempts did not settle before the reap deadline"
            );
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

/// Panic payloads may carry engine output, so they get the same sanitizing
/// as engine error messages.
fn panic_message(
    payload: &(dyn std::any::Any + Send),
    fallback: &str,
    managed_credentials: bool,
) -> String {
    sanitize_engine_message(
        panic_payload_message(payload).unwrap_or(fallback),
        managed_credentials,
    )
}

/// Identity of one recording attempt, carried by every event it publishes.
struct AttemptIdentity {
    download_id: String,
    streamer_id: String,
    streamer_name: String,
    session_id: String,
    engine_type: EngineType,
    protocol: DownloadProtocol,
}

impl AttemptIdentity {
    fn failed(
        &self,
        kind: DownloadFailureKind,
        error: String,
        recoverable: bool,
    ) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Failed {
            download_id: self.download_id.clone(),
            streamer_id: self.streamer_id.clone(),
            streamer_name: self.streamer_name.clone(),
            session_id: self.session_id.clone(),
            engine_type: self.engine_type,
            protocol: self.protocol,
            kind,
            error,
            recoverable,
        }
    }

    fn cancelled(&self, cause: DownloadStopCause) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Cancelled {
            download_id: self.download_id.clone(),
            streamer_id: self.streamer_id.clone(),
            streamer_name: self.streamer_name.clone(),
            session_id: self.session_id.clone(),
            cause,
        }
    }
}

fn choose_attempt_terminal(
    phase: &Mutex<AttemptPhase>,
    mut natural: DownloadTerminalEvent,
    id: &AttemptIdentity,
) -> DownloadTerminalEvent {
    let stop_cause = {
        let mut phase = phase.lock();
        let stop_cause = phase.stop_cause();
        *phase = AttemptPhase::TerminalChosen;
        stop_cause
    };

    match (stop_cause, &mut natural) {
        (Some(DownloadStopCause::Shutdown), _) => id.cancelled(DownloadStopCause::Shutdown),
        (Some(cause), DownloadTerminalEvent::Completed { stop_cause, .. }) => {
            *stop_cause = Some(cause);
            natural
        }
        (Some(cause), _) => id.cancelled(cause),
        (None, _) => natural,
    }
}

/// Runs the engine and, if it returns without queuing a terminal event,
/// queues a fallback failure so the translator can finish.
async fn run_engine(
    engine: Arc<dyn DownloadEngine>,
    handle: Arc<DownloadHandle>,
    managed_credentials: bool,
) {
    let outcome = AssertUnwindSafe(engine.run(handle.clone()))
        .catch_unwind()
        .await;
    let (kind, message) = match outcome {
        Ok(Ok(())) => (
            DownloadFailureKind::Other,
            "engine returned without a terminal event".to_string(),
        ),
        Ok(Err(error)) => {
            let message = sanitize_engine_message(&error.message, managed_credentials);
            error!(kind = ?error.kind, %message, "Download engine failed");
            (error.kind, message)
        }
        Err(payload) => {
            let message = panic_message(
                &*payload,
                "engine panicked with a non-string payload",
                managed_credentials,
            );
            error!(%message, "Download engine panicked");
            (DownloadFailureKind::Other, message)
        }
    };

    // Every production engine queues its terminal event before run()
    // returns. This fallback is therefore observed only when an engine
    // violates that contract or fails before it can publish a terminal.
    if let Err(send_error) = handle
        .event_tx
        .send(SegmentEvent::DownloadFailed { kind, message })
        .await
    {
        debug!(%send_error, "Engine fallback terminal was not needed");
    }
}

const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// Turns an engine's [`SegmentEvent`]s into manager events until the engine
/// reports a terminal, keeping the active-download snapshot current.
struct SegmentTranslator {
    id: Arc<AttemptIdentity>,
    events: DownloadEventPublisher,
    active_downloads: Arc<DashMap<String, ActiveDownload>>,
    phase: Arc<Mutex<AttemptPhase>>,
    handle: Arc<DownloadHandle>,
    session_segment_indices: Arc<DashMap<String, u32>>,
    output_root_gate: Option<Arc<OutputRootGate>>,
    circuit_breaker: Arc<CircuitBreaker>,
    /// Non-fatal required-event failures observed mid-run. The attempt task
    /// folds these into its outcome after both siblings settle, so a lost
    /// segment row still surfaces without ending the recording that produced it.
    lifecycle_errors: Arc<Mutex<FailureSamples>>,
    managed_credentials: bool,
    may_diagnose: bool,
    /// Completions publish immediately so the cut result precedes the next
    /// segment event; the watch branch then sees the same revision. Revisions
    /// only grow, so anything not newer was already sent.
    published_split_revision: u64,
    pending_cut_completion: Option<(u32, u64)>,
    last_started_sequence: Option<u32>,
    last_progress_emit: Instant,
    engine_to_session: HashMap<u32, u32>,
    engine_segment_paths: HashMap<u32, String>,
    output_failure_recorded: bool,
}

impl SegmentTranslator {
    async fn run(
        mut self,
        mut segment_rx: mpsc::Receiver<SegmentEvent>,
        mut split_updates: pipeline_common::ManualSplitUpdates,
    ) -> DownloadTerminalEvent {
        let mut split_expiration = tokio::time::interval(Duration::from_secs(1));
        split_expiration.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let natural_terminal = loop {
            let next = tokio::select! {
                event = segment_rx.recv() => event,
                _ = split_expiration.tick() => {
                    self.handle.manual_split.expire();
                    continue;
                }
                update = split_updates.changed() => {
                    if let Ok(state) = update { self.publish_split(state); }
                    continue;
                }
            };
            let Some(event) = next else {
                break self.id.failed(
                    DownloadFailureKind::Other,
                    "download engine event channel closed without a terminal event".to_string(),
                    true,
                );
            };

            let terminal = match event {
                SegmentEvent::SegmentCompleted(info) => {
                    self.on_segment_completed(info).await;
                    None
                }
                SegmentEvent::Progress(progress) => {
                    self.on_progress(progress);
                    None
                }
                SegmentEvent::DownloadCompleted {
                    total_bytes,
                    total_duration_secs,
                    total_segments,
                    engine_signal,
                } => Some(self.on_download_completed(
                    total_bytes,
                    total_duration_secs,
                    total_segments,
                    engine_signal,
                )),
                SegmentEvent::OutputIoError {
                    output_dir,
                    io_kind,
                    detail,
                } => {
                    self.on_output_io_error(&output_dir, io_kind, &detail);
                    None
                }
                SegmentEvent::DownloadFailed { kind, message } => {
                    Some(self.on_download_failed(kind, &message))
                }
                SegmentEvent::SegmentStarted {
                    path,
                    sequence,
                    started_at,
                } => self.on_segment_started(&path, sequence, started_at).await,
            };
            if let Some(terminal) = terminal {
                break terminal;
            }
        };

        self.handle.manual_split.close(matches!(
            natural_terminal,
            DownloadTerminalEvent::Failed { .. }
        ));
        self.publish_split(self.handle.manual_split.snapshot());

        // Circuit-breaker health is read from the engine's own outcome, not
        // from the terminal `choose_attempt_terminal` publishes. A stop
        // racing a genuine engine failure is still a failure of this engine
        // against this CDN, and rewriting it to `Cancelled` for the session
        // must not also hide it from the retry backoff.
        match &natural_terminal {
            DownloadTerminalEvent::Completed { .. } => self.circuit_breaker.record_success(),
            DownloadTerminalEvent::Failed { kind, .. } if kind.affects_circuit_breaker() => {
                self.circuit_breaker.record_failure();
            }
            DownloadTerminalEvent::Failed { .. }
            | DownloadTerminalEvent::Cancelled { .. }
            | DownloadTerminalEvent::Rejected { .. } => {}
        }
        if let DownloadTerminalEvent::Failed { kind, error, .. } = &natural_terminal {
            // `choose_attempt_terminal` may replace this with `Cancelled`,
            // which carries neither field, so log them before they are lost.
            warn!(
                download_id = %self.id.download_id,
                ?kind,
                %error,
                "Engine reported a failure while an attempt outcome was being chosen"
            );
        }

        if self.may_diagnose
            && matches!(&natural_terminal, DownloadTerminalEvent::Failed { kind, engine_type, .. } if kind.requests_credential_diagnostic(*engine_type))
        {
            // Keep stop requests effective during the bounded diagnostic.
            // The finalizer chooses the terminal after diagnostic work settles.
            natural_terminal
        } else {
            choose_attempt_terminal(&self.phase, natural_terminal, &self.id)
        }
    }

    fn publish_split(&mut self, state: pipeline_common::ManualSplitSnapshot) {
        if self.published_split_revision >= state.revision {
            return;
        }
        self.published_split_revision = state.revision;
        self.events.publish(DownloadManagerEvent::Progress(
            DownloadProgressEvent::ManualSplitChanged {
                download_id: self.id.download_id.clone(),
                streamer_id: self.id.streamer_id.clone(),
                streamer_name: self.id.streamer_name.clone(),
                session_id: self.id.session_id.clone(),
                state,
            },
        ));
    }

    fn session_segment_index(&mut self, engine_index: u32) -> u32 {
        *self
            .engine_to_session
            .entry(engine_index)
            .or_insert_with(|| {
                DownloadManager::allocate_next_session_segment_index(
                    &self.session_segment_indices,
                    &self.id.session_id,
                )
            })
    }

    fn progress_event(&self, progress: DownloadProgress) -> DownloadManagerEvent {
        DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
            download_id: self.id.download_id.clone(),
            streamer_id: self.id.streamer_id.clone(),
            streamer_name: self.id.streamer_name.clone(),
            session_id: self.id.session_id.clone(),
            status: if self.phase.lock().is_stop_requested() {
                DownloadStatus::Cancelled
            } else {
                DownloadStatus::Downloading
            },
            progress,
        })
    }

    /// Publishes the latest stored progress, so the last throttled sample is
    /// not lost when the attempt ends.
    fn publish_last_progress(&self) {
        if let Some(download) = self.active_downloads.get(&self.id.download_id) {
            self.events
                .publish(self.progress_event(download.progress.clone()));
        }
    }

    async fn on_segment_completed(&mut self, info: SegmentInfo) {
        let manual_request = info.manual_request_id();
        let split_reason_code = info.split_reason_code().map(str::to_owned);
        let split_reason_details_json = info.split_reason_details_json();
        let SegmentInfo {
            path,
            duration_secs,
            size_bytes,
            index,
            started_at: info_started_at,
            completed_at,
            ..
        } = info;
        let segment_path = self
            .engine_segment_paths
            .remove(&index)
            .unwrap_or_else(|| resolve_segment_path(&path));
        let started_at = info_started_at.or_else(|| {
            self.active_downloads
                .get(&self.id.download_id)
                .and_then(|download| {
                    if download.current_engine_segment_index == Some(index) {
                        download.current_segment_started_at
                    } else {
                        None
                    }
                })
        });
        let segment_index = self.session_segment_index(index);

        let completed_event =
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                download_id: self.id.download_id.clone(),
                streamer_id: self.id.streamer_id.clone(),
                streamer_name: self.id.streamer_name.clone(),
                session_id: self.id.session_id.clone(),
                segment_path: segment_path.clone(),
                segment_index,
                started_at,
                completed_at,
                duration_secs,
                size_bytes,
                split_reason_code,
                split_reason_details_json,
            });
        if let Err(delivery_error) = self.events.publish_and_wait(completed_event).await {
            // The segment file is already closed on disk; only its row is
            // missing. Ending the recording here would turn one lost row into
            // a lost stream, so the failure is carried to the attempt outcome
            // (`AttemptShutdownReport::failures`) and the engine keeps writing
            // the next segment.
            error!(
                download_id = %self.id.download_id,
                %delivery_error,
                "Required segment completion could not be applied; recording continues"
            );
            self.lifecycle_errors.lock().push(format!(
                "required segment completion could not be applied: {delivery_error}"
            ));
            if let Some(request_id) = manual_request {
                self.handle.manual_split.fail(request_id);
            }
        } else if let Some(request_id) = manual_request {
            // External remuxers finalize the old file after the next logical
            // segment starts; native writers close first.
            if self
                .last_started_sequence
                .is_some_and(|started| started > index)
            {
                self.handle.manual_split.complete(request_id);
                self.publish_split(self.handle.manual_split.snapshot());
            } else {
                self.pending_cut_completion = Some((index, request_id));
            }
        }

        if let Some(mut download) = self.active_downloads.get_mut(&self.id.download_id) {
            download.output_path = Some(segment_path);
            if download.current_engine_segment_index == Some(index) {
                download.current_engine_segment_index = None;
                download.current_segment_index = None;
                download.current_segment_path = None;
                download.current_segment_started_at = None;
            }
        }
        debug!(
            download_id = %self.id.download_id,
            path = %path.display(),
            "Segment completed"
        );
    }

    fn on_progress(&mut self, progress: DownloadProgress) {
        if self.last_progress_emit.elapsed() < PROGRESS_MIN_INTERVAL {
            if let Some(mut download) = self.active_downloads.get_mut(&self.id.download_id) {
                download.progress = progress;
                if !self.phase.lock().is_stop_requested() {
                    download.status = DownloadStatus::Downloading;
                }
            }
        } else {
            self.last_progress_emit = Instant::now();
            if let Some(mut download) = self.active_downloads.get_mut(&self.id.download_id) {
                download.progress = progress.clone();
                if !self.phase.lock().is_stop_requested() {
                    download.status = DownloadStatus::Downloading;
                }
            }

            self.events.publish(self.progress_event(progress));
        }
    }

    fn on_download_completed(
        &self,
        total_bytes: u64,
        total_duration_secs: f64,
        total_segments: u32,
        engine_signal: EngineEndSignal,
    ) -> DownloadTerminalEvent {
        self.publish_last_progress();

        let output_path = self
            .active_downloads
            .get(&self.id.download_id)
            .and_then(|download| download.output_path.clone());
        DownloadTerminalEvent::Completed {
            download_id: self.id.download_id.clone(),
            streamer_id: self.id.streamer_id.clone(),
            streamer_name: self.id.streamer_name.clone(),
            session_id: self.id.session_id.clone(),
            total_bytes,
            total_duration_secs,
            total_segments,
            file_path: output_path,
            engine_signal,
            stop_cause: None,
        }
    }

    fn on_output_io_error(&mut self, output_dir: &Path, io_kind: IoErrorKindSer, detail: &str) {
        let detail = sanitize_engine_message(detail, self.managed_credentials);
        if let Some(gate) = self.output_root_gate.as_ref() {
            let synthetic_io_err = std::io::Error::new(io_kind.to_io_kind(), detail);
            gate.record_failure(output_dir, &synthetic_io_err);
            self.output_failure_recorded = true;
        } else {
            debug!(
                "Output I/O event received without an output-root gate; retaining circuit-breaker backpressure"
            );
        }
    }

    fn on_download_failed(
        &self,
        kind: DownloadFailureKind,
        message: &str,
    ) -> DownloadTerminalEvent {
        let message = sanitize_engine_message(message, self.managed_credentials);
        // An engine must not bypass both storage gating and the breaker,
        // including early failures delivered by the engine fallback.
        let kind = if matches!(kind, DownloadFailureKind::OutputRootUnavailable { .. })
            && !self.output_failure_recorded
        {
            DownloadFailureKind::Io
        } else {
            kind
        };
        self.publish_last_progress();

        let recoverable = kind.is_recoverable();
        self.id.failed(kind, message, recoverable)
    }

    async fn on_segment_started(
        &mut self,
        path: &Path,
        sequence: u32,
        started_at: DateTime<Utc>,
    ) -> Option<DownloadTerminalEvent> {
        let segment_path = resolve_segment_path(path);
        self.engine_segment_paths
            .insert(sequence, segment_path.clone());
        let segment_index = self.session_segment_index(sequence);

        if let Some(mut download) = self.active_downloads.get_mut(&self.id.download_id) {
            download.current_engine_segment_index = Some(sequence);
            download.current_segment_index = Some(segment_index);
            download.current_segment_path = Some(segment_path.clone());
            download.current_segment_started_at = Some(started_at);
        }

        let started_event = DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
            download_id: self.id.download_id.clone(),
            streamer_id: self.id.streamer_id.clone(),
            streamer_name: self.id.streamer_name.clone(),
            session_id: self.id.session_id.clone(),
            segment_path,
            segment_index,
            started_at,
        });
        if let Err(delivery_error) = self.events.publish_and_wait(started_event).await {
            error!(
                download_id = %self.id.download_id,
                %delivery_error,
                "Required segment start could not be applied"
            );
            self.handle.cancel();
            return Some(self.id.failed(
                DownloadFailureKind::Other,
                format!("required segment start could not be applied: {delivery_error}"),
                true,
            ));
        }

        if let Some((previous_sequence, request_id)) = self.pending_cut_completion
            && previous_sequence.checked_add(1) == Some(sequence)
        {
            self.handle.manual_split.complete(request_id);
            self.pending_cut_completion = None;
            self.publish_split(self.handle.manual_split.snapshot());
        }
        self.last_started_sequence = Some(sequence);

        debug!(
            download_id = %self.id.download_id,
            path = %path.display(),
            engine_segment_index = sequence,
            segment_index,
            "Segment started"
        );
        None
    }
}

/// The supervised body of one recording attempt: owns the engine and
/// translator siblings, then publishes the chosen terminal.
struct AttemptTask {
    id: Arc<AttemptIdentity>,
    handle: Arc<DownloadHandle>,
    phase: Arc<Mutex<AttemptPhase>>,
    events: DownloadEventPublisher,
    finalizer: AttemptFinalizer,
    lifecycle_errors: Arc<Mutex<FailureSamples>>,
    credential_diagnostic: Option<CredentialDiagnostic>,
    terminal_feedback: Option<FeedbackPermit>,
    managed_credentials: bool,
}

impl AttemptTask {
    async fn run(
        self,
        engine: impl Future<Output = ()>,
        translator: impl Future<Output = DownloadTerminalEvent>,
    ) -> std::result::Result<(), String> {
        let Self {
            id,
            handle,
            phase,
            events,
            mut finalizer,
            lifecycle_errors,
            credential_diagnostic,
            terminal_feedback,
            managed_credentials,
        } = self;
        let mut engine_task = Box::pin(AssertUnwindSafe(engine).catch_unwind());
        let mut translator_task = Box::pin(AssertUnwindSafe(translator).catch_unwind());

        // The attempt owns both siblings directly. A translator panic
        // cancels the engine before awaiting its cleanup; an engine wrapper
        // panic publishes a fallback failure so the translator can finish.
        let (engine_result, translator_result) = tokio::select! {
            translator_result = &mut translator_task => {
                if translator_result.is_err() {
                    handle.cancel();
                }
                let engine_result = engine_task.await;
                (engine_result, translator_result)
            }
            engine_result = &mut engine_task => {
                if let Err(payload) = &engine_result {
                    handle.cancel();
                    let message = panic_message(
                        &**payload,
                        "engine wrapper panicked with a non-string payload",
                        managed_credentials,
                    );
                    if let Err(send_error) = handle
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
        let mut lifecycle_errors = std::mem::take(&mut *lifecycle_errors.lock()).into_report();
        if let Err(payload) = engine_result {
            let error = panic_message(
                &*payload,
                "engine wrapper panicked with a non-string payload",
                managed_credentials,
            );
            error!(
                download_id = %id.download_id,
                %error,
                "Engine task failed"
            );
            lifecycle_errors.push(format!("engine task failed: {error}"));
        }

        let mut terminal = match translator_result {
            Ok(terminal) => terminal,
            Err(payload) => {
                let error = panic_message(
                    &*payload,
                    "download event translator panicked with a non-string payload",
                    managed_credentials,
                );
                error!(
                    download_id = %id.download_id,
                    %error,
                    "Download event translator failed"
                );
                let message = format!("download event translator failed: {error}");
                lifecycle_errors.push(message.clone());
                choose_attempt_terminal(
                    &phase,
                    id.failed(DownloadFailureKind::Other, message, true),
                    &id,
                )
            }
        };

        if let DownloadTerminalEvent::Failed {
            kind, engine_type, ..
        } = &terminal
            && kind.requests_credential_diagnostic(*engine_type)
            && !matches!(*phase.lock(), AttemptPhase::TerminalChosen)
            && let Some(diagnostic) = credential_diagnostic
        {
            let replacement = tokio::select! {
                biased;
                _ = handle.cancellation_token.cancelled() => None,
                result = diagnostic(terminal.clone()) => result,
            };
            if let Some(replacement) = replacement
                && let DownloadTerminalEvent::Failed {
                    kind,
                    recoverable,
                    error,
                    ..
                } = &mut terminal
            {
                *kind = replacement;
                *recoverable = true;
                *error = if replacement == DownloadFailureKind::CredentialUnavailable {
                    "Recording awaits available credentials"
                } else {
                    "Bound media renewed after engine failure"
                }
                .to_string();
            }
            terminal = choose_attempt_terminal(&phase, terminal, &id);
        }
        let terminal_event = DownloadManagerEvent::Terminal(terminal);
        if let Err(delivery_error) = events.coordinate_and_wait(&terminal_event).await {
            error!(
                download_id = %id.download_id,
                %delivery_error,
                "Required terminal outcome could not be applied"
            );
            lifecycle_errors.push(format!(
                "required terminal outcome could not be applied: {delivery_error}"
            ));
        }

        finalizer.release_active();
        events.observe_reserved(terminal_event, terminal_feedback);
        let outcome = if lifecycle_errors.is_empty() {
            Ok(())
        } else {
            Err(lifecycle_errors.join("; "))
        };
        finalizer.set_outcome(outcome.clone());
        debug!(download_id = %id.download_id, "Recording attempt finished");
        outcome
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
        let (started_feedback, terminal_feedback) = self
            .events
            .reserve_feedback(&config.streamer_id)
            .map_err(|_| {
                if let Some(feedback) = self.events.feedback.get() {
                    feedback.request_recheck(&config.streamer_id);
                }
                crate::Error::SchedulerFeedbackBusy {
                    streamer_id: config.streamer_id.clone(),
                }
            })?;
        let active_slot = slot.into_active();
        Self::seed_session_segment_index(
            &self.session_segment_indices,
            &config.session_id,
            config.initial_segment_index,
        );

        let download_id = uuid::Uuid::new_v4().to_string();
        let (segment_tx, segment_rx) = mpsc::channel::<SegmentEvent>(32);
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
        };

        let id = Arc::new(AttemptIdentity {
            download_id: download_id.clone(),
            streamer_id: config.streamer_id.clone(),
            streamer_name: config.streamer_name.clone(),
            session_id: config.session_id.clone(),
            engine_type,
            protocol: config.protocol,
        });
        let cdn_host = crate::utils::url::extract_host(&config.url).unwrap_or_default();
        let credential_diagnostic = self.credential_diagnostic.get().cloned();
        let managed_credentials = config.managed_credentials;
        let lifecycle_errors = Arc::new(Mutex::new(FailureSamples::default()));

        let engine_future = run_engine(engine, handle.clone(), managed_credentials);
        let translator = SegmentTranslator {
            id: id.clone(),
            events: self.events.clone(),
            active_downloads: self.active_downloads.clone(),
            phase: phase.clone(),
            handle: handle.clone(),
            session_segment_indices: self.session_segment_indices.clone(),
            output_root_gate: self.output_root_gate.get().cloned(),
            circuit_breaker: self.circuit_breakers.get(&engine_key),
            lifecycle_errors: lifecycle_errors.clone(),
            managed_credentials,
            may_diagnose: credential_diagnostic.is_some() && managed_credentials,
            published_split_revision: 0,
            pending_cut_completion: None,
            last_started_sequence: None,
            last_progress_emit: Instant::now()
                .checked_sub(PROGRESS_MIN_INTERVAL)
                .unwrap_or_else(Instant::now),
            engine_to_session: HashMap::new(),
            engine_segment_paths: HashMap::new(),
            output_failure_recorded: false,
        };
        let translator_future = translator.run(segment_rx, handle.manual_split.updates());

        let attempt = AttemptTask {
            id: id.clone(),
            handle,
            phase,
            events: self.events.clone(),
            finalizer: AttemptFinalizer {
                download_id: download_id.clone(),
                active_downloads: self.active_downloads.clone(),
                completion,
                outcome: None,
                active_released: false,
            },
            lifecycle_errors,
            credential_diagnostic,
            terminal_feedback,
            managed_credentials,
        };
        let attempt_task = attempt.run(engine_future, translator_future);

        let started_url = if config.managed_credentials {
            String::new()
        } else {
            config.url.clone()
        };
        let admitted = self.attempts.spawn(
            download_id.clone(),
            || {
                self.active_downloads
                    .insert(download_id.clone(), active_download);
                self.events.observe_reserved(
                    DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                        download_id: id.download_id.clone(),
                        streamer_id: id.streamer_id.clone(),
                        streamer_name: id.streamer_name.clone(),
                        session_id: id.session_id.clone(),
                        engine_type,
                        cdn_host,
                        download_url: started_url,
                    }),
                    started_feedback,
                );
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
            streamer_id = id.streamer_id.as_str(),
            engine = %engine_type,
            "Starting download"
        );
        Ok(download_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn historical_failure_sampling_preserves_fatal_shutdown_failures() {
        let supervisor = AttemptSupervisor::new();
        for index in 0..256 {
            let (finished, completion) = tokio::sync::oneshot::channel();
            assert!(
                supervisor.spawn(format!("historical-{index}"), || {}, async move {
                    finished.send(()).unwrap();
                    Err("historical error".to_owned())
                },)
            );
            tokio::time::timeout(Duration::from_secs(5), completion)
                .await
                .unwrap()
                .unwrap();
        }
        // Admission reaps every earlier task before shutdown starts. This
        // last task is still owned by the shutdown join, so its complete
        // failure remains fatal even if it exceeds the sampling byte cap.
        let fatal = "fatal shutdown detail ".repeat(1024);
        let active_error = fatal.clone();
        assert!(supervisor.spawn("active".to_owned(), || {}, async move { Err(active_error) }));
        let report = supervisor
            .join_until(tokio::time::Instant::now() + Duration::from_secs(5))
            .await;

        assert_eq!(
            report.runtime_failures[0],
            "128 earlier runtime failures omitted (256 total; retaining the latest 128 samples)"
        );
        assert_eq!(report.runtime_failures.len(), 129);
        assert!(report.runtime_failures[1].contains("historical-128 failed"));
        assert_eq!(
            report.failures,
            [format!("download active failed during shutdown: {fatal}")]
        );
    }
}
