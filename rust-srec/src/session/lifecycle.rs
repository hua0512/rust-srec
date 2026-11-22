//! The single-owner recording-session state service.
//!
//! `SessionLifecycle` handles the three triggers that can open or close
//! a recording session and holds the authoritative in-memory view of which
//! sessions are still `Recording` and which have `Ended`:
//!
//! - A live monitor result calls [`SessionLifecycle::on_live_detected`]
//!   → one atomic `BEGIN IMMEDIATE` tx via
//!   [`SessionLifecycleRepository::start_or_resume`] → emit
//!   [`SessionTransition::Started`].
//! - An offline monitor result calls [`SessionLifecycle::on_offline_detected`]
//!   → one atomic tx via [`SessionLifecycleRepository::end`] → emit
//!   [`SessionTransition::Ended`] with [`TerminalCause::StreamerOffline`].
//! - [`DownloadTerminalEvent`] → [`SessionLifecycle::on_download_terminal`]
//!   → one light tx via [`SessionLifecycleRepository::end_session_only`]
//!   (streamer state / notification outbox untouched — the streamer may
//!   still be live until the monitor says otherwise) → emit
//!   [`SessionTransition::Ended`] with the event's mapped cause.
//!
//! Pipeline scheduling stays in `pipeline::manager`; only the session-complete
//! trigger consults `SessionTransition::Ended` via the broadcast channel
//! exposed by [`SessionLifecycle::subscribe`].
//!
//! The subscription loops bind this service to the
//! [`crate::monitor::MonitorEventBroadcaster`] and
//! [`crate::downloader::DownloadManager`] broadcast channels.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use jiff::Timestamp;
use parking_lot::Mutex;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, RwLock, broadcast};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::models::SessionEventDbModel;
use crate::database::repositories::{
    EndSessionInputs, EndSessionOutcome, SessionEventRepository, SessionLifecycleRepository,
    StartSessionInputs, StartSessionOutcome,
};
use crate::domain::StreamerState;
#[cfg(test)]
use crate::downloader::DownloadProtocol;
use crate::downloader::DownloadTerminalEvent;
#[cfg(test)]
use crate::downloader::engine::EngineType;
use crate::session::classifier::{EngineKind, OfflineClassifier};
use crate::session::events::SessionEventPayload;
use crate::session::hysteresis::{HysteresisConfig, HysteresisHandle};
use crate::session::state::{SessionState, TerminalCause};
use crate::session::transition::{SessionTransition, SessionTransitionSender};
use crate::utils::required_channel::Publish;
use crate::utils::task_supervisor::{DrainedTasks, TaskSupervisor};

/// Default broadcast capacity for [`SessionTransition`] subscribers.
pub const DEFAULT_TRANSITION_CHANNEL_CAPACITY: usize = 256;

/// Default retention for `Ended` entries in `self.sessions`. Long enough to
/// dedupe any plausible duplicate authoritative-end event (we have observed
/// the monitor emit a second `OfflineDetected` ~5 ms after the first when
/// two status checks race), short enough that the map stays bounded by
/// recent activity rather than the lifetime of the process.
pub const ENDED_RETENTION_DEFAULT: Duration = Duration::from_secs(60);

/// The single-owner service for recording-session state.
///
/// Owns three pieces of in-memory state:
///
/// - `sessions` — `session_id → SessionState`. Holds Recording/Hysteresis/Ended
///   entries. Bounded by `(active sessions) + (in-flight hysteresis windows)`
///   in steady state; entries are evicted on transition into Ended.
/// - `hysteresis` — `session_id → HysteresisHandle`. One entry per session
///   currently parked in the quiet-period. Cleaned up by the timer task on
///   completion (whether by deadline or by external cancellation).
/// - `streamer_current_sessions` — `streamer_id -> current session_id`.
///   This keeps streamer-scoped lookups deterministic while `sessions` also
///   retains recently-ended entries for duplicate-event dedupe.
/// - `classifier` — per-streamer Network-failure window used by `on_download_terminal`.
#[derive(Clone)]
pub struct SessionLifecycle {
    pub(super) repo: Arc<SessionLifecycleRepository>,
    /// Per-engine offline-signal classifier. On every Terminal::Failed,
    /// the classifier decides whether the failure is a high-confidence
    /// definitive-offline (N consecutive Mesio Network failures inside a
    /// window) so the session can be ended immediately without waiting on
    /// the hysteresis quiet-period.
    pub(super) classifier: Arc<OfflineClassifier>,
    /// `session_id` → in-memory session snapshot. Source of truth for the
    /// in-process `is_session_active` query (returns true for `Recording`
    /// AND `Hysteresis`). DB `end_time` is authoritative on cold-start.
    ///
    /// `Ended` entries are retained for `ended_retention` (default 60 s) so
    /// the CAS-style guard at the top of `enter_ended_state` actually catches
    /// duplicate authoritative-end events. A scheduled task evicts each
    /// `Ended` entry after the retention window — bounding the map at
    /// `O(active sessions + active hysteresis windows + sessions ended in
    /// the last `ended_retention` seconds)`.
    pub(super) sessions: Arc<DashMap<String, SessionState>>,
    /// `streamer_id` → current `session_id`. Unlike `sessions`, this has at
    /// most one entry per streamer and is the deterministic lookup path for
    /// streamer-scoped operations such as disable cleanup.
    pub(super) streamer_current_sessions: Arc<DashMap<String, String>>,
    /// `session_id` → `HysteresisHandle`. One entry per session in the
    /// hysteresis quiet-period. The handle owns the cancellation token
    /// that the timer task watches; cancellation can come from a resume
    /// (`on_live_detected`) or an authoritative end overriding hysteresis
    /// (`on_offline_detected`, danmu close, etc.).
    pub(super) hysteresis: Arc<DashMap<String, HysteresisHandle>>,
    /// Default hysteresis window length, used when no per-streamer
    /// resolver is wired in or the resolver returns nothing for a streamer.
    hysteresis_config: HysteresisConfig,
    /// Optional per-streamer override resolver. When `Some`, the lifecycle
    /// queries it at hysteresis-arming time so the backstop window tracks
    /// each streamer's effective `offline_check_*` (set on its
    /// `StreamerMetadata` by the config resolver).
    hysteresis_resolver: Option<HysteresisWindowFn>,
    /// How long to keep `Ended` entries in `sessions` before evicting them.
    /// Bounds memory while letting the idempotency guard dedupe any
    /// near-simultaneous duplicate authoritative-end events.
    ended_retention: Duration,
    /// Optional handle to the standalone session-event repository, used for
    /// the two best-effort writes that have no surrounding atomic tx
    /// (`hysteresis_entered`, `session_resumed`). The atomic-tx writes for
    /// `session_started` / `session_ended` go through
    /// `SessionLifecycleRepository` directly. When `None`, hysteresis /
    /// resumed events silently no-op — used by tests that don't care about
    /// the audit log.
    event_repo: Option<Arc<dyn SessionEventRepository>>,
    transition_tx: broadcast::Sender<SessionTransition>,
    required_transition_tx: Option<SessionTransitionSender>,
    /// Admission fence for every externally-triggered lifecycle mutation.
    /// Shutdown takes the write side before closing the required transition
    /// channel, which guarantees that no admitted operation can publish
    /// behind the channel's shutdown marker.
    operation_gate: Arc<RwLock<()>>,
    /// Fixed stripes bound lock storage even as streamers are added/deleted.
    /// A stripe covers DB commit, memory mutation, and transition publication.
    /// Public entrypoints acquire admission then a stripe; private FSM helpers
    /// rely on their caller's stripe. Timer completion transfers admission and
    /// its stripe to an owned operation before writing the database.
    streamer_operations: Arc<[Arc<AsyncMutex<()>>; 256]>,
    accepting_operations: Arc<AtomicBool>,
    committed_removal_bound: Arc<AtomicBool>,
    hysteresis_tasks: Arc<Mutex<JoinSet<()>>>,
    hysteresis_shutdown: Arc<AtomicBool>,
    /// Delayed in-memory cleanup has no terminal side effects and stops on shutdown.
    eviction_tasks: Arc<TaskSupervisor>,
    eviction_cancel: CancellationToken,
    operation_tasks: Arc<TaskSupervisor>,
}

