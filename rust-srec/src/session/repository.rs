//! Atomic transactional bundles for session lifecycle operations.
//!
//! [`SessionLifecycleRepository`] is the only place that performs the
//! multi-step DB transactions that combine session-row writes
//! ([`SessionTxOps`]), streamer-state writes ([`StreamerTxOps`]), and
//! monitor-event enqueue ([`MonitorOutboxTxOps`]). Two atomic verbs:
//!
//! - [`SessionLifecycleRepository::start_or_resume`] mirrors
//!   `monitor::service::handle_live`: pick reuse-active / resume / new on
//!   the most-recent session row, write it, mark the streamer Live, and
//!   enqueue [`MonitorEvent::StreamerLive`] — all in one `BEGIN IMMEDIATE`.
//! - [`SessionLifecycleRepository::end`] mirrors
//!   `monitor::service::handle_offline_with_session`: end the session by
//!   id (or fall back to active-by-streamer), mark the streamer Offline,
//!   optionally clear accumulated errors, and (if appropriate) enqueue
//!   [`MonitorEvent::StreamerOffline`] — also one tx.
//!
//! All four pipeline kinds (per-segment video/danmu, paired,
//! session-complete) remain owned by `pipeline::manager`. This module only
//! writes the authoritative session row and the streamer row that the rest
//! of the system reacts to.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use tracing::{debug, info};
use uuid::Uuid;

use crate::Result;
use crate::database::repositories::{MonitorOutboxTxOps, SessionTxOps, StreamerTxOps};
use crate::database::time::ms_to_datetime;
use crate::database::{WritePool, begin_immediate};
use crate::monitor::{MonitorEvent, StreamInfo};

/// Inputs required by [`SessionLifecycleRepository::start_or_resume`].
#[derive(Debug, Clone)]
pub struct StartSessionInputs {
    pub streamer_id: String,
    pub streamer_name: String,
    pub streamer_url: String,
    pub current_avatar: Option<String>,
    pub new_avatar: Option<String>,
    pub title: String,
    pub category: Option<String>,
    pub streams: Vec<StreamInfo>,
    pub media_headers: Option<HashMap<String, String>>,
    pub media_extras: Option<HashMap<String, String>>,
    /// Stream-side `started_at` used by the continuation rule.
    pub started_at: Option<DateTime<Utc>>,
    /// Reference time for the session `start_time` and event `timestamp`.
    pub now: DateTime<Utc>,
    /// Gap window (seconds) for the resume-by-gap rule.
    pub gap_threshold_secs: i64,
    /// Session id that `SessionLifecycle` has observed as hard-ended
    /// (e.g. via danmu stream close). When it matches the most-recent
    /// session in the DB, that session will not be resumed even inside the
    /// gap window; a fresh session is created instead. The match happens
    /// inside the same `BEGIN IMMEDIATE` transaction as the read, so there
    /// is no TOCTOU window against concurrent writers.
    pub hard_ended_session_id: Option<String>,
}

/// Outcome of [`SessionLifecycleRepository::start_or_resume`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartSessionOutcome {
    /// A brand-new session row was inserted.
    Created { session_id: String },
    /// An ended session was reopened (end_time cleared) within the gap or
    /// continuation window.
    Resumed { session_id: String },
    /// The most-recent session row was still active (no end_time); only its
    /// titles and the streamer state were updated.
    ReusedActive { session_id: String },
}

impl StartSessionOutcome {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Created { session_id }
            | Self::Resumed { session_id }
            | Self::ReusedActive { session_id } => session_id,
        }
    }
}

/// Inputs for [`SessionLifecycleRepository::end`].
#[derive(Debug, Clone)]
pub struct EndSessionInputs {
    pub streamer_id: String,
    pub streamer_name: String,
    /// Explicit session id to end; if `None`, falls back to the active
    /// session for `streamer_id` (if any).
    pub session_id: Option<String>,
    /// `true` when the streamer's pre-end state was `Live`. Together with
    /// `resolved_session_id` this drives whether to enqueue an offline event.
    pub state_was_live: bool,
    /// `true` to clear accumulated transient errors on a clean offline obs.
    pub clear_errors: bool,
    pub now: DateTime<Utc>,
}

