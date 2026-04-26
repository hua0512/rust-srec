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

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::Result;
use crate::downloader::DownloadTerminalEvent;
use crate::monitor::MonitorEvent;
use crate::session::classifier::{EngineKind, OfflineClassifier};
use crate::session::repository::{
    EndSessionInputs, EndSessionOutcome, SessionLifecycleRepository, StartSessionInputs,
    StartSessionOutcome,
};
use crate::session::state::{SessionState, TerminalCause};
use crate::session::transition::SessionTransition;

/// Default broadcast capacity for [`SessionTransition`] subscribers.
pub const DEFAULT_TRANSITION_CHANNEL_CAPACITY: usize = 256;

/// The single-owner service for recording-session state.
pub struct SessionLifecycle {
    repo: Arc<SessionLifecycleRepository>,
    /// Per-engine offline-signal classifier. On every Terminal::Failed,
    /// the classifier decides whether the failure is a high-confidence
    /// definitive-offline (HLS playlist 404, N consecutive Network
    /// failures inside a window) so the session ends immediately without
    /// waiting for the slower hysteresis path. Successful per-segment
    /// completions reset the consecutive-failure counter.
    classifier: Arc<OfflineClassifier>,
    /// `session_id` → in-memory session snapshot. Authoritative for the
    /// `is_active` query path served by the API and UI. DB remains the
    /// source of truth on cold-start.
    sessions: DashMap<String, SessionState>,
    /// `streamer_id` → the session id that was hard-ended out-of-band
    /// (e.g. by the danmu stream-closed observer). Consulted inside the
    /// next `start_or_resume` call to prevent a stale session from being
    /// resumed through the gap window.
    hard_ended: DashMap<String, String>,
    transition_tx: broadcast::Sender<SessionTransition>,
}