#[derive(Debug, Default)]
pub(crate) struct SessionLifecycleShutdownReport {
    /// Containment failures: a hysteresis timer task panicked or could not be
    /// joined, so `shutdown_until` cannot prove it stopped writing. Fatal.
    pub(crate) failures: Vec<String>,
    /// Soft-budget overruns that were nevertheless contained to completion.
    /// Reported so operators can raise the budget, never fatal — a drain that
    /// took longer than its grace period still finalized everything.
    pub(crate) overruns: Vec<String>,
    /// Timers that were still settling when the graceful deadline elapsed.
    /// They were already cancelled and were joined to completion afterwards.
    pub(crate) forced_timer_count: usize,
}

/// Closure type for resolving a per-streamer hysteresis window.
///
/// Wired at lifecycle construction (see
/// [`SessionLifecycle::with_hysteresis_resolver`]) — typically captures an
/// `Arc<StreamerManager>` and reads the per-streamer
/// resolved `offline_check_count` / `offline_check_delay_ms` values cached on
/// `StreamerMetadata`. Returning `None` falls back to the lifecycle's default
/// `HysteresisConfig`.
pub type HysteresisWindowFn = Arc<dyn Fn(&str) -> Option<HysteresisConfig> + Send + Sync>;

impl SessionLifecycle {
    pub(crate) fn bind_committed_streamers(
        &self,
        state: Arc<crate::streamer::CommittedStreamerState>,
    ) {
        if !self.committed_removal_bound.swap(true, Ordering::AcqRel) {
            let classifier = self.classifier.clone();
            state.on_removed(Arc::new(move |streamer_id| {
                classifier.purge_streamer(streamer_id);
            }));
        }
        self.repo.bind_committed_state(state);
    }

    /// Evict per-streamer failure classification state when a streamer is deleted.
    pub fn purge_streamer(&self, streamer_id: &str) {
        self.classifier.purge_streamer(streamer_id);
    }

    pub fn new(
        repo: Arc<SessionLifecycleRepository>,
        classifier: Arc<OfflineClassifier>,
        capacity: usize,
    ) -> Self {
        Self::with_config(repo, classifier, capacity, HysteresisConfig::default())
    }

    pub fn with_config(
        repo: Arc<SessionLifecycleRepository>,
        classifier: Arc<OfflineClassifier>,
        capacity: usize,
        hysteresis_config: HysteresisConfig,
    ) -> Self {
        let (transition_tx, _) = broadcast::channel(capacity);
        let eviction_cancel = CancellationToken::new();
        Self {
            repo,
            classifier,
            sessions: Arc::new(DashMap::new()),
            streamer_current_sessions: Arc::new(DashMap::new()),
            hysteresis: Arc::new(DashMap::new()),
            hysteresis_config,
            hysteresis_resolver: None,
            ended_retention: ENDED_RETENTION_DEFAULT,
            event_repo: None,
            transition_tx,
            required_transition_tx: None,
            operation_gate: Arc::new(RwLock::new(())),
            streamer_operations: Arc::new(std::array::from_fn(|_| Arc::new(AsyncMutex::new(())))),
            accepting_operations: Arc::new(AtomicBool::new(true)),
            committed_removal_bound: Arc::new(AtomicBool::new(false)),
            hysteresis_tasks: Arc::new(Mutex::new(JoinSet::new())),
            hysteresis_shutdown: Arc::new(AtomicBool::new(false)),
            eviction_tasks: Arc::new(TaskSupervisor::with_cancellation(eviction_cancel.clone())),
            eviction_cancel,
            operation_tasks: Arc::new(TaskSupervisor::new()),
        }
    }