/// Outcome of [`SessionLifecycleRepository::end`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndSessionOutcome {
    pub resolved_session_id: Option<String>,
    pub offline_event_emitted: bool,
}

/// Atomic transactional bundles for session lifecycle.
pub struct SessionLifecycleRepository {
    write_pool: WritePool,
}

impl SessionLifecycleRepository {
    pub fn new(write_pool: WritePool) -> Self {
        Self { write_pool }
    }

    /// Atomic "session started" bundle: one `BEGIN IMMEDIATE` transaction
    /// that picks the right action against the most-recent session row,
    /// marks the streamer Live, optionally refreshes the avatar, and
    /// enqueues [`MonitorEvent::StreamerLive`].
    pub async fn start_or_resume(
        &self,
        inputs: StartSessionInputs,
    ) -> Result<StartSessionOutcome> {
        let mut tx = begin_immediate(&self.write_pool).await?;

        let last = SessionTxOps::get_last_session(&mut tx, &inputs.streamer_id).await?;

        let outcome = match last {
            Some(session) => match session.end_time {
                None => {
                    debug!("Reusing active session {}", session.id);
                    SessionTxOps::update_titles(
                        &mut tx,
                        &session.id,
                        session.titles.as_deref(),
                        &inputs.title,
                        inputs.now,
                    )
                    .await?;
                    StartSessionOutcome::ReusedActive {
                        session_id: session.id,
                    }
                }
                Some(end_ms) => {
                    let end_time = ms_to_datetime(end_ms);
                    let end_time_str = end_time.to_rfc3339();

                    let is_hard_ended = inputs
                        .hard_ended_session_id
                        .as_deref()
                        .is_some_and(|h| h == session.id);
                    if is_hard_ended {
                        info!(
                            "Creating new session for {} (previous session {} was hard-ended)",
                            inputs.streamer_name, session.id
                        );
                        let new_id = Uuid::new_v4().to_string();
                        SessionTxOps::create_session(
                            &mut tx,
                            &new_id,
                            &inputs.streamer_id,
                            inputs.now,
                            &inputs.title,
                        )
                        .await?;
                        info!("Created new session {}", new_id);
                        StartSessionOutcome::Created { session_id: new_id }
                    } else if SessionTxOps::should_resume_by_continuation(
                        end_time,
                        inputs.started_at,
                    ) {
                        info!(
                            "Resuming session {} (stream started at {:?}, before session end at {})",
                            session.id, inputs.started_at, end_time_str
                        );
                        SessionTxOps::resume_session(&mut tx, &session.id).await?;
                        SessionTxOps::update_titles(
                            &mut tx,
                            &session.id,
                            session.titles.as_deref(),
                            &inputs.title,
                            inputs.now,
                        )
                        .await?;
                        StartSessionOutcome::Resumed {
                            session_id: session.id,
                        }
                    } else if SessionTxOps::should_resume_by_gap(
                        end_time,
                        inputs.now,
                        inputs.gap_threshold_secs,
                    ) {
                        let offline_secs = (inputs.now - end_time).num_seconds();
                        info!(
                            "Resuming session {} (offline for {}s, threshold: {}s)",
                            session.id, offline_secs, inputs.gap_threshold_secs
                        );
                        SessionTxOps::resume_session(&mut tx, &session.id).await?;
                        SessionTxOps::update_titles(
                            &mut tx,
                            &session.id,
                            session.titles.as_deref(),
                            &inputs.title,
                            inputs.now,
                        )
                        .await?;
                        StartSessionOutcome::Resumed {
                            session_id: session.id,
                        }
                    } else {
                        let offline_secs = (inputs.now - end_time).num_seconds();
                        info!(
                            "Creating new session for {} (offline for {}s exceeded threshold of {}s)",
                            inputs.streamer_name, offline_secs, inputs.gap_threshold_secs
                        );
                        let new_id = Uuid::new_v4().to_string();
                        SessionTxOps::create_session(
                            &mut tx,
                            &new_id,
                            &inputs.streamer_id,
                            inputs.now,
                            &inputs.title,
                        )
                        .await?;
                        info!("Created new session {}", new_id);
                        StartSessionOutcome::Created { session_id: new_id }
                    }
                }
            },
            None => {
                let new_id = Uuid::new_v4().to_string();
                SessionTxOps::create_session(
                    &mut tx,
                    &new_id,
                    &inputs.streamer_id,
                    inputs.now,
                    &inputs.title,
                )
                .await?;
                info!("Created new session {}", new_id);
                StartSessionOutcome::Created { session_id: new_id }
            }
        };

        StreamerTxOps::set_live(&mut tx, &inputs.streamer_id, inputs.now).await?;

        if let Some(ref new_avatar) = inputs.new_avatar
            && !new_avatar.is_empty()
            && inputs.new_avatar != inputs.current_avatar
        {
            StreamerTxOps::update_avatar(&mut tx, &inputs.streamer_id, new_avatar).await?;
        }

        let event = MonitorEvent::StreamerLive {
            streamer_id: inputs.streamer_id.clone(),
            session_id: outcome.session_id().to_string(),
            streamer_name: inputs.streamer_name.clone(),
            streamer_url: inputs.streamer_url.clone(),
            title: inputs.title.clone(),
            category: inputs.category.clone(),
            streams: inputs.streams.clone(),
            media_headers: inputs.media_headers.clone(),
            media_extras: inputs.media_extras.clone(),
            timestamp: inputs.now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut tx, &inputs.streamer_id, &event).await?;

        tx.commit().await?;

        Ok(outcome)
    }

