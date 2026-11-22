//! Hysteresis resume and terminal `Ended` state transitions for `SessionLifecycle`.

use jiff::Timestamp;
use tracing::{debug, info, warn};

use super::{LiveDetectedArgs, SessionLifecycle};
use crate::Result;
use crate::database::repositories::StartSessionOutcome;
use crate::session::events::SessionEventPayload;
use crate::session::state::{SessionState, TerminalCause};
use crate::session::transition::SessionTransition;

impl SessionLifecycle {
    /// Cancel an active hysteresis timer and transition `Hysteresis →
    /// Recording`. The session row's `end_time` remains unset during
    /// hysteresis, so no DB undo is needed. Emits `SessionTransition::Resumed`.
    ///
    /// Before claiming the exit, `mark_streamer_live` re-checks the
    /// streamer row's state at the DB serialization point: an inactive row
    /// (user disabled/cancelled, fatal) aborts the resume with
    /// `SuppressedInactive` and leaves the hysteresis handle armed, so the
    /// disable teardown or the timer ends the session normally.
    ///
    /// The caller's stripe excludes concurrent timer/end publication. Removing
    /// the handle after the live write claims the exit; an absent handle means
    /// this quiet period was already claimed, so the caller uses the normal
    /// start_or_resume path. The handle stays armed on inactive suppression.
    pub(super) async fn resume_from_hysteresis(
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

        crate::database::committed_writer::prepare_owned_commit().ok()?;
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
            jiff::SignedDuration::try_from(handle.elapsed()).unwrap_or(jiff::SignedDuration::ZERO);

        // The session API and the session-complete jobs name a session by its
        // latest title, and a resume is where a title changed mid-session is
        // usually first seen. Written only after the exit is claimed, so a
        // session the timer ended keeps its titles. A failure must not stop
        // the resume.
        if let Err(e) = self
            .repo
            .add_session_title(session_id, args.title, args.now)
            .await
        {
            warn!(
                streamer_id = args.streamer_id,
                session_id,
                error = %e,
                "resume_from_hysteresis: failed to record the session title"
            );
        }

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

        self.publish_transition(SessionTransition::Resumed {
            session_id: session_id.to_string(),
            streamer_id: args.streamer_id.to_string(),
            resumed_at: args.now,
            hysteresis_duration,
        });

        info!(
            streamer_id = %args.streamer_id,
            session_id,
            hysteresis_secs = hysteresis_duration.as_secs(),
            "Session resumed from hysteresis"
        );

        // Best-effort `session_resumed` audit row. We're already on an
        // async path so awaiting is fine; any failure is non-fatal.
        let resumed_secs = u64::try_from(hysteresis_duration.as_secs()).unwrap_or(0);
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
        // session "records" zero bytes for the rest of the broadcast.
        // Populating the sidecar from `args` is what supplies that
        // restart signal.
        self.publish_transition(SessionTransition::Started {
            session_id: session_id.to_string(),
            streamer_id: args.streamer_id.to_string(),
            streamer_name: args.streamer_name.to_string(),
            title: args.title.to_string(),
            category: args.category.map(|s| s.to_string()),
            started_at: args.now,
            from_hysteresis: true,
            download_start: Some(Box::new(crate::session::DownloadStartPayload {
                credential_binding: args.credential_binding.cloned(),
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
    /// The caller's stripe excludes concurrent resume and timer completion.
    /// A missing hysteresis handle indicates an already claimed quiet period.
    /// Keep the handle until SQL succeeds so a failed end leaves it recoverable.
    pub(super) async fn enter_ended_state(
        &self,
        transition: EndedStateTransition<'_>,
    ) -> Result<()> {
        let EndedStateTransition {
            session_id,
            streamer_id,
            streamer_name,
            cause,
            ended_at,
            via_hysteresis,
            db_write,
        } = transition;
        // CAS-style entry guard. If the session is already Ended, skip
        // (idempotent on duplicate authoritative-end events arriving in
        // tight succession).
        if let Some(entry) = self.sessions.get(session_id)
            && entry.is_ended()
        {
            debug!(session_id, "enter_ended_state: already Ended, skipping");
            return Ok(());
        }

        // A live hysteresis entry must still own its quiet-period handle.
        // The stripe prevents this check from changing during the SQL write.
        let was_in_hysteresis = self
            .sessions
            .get(session_id)
            .is_some_and(|e| matches!(e.value(), SessionState::Hysteresis { .. }));

        if was_in_hysteresis && !self.hysteresis.contains_key(session_id) {
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
                    let ended = self
                        .repo
                        .end_session_only(
                            streamer_id,
                            Some(session_id),
                            (&cause).into(),
                            via_hysteresis,
                            ended_at,
                        )
                        .await?;
                    if ended.is_none() {
                        return Ok(());
                    }
                }
            }
        }

        // Read before sealing the commit: no await may follow it.
        let started_at = self.known_started_at(session_id).await;

        crate::database::committed_writer::prepare_owned_commit()?;
        // The stripe excludes resume and timer completion until publication.
        // Keep the existing timer armed if the preceding SQL write fails.
        if let Some((_, handle)) = self.hysteresis.remove(session_id) {
            handle.cancel();
        }

        self.sessions.insert(
            session_id.to_string(),
            SessionState::ended(
                streamer_id,
                session_id,
                started_at.unwrap_or(ended_at),
                ended_at,
                cause.clone(),
            ),
        );
        self.refresh_current_session_if_current(streamer_id, session_id);

        info!(
            streamer_id,
            session_id,
            cause = cause.as_str(),
            via_hysteresis,
            "Session ended"
        );

        self.publish_transition(SessionTransition::Ended {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            started_at,
            ended_at,
            cause,
            via_hysteresis,
        });

        // Defer eviction by `ended_retention` so the CAS-style idempotency
        // guard at the top of this function actually catches a duplicate
        // authoritative-end event.
        self.schedule_ended_eviction(streamer_id, session_id);

        Ok(())
    }
}

/// Which DB-write path `enter_ended_state` should take. The DB write is
/// path-dependent because `on_offline_detected` runs the full atomic
/// bundle (end_session + set_offline + StreamerOffline outbox event) inside
/// `repo.end()` BEFORE calling `enter_ended_state`, while the
/// download-terminal path runs the lighter `end_session_only`.
#[derive(Debug, Clone, Copy)]
pub(super) enum DbWritePath {
    /// Caller already wrote `end_time` (e.g. via the full atomic bundle in
    /// `on_offline_detected`). `enter_ended_state` only updates in-memory
    /// state and emits the transition.
    Skip,
    /// `enter_ended_state` itself calls `repo.end_session_only` to write
    /// `end_time` without flipping streamer state. Used by the
    /// download-terminal path and the hysteresis-timer path.
    EndSessionOnly,
}

pub(super) struct EndedStateTransition<'a> {
    pub(super) session_id: &'a str,
    pub(super) streamer_id: &'a str,
    pub(super) streamer_name: &'a str,
    pub(super) cause: TerminalCause,
    pub(super) ended_at: Timestamp,
    pub(super) via_hysteresis: bool,
    pub(super) db_write: DbWritePath,
}
