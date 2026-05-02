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
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::Result;
use crate::database::models::SessionEventDbModel;
use crate::database::repositories::{
    MonitorOutboxTxOps, SessionEventTxOps, SessionTxOps, StreamerTxOps,
};
use crate::database::{WritePool, begin_immediate};
use crate::monitor::{MonitorEvent, StreamInfo};
use crate::session::events::{SessionEventPayload, TerminalCauseDto};

/// Inputs required by [`SessionLifecycleRepository::start_or_resume`].
///
/// Phase 3 of the hysteresis plan removed three fields that were vestigial
/// once the lifecycle owns intermittent-stream handling:
///
/// - `gap_threshold_secs` — gap-resume retired; lifecycle handles
///   intermittence via `Hysteresis` state instead.
/// - `started_at` — fed only the continuation rule, also retired.
/// - `hard_ended_session_id` — `SessionLifecycle::hard_ended` cache
///   deleted; with no gap-resume, the DB's `end_time` is the source of
///   truth and an explicit fence is unnecessary.
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
    /// Reference time for the session `start_time` and event `timestamp`.
    pub now: DateTime<Utc>,
}

/// Outcome of [`SessionLifecycleRepository::start_or_resume`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartSessionOutcome {
    /// A brand-new session row was inserted.
    Created { session_id: String },
    /// The most-recent session row was still active (no end_time); only its
    /// titles and the streamer state were updated. This is also what the
    /// lifecycle returns when resuming a session out of `Hysteresis` —
    /// the session row was never ended in DB so reusing it is correct.
    ReusedActive { session_id: String },
}

impl StartSessionOutcome {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Created { session_id } | Self::ReusedActive { session_id } => session_id,
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
    /// Wire-format cause for the `session_ended` audit row. Pre-computed by
    /// the caller (typically `SessionLifecycle::on_offline_detected`) so the
    /// audit row is written inside the same `BEGIN IMMEDIATE` boundary as
    /// the `live_sessions.end_time` update. The two cannot diverge.
    pub cause: crate::session::TerminalCauseDto,
    /// `true` if the session was in `Hysteresis` state when the end
    /// observation arrived. Recorded on the audit row so operators can tell
    /// "ended via timer expiry" vs "ended directly by authoritative event."
    pub via_hysteresis: bool,
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