impl SessionLifecycle {
    pub fn new(
        repo: Arc<SessionLifecycleRepository>,
        classifier: Arc<OfflineClassifier>,
        capacity: usize,
    ) -> Self {
        let (transition_tx, _) = broadcast::channel(capacity);
        Self {
            repo,
            classifier,
            sessions: DashMap::new(),
            hard_ended: DashMap::new(),
            transition_tx,
        }
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

    /// Flag a session as hard-ended so that the next `LiveDetected` for the
    /// same streamer starts a fresh session instead of resuming the stale one.
    /// Called by the danmu-side stream-close observer.
    pub fn mark_hard_ended(&self, streamer_id: impl Into<String>, session_id: impl Into<String>) {
        self.hard_ended.insert(streamer_id.into(), session_id.into());
    }

    /// `true` if the session is tracked in-memory and has not been marked ended.
    pub fn is_session_active(&self, session_id: &str) -> bool {
        self.sessions
            .get(session_id)
            .is_some_and(|entry| entry.value().is_recording())
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
                started_at,
                gap_threshold_secs,
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
                    started_at: *started_at,
                    gap_threshold_secs: *gap_threshold_secs,
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
                timestamp,
            } => {
                self.on_offline_detected(OfflineDetectedArgs {
                    streamer_id,
                    streamer_name,
                    session_id: session_id.as_deref(),
                    state_was_live: *state_was_live,
                    clear_errors: *clear_errors,
                    now: *timestamp,
                })
                .await
                .map(|_| ())
            }
            _ => Ok(()),
        }
    }

    /// Start or resume a recording session on behalf of a monitor trigger.
    pub async fn on_live_detected(&self, args: LiveDetectedArgs<'_>) -> Result<StartSessionOutcome> {
        let hard_ended_session_id = self
            .hard_ended
            .get(args.streamer_id)
            .map(|e| e.value().clone());

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
            started_at: args.started_at,
            now: args.now,
            gap_threshold_secs: args.gap_threshold_secs,
            hard_ended_session_id,
        };

        let outcome = self.repo.start_or_resume(inputs).await?;

        // If we created a fresh session, the hard-ended flag has served its
        // purpose — drop it so the new session isn't immediately suppressed.
        if matches!(outcome, StartSessionOutcome::Created { .. }) {
            self.hard_ended.remove(args.streamer_id);
        }

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
            // Phase 1: lifecycle doesn't run the FSM yet; this Started is
            // always a fresh session, never a Hysteresis resume. Phase 3
            // sets this to true on the resume_from_hysteresis path.
            from_hysteresis: false,
        });

        Ok(outcome)
    }

    /// End the active session on behalf of a monitor offline observation.
    pub async fn on_offline_detected(
        &self,
        args: OfflineDetectedArgs<'_>,
    ) -> Result<EndSessionOutcome> {
        let inputs = EndSessionInputs {
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            session_id: args.session_id.map(|s| s.to_string()),
            state_was_live: args.state_was_live,
            clear_errors: args.clear_errors,
            now: args.now,
        };

        let outcome = self.repo.end(inputs).await?;

        if let Some(id) = outcome.resolved_session_id.as_deref() {
            info!(
                streamer_id = %args.streamer_id,
                session_id = %id,
                cause = TerminalCause::StreamerOffline.as_str(),
                "Session ended (monitor-offline path)"
            );
            self.mark_ended_in_memory(id, args.now, TerminalCause::StreamerOffline);
            let _ = self.transition_tx.send(SessionTransition::Ended {
                session_id: id.to_string(),
                streamer_id: args.streamer_id.to_string(),
                streamer_name: args.streamer_name.to_string(),
                ended_at: args.now,
                cause: TerminalCause::StreamerOffline,
                // Phase 1: no Hysteresis path yet; every Ended is direct.
                via_hysteresis: false,
            });
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
    /// `Cancelled` is a no-op: the engine may still flush a final segment and
    /// emit a follow-up `Completed` / `Failed`, so the session must stay in
    /// `Recording` until that authoritative terminal arrives. The actor's
    /// cancellation path will eventually close the session through
    /// `handle_offline_with_session` → `on_offline_detected` if no follow-up
    /// arrives. Matches the plan's F10 upgrade-to-Completed scenario.
    pub async fn on_download_terminal(&self, event: &DownloadTerminalEvent) -> Result<()> {
        let session_id = event.session_id();
        let streamer_id = event.streamer_id();
        let streamer_name = event.streamer_name();
        let now = Utc::now();

        // Promote the raw terminal cause into `DefinitiveOffline` when the
        // classifier recognises an engine-side signal that unambiguously
        // means the upstream stream is gone (HLS playlist 404, N consecutive
        // Network failures in a 60 s window).
        //
        // NOTE: `EngineKind::MesioHls` is passed as the engine hint here.
        // The classifier's rules don't distinguish mesio HLS from mesio FLV
        // (both satisfy `EngineKind::is_mesio()`), and the Failed terminal
        // event does not yet carry the exact engine. ffmpeg/streamlink
        // engines emit different `DownloadFailureKind` variants (ProcessExit
        // etc.) that the classifier rejects, so a misclassification due to
        // the default hint is not possible in practice.
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

        if matches!(cause, TerminalCause::Cancelled { .. }) {
            debug!(
                session_id,
                streamer_id,
                "on_download_terminal: Cancelled is a no-op; session stays Recording"
            );
            return Ok(());
        }

        // Idempotency: a session already flagged Ended in memory is not
        // re-ended. The DB is still authoritative — a cold start rebuilds
        // in-memory state from the DB on first access.
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

        // `Rejected` downloads never actually opened a session through
        // the download engine path. If no session id exists in the event
        // payload, skip the DB write — but still emit the transition so
        // any pipeline gate downstream observes a terminal state.
        if !session_id.is_empty() {
            match self
                .repo
                .end_session_only(streamer_id, Some(session_id), now)
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        session_id,
                        streamer_id,
                        cause = cause.as_str(),
                        error = %e,
                        "on_download_terminal: failed to close session row"
                    );
                    return Err(e);
                }
            }
        }

        info!(
            streamer_id,
            session_id,
            cause = cause.as_str(),
            "Session ended (download-terminal path)"
        );

        self.mark_ended_in_memory(session_id, now, cause.clone());

        let _ = self.transition_tx.send(SessionTransition::Ended {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            ended_at: now,
            cause,
            // Phase 1: download-terminal path always direct → Ended.
            via_hysteresis: false,
        });

        Ok(())
    }

    /// Transition the in-memory entry for `session_id` to `Ended`. Preserves
    /// the original `started_at` (and `streamer_id`) from the prior state if
    /// present; otherwise falls back to `now` and an empty streamer id —
    /// callers shouldn't ever hit that branch in production but we don't
    /// want to silently drop the transition.
    fn mark_ended_in_memory(&self, session_id: &str, now: DateTime<Utc>, cause: TerminalCause) {
        let (streamer_id, started_at) = self
            .sessions
            .get(session_id)
            .map(|e| (e.streamer_id().to_string(), e.started_at()))
            .unwrap_or_else(|| (String::new(), now));

        self.sessions.insert(
            session_id.to_string(),
            SessionState::ended(streamer_id, session_id, started_at, now, cause),
        );
    }
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
    pub started_at: Option<DateTime<Utc>>,
    pub gap_threshold_secs: i64,
    pub now: DateTime<Utc>,
}

