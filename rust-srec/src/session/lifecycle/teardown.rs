//! Policy-driven session teardown (`end_for_disable`, `end_for_out_of_schedule`) and retro-attribution.

use jiff::Timestamp;
use tracing::{debug, info};

use super::SessionLifecycle;
use crate::Result;
use crate::database::repositories::EndForOutOfScheduleInputs;
use crate::domain::StreamerState;
use crate::session::state::{SessionState, TerminalCause};
use crate::session::transition::SessionTransition;

impl SessionLifecycle {
    /// Tear down the active session because the user disabled (or deleted)
    /// the streamer.
    ///
    /// Ends the DB row and the in-memory FSM together: writing
    /// `live_sessions.end_time` without also cancelling the in-memory
    /// Hysteresis handle would let a later re-enable take the
    /// `resume_from_hysteresis` short-circuit and silently restart a
    /// download under an already-ended `session_id`.
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
    /// If a hysteresis timer already ended the session with an ambiguous
    /// cause, reattribute that audit row to `user_disabled` and patch the
    /// in-memory `Ended.cause` to match. Authoritative end causes are
    /// preserved. The original `SessionTransition::
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
    pub(super) async fn end_for_disable_inner(
        &self,
        streamer_id: &str,
        streamer_name: &str,
    ) -> Result<Option<String>> {
        self.classifier.purge_streamer(streamer_id);
        let now = Timestamp::now();

        // Step 1: find the session in memory via the deterministic
        // per-streamer index (`current_session_for_streamer`); `sessions`
        // retains recently-ended entries and may hold several per streamer.
        // The in-memory map is the source of truth for FSM state; if it has
        // no entry we'll fall back to a DB lookup inside the repo to handle
        // cold-start / post-restart cases (see Step 4).
        let in_memory = self.current_session_for_streamer(streamer_id);
        let session_id_hint = in_memory.as_ref().map(|(sid, _)| sid.clone());
        let was_in_hysteresis = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_hysteresis()
        );
        let was_already_ended = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_ended()
        );

        // The stripe retains hysteresis ownership across the SQL write. Only
        // remove the handle after success so errors leave its timer armed.
        let lost_cas = was_in_hysteresis
            && !session_id_hint
                .as_ref()
                .is_some_and(|sid| self.hysteresis.contains_key(sid));

        // Only an existing timer end is eligible for reattribution. Missing
        // timer ownership alone never means the active DB row was closed.
        if was_already_ended {
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

        // Normal path. DB write first (commit → in-memory →
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

        if let Some(sid) = session_id_hint.as_ref()
            && let Some((_, handle)) = self.hysteresis.remove(sid)
        {
            handle.cancel();
        }
        let Some(session_id) = resolved else {
            debug!(streamer_id, "end_for_disable: no active session to end");
            return Ok(None);
        };

        // In-memory update. Pull `started_at` from the prior state if
        // present; otherwise default to `now` (cold-start case where we
        // recovered the session from the DB).
        let started_at = self.sessions.get(&session_id).map(|e| e.started_at());
        self.sessions.insert(
            session_id.clone(),
            SessionState::ended(
                streamer_id,
                &session_id,
                started_at.unwrap_or(now),
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
        self.publish_transition(SessionTransition::Ended {
            session_id: session_id.clone(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            started_at,
            ended_at: now,
            cause: TerminalCause::UserDisabled,
            via_hysteresis: was_in_hysteresis,
        });

        // Defer in-memory eviction (see `schedule_ended_eviction` for rationale).
        self.schedule_ended_eviction(streamer_id, &session_id);

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
    pub(super) async fn end_for_out_of_schedule_inner(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        old_state: StreamerState,
    ) -> Result<Option<String>> {
        let now = Timestamp::now();

        let in_memory = self.current_session_for_streamer(streamer_id);
        let session_id_hint = in_memory.as_ref().map(|(sid, _)| sid.clone());
        let was_in_hysteresis = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_hysteresis()
        );
        let was_already_ended = matches!(
            in_memory.as_ref(),
            Some((_, state)) if state.is_ended()
        );

        let lost_hysteresis_cas = was_in_hysteresis
            && !session_id_hint
                .as_ref()
                .is_some_and(|sid| self.hysteresis.contains_key(sid));
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

        if let Some(sid) = session_id_hint.as_ref()
            && let Some((_, handle)) = self.hysteresis.remove(sid)
        {
            handle.cancel();
        }
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
        let started_at = self.sessions.get(&session_id).map(|e| e.started_at());
        self.sessions.insert(
            session_id.clone(),
            SessionState::ended(
                streamer_id,
                &session_id,
                started_at.unwrap_or(now),
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

        self.publish_transition(SessionTransition::Ended {
            session_id: session_id.clone(),
            streamer_id: streamer_id.to_string(),
            streamer_name: streamer_name.to_string(),
            started_at,
            ended_at: now,
            cause: TerminalCause::OutOfSchedule,
            via_hysteresis: was_in_hysteresis,
        });

        self.schedule_ended_eviction(streamer_id, &session_id);

        Ok(Some(session_id))
    }

    /// Helper for [`Self::end_for_disable`] — retro-actively rewrite the
    /// most recent `session_ended` audit row's cause to `user_disabled`
    /// and update the in-memory snapshot. Used when the FSM state is
    /// already `Ended` by the time disable cleanup runs. The repository only
    /// accepts ambiguous timer ends; authoritative end causes are preserved.
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
        _now: Timestamp,
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
            debug!(
                streamer_id,
                session_id, lost_cas, "end_for_disable: no eligible timer end to reattribute"
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
