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

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::models::SessionEventDbModel;
use crate::database::repositories::{
    EndForOutOfScheduleInputs, EndSessionInputs, EndSessionOutcome, SessionEventRepository,
    SessionLifecycleRepository, StartSessionInputs, StartSessionOutcome,
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
use crate::session::transition::SessionTransition;

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
/// - `classifier` — stateful per-streamer Network-failure log; PR 2 work.
pub struct SessionLifecycle {
    repo: Arc<SessionLifecycleRepository>,
    /// Per-engine offline-signal classifier. On every Terminal::Failed,
    /// the classifier decides whether the failure is a high-confidence
    /// definitive-offline (N consecutive Mesio Network failures inside a
    /// window) so the session can be ended immediately without waiting on
    /// the hysteresis quiet-period.
    classifier: Arc<OfflineClassifier>,
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
    sessions: Arc<DashMap<String, SessionState>>,
    /// `streamer_id` → current `session_id`. Unlike `sessions`, this has at
    /// most one entry per streamer and is the deterministic lookup path for
    /// streamer-scoped operations such as disable cleanup.
    streamer_current_sessions: Arc<DashMap<String, String>>,
    /// `session_id` → `HysteresisHandle`. One entry per session in the
    /// hysteresis quiet-period. The handle owns the cancellation token
    /// that the timer task watches; cancellation can come from a resume
    /// (`on_live_detected`) or an authoritative end overriding hysteresis
    /// (`on_offline_detected`, danmu close, etc.).
    hysteresis: Arc<DashMap<String, HysteresisHandle>>,
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
}

/// Closure type for resolving a per-streamer hysteresis window.
///
/// Wired at lifecycle construction (see
/// [`SessionLifecycle::with_hysteresis_resolver`]) — typically captures an
/// `Arc<StreamerManager>` and reads the per-streamer
/// `effective_offline_check_count` / `effective_offline_check_delay_ms`
/// values cached on `StreamerMetadata`. Returning `None` falls back to the
/// lifecycle's default `HysteresisConfig`.
pub type HysteresisWindowFn = Arc<dyn Fn(&str) -> Option<HysteresisConfig> + Send + Sync>;

impl SessionLifecycle {
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
        }
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

    pub fn subscriber_count(&self) -> usize {
        self.transition_tx.receiver_count()
    }

    /// Best-effort write of an audit row for an in-memory transition that
    /// has no surrounding DB tx (`hysteresis_entered`, `session_resumed`).
    /// A failure logs and continues — the audit log must never block the
    /// in-memory FSM, and the eventual `session_ended` row is still
    /// written atomically and tells the full story.
    async fn record_event_best_effort(
        &self,
        session_id: &str,
        streamer_id: &str,
        payload: SessionEventPayload,
        occurred_at: DateTime<Utc>,
    ) {
        let Some(repo) = self.event_repo.as_ref() else {
            return;
        };
        let kind = payload.kind().as_str();
        let row = SessionEventDbModel {
            id: 0,
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            kind: kind.to_string(),
            occurred_at: occurred_at.timestamp_millis(),
            payload: serde_json::to_string(&payload).ok(),
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
    fn refresh_current_session_if_current(&self, streamer_id: &str, session_id: &str) {
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

    fn current_session_for_streamer(&self, streamer_id: &str) -> Option<(String, SessionState)> {
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
    ///   No gap-resume rule, no continuation rule, no `hard_ended` cache.
    ///   The DB's `end_time` is the source of truth.
    pub async fn on_live_detected(
        &self,
        args: LiveDetectedArgs<'_>,
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
        if let Some(session_id) = self.hysteresis_session_for_streamer(args.streamer_id)
            && let Some(outcome) = self.resume_from_hysteresis(&session_id, &args).await
        {
            return Ok(outcome);
        }

        // Step 2: Repository call. The simplified `start_or_resume` only
        // distinguishes "active session exists" (ReusedActive) from
        // "no active session" (Created).
        let inputs = StartSessionInputs {
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            streamer_url: args.streamer_url.to_string(),
            current_avatar: args.current_avatar.map(|s| s.to_string()),
            new_avatar: args.new_avatar.map(|s| s.to_string()),
            title: args.title.to_string(),
            category: args.category.map(|s| s.to_string()),
            streams: args.streams.clone(),
            media_headers: args.media_headers.cloned(),
            media_extras: args.media_extras.cloned(),
            now: args.now,
        };

        let outcome = self.repo.start_or_resume(inputs).await?;

        // The row-level guard inside `start_or_resume` observed a
        // user-disabled/cancelled (or fatal) streamer: nothing was written
        // and no `StreamerLive` outbox event exists, so there is no session
        // to track in memory either.
        if let StartSessionOutcome::SuppressedInactive { state } = &outcome {
            info!(
                streamer_id = args.streamer_id,
                state = %state,
                "live detection suppressed: streamer row is inactive"
            );
            return Ok(outcome);
        }

        self.sessions.insert(
            outcome.session_id().to_string(),
            SessionState::recording(
                args.streamer_id.to_string(),
                outcome.session_id().to_string(),
                args.now,
            ),
        );
        self.set_current_session(args.streamer_id, outcome.session_id());

        let _ = self.transition_tx.send(SessionTransition::Started {
            session_id: outcome.session_id().to_string(),
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            title: args.title.to_string(),
            category: args.category.map(|s| s.to_string()),
            started_at: args.now,
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
    pub async fn on_offline_detected(
        &self,
        args: OfflineDetectedArgs<'_>,
    ) -> Result<EndSessionOutcome> {
        // Early dedup: if the in-memory map says this session is already in
        // `Ended` state (within `ended_retention`), short-circuit before we
        // hit the DB. Without this guard a duplicate authoritative-end —
        // observed in production when the monitor races and emits two
        // `OfflineDetected` events ~5 ms apart — would re-run `repo.end`
        // and write a second `session_ended` audit row before the in-memory
        // CAS check at the top of `enter_ended_state` kicks in.
        if let Some(id) = args.session_id
            && self.sessions.get(id).is_some_and(|e| e.value().is_ended())
        {
            debug!(
                session_id = id,
                "on_offline_detected: session already Ended in memory, skipping"
            );
            return Ok(EndSessionOutcome {
                resolved_session_id: Some(id.to_string()),
                offline_event_emitted: false,
            });
        }

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
            cause: (&cause).into(),
            via_hysteresis: was_in_hysteresis,
            now: args.now,
        };
        let outcome = self.repo.end(inputs).await?;

        if let Some(id) = outcome.resolved_session_id.as_deref() {
            self.enter_ended_state(
                id,
                args.streamer_id,
                args.streamer_name,
                cause,
                args.now,
                was_in_hysteresis,
                DbWritePath::Skip,
            )
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
    ///    `Completed` with `EngineEndSignal::HlsEndlist`) → straight to
    ///    `Ended` via `enter_ended_state`. Pipeline fires
    ///    immediately.
    /// 5. **Ambiguous cause** (`Failed{Network/etc.}`, `Completed` with
    ///    `EngineEndSignal::CleanDisconnect` / `SubprocessExitZero` /
    ///    `Unknown`) → `Hysteresis` via `enter_hysteresis_state`.
    ///    A timer task will commit `Ended` if no resume arrives within the
    ///    window.
    pub async fn on_download_terminal(
        self: &Arc<Self>,
        event: &DownloadTerminalEvent,
    ) -> Result<()> {
        let session_id = event.session_id();
        let streamer_id = event.streamer_id();
        let streamer_name = event.streamer_name();
        let now = Utc::now();

        // Step 1: Cancelled is a no-op.
        if matches!(event, DownloadTerminalEvent::Cancelled { .. }) {
            debug!(
                session_id,
                streamer_id, "on_download_terminal: Cancelled is a no-op; session stays Recording"
            );
            return Ok(());
        }

        // Step 2: Build the typed cause.
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

        // Step 3: idempotency.
        if self
            .sessions
            .get(session_id)
            .is_some_and(|entry| entry.value().is_ended())
        {
            debug!(
                session_id,
                cause = cause.as_str(),
                "on_download_terminal: session already ended in memory — ignoring"
            );
            return Ok(());
        }

        // Step 4 / 5: authority routes to direct Ended; ambiguous to Hysteresis.
        let authoritative = cause.is_authoritative_end_with_signal(engine_signal);

        if authoritative {
            debug!(
                streamer_id,
                session_id,
                cause = cause.as_str(),
                "on_download_terminal: authoritative end → direct Ended"
            );
            self.enter_ended_state(
                session_id,
                streamer_id,
                streamer_name,
                cause,
                now,
                /* via_hysteresis */ false,
                DbWritePath::EndSessionOnly,
            )
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
        observed_at: DateTime<Utc>,
    ) {
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
            + chrono::Duration::from_std(window).unwrap_or(chrono::Duration::seconds(90));
        let _ = self.transition_tx.send(SessionTransition::Ending {
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

        // Spawn the timer task. It owns nothing but Arc-clones of the maps,
        // the repo, and the broadcast sender. When it fires, it calls back
        // into a static-style helper that takes those clones, so we don't
        // need an Arc<Self>-typed entry point for cancellation safety.
        let me = Arc::clone(self);
        let sid = session_id.to_string();
        let strm_id = streamer_id.to_string();
        let strm_name = streamer_name.to_string();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline_inst.into()) => {
                    // Deadline fired — confirm Ended unless cancelled meanwhile.
                    if cancel.is_cancelled() {
                        debug!(session_id = %sid,
                               "Hysteresis timer woke but cancellation already tripped");
                        return;
                    }
                    let now = Utc::now();
                    if let Err(e) = me.enter_ended_state(
                        &sid, &strm_id, &strm_name, cause, now,
                        /* via_hysteresis */ true,
                        /* db_write */ DbWritePath::EndSessionOnly,
                    ).await {
                        warn!(session_id = %sid, error = %e,
                              "Hysteresis timer: failed to confirm Ended");
                    }
                }
                _ = cancel.cancelled() => {
                    debug!(session_id = %sid,
                           "Hysteresis timer cancelled (resume or authoritative end)");
                }
            }
        });
    }

    /// Cancel an active hysteresis timer and transition `Hysteresis →
    /// Recording`. The session row's `end_time` was never written (DB
    /// strategy B), so no DB undo is needed. Emits `SessionTransition::Resumed`.
    ///
    /// Before claiming the exit, `mark_streamer_live` re-checks the
    /// streamer row's state at the DB serialization point: an inactive row
    /// (user disabled/cancelled, fatal) aborts the resume with
    /// `SuppressedInactive` and leaves the hysteresis handle armed, so the
    /// disable teardown or the timer ends the session normally.
    ///
    /// CAS contract: the `self.hysteresis.remove(session_id)` operation IS
    /// the atomic claim for the `Hysteresis → Recording` transition. If the
    /// handle is already gone, another path (timer fire / authoritative end
    /// from `on_offline_detected` or `on_download_terminal`) has already
    /// won the race; we return `None` and let the caller fall through to
    /// the normal start_or_resume flow (which will create a fresh
    /// `session_id` since the prior session is now Ended).
    ///
    /// Pairs with the equivalent CAS in [`Self::enter_ended_state`]:
    /// whichever caller successfully removes the handle wins; the loser
    /// detects `None` and bails. No `Started` after `Ended` (or vice
    /// versa) for the same `session_id` is emitted.
    async fn resume_from_hysteresis(
        &self,
        session_id: &str,
        args: &LiveDetectedArgs<'_>,
    ) -> Option<StartSessionOutcome> {
        // Set `streamers.state = LIVE` BEFORE claiming the hysteresis exit
        // and before broadcasting. This path reuses the existing session row
        // and does not go through `start_or_resume`, so without this write
        // the row keeps whatever `monitor::service::handle_error` last wrote
        // — `NOT_LIVE` after a non-authoritative download failure.
        // Subscribers of the broadcasts below read that state through
        // `streamer_manager`, so the value must be correct before the send.
        //
        // `mark_streamer_live` also carries the authoritative inactive
        // guard: it reads the row's state inside its own `BEGIN IMMEDIATE`
        // transaction, so a user disable that already committed is observed
        // here even while the in-memory metadata cache (which the monitor's
        // guards read) still lags. When blocked, the hysteresis handle
        // stays armed — the disable path's `end_for_disable`, or the
        // hysteresis timer if that broadcast is lost, ends the session
        // through the normal path — and returning `SuppressedInactive`
        // keeps `on_live_detected` from falling through to
        // `start_or_resume`.
        //
        // A DB error logs and continues with the resume: a transient write
        // failure must not kill the resume; the cache may stay stale until
        // the next state write.
        match self
            .repo
            .mark_streamer_live(args.streamer_id, args.now)
            .await
        {
            Ok(None) => {}
            Ok(Some(state)) => {
                info!(
                    streamer_id = %args.streamer_id,
                    session_id,
                    state = %state,
                    "hysteresis resume suppressed: streamer row is inactive"
                );
                return Some(StartSessionOutcome::SuppressedInactive { state });
            }
            Err(e) => {
                warn!(
                    streamer_id = args.streamer_id,
                    session_id,
                    error = %e,
                    "resume_from_hysteresis: set_live DB write failed; cache may stay stale"
                );
            }
        }

        // CAS: claim the hysteresis exit. None = another path won.
        let Some((_, handle)) = self.hysteresis.remove(session_id) else {
            debug!(
                session_id,
                streamer_id = args.streamer_id,
                "resume_from_hysteresis: hysteresis handle already gone (CAS lost — \
                 timer or authoritative end won); caller should fall through to start_or_resume"
            );
            return None;
        };
        handle.cancel();
        let hysteresis_duration =
            chrono::Duration::from_std(handle.elapsed()).unwrap_or(chrono::Duration::zero());

        // Restore in-memory state to Recording. Preserve the original
        // `started_at` from the prior entry.
        let started_at = self
            .sessions
            .get(session_id)
            .map(|e| e.started_at())
            .unwrap_or(args.now);
        self.sessions.insert(
            session_id.to_string(),
            SessionState::recording(args.streamer_id.to_string(), session_id, started_at),
        );
        self.refresh_current_session_if_current(args.streamer_id, session_id);

        let _ = self.transition_tx.send(SessionTransition::Resumed {
            session_id: session_id.to_string(),
            streamer_id: args.streamer_id.to_string(),
            resumed_at: args.now,
            hysteresis_duration,
        });

        info!(
            streamer_id = %args.streamer_id,
            session_id,
            hysteresis_secs = hysteresis_duration.num_seconds(),
            "Session resumed from hysteresis"
        );

        // Best-effort `session_resumed` audit row. We're already on an
        // async path so awaiting is fine; any failure is non-fatal.
        let resumed_secs = u64::try_from(hysteresis_duration.num_seconds()).unwrap_or(0);
        self.record_event_best_effort(
            session_id,
            args.streamer_id,
            SessionEventPayload::SessionResumed {
                hysteresis_duration_secs: resumed_secs,
            },
            args.now,
        )
        .await;

        // Also emit a Started so notification consumers that filter on
        // Started see the resume as a logical re-online (with from_hysteresis=true).
        //
        // Crucially: the resume path short-circuits before `start_or_resume`,
        // so no `MonitorEvent::StreamerLive` outbox event fires for this
        // session this time around. Without `download_start` populated here,
        // the container has no signal to restart the download → the FLV
        // engine that disconnected at hysteresis-entry stays dead and the
        // session "records" zero bytes for the rest of the broadcast (the
        // kinetic/2026-05-02 1.5h gap). Populating the sidecar from `args`
        // is what closes that gap.
        let _ = self.transition_tx.send(SessionTransition::Started {
            session_id: session_id.to_string(),
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            title: args.title.to_string(),
            category: args.category.map(|s| s.to_string()),
            started_at: args.now,
            from_hysteresis: true,
            download_start: Some(Box::new(crate::session::DownloadStartPayload {
                streamer_url: args.streamer_url.to_string(),
                streams: args.streams.clone(),
                media_headers: args.media_headers.cloned(),
                media_extras: args.media_extras.cloned(),
            })),
        });

        // The resume path doesn't go through `start_or_resume`, so the
        // atomic `session_started { from_hysteresis: false }` row from the
        // initial create is the only one in the audit log. Record a paired
        // best-effort `session_started { from_hysteresis: true }` so the
        // timeline shows the full Recording → Hysteresis → Recording loop.
        self.record_event_best_effort(
            session_id,
            args.streamer_id,
            SessionEventPayload::SessionStarted {
                from_hysteresis: true,
                title: Some(args.title.to_string()),
            },
            args.now,
        )
        .await;

        Some(StartSessionOutcome::ReusedActive {
            session_id: session_id.to_string(),
        })
    }

    /// Move `session_id` into the final `Ended` state. Source of truth for
    /// the DB `end_time` write (path-dependent — see [`DbWritePath`]).
    /// Tears down any active hysteresis handle. Idempotent: a session
    /// already in `Ended` short-circuits with a debug log.
    ///
    /// CAS contract: when the in-memory state shows `Hysteresis`,
    /// `self.hysteresis.remove(session_id)` IS the atomic claim. If the
    /// handle is already gone, [`Self::resume_from_hysteresis`] won the
    /// race and we must NOT proceed to write `Ended` — doing so would
    /// emit an `Ended` for a session that's already broadcasted `Resumed`
    /// plus `Started{from_hysteresis: true}` and is now actively recording.
    ///
    /// Pairs with the equivalent CAS in `resume_from_hysteresis`.
    #[allow(clippy::too_many_arguments)]
    async fn enter_ended_state(
        &self,
        session_id: &str,
        streamer_id: &str,
        streamer_name: &str,
        cause: TerminalCause,
        ended_at: DateTime<Utc>,
        via_hysteresis: bool,
        db_write: DbWritePath,
    ) -> Result<()> {
        // CAS-style entry guard. If the session is already Ended, skip
        // (idempotent on duplicate authoritative-end events arriving in
        // tight succession).
        if let Some(entry) = self.sessions.get(session_id)
            && entry.is_ended()
        {
            debug!(session_id, "enter_ended_state: already Ended, skipping");
            return Ok(());
        }

        // Snapshot the in-memory state BEFORE attempting the hysteresis
        // claim, so we can detect a lost CAS race.
        //
        //   was_in_hysteresis | claim         | meaning
        //   ------------------+---------------+--------------------------------
        //   true              | Some(handle)  | we won; cancel + proceed
        //   true              | None          | resume won; bail (CAS lost)
        //   false             | Some(handle)  | impossible in practice — defensive: cancel + proceed
        //   false             | None          | direct Recording → Ended path; proceed
        //
        // Pairs with the CAS in `resume_from_hysteresis` (which returns
        // `None` on the symmetric loss case). Together they guarantee at
        // most one of {`Resumed` + `Started{from_hysteresis: true}`,
        // `Ended`} broadcasts fires for a single Hysteresis exit, even
        // under timer/resume/authoritative-end races.
        let was_in_hysteresis = self
            .sessions
            .get(session_id)
            .is_some_and(|e| matches!(e.value(), SessionState::Hysteresis { .. }));

        let claim = self.hysteresis.remove(session_id).map(|(_, h)| h);
        if let Some(h) = &claim {
            h.cancel();
        }

        if was_in_hysteresis && claim.is_none() {
            debug!(
                session_id,
                streamer_id,
                "enter_ended_state: hysteresis already claimed by resume (CAS lost); skipping"
            );
            return Ok(());
        }

        // DB write — exactly the path the caller specified. The cause +
        // via_hysteresis are forwarded into the repo so the `session_ended`
        // audit row inside its tx carries the same values we're about to
        // broadcast on `SessionTransition::Ended`. The two cannot diverge.
        match db_write {
            DbWritePath::Skip => {
                debug!(session_id, "enter_ended_state: caller already wrote DB");
            }
            DbWritePath::EndSessionOnly => {
                if !session_id.is_empty() {
                    self.repo
                        .end_session_only(
                            streamer_id,
                            Some(session_id),
                            (&cause).into(),
                            via_hysteresis,
                            ended_at,
                        )
                        .await?;
                }
            }
        }

        // Pull `started_at` for the `Ended` state from the prior entry.
        let started_at = self
            .sessions
            .get(session_id)
            .map(|e| e.started_at())
            .unwrap_or(ended_at);

        self.sessions.insert(
            session_id.to_string(),
            SessionState::ended(streamer_id, session_id, started_at, ended_at, cause.clone()),
        );
        self.refresh_current_session_if_current(streamer_id, session_id);

        info!(
            streamer_id,
            session_id,
            cause = cause.as_str(),
            via_hysteresis,
            "Session ended"
        );

        let _ = self.transition_tx.send(SessionTransition::Ended {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            ended_at,
            cause,
            via_hysteresis,
        });

        // Defer eviction by `ended_retention` so the CAS-style idempotency
        // guard at the top of this function actually catches a duplicate
        // authoritative-end event (e.g. the monitor occasionally emits two
        // `OfflineDetected` events ~5 ms apart for the same streamer).
        // Without this delay the entry would be gone by the time the second
        // call lands and we'd broadcast `SessionTransition::Ended` twice.
        let sessions = self.sessions.clone();
        let streamer_current_sessions = self.streamer_current_sessions.clone();
        let streamer_id_owned = streamer_id.to_string();
        let session_id_owned = session_id.to_string();
        let retention = self.ended_retention;
        tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            sessions.remove(&session_id_owned);
            remove_streamer_current_session_if_matches(
                &streamer_current_sessions,
                &streamer_id_owned,
                &session_id_owned,
            );
        });

        Ok(())
    }

    /// Find the current session for `streamer_id`.
    ///
    /// This uses the deterministic per-streamer index instead of scanning
    /// `sessions`, because `sessions` deliberately retains old `Ended`
    /// entries for a short window and may therefore contain multiple entries
    /// for one streamer.
    fn find_session_for_streamer(&self, streamer_id: &str) -> Option<(String, SessionState)> {
        self.current_session_for_streamer(streamer_id)
    }

    /// Tear down the active session because the user disabled (or deleted)
    /// the streamer.
    ///
    /// Replaces the deleted `monitor::service::force_end_active_session`,
    /// which wrote `live_sessions.end_time` directly via SQL but never
    /// touched `SessionLifecycle`'s in-memory FSM. That divergence caused
    /// the disable/re-enable bug observed on `kinetic（无畏契约）` 2026-05-02:
    /// re-enable found the stale Hysteresis handle, took the
    /// `resume_from_hysteresis` short-circuit, and silently restarted a
    /// download under an already-ended `session_id` while the dashboard
    /// showed the streamer as offline.
    ///
    /// Behaviour:
    /// - Cancels any active hysteresis handle via the same CAS protocol
    ///   used by `enter_ended_state` / `resume_from_hysteresis` — no race
    ///   with concurrent resume or timer fire.
    /// - Commits `live_sessions.end_time` plus a `session_events` audit
    ///   row with [`TerminalCause::UserDisabled`], atomically.
    /// - Does NOT touch `streamers.state` (the API route owns it) and does
    ///   NOT enqueue `MonitorEvent::StreamerOffline` (the user knows they
    ///   disabled it; downstream notification integrations don't need a
    ///   synthetic offline push).
    /// - Broadcasts `SessionTransition::Ended { cause: UserDisabled }` so
    ///   pipeline-manager runs session-complete (captured bytes deserve
    ///   processing) and notification-service skips `StreamOffline`.
    ///
    /// CAS-loss path (rare: the hysteresis timer fires concurrently with
    /// the disable cleanup): we retro-actively rewrite the most recent
    /// `session_ended` audit row's cause to `user_disabled` and patch the
    /// in-memory `Ended.cause` to match. The original `SessionTransition::
    /// Ended` broadcast (with the stale cause) has already shipped — we do
    /// NOT re-broadcast, because subscribers like the notification service
    /// would re-fire on the second event. The trade-off: in this rare
    /// race, one trailing offline notification slips through with the
    /// stale cause; the audit log is the source of truth and reflects the
    /// user's actual intent.
    ///
    /// Returns:
    /// - `Ok(Some(session_id))` if a session was actually torn down (or
    ///   retro-corrected);
    /// - `Ok(None)` if no active or recently-ended session existed for
    ///   the streamer.
    pub async fn end_for_disable(
        &self,
        streamer_id: &str,
        streamer_name: &str,
    ) -> Result<Option<String>> {
        let now = Utc::now();

        // Step 1: find the session in memory. The in-memory map is the
        // source of truth for FSM state; if it has no entry we'll fall
        // back to a DB lookup inside the repo to handle cold-start /
        // post-restart cases (see Step 4).
        let in_memory = self.find_session_for_streamer(streamer_id);
        let session_id_hint = in_memory.as_ref().map(|(sid, _)| sid.clone());
        let was_in_hysteresis = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_hysteresis()
        );
        let was_already_ended = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_ended()
        );

        // Step 2: claim the hysteresis CAS. Mirrors the protocol used by
        // `enter_ended_state` / `resume_from_hysteresis` — keep this in
        // lockstep with those when the protocol changes.
        let claim = if let Some(sid) = session_id_hint.as_ref() {
            self.hysteresis.remove(sid).map(|(_, h)| h)
        } else {
            None
        };
        if let Some(h) = &claim {
            h.cancel();
        }

        let lost_cas = was_in_hysteresis && claim.is_none();

        // Step 3: retro-update path. Either the session is already Ended
        // (some other path wrote it) or we lost the CAS to a concurrent
        // timer/authoritative-end. Rewrite the audit row's cause to
        // user_disabled and patch the in-memory snapshot.
        if was_already_ended || lost_cas {
            let Some(sid) = session_id_hint else {
                debug!(
                    streamer_id,
                    "end_for_disable: no in-memory session id to retro-update"
                );
                return Ok(None);
            };
            return self
                .retro_update_user_disabled(&sid, streamer_id, streamer_name, now, lost_cas)
                .await;
        }

        // Step 4: normal path. DB write first (commit → in-memory →
        // broadcast). Repo handles the active-session lookup if we don't
        // have a session_id hint (cold-start / process-restart safety).
        let resolved = self
            .repo
            .end_for_disable(
                streamer_id,
                session_id_hint.as_deref(),
                was_in_hysteresis,
                now,
            )
            .await?;

        let Some(session_id) = resolved else {
            debug!(streamer_id, "end_for_disable: no active session to end");
            return Ok(None);
        };

        // In-memory update. Pull `started_at` from the prior state if
        // present; otherwise default to `now` (cold-start case where we
        // recovered the session from the DB).
        let started_at = self
            .sessions
            .get(&session_id)
            .map(|e| e.started_at())
            .unwrap_or(now);
        self.sessions.insert(
            session_id.clone(),
            SessionState::ended(
                streamer_id,
                &session_id,
                started_at,
                now,
                TerminalCause::UserDisabled,
            ),
        );
        self.refresh_current_session_if_current(streamer_id, &session_id);

        info!(
            streamer_id,
            session_id = %session_id,
            cause = "user_disabled",
            via_hysteresis = was_in_hysteresis,
            "Session ended"
        );

        // Broadcast last — subscribers querying `session_snapshot` from
        // inside the receiver must observe the post-update state.
        let _ = self.transition_tx.send(SessionTransition::Ended {
            session_id: session_id.clone(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            ended_at: now,
            cause: TerminalCause::UserDisabled,
            via_hysteresis: was_in_hysteresis,
        });

        // Defer in-memory eviction (see `enter_ended_state` for rationale).
        let sessions = self.sessions.clone();
        let streamer_current_sessions = self.streamer_current_sessions.clone();
        let streamer_id_owned = streamer_id.to_string();
        let session_id_owned = session_id.clone();
        let retention = self.ended_retention;
        tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            sessions.remove(&session_id_owned);
            remove_streamer_current_session_if_matches(
                &streamer_current_sessions,
                &streamer_id_owned,
                &session_id_owned,
            );
        });

        Ok(Some(session_id))
    }

    /// Tear down the active session because the configured recording
    /// schedule closed while the upstream stream may still be live.
    ///
    /// This follows the same in-memory/DB/broadcast ordering as
    /// [`Self::end_for_disable`], but persists the streamer state change
    /// and `StateChanged { reason: "out_of_schedule" }` outbox event in
    /// the same transaction as the session end. It never emits
    /// `StreamerOffline`, because this is policy-driven recording stop,
    /// not a platform offline observation.
    pub async fn end_for_out_of_schedule(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        old_state: StreamerState,
    ) -> Result<Option<String>> {
        let now = Utc::now();

        let in_memory = self.find_session_for_streamer(streamer_id);
        let session_id_hint = in_memory.as_ref().map(|(sid, _)| sid.clone());
        let was_in_hysteresis = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_hysteresis()
        );
        let was_already_ended = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_ended()
        );

        let claim = if let Some(sid) = session_id_hint.as_ref() {
            self.hysteresis.remove(sid).map(|(_, h)| h)
        } else {
            None
        };
        if let Some(h) = &claim {
            h.cancel();
        }

        let lost_hysteresis_cas = was_in_hysteresis && claim.is_none();
        if was_already_ended || lost_hysteresis_cas {
            debug!(
                streamer_id,
                session_id = session_id_hint.as_deref().unwrap_or("(none)"),
                was_already_ended,
                lost_hysteresis_cas,
                "end_for_out_of_schedule: will update schedule state; session end may already be claimed"
            );
        }

        let resolved = self
            .repo
            .end_for_out_of_schedule(EndForOutOfScheduleInputs {
                streamer_id: streamer_id.to_string(),
                streamer_name: streamer_name.to_string(),
                session_id: session_id_hint.clone(),
                old_state,
                via_hysteresis: was_in_hysteresis,
                now,
            })
            .await?;

        let Some(session_id) = resolved else {
            debug!(
                streamer_id,
                "end_for_out_of_schedule: state updated but no active session to end"
            );
            return Ok(None);
        };

        // If the process restarted before this cleanup, the DB can resolve
        // an active row that has no in-memory snapshot. The DB row remains
        // authoritative; this short-lived `Ended` snapshot only needs a
        // conservative timestamp until retention evicts it.
        let started_at = self
            .sessions
            .get(&session_id)
            .map(|e| e.started_at())
            .unwrap_or(now);
        self.sessions.insert(
            session_id.clone(),
            SessionState::ended(
                streamer_id,
                &session_id,
                started_at,
                now,
                TerminalCause::OutOfSchedule,
            ),
        );
        self.refresh_current_session_if_current(streamer_id, &session_id);

        info!(
            streamer_id,
            session_id = %session_id,
            cause = "out_of_schedule",
            via_hysteresis = was_in_hysteresis,
            "Session ended"
        );

        let _ = self.transition_tx.send(SessionTransition::Ended {
            session_id: session_id.clone(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            ended_at: now,
            cause: TerminalCause::OutOfSchedule,
            via_hysteresis: was_in_hysteresis,
        });

        let sessions = self.sessions.clone();
        let streamer_current_sessions = self.streamer_current_sessions.clone();
        let streamer_id_owned = streamer_id.to_string();
        let session_id_owned = session_id.clone();
        let retention = self.ended_retention;
        tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            sessions.remove(&session_id_owned);
            remove_streamer_current_session_if_matches(
                &streamer_current_sessions,
                &streamer_id_owned,
                &session_id_owned,
            );
        });

        Ok(Some(session_id))
    }

    /// Helper for [`Self::end_for_disable`] — retro-actively rewrite the
    /// most recent `session_ended` audit row's cause to `user_disabled`
    /// and update the in-memory snapshot. Used when the FSM state is
    /// already `Ended` by the time disable cleanup runs (CAS lost to a
    /// hysteresis timer or other authoritative path).
    ///
    /// Does NOT broadcast a fresh `SessionTransition::Ended`. The original
    /// broadcast (with the stale cause) has already shipped to subscribers
    /// like notification-service; re-broadcasting would double-fire side
    /// effects. The audit log + in-memory patch are sufficient for
    /// operators to see the corrected attribution.
    async fn retro_update_user_disabled(
        &self,
        session_id: &str,
        streamer_id: &str,
        _streamer_name: &str,
        _now: DateTime<Utc>,
        lost_cas: bool,
    ) -> Result<Option<String>> {
        let updated = self
            .repo
            .rewrite_session_ended_cause(
                session_id,
                crate::session::events::TerminalCauseDto::UserDisabled,
            )
            .await?;

        if !updated {
            warn!(
                streamer_id,
                session_id, lost_cas, "end_for_disable: no session_ended audit row to retro-update"
            );
            return Ok(None);
        }

        // Patch the in-memory `Ended.cause` so consumers of `session_snapshot`
        // and `subscribe()` receivers that re-query state see the corrected
        // attribution.
        if let Some(mut entry) = self.sessions.get_mut(session_id)
            && let SessionState::Ended { cause, .. } = entry.value_mut()
        {
            *cause = TerminalCause::UserDisabled;
        }

        info!(
            streamer_id,
            session_id,
            cause = "user_disabled",
            lost_cas,
            "Session end retroactively re-attributed to user_disabled"
        );

        Ok(Some(session_id.to_string()))
    }
}