    /// Stop admitting lifecycle mutations, drain every admitted operation,
    /// then cancel and join the owned hysteresis and eviction tasks. The deadline bounds
    /// the graceful phase; timer cleanup remains owned and is joined after it.
    /// Once this returns, no lifecycle producer can write session state or
    /// publish a transition — the return value only classifies how that was
    /// reached, so overruns land in `overruns` and never in `failures`.
    pub(crate) async fn shutdown_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> SessionLifecycleShutdownReport {
        self.accepting_operations.store(false, Ordering::Release);
        self.eviction_cancel.cancel();

        let (operation_fence, operation_deadline_exceeded) =
            match tokio::time::timeout_at(deadline, self.operation_gate.write()).await {
                Ok(guard) => (guard, false),
                Err(_) => {
                    self.hysteresis_shutdown.store(true, Ordering::Release);
                    for handle in self.hysteresis.iter() {
                        handle.cancel();
                    }
                    // An admitted operation may still reach timer admission or
                    // publish a required transition. Keep ownership and wait for
                    // it before the transition shutdown marker is allowed to run.
                    warn!(
                        "Session lifecycle operation grace period exceeded; awaiting containment"
                    );
                    (self.operation_gate.write().await, true)
                }
            };

        self.hysteresis_shutdown.store(true, Ordering::Release);
        for handle in self.hysteresis.iter() {
            handle.cancel();
        }

        // Returned to `hysteresis_tasks` if this future is dropped mid-join, so
        // a caller that stops awaiting the drain leaves the timers joinable
        // instead of aborting them.
        let mut tasks = DrainedTasks::take_from(&self.hysteresis_tasks);
        drop(operation_fence);
        let mut failures = Vec::new();
        let mut overruns = Vec::new();
        if operation_deadline_exceeded {
            overruns.push(
                "graceful session lifecycle operation deadline exceeded; admitted operations were contained to completion"
                    .to_string(),
            );
        }
        let mut forced_timer_count = 0;
        tasks
            .join_with_grace(
                deadline,
                |unfinished| {
                    forced_timer_count = unfinished;
                    overruns.push(format!(
                        "graceful hysteresis timer deadline exceeded; drained {forced_timer_count} cancelled timers to completion"
                    ));
                },
                |result, _| {
                    if let Err(error) = result {
                        failures.push(format!(
                            "hysteresis timer task failed during shutdown: {error}"
                        ));
                    }
                },
            )
            .await;
        if !self
            .eviction_tasks
            .shutdown(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
        {
            overruns.push("session eviction cleanup exceeded the graceful deadline".to_string());
        }
        if !self
            .operation_tasks
            .shutdown(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
        {
            overruns.push("owned lifecycle completion exceeded the graceful deadline".into());
        }
        SessionLifecycleShutdownReport {
            failures,
            overruns,
            forced_timer_count,
        }
    }

    /// Cancel and abort the owned hysteresis and eviction tasks.
    ///
    /// [`Self::shutdown_until`] keeps joining past its deadline, so a timer
    /// task that never returns holds it pending. Callers with no parent
    /// process to force-kill them use this.
    ///
    /// Returns the number of hysteresis timer tasks that were still running.
    /// A sealed operation is never aborted during COMMIT. If it cannot settle
    /// by the hard deadline, it remains supervised and the caller must leave
    /// database pools open.
    pub(crate) async fn abort_timers(&self, deadline: tokio::time::Instant) -> usize {
        self.accepting_operations.store(false, Ordering::Release);
        self.eviction_cancel.cancel();
        self.hysteresis_shutdown.store(true, Ordering::Release);
        for handle in self.hysteresis.iter() {
            handle.cancel();
        }

        let mut tasks = DrainedTasks::take_from(&self.hysteresis_tasks);
        let aborted = tasks.len();
        let unfinished = tasks.abort_and_reap(deadline, "Hysteresis timer").await;
        if unfinished > 0 {
            warn!(
                unfinished,
                "Aborted hysteresis timers did not settle before the reap deadline"
            );
        }

        self.eviction_tasks.abort_all(deadline).await;
        if tokio::time::timeout_at(
            deadline,
            self.operation_tasks
                .shutdown(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await
        .is_err()
        {
            warn!(
                "Lifecycle operations remain owned beyond the hard deadline; database pools must stay open"
            );
        }
        aborted
    }

    async fn begin_operation(&self) -> Result<OwnedRwLockReadGuard<()>> {
        if !self.accepting_operations.load(Ordering::Acquire) {
            return Err(crate::Error::Other(
                "session lifecycle is shutting down".to_string(),
            ));
        }

        let guard = self.operation_gate.clone().read_owned().await;
        if !self.accepting_operations.load(Ordering::Acquire) {
            return Err(crate::Error::Other(
                "session lifecycle is shutting down".to_string(),
            ));
        }
        Ok(guard)
    }

    async fn lock_streamer(&self, streamer_id: &str) -> OwnedMutexGuard<()> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        streamer_id.hash(&mut hasher);
        self.streamer_operations[hasher.finish() as usize % self.streamer_operations.len()]
            .clone()
            .lock_owned()
            .await
    }

    /// Runs an externally triggered mutation: admitted against shutdown,
    /// serialized on `streamer_id`'s stripe, and owned by `operation_tasks`
    /// so it still completes if the caller stops awaiting it. `operation` is
    /// not polled until both are held.
    async fn run_operation<T>(
        &self,
        streamer_id: &str,
        operation: impl std::future::Future<Output = Result<T>> + Send + 'static,
    ) -> Result<T>
    where
        T: Send + 'static,
    {
        let admission = self.begin_operation().await?;
        let stripe = self.lock_streamer(streamer_id).await;
        crate::database::committed_writer::own_operation(self.operation_tasks.clone(), async move {
            let (_admission, _stripe) = (admission, stripe);
            operation.await
        })
        .await
    }

    pub(crate) fn with_required_transition_sender(
        mut self,
        sender: SessionTransitionSender,
    ) -> Self {
        self.required_transition_tx = Some(sender);
        self
    }

    /// Attach a per-streamer hysteresis-window resolver. The lifecycle calls
    /// this at hysteresis-arming time; if the resolver returns `None`, the
    /// default `hysteresis_config` is used.
    pub fn with_hysteresis_resolver(mut self, resolver: HysteresisWindowFn) -> Self {
        self.hysteresis_resolver = Some(resolver);
        self
    }

    /// Attach the standalone session-event repository used for best-effort
    /// audit writes from in-memory transitions (`hysteresis_entered`,
    /// `session_resumed`). The atomic-tx writes for `session_started` /
    /// `session_ended` route through `SessionLifecycleRepository` and don't
    /// depend on this.
    pub fn with_event_repo(mut self, event_repo: Arc<dyn SessionEventRepository>) -> Self {
        self.event_repo = Some(event_repo);
        self
    }

    /// Override the retention applied to `Ended` entries before they are
    /// evicted from the in-memory `sessions` map. Reserved for tests that
    /// need to observe eviction without sleeping for the production default.
    #[cfg(test)]
    pub fn with_ended_retention(mut self, retention: Duration) -> Self {
        self.ended_retention = retention;
        self
    }

    pub fn with_default_capacity(
        repo: Arc<SessionLifecycleRepository>,
        classifier: Arc<OfflineClassifier>,
    ) -> Self {
        Self::new(repo, classifier, DEFAULT_TRANSITION_CHANNEL_CAPACITY)
    }

    pub(crate) fn repository(&self) -> &SessionLifecycleRepository {
        &self.repo
    }

    /// Note a successful per-segment completion for the streamer so the
    /// classifier's consecutive-failure counter resets. Called from the
    /// download-event subscription on [`crate::downloader::DownloadProgressEvent::
    /// SegmentCompleted`].
    pub fn on_segment_completed(&self, streamer_id: &str) {
        self.classifier.note_successful_segment(streamer_id);
    }

    /// Subscribe to session transitions. The first subscriber must attach
    /// before the first event is published; broadcast channel semantics
    /// apply (newly-attached subscribers miss prior events).
    pub fn subscribe(&self) -> broadcast::Receiver<SessionTransition> {
        self.transition_tx.subscribe()
    }

    pub(super) fn publish_transition(&self, transition: SessionTransition) {
        if let Some(required_tx) = &self.required_transition_tx
            && required_tx.publish(transition.clone()) != Publish::Accepted
        {
            warn!("Required session transition consumer is unavailable");
        }
        // The broadcast half feeds SSE and notification observers, which are a
        // live view rather than a record of what was applied. They are notified
        // even when the required consumer refused, so a coordination failure
        // does not also freeze every subscriber.
        let _ = self.transition_tx.send(transition);
    }

    /// Best-effort write of an audit row for an in-memory transition that
    /// has no surrounding DB tx (`hysteresis_entered`, `session_resumed`).
    /// A failure logs and continues — the audit log must never block the
    /// in-memory FSM, and the eventual `session_ended` row is still
    /// written atomically and tells the full story.
    pub(super) async fn record_event_best_effort(
        &self,
        session_id: &str,
        streamer_id: &str,
        payload: SessionEventPayload,
        occurred_at: Timestamp,
    ) {
        let Some(repo) = self.event_repo.as_ref() else {
            return;
        };
        let kind = payload.kind().as_str();
        let encoded_payload = match serde_json::to_string(&payload) {
            Ok(encoded) => encoded,
            Err(error) => {
                warn!(
                    session_id,
                    streamer_id,
                    kind,
                    %error,
                    "best-effort session event serialization failed"
                );
                return;
            }
        };
        let row = SessionEventDbModel {
            id: 0,
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            kind: kind.to_string(),
            occurred_at: occurred_at.as_millisecond(),
            payload: Some(encoded_payload),
        };
        if let Err(e) = repo.insert(&row).await {
            warn!(
                session_id,
                streamer_id,
                kind,
                error = %e,
                "best-effort session event persistence failed"
            );
        }
    }

    /// Mark `session_id` as the current session for `streamer_id`.
    ///
    /// Use this only when creating a new current session or when a DB lookup
    /// resolves the active session during cold-start style cleanup.
    fn set_current_session(&self, streamer_id: &str, session_id: &str) {
        self.streamer_current_sessions
            .insert(streamer_id.to_string(), session_id.to_string());
    }

    /// Ensure `streamer_id` has a current-session pointer.
    ///
    /// Existing pointers win: state changes for an old session must not steal
    /// the pointer from a newer session that went live while the old `Ended`
    /// entry is retained.
    pub(super) fn refresh_current_session_if_current(&self, streamer_id: &str, session_id: &str) {
        match self
            .streamer_current_sessions
            .entry(streamer_id.to_string())
        {
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                entry.insert(session_id.to_string());
            }
        }
    }

    /// Remove a streamer pointer only when it still points at `session_id`.
    fn remove_current_session_if_matches(&self, streamer_id: &str, session_id: &str) {
        remove_streamer_current_session_if_matches(
            &self.streamer_current_sessions,
            streamer_id,
            session_id,
        );
    }

    /// Id of the streamer's current in-memory session, whatever state it is in.
    ///
    /// For callers holding a `SessionCancelTokens` handle: a
    /// `run_live_download_pipeline` task that has already called
    /// `SessionCancelTokens::register` but has not reached `acquire_slot` owns a
    /// token and appears in neither `DownloadManager::snapshot_pending` nor
    /// `get_active_downloads`, so its session id is only reachable here.
    pub(crate) fn current_session_id_for_streamer(&self, streamer_id: &str) -> Option<String> {
        self.current_session_for_streamer(streamer_id)
            .map(|(session_id, _)| session_id)
    }

    pub(super) fn current_session_for_streamer(
        &self,
        streamer_id: &str,
    ) -> Option<(String, SessionState)> {
        let session_id = self
            .streamer_current_sessions
            .get(streamer_id)
            .map(|entry| entry.value().clone())?;

        match self.sessions.get(&session_id) {
            Some(state) if state.streamer_id() == streamer_id => {
                Some((session_id, state.value().clone()))
            }
            _ => {
                self.remove_current_session_if_matches(streamer_id, &session_id);
                None
            }
        }
    }

    /// `true` when the streamer's current in-memory session is active.
    ///
    /// This is the streamer-scoped companion to [`Self::is_session_active`]:
    /// `Recording` and `Hysteresis` both count as active, `Ended` does not.
    pub fn has_active_session_for_streamer(&self, streamer_id: &str) -> bool {
        self.current_session_for_streamer(streamer_id)
            .is_some_and(|(_, state)| state.is_active())
    }

    /// `true` if the session is tracked in-memory and has not committed to
    /// `Ended` — i.e. it is `Recording` *or* `Hysteresis`. The hysteresis
    /// state is the engine reporting an end while we wait to see if a
    /// resume happens; from the API perspective the session is still
    /// "alive" until that decision lands.
    pub fn is_session_active(&self, session_id: &str) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|entry| entry.value().is_active())
    }