    /// Light "session ended" bundle: close the session row only, without
    /// touching streamer state or the outbox.
    ///
    /// Used on the `DownloadTerminalEvent` path — the download stopped but
    /// the monitor has not (yet) observed offline, so we must not flip the
    /// streamer to `NOT_LIVE` or broadcast a `StreamerOffline` notification.
    /// If `session_id` is `None`, falls back to `end_active_session` for the
    /// streamer. Returns the session id that was actually closed (if any).
    pub async fn end_session_only(
        &self,
        streamer_id: &str,
        session_id: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<Option<String>> {
        let mut tx = begin_immediate(&self.write_pool).await?;

        let resolved = if let Some(id) = session_id {
            SessionTxOps::end_session(&mut tx, id, now).await?;
            Some(id.to_string())
        } else {
            SessionTxOps::end_active_session(&mut tx, streamer_id, now).await?
        };

        tx.commit().await?;

        Ok(resolved)
    }

    /// Atomic "session ended" bundle: one `BEGIN IMMEDIATE` transaction
    /// that closes the session row, marks the streamer Offline, optionally
    /// clears accumulated errors, and (if appropriate) enqueues
    /// [`MonitorEvent::StreamerOffline`].
    pub async fn end(&self, inputs: EndSessionInputs) -> Result<EndSessionOutcome> {
        let mut tx = begin_immediate(&self.write_pool).await?;

        let resolved_session_id = if let Some(ref id) = inputs.session_id {
            SessionTxOps::end_session(&mut tx, id, inputs.now).await?;
            Some(id.clone())
        } else {
            SessionTxOps::end_active_session(&mut tx, &inputs.streamer_id, inputs.now).await?
        };

        let should_emit = inputs.state_was_live || resolved_session_id.is_some();

        StreamerTxOps::set_offline(&mut tx, &inputs.streamer_id).await?;

        if inputs.clear_errors {
            StreamerTxOps::clear_error_state(&mut tx, &inputs.streamer_id).await?;
        }

        if should_emit {
            let event = MonitorEvent::StreamerOffline {
                streamer_id: inputs.streamer_id.clone(),
                streamer_name: inputs.streamer_name.clone(),
                session_id: resolved_session_id.clone(),
                timestamp: inputs.now,
            };
            MonitorOutboxTxOps::enqueue_event(&mut tx, &inputs.streamer_id, &event).await?;
        }

        tx.commit().await?;

        Ok(EndSessionOutcome {
            resolved_session_id,
            offline_event_emitted: should_emit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{Row, SqlitePool};

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

    fn start_inputs(
        now: DateTime<Utc>,
        hard_ended_session_id: Option<String>,
    ) -> StartSessionInputs {
        StartSessionInputs {
            streamer_id: STREAMER_ID.to_string(),
            streamer_name: "Test".to_string(),
            streamer_url: "https://example.com".to_string(),
            current_avatar: None,
            new_avatar: None,
            title: "Live!".to_string(),
            category: None,
            streams: vec![],
            media_headers: None,
            media_extras: None,
            started_at: None,
            now,
            gap_threshold_secs: 60,
            hard_ended_session_id,
        }
    }

    fn end_inputs(
        session_id: Option<String>,
        state_was_live: bool,
        now: DateTime<Utc>,
    ) -> EndSessionInputs {
        EndSessionInputs {
            streamer_id: STREAMER_ID.to_string(),
            streamer_name: "Test".to_string(),
            session_id,
            state_was_live,
            clear_errors: false,
            now,
        }
    }

    async fn streamer_state(pool: &SqlitePool) -> String {
        sqlx::query("SELECT state FROM streamers WHERE id = ?")
            .bind(STREAMER_ID)
            .fetch_one(pool)
            .await
            .unwrap()
            .get::<String, _>(0)
    }

    async fn outbox_event_types(pool: &SqlitePool) -> Vec<String> {
        sqlx::query("SELECT event_type FROM monitor_event_outbox ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.get::<String, _>(0))
            .collect()
    }

    #[tokio::test]
    async fn start_or_resume_creates_first_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let outcome = repo.start_or_resume(start_inputs(now, None)).await.unwrap();

        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert_eq!(streamer_state(&pool).await, "LIVE");
        assert_eq!(outbox_event_types(&pool).await, vec!["StreamerLive"]);
    }

    #[tokio::test]
    async fn start_or_resume_reuses_active_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let first = repo.start_or_resume(start_inputs(now, None)).await.unwrap();
        let again = repo.start_or_resume(start_inputs(now, None)).await.unwrap();

        assert!(matches!(again, StartSessionOutcome::ReusedActive { .. }));
        assert_eq!(first.session_id(), again.session_id());
        assert_eq!(outbox_event_types(&pool).await.len(), 2);
    }

    #[tokio::test]
    async fn start_or_resume_within_gap_resumes_ended_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let started_at = Utc::now() - chrono::Duration::seconds(120);

        let first = repo
            .start_or_resume(start_inputs(started_at, None))
            .await
            .unwrap();
        let session_id = first.session_id().to_string();

        let ended_at = started_at + chrono::Duration::seconds(60);
        repo.end(end_inputs(Some(session_id.clone()), true, ended_at))
            .await
            .unwrap();

        // Within gap window (gap_threshold_secs = 60, offline = 30s).
        let restart_now = ended_at + chrono::Duration::seconds(30);
        let resumed = repo
            .start_or_resume(start_inputs(restart_now, None))
            .await
            .unwrap();

        assert!(matches!(resumed, StartSessionOutcome::Resumed { .. }));
        assert_eq!(resumed.session_id(), &session_id);
    }

    #[tokio::test]
    async fn start_or_resume_outside_gap_creates_new_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let started_at = Utc::now() - chrono::Duration::seconds(600);

        let first = repo
            .start_or_resume(start_inputs(started_at, None))
            .await
            .unwrap();

        let ended_at = started_at + chrono::Duration::seconds(60);
        repo.end(end_inputs(Some(first.session_id().to_string()), true, ended_at))
            .await
            .unwrap();

        // Far outside the 60s gap window.
        let restart_now = ended_at + chrono::Duration::seconds(300);
        let outcome = repo
            .start_or_resume(start_inputs(restart_now, None))
            .await
            .unwrap();

        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert_ne!(outcome.session_id(), first.session_id());
    }

    #[tokio::test]
    async fn start_or_resume_suppress_forces_new_inside_gap() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let started_at = Utc::now() - chrono::Duration::seconds(120);

        let first = repo
            .start_or_resume(start_inputs(started_at, None))
            .await
            .unwrap();

        let ended_at = started_at + chrono::Duration::seconds(60);
        repo.end(end_inputs(Some(first.session_id().to_string()), true, ended_at))
            .await
            .unwrap();

        // Inside gap window but caller flags the previous session as hard-ended.
        let restart_now = ended_at + chrono::Duration::seconds(30);
        let outcome = repo
            .start_or_resume(start_inputs(
                restart_now,
                Some(first.session_id().to_string()),
            ))
            .await
            .unwrap();

        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert_ne!(outcome.session_id(), first.session_id());
    }