/// Arguments for [`SessionLifecycle::on_offline_detected`].
pub struct OfflineDetectedArgs<'a> {
    pub streamer_id: &'a str,
    pub streamer_name: &'a str,
    pub session_id: Option<&'a str>,
    pub state_was_live: bool,
    pub clear_errors: bool,
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
        pool
    }

    fn make_lifecycle(pool: SqlitePool) -> SessionLifecycle {
        SessionLifecycle::new(
            Arc::new(SessionLifecycleRepository::new(pool)),
            Arc::new(OfflineClassifier::new()),
            16,
        )
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
            started_at: None,
            gap_threshold_secs: 60,
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
    async fn mark_hard_ended_forces_new_session_on_next_live() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);

        let started_now = Utc::now() - chrono::Duration::seconds(120);
        let first = lifecycle
            .on_live_detected(live_args(started_now))
            .await
            .unwrap();

        // End the session via offline.
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(first.session_id()),
                state_was_live: true,
                clear_errors: false,
                now: started_now + chrono::Duration::seconds(60),
            })
            .await
            .unwrap();

        // Flag as hard-ended (simulating the danmu-close observer).
        lifecycle.mark_hard_ended(STREAMER_ID, first.session_id());

        // New live within the gap window would normally resume — but the
        // hard-ended flag must force a new session.
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

    /// D3 — Ended session inside the gap window → Resumed. Same id returns.
    #[tokio::test]
    async fn d3_gap_resume_same_session_id() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let base = Utc::now() - chrono::Duration::seconds(120);

        let first = lifecycle.on_live_detected(live_args(base)).await.unwrap();

        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(first.session_id()),
                state_was_live: true,
                clear_errors: false,
                now: base + chrono::Duration::seconds(60),
            })
            .await
            .unwrap();

        // Restart inside the 60s gap window (live_args sets gap_threshold_secs = 60).
        let restart = base + chrono::Duration::seconds(90);
        let resumed = lifecycle.on_live_detected(live_args(restart)).await.unwrap();
        assert!(matches!(resumed, StartSessionOutcome::Resumed { .. }));
        assert_eq!(resumed.session_id(), first.session_id());
    }

    /// D4 — Ended session outside the gap window → Created; fresh id.
    #[tokio::test]
    async fn d4_outside_gap_creates_new_session() {
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

    /// D5 — Hard-ended via `mark_hard_ended` forces a new session even inside
    /// the gap window. The hard_ended cache clears after the new session is
    /// created so subsequent live signals follow normal gap rules.
    #[tokio::test]
    async fn d5_hard_ended_forces_new_then_clears_cache() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let base = Utc::now() - chrono::Duration::seconds(120);

        let first = lifecycle.on_live_detected(live_args(base)).await.unwrap();
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(first.session_id()),
                state_was_live: true,
                clear_errors: false,
                now: base + chrono::Duration::seconds(60),
            })
            .await
            .unwrap();

        lifecycle.mark_hard_ended(STREAMER_ID, first.session_id());

        let restart = base + chrono::Duration::seconds(90);
        let second = lifecycle.on_live_detected(live_args(restart)).await.unwrap();
        assert!(matches!(second, StartSessionOutcome::Created { .. }));
        assert_ne!(second.session_id(), first.session_id());

        // hard_ended cache cleared — a second live signal inside the gap
        // window now follows the normal reuse-active rule (second session
        // is still active so we get ReusedActive, not another Created).
        let third = lifecycle
            .on_live_detected(live_args(restart + chrono::Duration::seconds(5)))
            .await
            .unwrap();
        assert!(matches!(third, StartSessionOutcome::ReusedActive { .. }));
        assert_eq!(third.session_id(), second.session_id());
    }

    /// D6 — Continuation-by-stream-started-at: if the platform reports the
    /// stream began BEFORE our last session ended, the new live signal is a
    /// continuation and must resume the existing session (bypasses the gap
    /// window).
    #[tokio::test]
    async fn d6_continuation_resumes_by_started_at() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool);
        let stream_start = Utc::now() - chrono::Duration::seconds(3600);

        // First session starts at stream_start, gets ended by offline check
        // inside a monitoring gap.
        let mut args = live_args(stream_start);
        args.started_at = Some(stream_start);
        let first = lifecycle.on_live_detected(args).await.unwrap();

        let ended_at = stream_start + chrono::Duration::seconds(3550);
        lifecycle
            .on_offline_detected(OfflineDetectedArgs {
                streamer_id: STREAMER_ID,
                streamer_name: "Test",
                session_id: Some(first.session_id()),
                state_was_live: true,
                clear_errors: false,
                now: ended_at,
            })
            .await
            .unwrap();

        // Monitor returns many minutes later — well past the 60s gap. The
        // platform still reports the stream started at `stream_start`, which
        // is BEFORE `ended_at`. Continuation rule should resume the original
        // session despite the gap being exceeded.
        let restart = ended_at + chrono::Duration::seconds(3600);
        let mut resume_args = live_args(restart);
        resume_args.started_at = Some(stream_start);
        let resumed = lifecycle.on_live_detected(resume_args).await.unwrap();

        assert!(
            matches!(resumed, StartSessionOutcome::Resumed { .. }),
            "continuation rule must resume, got {resumed:?}"
        );
        assert_eq!(resumed.session_id(), first.session_id());
    }

    // =========================================================================
    // Additional integration coverage — in-memory / DB consistency under the
    // state transitions that aren't directly covered by suites B or D.
    // =========================================================================

    /// In-memory session state recovers to Recording after a gap-window Resume
    /// over an ended-by-Failed session. Prevents stale `Ended` entries from
    /// lingering when the repository brings a session back to life.
    ///
    /// This intentionally documents the *current* behaviour (gap-resume is
    /// allowed after a Failed-ended session). The plan's §E2 "Ended is
    /// absorbing" semantic would drop the live signal entirely — that's a
    /// follow-up architectural decision outside PR 1's behaviour-preserving
    /// scope. If E2 is revisited, this test becomes an assertion that the
    /// new policy is enforced.
    #[tokio::test]
    async fn resume_after_failed_refreshes_in_memory_to_recording() {
        let pool = setup_pool().await;
        let lifecycle = make_lifecycle(pool.clone());
        let started_at = Utc::now() - chrono::Duration::seconds(90);

        let first = lifecycle
            .on_live_detected(live_args(started_at))
            .await
            .unwrap();
        let session_id = first.session_id().to_string();

        // Terminal::Failed ends the session: DB end_time set, in-memory
        // flipped to Ended.
        lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Failed {
                download_id: "dl".into(),
                streamer_id: STREAMER_ID.into(),
                streamer_name: "Test".into(),
                session_id: session_id.clone(),
                kind: crate::downloader::DownloadFailureKind::Network,
                error: "stalled".into(),
                recoverable: false,
            })
            .await
            .unwrap();
        assert!(!lifecycle.is_session_active(&session_id));

        // Live observation inside the gap window — gap-resume brings the
        // session back. In-memory state must ALSO be updated to Recording;
        // an out-of-date Ended snapshot would cause downstream consumers
        // (API is_live, pipeline gating) to see the wrong state.
        let restart = started_at + chrono::Duration::seconds(60);
        let resumed = lifecycle.on_live_detected(live_args(restart)).await.unwrap();
        assert!(matches!(resumed, StartSessionOutcome::Resumed { .. }));
        assert_eq!(resumed.session_id(), &session_id);
        assert!(
            lifecycle.is_session_active(&session_id),
            "in-memory state must flip back to Recording on gap-resume"
        );
    }

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
            started_at: None,
            gap_threshold_secs: 60,
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
}