    /// Look up the active hysteresis session id for a streamer, if any.
    /// Used by `on_live_detected` to decide whether to resume.
    fn hysteresis_session_for_streamer(&self, streamer_id: &str) -> Option<String> {
        let (session_id, state) = self.current_session_for_streamer(streamer_id)?;
        if state.is_hysteresis() && self.hysteresis.contains_key(&session_id) {
            Some(session_id)
        } else {
            None
        }
    }

    /// Snapshot of the session state, if tracked.
    pub fn session_snapshot(&self, session_id: &str) -> Option<SessionState> {
        self.sessions.get(session_id).map(|e| e.value().clone())
    }

    /// Evict the `Ended` snapshot for `session_id` after `ended_retention`.
    ///
    /// The delay keeps the idempotency guards in `enter_ended_state` /
    /// `on_offline_detected` able to dedupe a duplicate authoritative-end
    /// event (the monitor can emit two `OfflineDetected` events a few ms
    /// apart). Once the window passes, the `sessions` entry and the
    /// `streamer_current_sessions` pointer (only if it still names this
    /// session) are removed.
    pub(super) fn schedule_ended_eviction(&self, streamer_id: &str, session_id: &str) {
        let sessions = self.sessions.clone();
        let streamer_current_sessions = self.streamer_current_sessions.clone();
        let streamer_id = streamer_id.to_string();
        let session_id = session_id.to_string();
        let cancel = self.eviction_cancel.clone();
        if cancel.is_cancelled() {
            return;
        }
        // Anchor retention to scheduling, even if the task is first polled later.
        let retention = tokio::time::sleep(self.ended_retention);
        self.eviction_tasks
            .spawn("ended session eviction", async move {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    _ = retention => {}
                }
                sessions.remove(&session_id);
                remove_streamer_current_session_if_matches(
                    &streamer_current_sessions,
                    &streamer_id,
                    &session_id,
                );
            });
    }

    pub(crate) async fn commit_credential_binding(
        &self,
        session_id: &str,
        streamer_id: &str,
        binding: crate::credentials::CredentialBinding,
    ) -> Result<crate::credentials::CredentialBinding> {
        let lifecycle = self.clone();
        let session_id = session_id.to_owned();
        let owned_streamer_id = streamer_id.to_owned();
        self.run_operation(streamer_id, async move {
            let binding = lifecycle
                .repo
                .commit_credential_binding(session_id.clone(), owned_streamer_id.clone(), binding)
                .await?;
            if !lifecycle.sessions.contains_key(&session_id)
                && let Some(started_at) = lifecycle
                    .repo
                    .active_session_started_at(&session_id)
                    .await?
            {
                lifecycle.sessions.insert(
                    session_id.clone(),
                    SessionState::recording(
                        owned_streamer_id.clone(),
                        session_id.clone(),
                        started_at,
                    ),
                );
                lifecycle.set_current_session(&owned_streamer_id, &session_id);
            }
            Ok(binding)
        })
        .await
    }

    pub(crate) async fn end_for_credential_retirement(
        &self,
        session_id: &str,
        streamer_id: &str,
        streamer_name: &str,
        binding: crate::credentials::CredentialBinding,
    ) -> Result<()> {
        let lifecycle = self.clone();
        let session_id = session_id.to_owned();
        let owned_streamer_id = streamer_id.to_owned();
        let streamer_name = streamer_name.to_owned();
        self.run_operation(streamer_id, async move {
            if lifecycle
                .repo
                .credential_binding_is_retiring(&session_id, &binding)
                .await?
                && lifecycle
                    .current_session_id_for_streamer(&owned_streamer_id)
                    .is_none_or(|current| current == session_id)
            {
                lifecycle
                    .end_for_disable_inner(&owned_streamer_id, &streamer_name)
                    .await?;
            }
            Ok(())
        })
        .await
    }

    /// Start or resume a recording session on behalf of a monitor trigger.
    ///
    /// Decision tree:
    ///   1. **Streamer has a session in `Hysteresis`** → cancel the timer,
    ///      transition `Hysteresis → Recording`, emit `Resumed`. Same
    ///      `session_id` continues; no DB writes (end_time was never set).
    ///   2. **Otherwise** → delegate to repository:
    ///      - active prior session (`end_time IS NULL`) → `ReusedActive`
    ///      - ended prior session, or no prior session → `Created`
    ///
    ///   Both steps carry the repository's authoritative inactive guard
    ///   (`SessionLifecycleRepository::inactive_state`): a streamer row
    ///   whose state forbids sessions (user disabled/cancelled, fatal)
    ///   yields `SuppressedInactive` — nothing written, nothing broadcast.
    ///
    ///   `start_or_resume` uses the DB's `end_time` to decide whether the
    ///   session can continue or a new session must be created.
    pub async fn on_live_detected(
        &self,
        args: LiveDetectedArgs<'_>,
    ) -> Result<StartSessionOutcome> {
        let owned = StartSessionInputs {
            credential_binding: args.credential_binding.cloned(),
            streamer_id: args.streamer_id.into(),
            streamer_name: args.streamer_name.into(),
            streamer_url: args.streamer_url.into(),
            current_avatar: args.current_avatar.map(str::to_owned),
            new_avatar: args.new_avatar.map(str::to_owned),
            title: args.title.into(),
            category: args.category.map(str::to_owned),
            streams: args.streams.clone(),
            media_headers: args.media_headers.cloned(),
            media_extras: args.media_extras.cloned(),
            now: args.now,
        };
        let lifecycle = self.clone();
        self.run_operation(args.streamer_id, async move {
            lifecycle.on_live_detected_inner(owned).await
        })
        .await
    }

    pub async fn on_offline_detected(
        &self,
        args: OfflineDetectedArgs<'_>,
    ) -> Result<EndSessionOutcome> {
        let (id, name, session) = (
            args.streamer_id.to_owned(),
            args.streamer_name.to_owned(),
            args.session_id.map(str::to_owned),
        );
        let (state_was_live, clear_errors, checks_restored_threshold, signal, now) = (
            args.state_was_live,
            args.clear_errors,
            args.checks_restored_threshold,
            args.signal,
            args.now,
        );
        let lifecycle = self.clone();
        self.run_operation(args.streamer_id, async move {
            lifecycle
                .on_offline_detected_inner(OfflineDetectedArgs {
                    streamer_id: &id,
                    streamer_name: &name,
                    session_id: session.as_deref(),
                    state_was_live,
                    clear_errors,
                    checks_restored_threshold,
                    signal,
                    now,
                })
                .await
        })
        .await
    }

    pub async fn on_download_terminal(
        self: &Arc<Self>,
        event: &DownloadTerminalEvent,
    ) -> Result<()> {
        let lifecycle = self.clone();
        let owned = event.clone();
        self.run_operation(event.streamer_id(), async move {
            lifecycle.on_download_terminal_inner(&owned).await
        })
        .await
    }

    pub async fn end_for_disable(
        &self,
        streamer_id: &str,
        streamer_name: &str,
    ) -> Result<Option<String>> {
        let lifecycle = self.clone();
        let (id, name) = (streamer_id.to_owned(), streamer_name.to_owned());
        self.run_operation(streamer_id, async move {
            lifecycle.end_for_disable_inner(&id, &name).await
        })
        .await
    }

    pub async fn end_for_out_of_schedule(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        old_state: StreamerState,
    ) -> Result<Option<String>> {
        let lifecycle = self.clone();
        let (id, name) = (streamer_id.to_owned(), streamer_name.to_owned());
        self.run_operation(streamer_id, async move {
            lifecycle
                .end_for_out_of_schedule_inner(&id, &name, old_state)
                .await
        })
        .await
    }

    /// When the session started: from its in-memory state, else from its
    /// row, which is all a restart leaves. `None` when neither has it.
    pub(super) async fn known_started_at(&self, session_id: &str) -> Option<Timestamp> {
        let in_memory = self.sessions.get(session_id).map(|e| e.started_at());
        if in_memory.is_some() {
            return in_memory;
        }
        match self.repo.session_started_at(session_id).await {
            Ok(started_at) => started_at,
            Err(error) => {
                warn!(session_id, %error, "failed to read the session's start time");
                None
            }
        }
    }

    async fn on_live_detected_inner(
        &self,
        inputs: StartSessionInputs,
    ) -> Result<StartSessionOutcome> {
        // Step 1: Hysteresis resume.
        //
        // `resume_from_hysteresis` returns `None` if the CAS-claim
        // (atomic remove of the hysteresis handle) lost — i.e., the
        // hysteresis timer fired or an authoritative end (`on_offline_detected`
        // / `on_download_terminal` direct path) committed `Ended` between
        // our `hysteresis_session_for_streamer` check and the resume
        // attempt. In that case the session is already Ended; fall
        // through to `start_or_resume` which produces a fresh `session_id`
        // (the prior session has `end_time` set).
        if let Some(session_id) = self.hysteresis_session_for_streamer(&inputs.streamer_id) {
            let args = LiveDetectedArgs {
                credential_binding: inputs.credential_binding.as_ref(),
                streamer_id: &inputs.streamer_id,
                streamer_name: &inputs.streamer_name,
                streamer_url: &inputs.streamer_url,
                current_avatar: inputs.current_avatar.as_deref(),
                new_avatar: inputs.new_avatar.as_deref(),
                title: &inputs.title,
                category: inputs.category.as_deref(),
                streams: &inputs.streams,
                media_headers: inputs.media_headers.as_ref(),
                media_extras: inputs.media_extras.as_ref(),
                now: inputs.now,
            };
            if let Some(outcome) = self.resume_from_hysteresis(&session_id, &args).await {
                return Ok(outcome);
            }
        }

        // Step 2: Repository call. The simplified `start_or_resume` only
        // distinguishes "active session exists" (ReusedActive) from
        // "no active session" (Created).
        let streamer_id = inputs.streamer_id.clone();
        let streamer_name = inputs.streamer_name.clone();
        let title = inputs.title.clone();
        let category = inputs.category.clone();
        let now = inputs.now;
        let outcome = self.repo.start_or_resume(inputs).await?;

        // The row-level guard inside `start_or_resume` observed a
        // user-disabled (or fatal) streamer: nothing was written
        // and no `StreamerLive` outbox event exists, so there is no session
        // to track in memory either.
        if let StartSessionOutcome::SuppressedInactive { state } = &outcome {
            info!(
                streamer_id = %streamer_id,
                state = %state,
                "live detection suppressed: streamer row is inactive"
            );
            return Ok(outcome);
        }

        // A reused session keeps its start, which after a restart only its
        // row still holds.
        let started_at = match &outcome {
            StartSessionOutcome::ReusedActive { session_id } => {
                self.known_started_at(session_id).await.unwrap_or(now)
            }
            _ => now,
        };
        self.sessions.insert(
            outcome.session_id().to_string(),
            SessionState::recording(
                streamer_id.clone(),
                outcome.session_id().to_string(),
                started_at,
            ),
        );
        self.set_current_session(&streamer_id, outcome.session_id());

        self.publish_transition(SessionTransition::Started {
            session_id: outcome.session_id().to_string(),
            streamer_id,
            streamer_name,
            title,
            category,
            started_at: now,
            from_hysteresis: false,
            // Fresh-session path: the `MonitorEvent::StreamerLive` outbox
            // event from the atomic tx in `start_or_resume` already drives
            // the container's download-start path, so this sidecar would
            // be redundant. Pass `None` to keep `Started` notification-only
            // for fresh sessions; the resume path is the only producer
            // that needs to also drive the download via this channel.
            download_start: None,
        });

        Ok(outcome)
    }

    /// End the active session on behalf of a monitor offline observation.
    /// `StreamerOffline` is authoritative (the platform's status API said
    /// the streamer is no longer live), so this always commits `Ended`
    /// directly — no hysteresis. If the session was already in
    /// `Hysteresis` (e.g. mesio FLV clean disconnect happened first, then
    /// monitor confirmed offline), `enter_ended_state` cancels the
    /// timer.
    async fn on_offline_detected_inner(
        &self,
        args: OfflineDetectedArgs<'_>,
    ) -> Result<EndSessionOutcome> {
        // The repository deduplicates session ends. Always apply the offline
        // observation: a download may already have ended while state is LIVE.
        // Resolve cause and via_hysteresis BEFORE the DB write so the audit
        // row inside `repo.end`'s atomic transaction carries the same values
        // the in-memory `SessionTransition::Ended` broadcast will carry.
        //
        // When the caller supplied a definitive-offline signal (e.g. the
        // danmu observer plumbing through `DanmuStreamClosed`), promote the
        // cause to `DefinitiveOffline { signal }` so the audit log and
        // downstream telemetry preserve *what* caused the end — not just
        // "monitor said offline."
        let cause = match args.signal.clone() {
            Some(signal) => TerminalCause::DefinitiveOffline { signal },
            None => TerminalCause::StreamerOffline,
        };
        // `was_in_hysteresis` is an in-memory check; the repo can't see it.
        // If the caller named an explicit session, consult its state
        // directly. Otherwise scan the small in-memory hysteresis map for a
        // matching streamer (typically 0 or 1 entries, a hysteresis session
        // is by definition the only active session for its streamer).
        let was_in_hysteresis = match args.session_id {
            Some(id) => self
                .sessions
                .get(id)
                .is_some_and(|e| e.value().is_hysteresis()),
            None => self
                .hysteresis_session_for_streamer(args.streamer_id)
                .is_some(),
        };

        // The full atomic bundle (end_session + set_offline + clear_errors
        // + StreamerOffline outbox event + `session_ended` audit row) lives
        // in `repo.end`. We run it first so the DB writes commit, then
        // update in-memory state and emit the transition via
        // `enter_ended_state` (with DB-write Skip since we already wrote).
        let inputs = EndSessionInputs {
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            session_id: args.session_id.map(|s| s.to_string()),
            state_was_live: args.state_was_live,
            clear_errors: args.clear_errors,
            checks_restored_threshold: args.checks_restored_threshold,
            cause: (&cause).into(),
            via_hysteresis: was_in_hysteresis,
            now: args.now,
        };
        let outcome = self.repo.end(inputs).await?;

        if let Some(id) = outcome.resolved_session_id.as_deref() {
            self.enter_ended_state(EndedStateTransition {
                session_id: id,
                streamer_id: args.streamer_id,
                streamer_name: args.streamer_name,
                cause,
                ended_at: args.now,
                via_hysteresis: was_in_hysteresis,
                db_write: DbWritePath::Skip,
            })
            .await?;
        } else {
            debug!(
                streamer_id = %args.streamer_id,
                "OfflineDetected with no active session to close"
            );
        }

        Ok(outcome)
    }

    /// End a recording session on a terminal download event. Streamer state
    /// and notification outbox are intentionally untouched — authoritative
    /// offline is still the monitor's call.
    ///
    /// Decision tree:
    ///
    /// 1. **Cancelled → no-op.** Engine may still flush a final
    ///    Completed/Failed; the session stays in `Recording` until that
    ///    authoritative terminal arrives.
    /// 2. Compute the typed [`TerminalCause`] from the event. `Failed`
    ///    events go through the classifier so consecutive Mesio Network
    ///    failures get promoted to `DefinitiveOffline`.
    /// 3. **Already Ended → no-op** (idempotency).
    /// 4. **Authoritative cause** (`DefinitiveOffline`, `Rejected`, OR
    ///    `Completed` with `EngineEndSignal::HlsEndlist` / `DashEnd`) → straight to
    ///    `Ended` via `enter_ended_state`. Pipeline fires
    ///    immediately.
    /// 5. **Ambiguous cause** (`Failed{Network/etc.}`, `Completed` with
    ///    `EngineEndSignal::CleanDisconnect` / `SubprocessExitZero` /
    ///    `Unknown`) → `Hysteresis` via `enter_hysteresis_state`.
    ///    A timer task will commit `Ended` if no resume arrives within the
    ///    window.
    async fn on_download_terminal_inner(
        self: &Arc<Self>,
        event: &DownloadTerminalEvent,
    ) -> Result<()> {
        let session_id = event.session_id();
        let streamer_id = event.streamer_id();
        let streamer_name = event.streamer_name();
        let now = Timestamp::now();

        // Step 1: The control-plane stop owner applies cancellation policy.
        if matches!(
            event,
            DownloadTerminalEvent::Failed {
                kind: crate::downloader::DownloadFailureKind::CredentialRecovery
                    | crate::downloader::DownloadFailureKind::CredentialUnavailable,
                ..
            }
        ) {
            return Ok(());
        }
        if matches!(event, DownloadTerminalEvent::Cancelled { .. }) {
            debug!(
                session_id,
                streamer_id,
                "on_download_terminal: deferring Cancelled to the control-plane stop owner"
            );
            return Ok(());
        }

        // Step 2: idempotency — check before mutating per-streamer failure
        // counters so duplicate or post-end terminal events cannot advance
        // `OfflineClassifier` toward `DefinitiveOffline`.
        if self
            .sessions
            .get(session_id)
            .is_some_and(|entry| entry.value().is_ended())
        {
            debug!(
                session_id,
                "on_download_terminal: session already ended in memory — ignoring"
            );
            return Ok(());
        }

        if !self.sessions.contains_key(session_id)
            && !self.repo.is_session_active(session_id).await?
        {
            return Ok(());
        }

        // Step 3: Build the typed cause.
        // Failed runs through the classifier (consecutive Mesio Network
        // failures promote to DefinitiveOffline). Other variants map directly.
        let cause = match event {
            DownloadTerminalEvent::Failed {
                engine_type,
                protocol,
                kind,
                ..
            } => {
                let engine_kind = EngineKind::from_engine_and_protocol(*engine_type, *protocol);
                match self
                    .classifier
                    .classify_failure(streamer_id, &engine_kind, kind)
                {
                    Some(signal) => {
                        info!(
                            streamer_id,
                            session_id,
                            engine_type = engine_type.as_str(),
                            protocol = protocol.as_str(),
                            signal = signal.as_str(),
                            "on_download_terminal: promoted Failed → DefinitiveOffline"
                        );
                        TerminalCause::DefinitiveOffline { signal }
                    }
                    None => TerminalCause::Failed { kind: *kind },
                }
            }
            _ => terminal_cause_from(event),
        };

        // Engine signal for authority decision (only Completed carries one).
        let engine_signal = match event {
            DownloadTerminalEvent::Completed { engine_signal, .. } => Some(*engine_signal),
            _ => None,
        };

        // Step 4 / 5: authority routes to direct Ended; ambiguous to Hysteresis.
        let authoritative = cause.is_authoritative_end_with_signal(engine_signal);

        if authoritative {
            debug!(
                streamer_id,
                session_id,
                cause = cause.as_str(),
                "on_download_terminal: authoritative end → direct Ended"
            );
            self.enter_ended_state(EndedStateTransition {
                session_id,
                streamer_id,
                streamer_name,
                cause,
                ended_at: now,
                via_hysteresis: false,
                db_write: DbWritePath::EndSessionOnly,
            })
            .await?;
        } else {
            debug!(
                streamer_id,
                session_id,
                cause = cause.as_str(),
                engine_signal = engine_signal
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or("(none)"),
                "on_download_terminal: ambiguous end → Hysteresis"
            );
            self.enter_hysteresis_state(session_id, streamer_id, streamer_name, cause, now)
                .await;
        }

        Ok(())
    }

    // -------------------------------------------------------------------
    // FSM driver helpers.
    //
    // The state machine has three states (Recording, Hysteresis, Ended) and
    // five external events:
    //   - LiveDetected            → `on_live_detected`
    //   - Authoritative end       → `on_offline_detected` / direct-Ended path
    //                                in `on_download_terminal`
    //   - Ambiguous end           → hysteresis path in `on_download_terminal`
    //   - Hysteresis timer fires  → fired by the timer task
    //   - Authoritative end while in hysteresis → cancel-and-Ended via
    //                                `enter_ended_state` (it tears down any
    //                                active hysteresis handle before writing
    //                                to DB and emitting the transition).
    //
    // The three helpers below are the only places where in-memory state
    // transitions happen. Each is idempotent at the in-memory level via
    // an explicit early return when the target state is already reached.
    // -------------------------------------------------------------------

    /// Park `session_id` in `Hysteresis`. Spawns a tokio task that fires
    /// `Ended` when the deadline elapses. The task observes the handle's
    /// cancellation token so a resume or authoritative end can pre-empt.
    ///
    /// Idempotent: a second call for a session already in Hysteresis is a
    /// no-op (the original timer wins). Repeat ambiguous events for the
    /// same session inside the window therefore don't extend the window.
    async fn enter_hysteresis_state(
        self: &Arc<Self>,
        session_id: &str,
        streamer_id: &str,
        streamer_name: &str,
        cause: TerminalCause,
        observed_at: Timestamp,
    ) {
        if self.hysteresis_shutdown.load(Ordering::Acquire) {
            debug!(
                session_id,
                "Skipping hysteresis admission because session lifecycle is shutting down"
            );
            return;
        }

        // Idempotency: if we're already in Hysteresis (or already Ended),
        // skip. The original timer / Ended state wins.
        if let Some(entry) = self.sessions.get(session_id)
            && (entry.is_hysteresis() || entry.is_ended())
        {
            debug!(
                session_id,
                state = entry.kind_str(),
                "enter_hysteresis_state: already past Recording, skipping"
            );
            return;
        }

        let started_at = self
            .sessions
            .get(session_id)
            .map(|e| e.started_at())
            .unwrap_or(observed_at);
        // Backstop window. The actor's existing offline-confirmation
        // hysteresis (count × interval) is the *primary* mechanism that
        // resolves a hysteresis state; the timer below only fires if the
        // actor never calls back. Window is derived from the same
        // scheduler config the actor uses — see `HysteresisConfig`. If a
        // per-streamer resolver was wired in we ask it first so platform /
        // template / streamer overrides take effect.
        let resolved_config = self
            .hysteresis_resolver
            .as_ref()
            .and_then(|r| r(streamer_id))
            .unwrap_or(self.hysteresis_config);
        let window = resolved_config.window();
        let handle = HysteresisHandle::new(window);
        let deadline_inst = handle.deadline;
        let cancel = handle.cancel.clone();

        if crate::database::committed_writer::prepare_owned_commit().is_err() {
            return;
        }
        // Update in-memory state to Hysteresis.
        self.sessions.insert(
            session_id.to_string(),
            SessionState::hysteresis(
                streamer_id,
                session_id,
                started_at,
                observed_at,
                cause.clone(),
                deadline_inst,
            ),
        );
        self.refresh_current_session_if_current(streamer_id, session_id);
        self.hysteresis.insert(session_id.to_string(), handle);

        let resume_deadline = observed_at
            + jiff::SignedDuration::try_from(window).unwrap_or(jiff::SignedDuration::from_secs(90));
        self.publish_transition(SessionTransition::Ending {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            cause: cause.clone(),
            observed_at,
            resume_deadline,
        });

        info!(
            streamer_id,
            session_id,
            cause = cause.as_str(),
            window_secs = window.as_secs(),
            "Session entering hysteresis quiet-period"
        );

        // Best-effort audit row. The function logs and continues on failure
        // so a transient DB hiccup doesn't block the in-memory FSM. Awaiting
        // inline (vs spawning) keeps tests deterministic and ensures we
        // never lose the row to runtime shutdown racing the spawned task.
        self.record_event_best_effort(
            session_id,
            streamer_id,
            SessionEventPayload::HysteresisEntered {
                cause: (&cause).into(),
                resume_deadline,
            },
            observed_at,
        )
        .await;

        // Keep only a weak lifecycle reference while waiting, avoiding a cycle
        // between the lifecycle and its timer JoinSet. A firing timer transfers
        // its guards to the operation owner before COMMIT can begin.
        let lifecycle = Arc::downgrade(self);
        let sid = session_id.to_string();
        let strm_id = streamer_id.to_string();
        let strm_name = streamer_name.to_string();
        let timer = async move {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline_inst.into()) => {}
                _ = cancel.cancelled() => return,
            }
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                let Some(lifecycle) = lifecycle.upgrade() else {
                    return;
                };
                let (operation, stripe) = tokio::select! {
                    _ = cancel.cancelled() => return,
                    guards = async {
                        let operation = lifecycle.operation_gate.clone().read_owned().await;
                        let stripe = lifecycle.lock_streamer(&strm_id).await;
                        (operation, stripe)
                    } => guards,
                };
                if cancel.is_cancelled()
                    || !lifecycle
                        .hysteresis
                        .get(&sid)
                        .is_some_and(|handle| handle.cancel == cancel)
                {
                    return;
                }
                let supervisor = lifecycle.operation_tasks.clone();
                let (attempt_sid, attempt_id, attempt_name, attempt_cause) = (
                    sid.clone(),
                    strm_id.clone(),
                    strm_name.clone(),
                    cause.clone(),
                );
                let result =
                    crate::database::committed_writer::own_operation(supervisor, async move {
                        let (_operation, _stripe) = (operation, stripe);
                        lifecycle
                            .enter_ended_state(EndedStateTransition {
                                session_id: &attempt_sid,
                                streamer_id: &attempt_id,
                                streamer_name: &attempt_name,
                                cause: attempt_cause,
                                ended_at: Timestamp::now(),
                                via_hysteresis: true,
                                db_write: DbWritePath::EndSessionOnly,
                            })
                            .await
                    })
                    .await;
                match result {
                    Ok(()) => return,
                    Err(error) => {
                        warn!(session_id = %sid, %error, "Hysteresis timer end failed; retrying while its handle remains active")
                    }
                }
                // A failed SQL write leaves the original handle active. Reuse
                // this timer, releasing admission and the stripe between tries.
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }
            }
        };

        let mut tasks = self.hysteresis_tasks.lock();
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                warn!(%error, "Hysteresis timer task failed");
            }
        }
        if self.hysteresis_shutdown.load(Ordering::Acquire) {
            if let Some((_, handle)) = self.hysteresis.remove(session_id) {
                handle.cancel();
            }
            return;
        }
        tasks.spawn(timer);
    }
}