    #[tokio::test]
    async fn end_with_explicit_id_emits_offline_and_marks_streamer() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now, None)).await.unwrap();
        let id = started.session_id().to_string();

        let outcome = repo
            .end(end_inputs(Some(id.clone()), true, now + chrono::Duration::seconds(10)))
            .await
            .unwrap();

        assert_eq!(outcome.resolved_session_id.as_deref(), Some(id.as_str()));
        assert!(outcome.offline_event_emitted);
        assert_eq!(streamer_state(&pool).await, "NOT_LIVE");
        assert_eq!(
            outbox_event_types(&pool).await,
            vec!["StreamerLive", "StreamerOffline"]
        );
    }

    #[tokio::test]
    async fn end_with_no_id_falls_back_to_active_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now, None)).await.unwrap();

        let outcome = repo
            .end(end_inputs(None, true, now + chrono::Duration::seconds(10)))
            .await
            .unwrap();

        assert_eq!(
            outcome.resolved_session_id.as_deref(),
            Some(started.session_id())
        );
        assert!(outcome.offline_event_emitted);
    }

    #[tokio::test]
    async fn end_session_only_closes_row_without_touching_streamer_or_outbox() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now, None)).await.unwrap();
        let id = started.session_id().to_string();

        let resolved = repo
            .end_session_only(STREAMER_ID, Some(&id), now + chrono::Duration::seconds(5))
            .await
            .unwrap();

        assert_eq!(resolved.as_deref(), Some(id.as_str()));
        // Streamer row stays Live — this path is meant for download-terminal
        // events that do not represent an authoritative offline observation.
        assert_eq!(streamer_state(&pool).await, "LIVE");
        // Only the start event is in the outbox; no offline event emitted.
        assert_eq!(outbox_event_types(&pool).await, vec!["StreamerLive"]);
    }

    #[tokio::test]
    async fn end_session_only_active_fallback_when_id_is_none() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now, None)).await.unwrap();

        let resolved = repo
            .end_session_only(STREAMER_ID, None, now + chrono::Duration::seconds(5))
            .await
            .unwrap();

        assert_eq!(resolved.as_deref(), Some(started.session_id()));
    }

    #[tokio::test]
    async fn end_without_session_or_live_state_skips_event() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        // No session ever started; streamer not Live; nothing to emit.
        let outcome = repo.end(end_inputs(None, false, now)).await.unwrap();

        assert!(outcome.resolved_session_id.is_none());
        assert!(!outcome.offline_event_emitted);
        assert!(outbox_event_types(&pool).await.is_empty());
    }
}