    /// Build the canonical [`SessionEventDbModel`] for a typed payload.
    /// Centralised so the JSON encoding rules and the `kind` discriminator
    /// stay aligned in one place.
    fn event_row(
        session_id: &str,
        streamer_id: &str,
        payload: &SessionEventPayload,
        occurred_at: DateTime<Utc>,
    ) -> SessionEventDbModel {
        SessionEventDbModel {
            id: 0,
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            kind: payload.kind().as_str().to_string(),
            occurred_at: occurred_at.timestamp_millis(),
            // `to_string` on a typed enum should never fail; the `unwrap_or`
            // keeps the audit row alive even if it somehow does.
            payload: serde_json::to_string(payload).ok(),
        }
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

        // Self-heal: end any stale active session rows for this streamer.
        //
        // The partial unique index `live_sessions_one_active_per_streamer`
        // (initial schema) caps the candidate set at 0 or 1 row in normal
        // operation, so this is a no-op on a healthy DB. But the
        // 2026-04-28 production logs (柔柔) showed at least one buggy
        // build leaked enough state that operators could end up with
        // multiple `end_time IS NULL` rows for one streamer. Without this
        // step, the next `start_or_resume` would either:
        //   - on the Created path, trip the unique index on INSERT and
        //     fail the request loudly; or
        //   - on the ReusedActive path, pick the most-recent active and
        //     leave the older stale rows orphaned forever.
        //
        // Strategy: keep the most-recent active row (the one ReusedActive
        // is about to pick up) and end every other active row inside this
        // same `BEGIN IMMEDIATE` tx. If `last` is missing or already
        // ended, `keep` is `None` and ALL actives are cleaned up.
        //
        // Each cleaned-up session also gets a
        // `session_ended { rejected, "stale_active_replaced" }` audit row
        // so the heal is observable. No `SessionTransition::Ended`
        // broadcast — these rows represent state that should never have
        // existed; firing the session-complete pipeline DAG retroactively
        // would re-upload, re-notify, etc.
        let keep_id = last
            .as_ref()
            .filter(|s| s.end_time.is_none())
            .map(|s| s.id.as_str());
        let cleaned = SessionTxOps::end_all_active_for_streamer(
            &mut tx,
            &inputs.streamer_id,
            keep_id,
            inputs.now,
        )
        .await?;
        if !cleaned.is_empty() {
            warn!(
                streamer_id = %inputs.streamer_id,
                kept = keep_id.unwrap_or("<none>"),
                count = cleaned.len(),
                "Cleaned up stale active session(s)"
            );
        }
        for stale_id in &cleaned {
            let payload = SessionEventPayload::SessionEnded {
                cause: TerminalCauseDto::Rejected {
                    reason: "stale_active_replaced".to_string(),
                },
                via_hysteresis: false,
            };
            let row = Self::event_row(
                stale_id,
                &inputs.streamer_id,
                &payload,
                inputs.now,
            );
            SessionEventTxOps::insert(&mut tx, &row).await?;
        }

        // Two-branch decision (down from five pre-Phase-3):
        //
        //   - last session has `end_time IS NULL` → reuse it (ReusedActive)
        //   - any other case (no prior session, or prior session is Ended)
        //     → create a fresh session
        //
        // No gap-resume rule, no continuation rule, no hard-ended fence.
        // Intermittent-stream handling moved out of here entirely; it lives
        // in `SessionLifecycle`'s Hysteresis state machine. The DB's
        // `end_time` is the source of truth for "this recording is over."
        let outcome = match last {
            Some(session) if session.end_time.is_none() => {
                debug!("Reusing active session {}", session.id);
                SessionTxOps::update_titles(
                    &mut tx,
                    &session.id,
                    session.titles.as_deref(),
                    &inputs.title,
                    inputs.now,
                )
                .await?;
                // No `session_started` row on the reuse path — the prior
                // `Created` insert already wrote one. Title changes are
                // captured by the existing `titles` JSON array.
                StartSessionOutcome::ReusedActive {
                    session_id: session.id,
                }
            }
            _ => {
                let new_id = Uuid::new_v4().to_string();
                SessionTxOps::create_session(
                    &mut tx,
                    &new_id,
                    &inputs.streamer_id,
                    inputs.now,
                    &inputs.title,
                )
                .await?;
                // Record the `session_started` row in the same tx as the
                // `live_sessions` insert. `from_hysteresis` is always
                // `false` here — the resume-out-of-hysteresis path returns
                // early in `SessionLifecycle::on_live_detected` and never
                // calls `start_or_resume`.
                let payload = SessionEventPayload::SessionStarted {
                    from_hysteresis: false,
                    title: Some(inputs.title.clone()),
                };
                let row = Self::event_row(
                    &new_id,
                    &inputs.streamer_id,
                    &payload,
                    inputs.now,
                );
                SessionEventTxOps::insert(&mut tx, &row).await?;
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
    /// touching streamer state or the outbox. Writes a `session_ended`
    /// audit row in the same transaction so the `live_sessions.end_time`
    /// flip and the audit log can never disagree.
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
        cause: TerminalCauseDto,
        via_hysteresis: bool,
        now: DateTime<Utc>,
    ) -> Result<Option<String>> {
        let mut tx = begin_immediate(&self.write_pool).await?;

        let resolved = if let Some(id) = session_id {
            SessionTxOps::end_session(&mut tx, id, now).await?;
            Some(id.to_string())
        } else {
            SessionTxOps::end_active_session(&mut tx, streamer_id, now).await?
        };

        // Same-tx audit row. Skipped when no session was actually closed
        // (e.g. caller passed `None` and there was no active session) —
        // there's nothing for the row to reference.
        if let Some(ref id) = resolved {
            let payload = SessionEventPayload::SessionEnded {
                cause,
                via_hysteresis,
            };
            let row = Self::event_row(id, streamer_id, &payload, now);
            SessionEventTxOps::insert(&mut tx, &row).await?;
        }

        tx.commit().await?;

        Ok(resolved)
    }

    /// Atomic "user disabled" tear-down: one `BEGIN IMMEDIATE` transaction
    /// that closes the active session row and inserts the
    /// `session_ended { cause: user_disabled }` audit row.
    ///
    /// Differs from [`Self::end`] in three ways:
    ///
    /// - does NOT touch `streamers.state` — the API route already wrote
    ///   `Disabled` (or `Deleted`) before this is called;
    /// - does NOT enqueue a [`MonitorEvent::StreamerOffline`] — the user
    ///   knows they disabled the streamer; their downstream notification
    ///   integrations don't need a synthetic offline push;
    /// - is idempotent on a session that's already ended (returns
    ///   `Ok(None)`); concurrent disable calls collapse to one effective
    ///   tear-down.
    ///
    /// If `session_id` is `None`, falls back to the active session for the
    /// streamer (matches `repo.end_session_only`'s shape). Returns the
    /// session id that was actually closed, or `None` if there was no
    /// active row to close.
    pub async fn end_for_disable(
        &self,
        streamer_id: &str,
        session_id: Option<&str>,
        via_hysteresis: bool,
        now: DateTime<Utc>,
    ) -> Result<Option<String>> {
        let mut tx = begin_immediate(&self.write_pool).await?;

        // Resolve the active session id. When the caller named one, only
        // proceed if its `end_time IS NULL` — that's the idempotency guard
        // for repeated disable events. Without this, a second call would
        // insert a duplicate `session_ended` audit row for the same id.
        let resolved: Option<String> = if let Some(id) = session_id {
            let still_active: Option<bool> = sqlx::query_scalar(
                "SELECT end_time IS NULL FROM live_sessions WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            match still_active {
                Some(true) => Some(id.to_string()),
                _ => None,
            }
        } else {
            crate::database::repositories::SessionTxOps::get_active_session_id(
                &mut tx,
                streamer_id,
            )
            .await?
        };

        let Some(sid) = resolved else {
            tx.commit().await?;
            return Ok(None);
        };

        crate::database::repositories::SessionTxOps::end_session(&mut tx, &sid, now).await?;

        let payload = SessionEventPayload::SessionEnded {
            cause: TerminalCauseDto::UserDisabled,
            via_hysteresis,
        };
        let row = Self::event_row(&sid, streamer_id, &payload, now);
        SessionEventTxOps::insert(&mut tx, &row).await?;

        tx.commit().await?;

        Ok(Some(sid))
    }

    /// Retro-actively rewrite the most recent `session_ended` audit row's
    /// cause for `session_id`. Used when the lifecycle's `end_for_disable`
    /// loses a CAS race to the hysteresis timer (or any other authoritative
    /// path) — the row was written with the wrong cause; we correct the
    /// audit log to reflect the user's actual intent.
    ///
    /// Returns `true` if a row was updated, `false` if no `session_ended`
    /// row existed for the session (defensive — should not happen in
    /// practice because the caller only invokes this after observing the
    /// session is already Ended).
    pub async fn rewrite_session_ended_cause(
        &self,
        session_id: &str,
        new_cause: TerminalCauseDto,
    ) -> Result<bool> {
        let mut tx = begin_immediate(&self.write_pool).await?;

        // Pick the most recent `session_ended` row. Multiple rows can exist
        // only as the result of a buggy retro-update path; we update the
        // newest one and let any older row stand as historical record.
        let existing: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, payload FROM session_events
             WHERE session_id = ? AND kind = 'session_ended'
             ORDER BY occurred_at DESC, id DESC LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((row_id, payload_json)) = existing else {
            tx.commit().await?;
            return Ok(false);
        };

        // Parse → mutate cause → reserialise. Preserve `via_hysteresis` so
        // operators can still tell "ended via timer expiry" vs "direct" —
        // the user's tear-down landed on top of an existing FSM state and
        // the via_hysteresis bit reflects that history.
        let mut payload: SessionEventPayload = match payload_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
        {
            Some(p) => p,
            None => {
                tx.commit().await?;
                return Ok(false);
            }
        };

        match &mut payload {
            SessionEventPayload::SessionEnded { cause, .. } => *cause = new_cause,
            _ => {
                tx.commit().await?;
                return Ok(false);
            }
        }

        let new_json = serde_json::to_string(&payload)?;
        sqlx::query("UPDATE session_events SET payload = ? WHERE id = ?")
            .bind(new_json)
            .bind(row_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(true)
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

        // Same-tx audit row. Skipped when no session was resolved — see the
        // mirror branch in `end_session_only`.
        if let Some(ref id) = resolved_session_id {
            let payload = SessionEventPayload::SessionEnded {
                cause: inputs.cause,
                via_hysteresis: inputs.via_hysteresis,
            };
            let row = Self::event_row(id, &inputs.streamer_id, &payload, inputs.now);
            SessionEventTxOps::insert(&mut tx, &row).await?;
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
        // Mirror the production migration's partial unique index so tests
        // exercise the same constraint. Without this, the cleanup-on-create
        // tests would silently allow multi-active rows that production
        // would reject.
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

        // Mirror the production migration; the `CHECK` constraint is the
        // production guard against typos in the `kind` discriminator.
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

    fn start_inputs(now: DateTime<Utc>) -> StartSessionInputs {
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
            now,
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
            cause: TerminalCauseDto::StreamerOffline,
            via_hysteresis: false,
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

        let outcome = repo.start_or_resume(start_inputs(now)).await.unwrap();

        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert_eq!(streamer_state(&pool).await, "LIVE");
        assert_eq!(outbox_event_types(&pool).await, vec!["StreamerLive"]);
    }

    #[tokio::test]
    async fn start_or_resume_reuses_active_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let first = repo.start_or_resume(start_inputs(now)).await.unwrap();
        let again = repo.start_or_resume(start_inputs(now)).await.unwrap();

        assert!(matches!(again, StartSessionOutcome::ReusedActive { .. }));
        assert_eq!(first.session_id(), again.session_id());
        assert_eq!(outbox_event_types(&pool).await.len(), 2);
    }

    #[tokio::test]
    async fn start_or_resume_after_ended_creates_new_session() {
        // Phase 3 simplification: with gap-resume retired, ANY ended session
        // followed by a new LiveDetected creates a fresh session row.
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let started_at = Utc::now() - chrono::Duration::seconds(60);

        let first = repo.start_or_resume(start_inputs(started_at)).await.unwrap();
        let ended_at = started_at + chrono::Duration::seconds(30);
        repo.end(end_inputs(Some(first.session_id().to_string()), true, ended_at))
            .await
            .unwrap();

        // No matter how soon we restart — no gap rule.
        let restart_now = ended_at + chrono::Duration::seconds(5);
        let outcome = repo.start_or_resume(start_inputs(restart_now)).await.unwrap();

        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));
        assert_ne!(outcome.session_id(), first.session_id());
    }

    #[tokio::test]
    async fn end_with_explicit_id_emits_offline_and_marks_streamer() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
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

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();

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

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
        let id = started.session_id().to_string();

        let resolved = repo
            .end_session_only(
                STREAMER_ID,
                Some(&id),
                TerminalCauseDto::Completed,
                false,
                now + chrono::Duration::seconds(5),
            )
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

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();

        let resolved = repo
            .end_session_only(
                STREAMER_ID,
                None,
                TerminalCauseDto::Completed,
                false,
                now + chrono::Duration::seconds(5),
            )
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

    /// Self-heal path A: a previous buggy build left two stale active
    /// rows for the same streamer (multiple `end_time IS NULL`). The next
    /// `start_or_resume` reuses the most-recent active row (`stale-B`)
    /// and ends the older one (`stale-A`) in the same `BEGIN IMMEDIATE`
    /// tx, with a `session_ended { rejected, "stale_active_replaced" }`
    /// audit row.
    ///
    /// Without this, the older stale row would stay `end_time IS NULL`
    /// forever and the next `Created` path on this streamer would trip
    /// the partial unique index.
    #[tokio::test]
    async fn start_or_resume_cleans_stale_active_keeping_most_recent() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        // Drop the unique index, seed two stale active rows, then leave
        // the index off (recreating it would fail with two active rows).
        // This represents the corrupt state a buggy old build could have
        // left in production.
        sqlx::query("DROP INDEX live_sessions_one_active_per_streamer")
            .execute(&pool)
            .await
            .unwrap();
        for (id, offset) in [("stale-A", -120), ("stale-B", -60)] {
            sqlx::query(
                "INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, danmu_statistics_id, total_size_bytes)
                 VALUES (?, ?, ?, NULL, '[]', NULL, 0)",
            )
            .bind(id)
            .bind(STREAMER_ID)
            .bind((now + chrono::Duration::seconds(offset)).timestamp_millis())
            .execute(&pool)
            .await
            .unwrap();
        }

        let outcome = repo.start_or_resume(start_inputs(now)).await.unwrap();
        // `get_last_session` returns the most-recent active (stale-B), so
        // ReusedActive picks it up. The OLDER stale row (stale-A) is what
        // gets cleaned.
        match outcome {
            StartSessionOutcome::ReusedActive { ref session_id } => {
                assert_eq!(session_id, "stale-B", "the most-recent active row is reused");
            }
            other => panic!("expected ReusedActive(stale-B), got {other:?}"),
        }

        // After cleanup: stale-A is ended, stale-B is still active.
        let active: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM live_sessions
             WHERE streamer_id = ? AND end_time IS NULL",
        )
        .bind(STREAMER_ID)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(active, vec!["stale-B".to_string()]);

        // One `session_ended { rejected, stale_active_replaced }` for
        // stale-A. No `session_started` (this is a reuse, not a create).
        let events: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT kind, payload FROM session_events
             WHERE streamer_id = ? ORDER BY occurred_at ASC, id ASC",
        )
        .bind(STREAMER_ID)
        .fetch_all(&pool)
        .await
        .unwrap();
        let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, vec!["session_ended"]);

        let (_, payload) = &events[0];
        let parsed: serde_json::Value =
            serde_json::from_str(payload.as_deref().unwrap()).unwrap();
        assert_eq!(parsed["cause"]["type"], "rejected");
        assert_eq!(parsed["cause"]["reason"], "stale_active_replaced");
        assert_eq!(parsed["via_hysteresis"], false);
    }

    /// Self-heal path B: stale active rows exist, but the most-recent
    /// session for the streamer is already ended. Cleanup must end ALL
    /// active rows (since none of them is the canonical "current"),
    /// then `Created` writes a fresh session.
    #[tokio::test]
    async fn start_or_resume_cleans_all_stale_active_when_most_recent_is_ended() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        sqlx::query("DROP INDEX live_sessions_one_active_per_streamer")
            .execute(&pool)
            .await
            .unwrap();
        // stale-A and stale-B: end_time NULL (corrupt). stale-C: properly
        // ended, but with the *latest* start_time so `get_last_session`
        // returns stale-C with end_time set → goes to Created branch.
        for (id, offset, ended) in [
            ("stale-A", -180, false),
            ("stale-B", -120, false),
            ("stale-C", -60, true),
        ] {
            let end_time = if ended {
                Some((now + chrono::Duration::seconds(offset + 30)).timestamp_millis())
            } else {
                None
            };
            sqlx::query(
                "INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, danmu_statistics_id, total_size_bytes)
                 VALUES (?, ?, ?, ?, '[]', NULL, 0)",
            )
            .bind(id)
            .bind(STREAMER_ID)
            .bind((now + chrono::Duration::seconds(offset)).timestamp_millis())
            .bind(end_time)
            .execute(&pool)
            .await
            .unwrap();
        }

        let outcome = repo.start_or_resume(start_inputs(now)).await.unwrap();
        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));

        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM live_sessions
             WHERE streamer_id = ? AND end_time IS NULL AND id != ?",
        )
        .bind(STREAMER_ID)
        .bind(outcome.session_id())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active, 0, "stale-A and stale-B must both be ended");

        // Two `session_ended` (for the two stale actives) + one
        // `session_started` (for the new session).
        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM session_events
             WHERE streamer_id = ? ORDER BY occurred_at ASC, id ASC",
        )
        .bind(STREAMER_ID)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            kinds,
            vec!["session_ended", "session_ended", "session_started"]
        );
    }

    // -------------------------------------------------------------------
    // Suite Or — `end_for_disable` repository helper.
    //
    // Mirrors the lifecycle suite O but anchored at the repository tx
    // boundary so we can spot-check atomicity, idempotency, and the
    // audit-row payload shape independently of the FSM.
    // -------------------------------------------------------------------

    /// Or1 — `end_for_disable` writes `end_time`, inserts a
    /// `session_ended` audit row with `cause: user_disabled`, and does
    /// NOT touch streamer state or enqueue StreamerOffline.
    #[tokio::test]
    async fn or1_end_for_disable_atomic_writes() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
        let sid = started.session_id().to_string();

        let resolved = repo
            .end_for_disable(
                STREAMER_ID,
                Some(&sid),
                false,
                now + chrono::Duration::seconds(5),
            )
            .await
            .unwrap();
        assert_eq!(resolved.as_deref(), Some(sid.as_str()));

        // end_time set.
        let end_time: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(end_time.is_some());

        // Streamer state unchanged (still LIVE).
        assert_eq!(streamer_state(&pool).await, "LIVE");

        // Outbox has only the start event.
        assert_eq!(outbox_event_types(&pool).await, vec!["StreamerLive"]);

        // Audit row carries user_disabled.
        let payload: String = sqlx::query_scalar(
            "SELECT payload FROM session_events
             WHERE session_id = ? AND kind = 'session_ended'
             ORDER BY id DESC LIMIT 1",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(payload.contains("\"user_disabled\""), "got: {payload}");
    }

    /// Or2 — `end_for_disable` is idempotent at the tx level. A second
    /// call against the same already-ended id is a no-op (returns None),
    /// inserts no second audit row.
    #[tokio::test]
    async fn or2_end_for_disable_idempotent_on_already_ended_id() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
        let sid = started.session_id().to_string();

        let _first = repo
            .end_for_disable(STREAMER_ID, Some(&sid), false, now)
            .await
            .unwrap();
        let second = repo
            .end_for_disable(STREAMER_ID, Some(&sid), false, now)
            .await
            .unwrap();
        assert!(
            second.is_none(),
            "second call against already-ended session must return Ok(None)"
        );

        // Exactly one session_ended audit row exists for this session.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session_events
             WHERE session_id = ? AND kind = 'session_ended'",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    /// Or3 — fallback to active session when `session_id` is `None`.
    #[tokio::test]
    async fn or3_end_for_disable_falls_back_to_active_session() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
        let resolved = repo
            .end_for_disable(STREAMER_ID, None, false, now)
            .await
            .unwrap();
        assert_eq!(resolved.as_deref(), Some(started.session_id()));
    }

    /// Or4 — `rewrite_session_ended_cause` updates only the latest
    /// `session_ended` row's payload cause and preserves
    /// `via_hysteresis`.
    #[tokio::test]
    async fn or4_rewrite_session_ended_cause_updates_only_latest() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let started = repo.start_or_resume(start_inputs(now)).await.unwrap();
        let sid = started.session_id().to_string();

        // End normally (cause=Completed, via_hysteresis=true) so the audit
        // row exists. Use the light path so streamer state stays LIVE.
        repo.end_session_only(STREAMER_ID, Some(&sid), TerminalCauseDto::Completed, true, now)
            .await
            .unwrap();

        let updated = repo
            .rewrite_session_ended_cause(&sid, TerminalCauseDto::UserDisabled)
            .await
            .unwrap();
        assert!(updated);

        let payload: String = sqlx::query_scalar(
            "SELECT payload FROM session_events
             WHERE session_id = ? AND kind = 'session_ended'
             ORDER BY id DESC LIMIT 1",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(payload.contains("\"user_disabled\""), "got: {payload}");
        assert!(
            payload.contains("\"via_hysteresis\":true"),
            "via_hysteresis must be preserved across rewrite, got: {payload}"
        );
    }

    /// Or5 — `rewrite_session_ended_cause` returns false when there is no
    /// session_ended row to rewrite.
    #[tokio::test]
    async fn or5_rewrite_session_ended_cause_returns_false_when_no_row() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());

        let updated = repo
            .rewrite_session_ended_cause("nonexistent", TerminalCauseDto::UserDisabled)
            .await
            .unwrap();
        assert!(!updated);
    }

    /// Inverse case: a clean DB. `start_or_resume` must NOT write any
    /// stale-replace audit rows when there's nothing to clean up.
    #[tokio::test]
    async fn start_or_resume_no_cleanup_when_no_stale_rows() {
        let pool = setup_pool().await;
        let repo = SessionLifecycleRepository::new(pool.clone());
        let now = Utc::now();

        let outcome = repo.start_or_resume(start_inputs(now)).await.unwrap();
        assert!(matches!(outcome, StartSessionOutcome::Created { .. }));

        let kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM session_events
             WHERE streamer_id = ? ORDER BY occurred_at ASC, id ASC",
        )
        .bind(STREAMER_ID)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            kinds,
            vec!["session_started"],
            "no stale rows should produce no cleanup audit rows"
        );
    }
}