/// Arguments for [`SessionLifecycle::on_live_detected`].
pub struct LiveDetectedArgs<'a> {
    pub credential_binding: Option<&'a crate::credentials::CredentialBinding>,
    pub streamer_id: &'a str,
    pub streamer_name: &'a str,
    pub streamer_url: &'a str,
    pub current_avatar: Option<&'a str>,
    pub new_avatar: Option<&'a str>,
    pub title: &'a str,
    pub category: Option<&'a str>,
    pub streams: &'a Vec<crate::monitor::StreamInfo>,
    pub media_headers: Option<&'a std::collections::HashMap<String, String>>,
    pub media_extras: Option<&'a std::collections::HashMap<String, String>>,
    pub now: Timestamp,
}

/// Arguments for [`SessionLifecycle::on_offline_detected`].
pub struct OfflineDetectedArgs<'a> {
    pub streamer_id: &'a str,
    pub streamer_name: &'a str,
    pub session_id: Option<&'a str>,
    pub state_was_live: bool,
    pub clear_errors: bool,
    /// See [`EndSessionInputs::checks_restored_threshold`].
    pub checks_restored_threshold: Option<i32>,
    /// Optional definitive-offline signal that originated this call. Set by
    /// the danmu observer (`DanmuStreamClosed`) and other engine-boundary
    /// detectors that can confidently say "the stream is over." When
    /// `Some`, the lifecycle records the session-end cause as
    /// [`TerminalCause::DefinitiveOffline`] (carrying the signal) instead
    /// of the default [`TerminalCause::StreamerOffline`] — which preserves
    /// the trigger detail in the audit log and downstream telemetry.
    pub signal: Option<crate::session::state::OfflineSignal>,
    pub now: Timestamp,
}

fn terminal_cause_from(event: &DownloadTerminalEvent) -> TerminalCause {
    match event {
        DownloadTerminalEvent::Completed { .. } => TerminalCause::Completed,
        DownloadTerminalEvent::Failed { kind, .. } => TerminalCause::Failed { kind: *kind },
        DownloadTerminalEvent::Cancelled { cause, .. } => TerminalCause::Cancelled {
            cause: cause.clone(),
        },
        DownloadTerminalEvent::Rejected { reason, .. } => TerminalCause::Rejected {
            reason: reason.clone(),
        },
    }
}

fn remove_streamer_current_session_if_matches(
    streamer_current_sessions: &DashMap<String, String>,
    streamer_id: &str,
    session_id: &str,
) {
    if let Entry::Occupied(entry) = streamer_current_sessions.entry(streamer_id.to_string())
        && entry.get() == session_id
    {
        entry.remove();
    }
}

mod teardown;
#[cfg(test)]
mod tests;
mod transitions;

use transitions::{DbWritePath, EndedStateTransition};
