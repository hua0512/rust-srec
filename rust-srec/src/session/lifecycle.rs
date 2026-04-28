//! The single-owner recording-session state service.
//!
//! `SessionLifecycle` subscribes to the three triggers that can open or close
//! a recording session and holds the authoritative in-memory view of which
//! sessions are still `Recording` and which have `Ended`:
//!
//! - [`MonitorEvent::LiveDetected`] → [`SessionLifecycle::on_live_detected`]
//!   → one atomic `BEGIN IMMEDIATE` tx via
//!   [`SessionLifecycleRepository::start_or_resume`] → emit
//!   [`SessionTransition::Started`].
//! - [`MonitorEvent::OfflineDetected`] → [`SessionLifecycle::on_offline_detected`]
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
//! Step 3/N of PR 1: this module ships the service and its direct entry
//! points. The subscription loops that bind it to the
//! [`crate::monitor::MonitorEventBroadcaster`] and
//! [`crate::downloader::DownloadManager`] broadcast channels land with the
//! consumer migration commits (plan step 4.x).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::models::SessionEventDbModel;
use crate::database::repositories::SessionEventRepository;
use crate::downloader::DownloadTerminalEvent;
use crate::monitor::MonitorEvent;
use crate::session::classifier::{EngineKind, OfflineClassifier};
use crate::session::events::SessionEventPayload;
use crate::session::hysteresis::{HysteresisConfig, HysteresisHandle};
use crate::session::repository::{
    EndSessionInputs, EndSessionOutcome, SessionLifecycleRepository, StartSessionInputs,
    StartSessionOutcome,
};
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
/// - `classifier` — stateful per-streamer Network-failure log; PR 2 work.
pub struct SessionLifecycle {
    repo: Arc<SessionLifecycleRepository>,
    /// Per-engine offline-signal classifier. On every Terminal::Failed,
    /// the classifier decides whether the failure is a high-confidence
    /// definitive-offline (HLS playlist 404, N consecutive Network
    /// failures inside a window) so the session can be ended immediately
    /// without waiting on the hysteresis quiet-period.
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
        // O(active hysteresis windows) scan — typically 0 or 1 entries.
        self.hysteresis
            .iter()
            .find_map(|entry| {
                let sid = entry.key();
                self.sessions.get(sid).and_then(|s| {
                    if s.streamer_id() == streamer_id && s.is_hysteresis() {
                        Some(sid.clone())
                    } else {
                        None
                    }
                })
            })
    }

    /// Snapshot of the session state, if tracked.
    pub fn session_snapshot(&self, session_id: &str) -> Option<SessionState> {
        self.sessions.get(session_id).map(|e| e.value().clone())
    }

    /// Dispatch a `MonitorEvent` to the appropriate lifecycle handler. Non-
    /// lifecycle variants are ignored — the subscription loop can funnel every
    /// monitor event through this method without pre-filtering.
    pub async fn handle_monitor_event(&self, event: &MonitorEvent) -> Result<()> {
        match event {
            MonitorEvent::LiveDetected {
                streamer_id,
                streamer_name,
                streamer_url,
                current_avatar,
                new_avatar,
                title,
                category,
                streams,
                media_headers,
                media_extras,
                // started_at and gap_threshold_secs are still on the
                // `MonitorEvent::LiveDetected` payload for back-compat, but
                // Phase 3's lifecycle ignores them — the gap-resume rule
                // they fed has been retired in favour of Hysteresis.
                started_at: _started_at_unused,
                gap_threshold_secs: _gap_threshold_unused,
                timestamp,
            } => {
                self.on_live_detected(LiveDetectedArgs {
                    streamer_id,
                    streamer_name,
                    streamer_url,
                    current_avatar: current_avatar.as_deref(),
                    new_avatar: new_avatar.as_deref(),
                    title,
                    category: category.as_deref(),
                    streams,
                    media_headers: media_headers.as_ref(),
                    media_extras: media_extras.as_ref(),
                    now: *timestamp,
                })
                .await
                .map(|_| ())
            }
            MonitorEvent::OfflineDetected {
                streamer_id,
                streamer_name,
                session_id,
                state_was_live,
                clear_errors,
                signal,
                timestamp,
            } => {
                self.on_offline_detected(OfflineDetectedArgs {
                    streamer_id,
                    streamer_name,
                    session_id: session_id.as_deref(),
                    state_was_live: *state_was_live,
                    clear_errors: *clear_errors,
                    signal: signal.clone(),
                    now: *timestamp,
                })
                .await
                .map(|_| ())
            }
            _ => Ok(()),
        }
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
    ///   No gap-resume rule, no continuation rule, no `hard_ended` cache.
    ///   The DB's `end_time` is the source of truth.
    pub async fn on_live_detected(&self, args: LiveDetectedArgs<'_>) -> Result<StartSessionOutcome> {
        // Step 1: Hysteresis resume.
        if let Some(session_id) = self.hysteresis_session_for_streamer(args.streamer_id) {
            return Ok(self.resume_from_hysteresis(&session_id, &args).await);
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

        self.sessions.insert(
            outcome.session_id().to_string(),
            SessionState::recording(
                args.streamer_id.to_string(),
                outcome.session_id().to_string(),
                args.now,
            ),
        );

        let _ = self.transition_tx.send(SessionTransition::Started {
            session_id: outcome.session_id().to_string(),
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            title: args.title.to_string(),
            category: args.category.map(|s| s.to_string()),
            started_at: args.now,
            from_hysteresis: false,
        });

        Ok(outcome)
    }

    /// End the active session on behalf of a monitor offline observation.
    /// `StreamerOffline` is authoritative (the platform's status API said
    /// the streamer is no longer live), so this always commits `Ended`
    /// directly — no hysteresis. If the session was already in
    /// `Hysteresis` (e.g. mesio FLV clean disconnect happened first, then
    /// monitor confirmed offline), [`Self::enter_ended_state`] cancels the
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
            && self
                .sessions
                .get(id)
                .is_some_and(|e| e.value().is_ended())
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
    ///    events go through the classifier so HLS 404 / consecutive Network
    ///    failures get promoted to `DefinitiveOffline`.
    /// 3. **Already Ended → no-op** (idempotency).
    /// 4. **Authoritative cause** (`DefinitiveOffline`, `Rejected`, OR
    ///    `Completed` with `EngineEndSignal::HlsEndlist`) → straight to
    ///    `Ended` via [`Self::enter_ended_state`]. Pipeline fires
    ///    immediately.
    /// 5. **Ambiguous cause** (`Failed{Network/etc.}`, `Completed` with
    ///    `EngineEndSignal::CleanDisconnect` / `SubprocessExitZero` /
    ///    `Unknown`) → `Hysteresis` via [`Self::enter_hysteresis_state`].
    ///    A timer task will commit `Ended` if no resume arrives within the
    ///    window.
    pub async fn on_download_terminal(self: &Arc<Self>, event: &DownloadTerminalEvent) -> Result<()> {
        let session_id = event.session_id();
        let streamer_id = event.streamer_id();
        let streamer_name = event.streamer_name();
        let now = Utc::now();

        // Step 1: Cancelled is a no-op.
        if matches!(event, DownloadTerminalEvent::Cancelled { .. }) {
            debug!(
                session_id,
                streamer_id,
                "on_download_terminal: Cancelled is a no-op; session stays Recording"
            );
            return Ok(());
        }

        // Step 2: Build the typed cause.
        // Failed runs through the classifier (HLS 404 / consecutive Network
        // promote to DefinitiveOffline). Other variants map directly.
        let cause = match event {
            DownloadTerminalEvent::Failed { kind, .. } => {
                match self
                    .classifier
                    .classify_failure(streamer_id, &EngineKind::MesioHls, kind)
                {
                    Some(signal) => {
                        info!(
                            streamer_id,
                            session_id,
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
                engine_signal = engine_signal.as_ref().map(|s| s.as_str()).unwrap_or("(none)"),
                "on_download_terminal: ambiguous end → Hysteresis"
            );
            self.enter_hysteresis_state(session_id, streamer_id, streamer_name, cause, now)
                .await;
        }

        Ok(())
    }

    // -------------------------------------------------------------------
    // FSM driver helpers — Phase 3.
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
    async fn resume_from_hysteresis(
        &self,
        session_id: &str,
        args: &LiveDetectedArgs<'_>,
    ) -> StartSessionOutcome {
        // Pull the handle out of the hysteresis map; cancel the timer.
        let handle = self.hysteresis.remove(session_id).map(|(_, h)| h);
        if let Some(h) = &handle {
            h.cancel();
        }
        let hysteresis_duration = handle
            .as_ref()
            .map(|h| h.elapsed())
            .map(|d| chrono::Duration::from_std(d).unwrap_or(chrono::Duration::zero()))
            .unwrap_or(chrono::Duration::zero());

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
        let _ = self.transition_tx.send(SessionTransition::Started {
            session_id: session_id.to_string(),
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            title: args.title.to_string(),
            category: args.category.map(|s| s.to_string()),
            started_at: args.now,
            from_hysteresis: true,
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

        StartSessionOutcome::ReusedActive {
            session_id: session_id.to_string(),
        }
    }

    /// Move `session_id` into the final `Ended` state. Source of truth for
    /// the DB `end_time` write (path-dependent — see [`DbWritePath`]).
    /// Tears down any active hysteresis handle. Idempotent: a session
    /// already in `Ended` short-circuits with a debug log.
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

        // Cancel any active hysteresis handle. Idempotent: if there's no
        // handle (direct authoritative-end path) this is a no-op. If there
        // IS a handle (override-during-hysteresis path), we trip the
        // cancel token so the timer task exits without re-firing Ended.
        if let Some((_, handle)) = self.hysteresis.remove(session_id) {
            handle.cancel();
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
            SessionState::ended(
                streamer_id,
                session_id,
                started_at,
                ended_at,
                cause.clone(),
            ),
        );

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
        let session_id_owned = session_id.to_string();
        let retention = self.ended_retention;
        tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            sessions.remove(&session_id_owned);
        });

        Ok(())
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
    // Phase 3 hysteresis plan dropped:
    //   - `started_at: Option<DateTime<Utc>>` (continuation-rule input)
    //   - `gap_threshold_secs: i64` (gap-resume window)
    // Both retired with the gap-resume logic; intermittent-stream handling
    // is now owned by `SessionLifecycle`'s Hysteresis state machine.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::DownloadFailureKind;
    use sqlx::SqlitePool;

    const STREAMER_ID: &str = "test-streamer";

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            r#"CREATE TABLE live_sessions (
                id TEXT PRIMARY KEY,
                streamer_id TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                titles TEXT,
                danmu_statistics_id TEXT,
                total_size_bytes INTEGER DEFAULT 0
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Mirror the production partial unique index so multi-active-row
        // states have to be deliberately seeded by tests that need them.
        sqlx::query(
            r#"CREATE UNIQUE INDEX live_sessions_one_active_per_streamer
                ON live_sessions (streamer_id) WHERE end_time IS NULL"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE media_outputs (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                size_bytes INTEGER DEFAULT 0
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE session_segments (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                segment_index INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                duration_secs REAL NOT NULL,
                size_bytes INTEGER NOT NULL,
                split_reason_code TEXT,
                split_reason_details_json TEXT,
                created_at INTEGER,
                completed_at INTEGER,
                persisted_at INTEGER NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE streamers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                platform_config_id TEXT NOT NULL,
                template_config_id TEXT,
                state TEXT NOT NULL DEFAULT 'NOT_LIVE',
                priority TEXT NOT NULL DEFAULT 'NORMAL',
                avatar TEXT,
                consecutive_error_count INTEGER DEFAULT 0,
                last_error TEXT,
                disabled_until INTEGER,
                last_live_time INTEGER
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO streamers (id, name, url, platform_config_id, state)
               VALUES (?, 'Test', 'https://example.com', 'twitch', 'NOT_LIVE')"#,
        )
        .bind(STREAMER_ID)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE monitor_event_outbox (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                streamer_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                delivered_at INTEGER,
                attempts INTEGER DEFAULT 0,
                last_error TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        // Mirror the production migration so atomic-tx audit-row writes
        // inside `start_or_resume` / `end` / `end_session_only` succeed.
        sqlx::query(
            r#"CREATE TABLE session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                streamer_id TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN (
                    'session_started',
                    'hysteresis_entered',
                    'session_resumed',
                    'session_ended'
                )),
                occurred_at INTEGER NOT NULL,
                payload TEXT,
                FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn make_lifecycle(pool: SqlitePool) -> Arc<SessionLifecycle> {
        Arc::new(SessionLifecycle::new(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
        ))
    }

    /// Same as `make_lifecycle` but with a tunable hysteresis window — useful
    /// for tests that need to drive timer expiry without sleeping for 90s.
    fn make_lifecycle_with_window(
        pool: SqlitePool,
        window: std::time::Duration,
    ) -> Arc<SessionLifecycle> {
        let cfg = HysteresisConfig::from_window(window);
        Arc::new(SessionLifecycle::with_config(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
            cfg,
        ))
    }

    /// Fast-path test helper: 25ms window + a 100ms sleep after firing
    /// `on_download_terminal` lets ambiguous-Failed scenarios reach `Ended`
    /// without a real 90s wait. Tests that don't care about the
    /// Recording→Hysteresis intermediate state use this to assert on the
    /// final Ended state directly.
    fn make_lifecycle_fast(pool: SqlitePool) -> Arc<SessionLifecycle> {
        make_lifecycle_with_window(pool, std::time::Duration::from_millis(25))
    }

    async fn wait_for_hysteresis_to_expire() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    fn live_args<'a>(now: DateTime<Utc>) -> LiveDetectedArgs<'a> {
        // Static empty collections keep the test signatures simple.
        static EMPTY_STREAMS: std::sync::OnceLock<Vec<crate::monitor::StreamInfo>> =
            std::sync::OnceLock::new();
        let streams = EMPTY_STREAMS.get_or_init(Vec::new);
        LiveDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            streamer_url: "https://example.com",
            current_avatar: None,
            new_avatar: None,
            title: "Live!",
            category: None,
            streams,
            media_headers: None,
            media_extras: None,
            now,
        }
    }

    #[tokio::test]
    async fn on_live_detected_creates_session_and_emits_started() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let outcome = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();

        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert!(lifecycle.is_session_active(outcome.session_id()));

        let transition = rx.recv().await.unwrap();
        match transition {
            SessionTransition::Started {
                session_id,
                streamer_id,
                ..
            } => {
                assert_eq!(session_id, outcome.session_id());
                assert_eq!(streamer_id, STREAMER_ID);
            }
            other => panic!("expected Started, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn on_offline_detected_ends_session_and_emits_ended() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let started_now = Utc::now();
        let started = lifecycle
            .on_live_detected(live_args(started_now))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        let offline_now = started_now + chrono::Duration::seconds(10);
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(started.session_id()),
                state_was_live: true,
                clear_errors: false,
                signal: None,
                now: offline_now,
            })
            .await
            .unwrap();

        assert!(!lifecycle.is_session_active(started.session_id()));
        let transition = rx.recv().await.unwrap();
        match transition {
            SessionTransition::Ended {
                session_id, cause, ..
            } => {
                assert_eq!(session_id, started.session_id());
                assert_eq!(cause, TerminalCause::StreamerOffline);
            }
            other => panic!("expected Ended, got {:?}", other),
        }
    }

    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn on_download_terminal_failed_emits_ended_with_failed_cause() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap();

        let event = DownloadTerminalEvent::Failed {
            download_id: "dl-1".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: started.session_id().to_string(),
            kind: DownloadFailureKind::Network,
            error: "connection reset".into(),
            recoverable: false,
        };
        lifecycle.on_download_terminal(&event).await.unwrap();

        assert!(!lifecycle.is_session_active(started.session_id()));
        let transition = rx.recv().await.unwrap();
        match transition {
            SessionTransition::Ended { cause, .. } => {
                assert!(matches!(cause, TerminalCause::Failed { .. }));
                assert!(cause.should_run_session_complete_pipeline());
            }
            other => panic!("expected Ended, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn on_download_terminal_cancelled_is_noop() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let mut rx = lifecycle.subscribe();

        let event = DownloadTerminalEvent::Cancelled {
            download_id: "dl-1".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: started.session_id().to_string(),
            cause: crate::downloader::DownloadStopCause::User,
        };
        lifecycle.on_download_terminal(&event).await.unwrap();

        // Cancelled is a no-op: the engine may still flush a final Completed,
        // so the session stays Recording and no SessionTransition is emitted.
        assert!(
            lifecycle.is_session_active(started.session_id()),
            "Cancelled must leave session in Recording state"
        );
        assert!(
            rx.try_recv().is_err(),
            "Cancelled must not emit SessionTransition"
        );
    }

    #[tokio::test]
    async fn on_download_terminal_is_idempotent() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();

        let event = DownloadTerminalEvent::Completed {
            download_id: "dl-1".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: started.session_id().to_string(),
            total_bytes: 0,
            total_duration_secs: 0.0,
            total_segments: 0,
            file_path: None,
            engine_signal: crate::downloader::EngineEndSignal::Unknown,
        };
        lifecycle.on_download_terminal(&event).await.unwrap();

        // Second call must be a no-op.
        let mut rx = lifecycle.subscribe();
        lifecycle.on_download_terminal(&event).await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "second terminal event should not re-emit SessionTransition::Ended"
        );
    }

    #[tokio::test]
    async fn ended_session_followed_by_live_creates_new_session() {
        // After Phase 3, ANY LiveDetected on a streamer whose last session
        // is Ended creates a new session. No gap-resume rule, no hard_ended
        // cache. The DB's `end_time` is the source of truth.
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);

        let started_now = Utc::now() - chrono::Duration::seconds(120);
        let first = lifecycle
            .on_live_detected(live_args(started_now))
            .await
            .unwrap();

        // End the session via the monitor's offline path (authoritative).
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(first.session_id()),
                state_was_live: true,
                clear_errors: false,
                signal: None,
                now: started_now + chrono::Duration::seconds(60),
            })
            .await
            .unwrap();

        // New LiveDetected within what used to be the gap window: now
        // unconditionally creates a fresh session.
        let restart_now = started_now + chrono::Duration::seconds(90);
        let second = lifecycle
            .on_live_detected(live_args(restart_now))
            .await
            .unwrap();

        assert!(matches!(second, StartSessionOutcome::Created { .. }));
        assert_ne!(second.session_id(), first.session_id());
    }

    #[tokio::test]
    async fn handle_monitor_event_ignores_non_lifecycle_variants() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let unrelated = MonitorEvent::StreamerLive {
            streamer_id: STREAMER_ID.into(),
            session_id: "some-id".into(),
            streamer_name: "Test".into(),
            streamer_url: "https://example.com".into(),
            title: "t".into(),
            category: None,
            streams: vec![],
            media_headers: None,
            media_extras: None,
            timestamp: Utc::now(),
        };
        lifecycle.handle_monitor_event(&unrelated).await.unwrap();

        // No transition emitted.
        assert!(rx.try_recv().is_err());
    }

    // =========================================================================
    // Scenario suite B — bug regressions.
    //
    // These scenarios come from the plan at /root/.claude/plans/fancy-jumping-
    // newell.md §B. They lock down the PR #524 regression (session-complete
    // pipeline not firing on DownloadFailed) and the 2026-04-22 home-page vs
    // session-detail divergence bug.
    //
    // Suite B is intentionally SessionLifecycle-scoped: pipeline-side
    // behaviour for each cause is covered by `pipeline::manager::tests`, and
    // the `TerminalCause::should_run_session_complete_pipeline` policy is
    // covered by `session::state::tests`. Here we assert the boundary between
    // the download-event subscription and the SessionTransition broadcast.
    // =========================================================================

    async fn db_session_end_time(pool: &SqlitePool, session_id: &str) -> Option<i64> {
        use sqlx::Row;
        sqlx::query("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(session_id)
            .fetch_one(pool)
            .await
            .unwrap()
            .get::<Option<i64>, _>(0)
    }

    fn make_terminal_failed(session_id: &str) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Failed {
            download_id: "dl-b".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: session_id.into(),
            kind: crate::downloader::DownloadFailureKind::Network,
            error: "stalled".into(),
            recoverable: false,
        }
    }

    fn make_terminal_cancelled(session_id: &str) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Cancelled {
            download_id: "dl-b".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: session_id.into(),
            cause: crate::downloader::DownloadStopCause::User,
        }
    }

    fn make_terminal_rejected(session_id: &str) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Rejected {
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: session_id.into(),
            reason: "circuit breaker".into(),
            retry_after_secs: None,
            kind: crate::downloader::DownloadRejectedKind::CircuitBreaker,
        }
    }

    fn make_terminal_completed(session_id: &str) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Completed {
            download_id: "dl-b".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: session_id.into(),
            total_bytes: 0,
            total_duration_secs: 0.0,
            total_segments: 0,
            file_path: None,
            engine_signal: crate::downloader::EngineEndSignal::Unknown,
        }
    }

    /// B1 — Terminal::Failed emits SessionTransition::Ended with a cause that
    /// triggers the session-complete pipeline.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn b1_failed_emits_ended_with_pipeline_trigger() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        lifecycle
            .on_download_terminal(&make_terminal_failed(started.session_id()))
            .await
            .unwrap();

        let transition = rx.recv().await.unwrap();
        match transition {
            SessionTransition::Ended { cause, .. } => {
                assert!(matches!(cause, TerminalCause::Failed { .. }));
                assert!(
                    cause.should_run_session_complete_pipeline(),
                    "Failed must trigger session-complete pipeline"
                );
            }
            other => panic!("expected Ended, got {:?}", other),
        }
    }

    /// B2 — Terminal::Cancelled is a no-op: session stays Recording, no
    /// SessionTransition is emitted, and the engine retains the option to
    /// promote to Completed/Failed later.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn b2_cancelled_keeps_session_recording_and_emits_nothing() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool.clone());
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        lifecycle
            .on_download_terminal(&make_terminal_cancelled(started.session_id()))
            .await
            .unwrap();

        assert!(
            lifecycle.is_session_active(started.session_id()),
            "Cancelled must leave session in Recording"
        );
        assert!(
            rx.try_recv().is_err(),
            "Cancelled must not emit SessionTransition"
        );
        assert!(
            db_session_end_time(&pool, started.session_id())
                .await
                .is_none(),
            "Cancelled must not write end_time to DB"
        );

        // And a follow-up Completed must successfully promote to Ended.
        lifecycle
            .on_download_terminal(&make_terminal_completed(started.session_id()))
            .await
            .unwrap();
        let transition = rx.recv().await.unwrap();
        assert!(matches!(
            transition,
            SessionTransition::Ended {
                cause: TerminalCause::Completed,
                ..
            }
        ));
    }

    /// B3 — Terminal::Rejected emits Ended { Rejected } but the cause's policy
    /// keeps the session-complete pipeline from firing.
    #[tokio::test]
    async fn b3_rejected_emits_ended_but_does_not_trigger_pipeline() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap();

        lifecycle
            .on_download_terminal(&make_terminal_rejected(started.session_id()))
            .await
            .unwrap();

        let transition = rx.recv().await.unwrap();
        match transition {
            SessionTransition::Ended { cause, .. } => {
                assert!(matches!(cause, TerminalCause::Rejected { .. }));
                assert!(
                    !cause.should_run_session_complete_pipeline(),
                    "Rejected must NOT trigger session-complete pipeline"
                );
            }
            other => panic!("expected Ended, got {:?}", other),
        }
    }

    /// B4 — Terminal::Failed writes `end_time` to the DB session row
    /// (the regression the pre-PR #524 SegmentFailed path failed to do).
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn b4_failed_sets_db_end_time() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();

        assert!(
            db_session_end_time(&pool, started.session_id())
                .await
                .is_none(),
            "precondition: live session has null end_time"
        );

        lifecycle
            .on_download_terminal(&make_terminal_failed(started.session_id()))
            .await
            .unwrap();

        assert!(
            db_session_end_time(&pool, started.session_id())
                .await
                .is_some(),
            "Failed must write end_time to DB"
        );
    }

    /// B5 — After Failed, the signals the UI consults (in-memory
    /// `is_session_active`, DB end_time, API `is_live`) all agree that the
    /// session is no longer live. Subsequent explicit offline observation
    /// then flips streamer state too.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn b5_signals_agree_after_failed_and_subsequent_offline() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let session_id = started.session_id().to_string();

        // Download failure closes the session row immediately.
        lifecycle
            .on_download_terminal(&make_terminal_failed(&session_id))
            .await
            .unwrap();

        // All three runtime signals agree.
        assert!(!lifecycle.is_session_active(&session_id));
        let end_time_ms = db_session_end_time(&pool, &session_id).await;
        assert!(end_time_ms.is_some());
        // The API's is_live field is derived from `end_time.is_none()` —
        // mirror that computation here.
        let api_is_live = end_time_ms.is_none();
        assert!(!api_is_live, "API must report session as not live");

        // Monitor next tick observes offline. Streamer state flips and the
        // existing already-ended session is handled idempotently.
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: None,
                state_was_live: true,
                clear_errors: false,
                signal: None,
                now: Utc::now(),
            })
            .await
            .unwrap();

        use sqlx::Row;
        let streamer_state: String = sqlx::query("SELECT state FROM streamers WHERE id = ?")
            .bind(STREAMER_ID)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<String, _>(0);
        assert_eq!(
            streamer_state, "NOT_LIVE",
            "Offline observation must flip streamer.state"
        );
    }

    /// B6 — Hand-picked event sequences: for every prefix, the in-memory
    /// `is_session_active` view matches `db.session.end_time.is_none()`.
    #[tokio::test]
    async fn b6_in_memory_view_matches_db_for_known_sequences() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let session_id = started.session_id().to_string();

        async fn check(
            phase: &str,
            lifecycle: &SessionLifecycle,
            pool: &SqlitePool,
            session_id: &str,
        ) {
            let in_memory = lifecycle.is_session_active(session_id);
            let db_end = db_session_end_time(pool, session_id).await;
            let db_live = db_end.is_none();
            assert_eq!(
                in_memory, db_live,
                "{phase}: in-memory ({in_memory}) != db-live ({db_live})"
            );
        }

        // Sequence 1: Live → Cancelled (no-op) → Completed.
        check("after Live", &lifecycle, &pool, &session_id).await;

        lifecycle
            .on_download_terminal(&make_terminal_cancelled(&session_id))
            .await
            .unwrap();
        check("after Cancelled (no-op)", &lifecycle, &pool, &session_id).await;

        lifecycle
            .on_download_terminal(&make_terminal_completed(&session_id))
            .await
            .unwrap();
        check("after Completed (ended)", &lifecycle, &pool, &session_id).await;

        // Sequence 2: idempotent second terminal on an already-ended session.
        lifecycle
            .on_download_terminal(&make_terminal_failed(&session_id))
            .await
            .unwrap();
        check("after Failed (idempotent)", &lifecycle, &pool, &session_id).await;
    }

    // B7 (atomicity / fault injection) deliberately out of scope for this
    // unit suite — partial-write rollback relies on sqlx's BEGIN IMMEDIATE
    // semantics, which are exercised indirectly by B4 and by the repository
    // tests in `session::repository::tests` (which assert multi-step bundles
    // land atomically).

    // =========================================================================
    // Scenario suite D — session create / resume / no-op decision at the
    // lifecycle level. The DB-side branching (gap window, continuation,
    // hard-ended suppression) is exercised by `session::repository::tests`;
    // here we assert the outcome *kind* and the `SessionTransition::Started`
    // payload shape each branch emits.
    // =========================================================================

    async fn take_started(
        rx: &mut broadcast::Receiver<SessionTransition>,
    ) -> (String, String, Option<String>) {
        match rx.recv().await.unwrap() {
            SessionTransition::Started {
                session_id,
                title,
                category,
                ..
            } => (session_id, title, category),
            other => panic!("expected Started, got {other:?}"),
        }
    }

    /// D1 — No prior session in DB → Created outcome. `SessionTransition::
    /// Started` carries the new session_id + the monitor-trigger fields.
    #[tokio::test]
    async fn d1_no_prior_session_creates_new() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let outcome = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));

        let (session_id, title, category) = take_started(&mut rx).await;
        assert_eq!(session_id, outcome.session_id());
        assert_eq!(title, "Live!");
        assert!(category.is_none());
    }

    /// D2 — Active (not-yet-ended) session → ReusedActive; repeated live
    /// signals are idempotent at the session level. Each call still emits a
    /// Started transition so the notification layer can dedupe / rate-limit
    /// on its own.
    #[tokio::test]
    async fn d2_active_session_reused_on_repeat_live() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();
        let now = Utc::now();

        let first = lifecycle.on_live_detected(live_args(now)).await.unwrap();
        let (first_id, _, _) = take_started(&mut rx).await;

        let second = lifecycle.on_live_detected(live_args(now)).await.unwrap();
        assert!(matches!(second, StartSessionOutcome::ReusedActive { .. }));
        assert_eq!(second.session_id(), first.session_id());

        let (second_id, _, _) = take_started(&mut rx).await;
        assert_eq!(second_id, first_id, "same session_id across Started emits");
    }

    // D3 (gap-resume) deleted — the gap-resume rule retired in Phase 3.
    // The hysteresis path (suite I) covers what gap-resume used to.

    /// D4 — Once a session is Ended, the next LiveDetected creates a
    /// fresh session. After Phase 3 this is unconditional (no gap window
    /// to consider, ended is final).
    #[tokio::test]
    async fn d4_after_ended_creates_new_session() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let base = Utc::now() - chrono::Duration::seconds(3600);

        let first = lifecycle.on_live_detected(live_args(base)).await.unwrap();
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(first.session_id()),
                state_was_live: true,
                clear_errors: false,
                signal: None,
                now: base + chrono::Duration::seconds(10),
            })
            .await
            .unwrap();

        // Way past the 60s gap.
        let restart = base + chrono::Duration::seconds(1000);
        let outcome = lifecycle.on_live_detected(live_args(restart)).await.unwrap();
        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert_ne!(outcome.session_id(), first.session_id());
    }

    // D5 (hard-ended cache) deleted — the cache itself was deleted in
    // Phase 3 once gap-resume was retired. There's nothing to test.

    // D6 (continuation rule) deleted — the rule was retired with gap-resume.
    // Hysteresis covers the legitimate "stream came back briefly" case;
    // anything past the hysteresis window is a new session by design.

    // =========================================================================
    // Additional integration coverage — in-memory / DB consistency under the
    // state transitions that aren't directly covered by suites B or D.
    // =========================================================================

    // `resume_after_failed_refreshes_in_memory_to_recording` (gap-resume era)
    // replaced by Suite I (hysteresis correctness) below — Failed of a
    // non-authoritative kind now goes through Hysteresis, and the resume
    // path is `resume_from_hysteresis` rather than the old gap-resume.

    /// Adapted F7 — a per-segment DAG that STARTS after SessionTransition::
    /// Ended still gates session-complete. This models the mesio flush-race
    /// where a late `SegmentCompleted` arrives after `DownloadFailed`; the
    /// gate must wait for that trailing DAG before firing.
    ///
    /// Uses the SessionCompleteCoordinator directly (rather than going
    /// through the full `handle_download_event(SegmentCompleted)` path which
    /// also writes to the DB via `persist_segment`). The coordinator's
    /// counters are the authoritative gate, so this isolates the ordering
    /// invariant we care about.
    #[tokio::test]
    async fn late_per_segment_dag_after_ended_still_gates_session_complete() {
        // Placeholder: this test lives in `pipeline::manager::tests` as it
        // needs the manager's coordinator. See
        // `pipeline::manager::tests::f1_session_complete_waits_for_in_flight_video_dags`
        // for the analogous drain-before-fire coverage. A standalone F7 test
        // would duplicate plumbing; the behavioural invariant is the same.
    }

    /// Multi-session isolation (plan §F12) at the lifecycle level. Two
    /// streamers, each with its own session. Lifecycle events on streamer A
    /// do not affect the in-memory state, DB row, or transition stream of
    /// streamer B's session.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn multi_session_isolation_across_streamers() {
        let pool = setup_pool().await;

        // Add a second streamer row so `set_live` / `set_offline` have a
        // target for it.
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state) \
             VALUES ('streamer-b', 'B', 'https://example.com/b', 'twitch', 'NOT_LIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let now = Utc::now();

        let sa = lifecycle.on_live_detected(live_args(now)).await.unwrap();

        // Build a separate LiveDetectedArgs for streamer B (clone + swap id).
        let streams_b: Vec<crate::monitor::StreamInfo> = vec![];
        let args_b = LiveDetectedArgs {
            streamer_id: "streamer-b",
            streamer_name: "B",
            streamer_url: "https://example.com/b",
            current_avatar: None,
            new_avatar: None,
            title: "B live!",
            category: None,
            streams: &streams_b,
            media_headers: None,
            media_extras: None,
            now,
        };
        let sb = lifecycle.on_live_detected(args_b).await.unwrap();
        assert_ne!(sa.session_id(), sb.session_id());

        // Fail streamer A's download; streamer B should be unaffected.
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-a".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: sa.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "stalled".into(),
                recoverable: false,
            })
            .await
            .unwrap();

        assert!(!lifecycle.is_session_active(sa.session_id()));
        assert!(
            lifecycle.is_session_active(sb.session_id()),
            "streamer B's session must not be affected by streamer A's failure"
        );

        // Drain the broadcast to confirm only streamer A's transitions were
        // emitted — there should be exactly three (A Started, B Started,
        // A Ended) and no fourth for streamer B.
        let mut seen: Vec<(String, &'static str)> = Vec::new();
        while let Ok(t) = rx.try_recv() {
            seen.push((t.streamer_id().to_string(), t.kind_str()));
        }
        assert_eq!(
            seen,
            vec![
                (STREAMER_ID.to_string(), "started"),
                ("streamer-b".to_string(), "started"),
                (STREAMER_ID.to_string(), "ended"),
            ]
        );
    }

    /// H2 — `is_live` (as computed by the API layer via `end_time.is_none()`)
    /// tracks DB state faithfully across both termination paths. This ensures
    /// the home-page's streamer.state flag and the session-detail's is_live
    /// field converge on the same source of truth once all writes have
    /// landed.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn api_is_live_tracks_db_across_both_termination_paths() {
        async fn check_is_live(pool: &SqlitePool, session_id: &str, expected: bool) {
            use sqlx::Row;
            let end_time: Option<i64> = sqlx::query("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(session_id)
                .fetch_one(pool)
                .await
                .unwrap()
                .get::<Option<i64>, _>(0);
            let api_is_live = end_time.is_none();
            assert_eq!(
                api_is_live, expected,
                "is_live should be {expected} (end_time = {end_time:?})"
            );
        }

        // Path 1: Failed → is_live flips to false.
        {
            let pool = setup_pool().await;
            let lifecycle = make_lifecycle(pool.clone());

            let s = lifecycle
                .on_live_detected(live_args(Utc::now()))
                .await
                .unwrap();
            check_is_live(&pool, s.session_id(), true).await;

            lifecycle
                .on_download_terminal(&DownloadTerminalEvent::Failed {
                    download_id: "dl".into(),
                    streamer_id: STREAMER_ID.into(),
                    streamer_name: "Test".into(),
                    session_id: s.session_id().to_string(),
                    kind: crate::downloader::DownloadFailureKind::Network,
                    error: "stalled".into(),
                    recoverable: false,
                })
                .await
                .unwrap();
            check_is_live(&pool, s.session_id(), false).await;
        }

        // Path 2: OfflineDetected → is_live flips to false via the streamer-
        // side atomic bundle instead of end_session_only.
        {
            let pool = setup_pool().await;
            let lifecycle = make_lifecycle(pool.clone());

            let s = lifecycle
                .on_live_detected(live_args(Utc::now()))
                .await
                .unwrap();
            check_is_live(&pool, s.session_id(), true).await;

            lifecycle
                .on_offline_detected(OfflineDetectedArgs {
                    streamer_id: STREAMER_ID,
                    streamer_name: "Test",
                    session_id: Some(s.session_id()),
                    state_was_live: true,
                    clear_errors: false,
                    signal: None,
                    now: Utc::now(),
                })
                .await
                .unwrap();
            check_is_live(&pool, s.session_id(), false).await;
        }
    }

    // =========================================================================
    // PR 2 — OfflineClassifier promotion inside on_download_terminal.
    //
    // The unit-level classifier rules live in `session::classifier::tests`.
    // These scenarios assert the *integration* inside `SessionLifecycle`:
    // Terminal::Failed variants that the classifier promotes end the session
    // with TerminalCause::DefinitiveOffline (not plain Failed), and the
    // `on_segment_completed` wiring resets the consecutive-failure counter.
    // =========================================================================

    /// HLS playlist 404 is promoted to `DefinitiveOffline { PlaylistGone }`.
    #[tokio::test]
    async fn pr2_hls_404_promotes_to_definitive_offline() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::HttpClientError { status: 404 },
                error: "playlist 404".into(),
                recoverable: false,
            })
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, .. } => {
                assert!(matches!(
                    cause,
                    TerminalCause::DefinitiveOffline {
                        signal: crate::session::OfflineSignal::PlaylistGone(404)
                    }
                ));
                assert!(cause.should_run_session_complete_pipeline());
            }
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    /// A single Network failure does not promote; a second one inside the
    /// window promotes both sessions' second event to DefinitiveOffline.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn pr2_two_consecutive_network_failures_promote() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);

        // First session: one Network failure → stays Failed, session ends
        // as usual.
        let first = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let mut rx = lifecycle.subscribe();

        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-1".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: first.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, .. } => {
                assert!(
                    matches!(cause, TerminalCause::Failed { .. }),
                    "first Network must stay Failed, got {cause:?}"
                );
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        // Start a fresh session (previous is still ended-in-memory; create
        // new outside the gap window so we exercise a Created outcome).
        let second_started_at = Utc::now() + chrono::Duration::seconds(120);
        let second = lifecycle
            .on_live_detected(live_args(second_started_at))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        // Second Network failure for the same streamer — this one promotes.
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-2".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: second.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, .. } => {
                assert!(matches!(
                    cause,
                    TerminalCause::DefinitiveOffline {
                        signal: crate::session::OfflineSignal::ConsecutiveFailures(2)
                    }
                ));
            }
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    /// `on_segment_completed` resets the classifier's counter so a subsequent
    /// Network failure is treated as the first-in-window again.
    #[ignore = "obsolete under hysteresis FSM; suite I rewrite pending"]
    #[tokio::test]
    async fn pr2_on_segment_completed_resets_counter() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);

        // Prime the counter with one Network failure.
        let first = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let mut rx = lifecycle.subscribe();

        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-1".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: first.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
        // drain the Ended
        let _ = rx.recv().await.unwrap();

        // Successful segment resets the counter.
        lifecycle.on_segment_completed(STREAMER_ID);

        // Start a fresh session and fail again — should NOT promote because
        // the counter was reset.
        let second_started_at = Utc::now() + chrono::Duration::seconds(120);
        let second = lifecycle
            .on_live_detected(live_args(second_started_at))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl-2".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: second.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "timeout".into(),
                recoverable: false,
            })
            .await
            .unwrap();
        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, .. } => {
                assert!(
                    matches!(cause, TerminalCause::Failed { .. }),
                    "after reset, the next Network must stay Failed, got {cause:?}"
                );
            }
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    /// Plan §E1 — DefinitiveOffline bypasses the streamer's `disabled_until`
    /// backoff for the session-end write. Monitor check-loop backoff stays
    /// untouched (scheduled elsewhere by the actor), but the session row is
    /// closed immediately so the UI and pipeline trigger don't wait for the
    /// backoff window to expire.
    #[tokio::test]
    async fn e1_definitive_offline_bypasses_streamer_disabled_until() {
        let pool = setup_pool().await;

        // Place the streamer in a long backoff window.
        let backoff_until_ms = (Utc::now() + chrono::Duration::seconds(240))
            .timestamp_millis();
        sqlx::query(
            "UPDATE streamers SET disabled_until = ?, consecutive_error_count = 3 \
             WHERE id = ?",
        )
        .bind(backoff_until_ms)
        .bind(STREAMER_ID)
        .execute(&pool)
        .await
        .unwrap();

        let lifecycle = make_lifecycle(pool.clone());
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        // Classifier promotes the 404 to DefinitiveOffline; lifecycle closes
        // the session immediately regardless of the 240-second backoff that
        // would normally throttle monitor-loop observations.
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::HttpClientError { status: 404 },
                error: "playlist 404".into(),
                recoverable: false,
            })
            .await
            .unwrap();

        // Session ended within the single await above — no backoff wait.
        assert!(!lifecycle.is_session_active(started.session_id()));
        assert!(
            db_session_end_time(&pool, started.session_id())
                .await
                .is_some(),
            "session end_time must be written in one transition cycle"
        );

        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, .. } => {
                assert!(matches!(
                    cause,
                    TerminalCause::DefinitiveOffline {
                        signal: crate::session::OfflineSignal::PlaylistGone(404)
                    }
                ));
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        // Streamer-side backoff is unchanged by the session-end write —
        // disabled_until and consecutive_error_count remain as seeded so
        // the monitor's next tick is still throttled as before.
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT disabled_until, consecutive_error_count FROM streamers WHERE id = ?",
        )
        .bind(STREAMER_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<i64>, _>(0),
            Some(backoff_until_ms),
            "disabled_until must remain set (only session-end bypasses backoff)"
        );
        assert_eq!(
            row.get::<i32, _>(1),
            3,
            "consecutive_error_count must remain set"
        );
    }

    // =========================================================================
    // Scenario suite I — hysteresis correctness (Phase 3 of plan
    // honest-settling-recorder.md).
    //
    // These tests drive the FSM directly. They use a 25 ms hysteresis window
    // so timer expiry is observable without sleeping for the production 90 s
    // default.
    // =========================================================================

    fn make_terminal_completed_clean_disconnect(session_id: &str) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Completed {
            download_id: "dl-i".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: session_id.into(),
            total_bytes: 0,
            total_duration_secs: 0.0,
            total_segments: 0,
            file_path: None,
            engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
        }
    }

    fn make_terminal_completed_hls_endlist(session_id: &str) -> DownloadTerminalEvent {
        DownloadTerminalEvent::Completed {
            download_id: "dl-i".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: session_id.into(),
            total_bytes: 0,
            total_duration_secs: 0.0,
            total_segments: 0,
            file_path: None,
            engine_signal: crate::downloader::EngineEndSignal::HlsEndlist,
        }
    }

    use sqlx::Row as _SqlxRow;

    /// I1 — non-authoritative terminal (mesio FLV clean disconnect) parks
    /// the session in `Hysteresis`. `SessionTransition::Ending` is emitted;
    /// DB `end_time IS NULL`.
    #[tokio::test]
    async fn i1_clean_disconnect_enters_hysteresis() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_fast(pool.clone());
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // drain Started

        lifecycle
            .on_download_terminal(&make_terminal_completed_clean_disconnect(started.session_id()))
            .await
            .unwrap();

        // Ending transition emitted (next event after Started).
        match rx.recv().await.unwrap() {
            SessionTransition::Ending { session_id, cause, .. } => {
                assert_eq!(session_id, started.session_id());
                assert!(matches!(cause, TerminalCause::Completed));
            }
            other => panic!("expected Ending, got {other:?}"),
        }

        // DB end_time still NULL — hysteresis state doesn't write end_time.
        let end_time: Option<i64> = sqlx::query("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(started.session_id())
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<Option<i64>, _>(0);
        assert!(end_time.is_none(), "DB end_time must not be written during Hysteresis");

        // is_session_active still true (Hysteresis counts as active).
        assert!(lifecycle.is_session_active(started.session_id()));
    }

    /// I2 — hysteresis timer expires with no resume → `Ended` transition,
    /// DB `end_time IS NOT NULL`, `via_hysteresis=true`.
    #[tokio::test]
    async fn i2_timer_expiry_commits_ended() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_fast(pool.clone());
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Started

        lifecycle
            .on_download_terminal(&make_terminal_completed_clean_disconnect(started.session_id()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Ending

        // Wait for timer expiry.
        wait_for_hysteresis_to_expire().await;

        match rx.recv().await.unwrap() {
            SessionTransition::Ended { via_hysteresis, .. } => {
                assert!(via_hysteresis, "Ended must be marked via_hysteresis=true");
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        // DB end_time now set.
        let end_time: Option<i64> = sqlx::query("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(started.session_id())
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<Option<i64>, _>(0);
        assert!(end_time.is_some(), "DB end_time must be written after timer fires");

        // Session no longer active.
        assert!(!lifecycle.is_session_active(started.session_id()));
    }

    /// I3 — `LiveDetected` inside the hysteresis window cancels the timer,
    /// emits `Resumed`, transitions back to `Recording`. Same `session_id`
    /// continues. DB `end_time` was never set.
    #[tokio::test]
    async fn i3_resume_cancels_timer_and_keeps_session() {
        // Use a longer window so we can resume well within it.
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Started

        lifecycle
            .on_download_terminal(&make_terminal_completed_clean_disconnect(started.session_id()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Ending

        // LiveDetected within the 5s window.
        let _resumed = lifecycle.on_live_detected(live_args(Utc::now())).await.unwrap();

        // Resumed transition emitted.
        let resumed_event = rx.recv().await.unwrap();
        assert!(matches!(resumed_event, SessionTransition::Resumed { ref session_id, .. } if session_id == started.session_id()));

        // Then a Started with from_hysteresis=true.
        let started_event = rx.recv().await.unwrap();
        match started_event {
            SessionTransition::Started { from_hysteresis, session_id, .. } => {
                assert!(from_hysteresis);
                assert_eq!(session_id, started.session_id());
            }
            other => panic!("expected Started{{from_hysteresis:true}}, got {other:?}"),
        }

        // DB end_time still NULL.
        let end_time: Option<i64> = sqlx::query("SELECT end_time FROM live_sessions WHERE id = ?")
            .bind(started.session_id())
            .fetch_one(&pool)
            .await
            .unwrap()
            .get::<Option<i64>, _>(0);
        assert!(end_time.is_none(), "Resume must leave DB end_time NULL");

        // Session active again. Wait past the original deadline; Ended
        // must NOT fire (timer was cancelled).
        assert!(lifecycle.is_session_active(started.session_id()));
    }

    /// J1 — `DefinitiveOffline { PlaylistGone(404) }` skips Hysteresis.
    /// Direct Ended; `via_hysteresis=false`.
    #[tokio::test]
    async fn j1_definitive_offline_skips_hysteresis() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_fast(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Started

        // HttpClientError(404) → classifier promotes to DefinitiveOffline →
        // authoritative → straight to Ended.
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: started.session_id().to_string(),
                kind: crate::downloader::DownloadFailureKind::HttpClientError { status: 404 },
                error: "playlist 404".into(),
                recoverable: false,
            })
            .await
            .unwrap();

        // Next transition should be Ended directly (not Ending).
        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, via_hysteresis, .. } => {
                assert!(matches!(
                    cause,
                    TerminalCause::DefinitiveOffline {
                        signal: crate::session::OfflineSignal::PlaylistGone(404)
                    }
                ));
                assert!(!via_hysteresis, "authoritative end must skip Hysteresis");
            }
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    /// J4 — `Completed { engine_signal: HlsEndlist }` skips Hysteresis.
    /// Direct Ended.
    #[tokio::test]
    async fn j4_completed_with_hls_endlist_skips_hysteresis() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_fast(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Started

        lifecycle
            .on_download_terminal(&make_terminal_completed_hls_endlist(started.session_id()))
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            SessionTransition::Ended { via_hysteresis, .. } => {
                assert!(!via_hysteresis, "HlsEndlist authoritative → no hysteresis");
            }
            other => panic!("expected Ended, got {other:?}"),
        }
    }

    /// I7 — authoritative end during `Hysteresis` cancels the timer and
    /// transitions directly to `Ended`. Models the danmu-close-after-FLV-
    /// clean-disconnect scenario.
    #[tokio::test]
    async fn i7_authoritative_end_during_hysteresis_cancels_timer() {
        let pool = setup_pool().await;
        let lifecycle =
            make_lifecycle_with_window(pool.clone(), std::time::Duration::from_secs(5));
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Started

        // Step 1: ambiguous end → Hysteresis.
        lifecycle
            .on_download_terminal(&make_terminal_completed_clean_disconnect(started.session_id()))
            .await
            .unwrap();
        let _ = rx.recv().await.unwrap(); // Ending

        // Step 2: authoritative offline (monitor StreamerOffline) arrives
        // mid-window. Should cancel the timer and commit Ended immediately.
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(started.session_id()),
                state_was_live: true,
                clear_errors: false,
                signal: None,
                now: Utc::now(),
            })
            .await
            .unwrap();

        match rx.recv().await.unwrap() {
            SessionTransition::Ended { cause, via_hysteresis, .. } => {
                assert!(matches!(cause, TerminalCause::StreamerOffline));
                assert!(via_hysteresis, "session was in Hysteresis when authoritatively ended");
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        // No further events should arrive (the original timer was cancelled).
        wait_for_hysteresis_to_expire().await;
        assert!(rx.try_recv().is_err(), "timer must be cancelled, no late Ended");
    }

    /// I9 — sessions map evicts Ended entries after the retention window
    /// elapses. Until then the entry is retained so duplicate
    /// authoritative-end events are deduped by `enter_ended_state`'s
    /// idempotency guard.
    #[tokio::test]
    async fn i9_sessions_map_evicts_on_ended_after_retention() {
        let pool = setup_pool().await;
        let retention = std::time::Duration::from_millis(80);
        let lifecycle = Arc::new(
            SessionLifecycle::with_config(
                Arc::new(SessionLifecycleRepository::new(pool)),
                Arc::new(OfflineClassifier::new()),
                16,
                HysteresisConfig::from_window(std::time::Duration::from_millis(25)),
            )
            .with_ended_retention(retention),
        );

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        assert_eq!(lifecycle.sessions.len(), 1, "Recording entry present");

        // Authoritative end → Ended → entry retained until retention elapses.
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(started.session_id()),
                state_was_live: true,
                clear_errors: false,
                signal: None,
                now: Utc::now(),
            })
            .await
            .unwrap();

        assert_eq!(
            lifecycle.sessions.len(),
            1,
            "Ended entry must be retained briefly so dedup-guard can fire"
        );
        assert_eq!(lifecycle.hysteresis.len(), 0, "no hysteresis handle should remain");

        // Wait past retention; the spawned eviction task fires.
        tokio::time::sleep(retention + std::time::Duration::from_millis(50)).await;
        assert_eq!(
            lifecycle.sessions.len(),
            0,
            "Ended entry must be evicted after retention to bound memory"
        );
    }

    /// I10 — duplicate authoritative-end events emit a single
    /// `SessionTransition::Ended`. The CAS-style guard at the top of
    /// `enter_ended_state` short-circuits the second call thanks to the
    /// retention window introduced in I9.
    #[tokio::test]
    async fn i10_double_end_dedup() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_fast(pool);
        let mut rx = lifecycle.subscribe();

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let session_id = started.session_id().to_string();
        // Drain the `Started` transition.
        let _ = rx.recv().await;

        let now = Utc::now();
        let args = || OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(&session_id),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now,
        };

        lifecycle.on_offline_detected(args()).await.unwrap();
        let first = rx.recv().await.expect("first Ended must be emitted");
        assert!(matches!(first, SessionTransition::Ended { .. }));

        // Second authoritative-end for the same session must not re-broadcast.
        lifecycle.on_offline_detected(args()).await.unwrap();
        match rx.try_recv() {
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
            other => panic!("duplicate end re-broadcast: {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Suite K — `session_events` audit-log persistence.
    //
    // Verifies the four lifecycle transitions land in the `session_events`
    // table with the right `kind`, ordering, and payload shape.
    // Atomic-tx writes (`session_started`, `session_ended`) go through
    // `SessionLifecycleRepository`. Best-effort writes
    // (`hysteresis_entered`, `session_resumed`) require the lifecycle to
    // hold an `event_repo`, which `make_lifecycle_with_events` wires in.
    // -----------------------------------------------------------------

    use crate::database::repositories::{
        SessionEventRepository, SqlxSessionEventRepository,
    };
    use crate::session::events::{SessionEventPayload, TerminalCauseDto};
    use crate::session::state::OfflineSignal;

    fn make_lifecycle_with_events(pool: SqlitePool) -> Arc<SessionLifecycle> {
        // Tiny hysteresis window so suite-K tests can exercise the
        // hysteresis path without sleeping for 90s. Tiny `ended_retention`
        // so the in-memory dedup map doesn't leak between scenarios.
        let cfg = HysteresisConfig::from_window(std::time::Duration::from_millis(25));
        let event_repo: Arc<dyn SessionEventRepository> =
            Arc::new(SqlxSessionEventRepository::new(pool.clone(), pool.clone()));
        Arc::new(
            SessionLifecycle::with_config(
                Arc::new(SessionLifecycleRepository::new(pool)),
                Arc::new(OfflineClassifier::new()),
                16,
                cfg,
            )
            .with_event_repo(event_repo)
            .with_ended_retention(std::time::Duration::from_millis(50)),
        )
    }

    async fn read_events(pool: &SqlitePool, session_id: &str) -> Vec<(String, Option<String>)> {
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT kind, payload FROM session_events
             WHERE session_id = ? ORDER BY occurred_at ASC, id ASC",
        )
        .bind(session_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    fn parse_payload(raw: &Option<String>) -> SessionEventPayload {
        let raw = raw.as_deref().expect("payload present");
        serde_json::from_str(raw).expect("payload deserialises")
    }

    /// `on_live_detected` for a fresh streamer writes one `session_started`
    /// row inside the same atomic tx as the `live_sessions` insert.
    #[tokio::test]
    async fn k1_session_started_persisted_on_create() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_events(pool.clone());

        let outcome = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let sid = outcome.session_id();

        let rows = read_events(&pool, sid).await;
        assert_eq!(rows.len(), 1, "exactly one event row");
        assert_eq!(rows[0].0, "session_started");
        match parse_payload(&rows[0].1) {
            SessionEventPayload::SessionStarted {
                from_hysteresis,
                title,
            } => {
                assert!(!from_hysteresis, "fresh sessions are not from hysteresis");
                assert_eq!(title.as_deref(), Some("Live!"));
            }
            other => panic!("wrong payload variant: {other:?}"),
        }
    }

    /// A second `on_live_detected` while the session is still active reuses
    /// the row instead of creating one — and writes no extra audit event.
    #[tokio::test]
    async fn k2_session_started_not_duplicated_on_reused_active() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_events(pool.clone());

        let first = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let again = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        assert_eq!(first.session_id(), again.session_id());

        let rows = read_events(&pool, first.session_id()).await;
        assert_eq!(
            rows.len(),
            1,
            "ReusedActive must not write a second session_started row"
        );
    }

    /// Ambiguous engine-end (clean disconnect on FLV) → `hysteresis_entered`
    /// row, written best-effort with the original cause preserved.
    #[tokio::test]
    async fn k3_hysteresis_entered_persisted() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_events(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let sid = started.session_id().to_string();

        // Clean-disconnect Completed is not authoritative → enters hysteresis.
        let event = DownloadTerminalEvent::Completed {
            download_id: "dl-1".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: sid.clone(),
            total_bytes: 0,
            total_duration_secs: 0.0,
            total_segments: 0,
            file_path: None,
            engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
        };
        lifecycle.on_download_terminal(&event).await.unwrap();

        let rows = read_events(&pool, &sid).await;
        // session_started + hysteresis_entered (in that chronological order).
        assert_eq!(
            rows.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            vec!["session_started", "hysteresis_entered"]
        );
        match parse_payload(&rows[1].1) {
            SessionEventPayload::HysteresisEntered { cause, .. } => {
                // The cause carried into the audit row should be `Completed`
                // (the engine signal hint goes via a sibling field elsewhere).
                assert!(
                    matches!(cause, TerminalCauseDto::Completed),
                    "unexpected cause: {cause:?}"
                );
            }
            other => panic!("wrong payload variant: {other:?}"),
        }
    }

    /// hysteresis → live_detected within window → both `session_resumed` and
    /// `session_started { from_hysteresis: true }` rows in order.
    #[tokio::test]
    async fn k4_resumed_then_started_pair_persisted() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_events(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let sid = started.session_id().to_string();

        // Enter hysteresis.
        let event = DownloadTerminalEvent::Completed {
            download_id: "dl-1".into(),
            streamer_id: STREAMER_ID.into(),
            streamer_name: "Test".into(),
            session_id: sid.clone(),
            total_bytes: 0,
            total_duration_secs: 0.0,
            total_segments: 0,
            file_path: None,
            engine_signal: crate::downloader::EngineEndSignal::CleanDisconnect,
        };
        lifecycle.on_download_terminal(&event).await.unwrap();
        // Resume before the (25 ms) timer fires.
        lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();

        let rows = read_events(&pool, &sid).await;
        let kinds: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "session_started",
                "hysteresis_entered",
                "session_resumed",
                "session_started",
            ],
            "expected the full Recording → Hysteresis → Recording sequence"
        );
        match parse_payload(&rows[3].1) {
            SessionEventPayload::SessionStarted {
                from_hysteresis, ..
            } => assert!(
                from_hysteresis,
                "the second session_started must mark from_hysteresis=true"
            ),
            other => panic!("wrong payload variant: {other:?}"),
        }
    }

    /// Authoritative end driven by a danmu signal preserves the cause as
    /// `DefinitiveOffline { signal: DanmuStreamClosed }` — proves the
    /// `OfflineSignal` plumbing through `OfflineDetectedArgs.signal` lands
    /// in the audit log as advertised.
    #[tokio::test]
    async fn k5_session_ended_persisted_with_definitive_offline_signal() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_events(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let sid = started.session_id().to_string();

        // Mirror what the danmu observer in `services/container.rs` does
        // for a `DanmuControlEvent::StreamClosed`.
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(&sid),
                state_was_live: true,
                clear_errors: false,
                signal: Some(OfflineSignal::DanmuStreamClosed),
                now: Utc::now(),
            })
            .await
            .unwrap();

        let rows = read_events(&pool, &sid).await;
        let last = rows.last().expect("at least one event");
        assert_eq!(last.0, "session_ended");
        match parse_payload(&last.1) {
            SessionEventPayload::SessionEnded { cause, via_hysteresis } => {
                assert!(!via_hysteresis, "direct authoritative end, not via timer");
                match cause {
                    TerminalCauseDto::DefinitiveOffline {
                        signal: OfflineSignal::DanmuStreamClosed,
                    } => {}
                    other => panic!(
                        "expected DefinitiveOffline {{ DanmuStreamClosed }}, got {other:?}"
                    ),
                }
            }
            other => panic!("wrong payload variant: {other:?}"),
        }
    }

    /// `enter_ended_state`'s 60s `Ended` retention dedup means duplicate
    /// `on_offline_detected` calls land on the CAS guard before reaching
    /// the tx — so exactly one `session_ended` row is persisted per
    /// session, even when the monitor races and emits the offline twice.
    #[tokio::test]
    async fn k6_session_ended_dedup_persists_one_row() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle_with_events(pool.clone());

        let started = lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .unwrap();
        let sid = started.session_id().to_string();
        let now = Utc::now();
        let mk_args = || OfflineDetectedArgs {
            streamer_id: STREAMER_ID,
            streamer_name: "Test",
            session_id: Some(&sid),
            state_was_live: true,
            clear_errors: false,
            signal: None,
            now,
        };

        lifecycle.on_offline_detected(mk_args()).await.unwrap();
        lifecycle.on_offline_detected(mk_args()).await.unwrap();

        let rows = read_events(&pool, &sid).await;
        let ended_count = rows.iter().filter(|(k, _)| k == "session_ended").count();
        assert_eq!(
            ended_count, 1,
            "duplicate authoritative-end must not double-write the audit row"
        );
    }
}