/// Which DB-write path `enter_ended_state` should take. The DB write is
/// path-dependent because `on_offline_detected` runs the full atomic
/// bundle (end_session + set_offline + StreamerOffline outbox event) inside
/// `repo.end()` BEFORE calling `enter_ended_state`, while the
/// download-terminal path runs the lighter `end_session_only`.
#[derive(Debug, Clone, Copy)]
enum DbWritePath {
    /// Caller already wrote `end_time` (e.g. via the full atomic bundle in
    /// `on_offline_detected`). `enter_ended_state` only updates in-memory
    /// state and emits the transition.
    Skip,
    /// `enter_ended_state` itself calls `repo.end_session_only` to write
    /// `end_time` without flipping streamer state. Used by the
    /// download-terminal path and the hysteresis-timer path.
    EndSessionOnly,
}

/// Arguments for [`SessionLifecycle::on_live_detected`].
pub struct LiveDetectedArgs<'a> {
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
    pub now: DateTime<Utc>,
}

/// Arguments for [`SessionLifecycle::on_offline_detected`].
pub struct OfflineDetectedArgs<'a> {
    pub streamer_id: &'a str,
    pub streamer_name: &'a str,
    pub session_id: Option<&'a str>,
    pub state_was_live: bool,
    pub clear_errors: bool,
    /// Optional definitive-offline signal that originated this call. Set by
    /// the danmu observer (`DanmuStreamClosed`) and other engine-boundary
    /// detectors that can confidently say "the stream is over." When
    /// `Some`, the lifecycle records the session-end cause as
    /// [`TerminalCause::DefinitiveOffline`] (carrying the signal) instead
    /// of the default [`TerminalCause::StreamerOffline`] — which preserves
    /// the trigger detail in the audit log and downstream telemetry.
    pub signal: Option<crate::session::state::OfflineSignal>,
    pub now: DateTime<Utc>,
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

#[cfg(test)]
mod tests;
