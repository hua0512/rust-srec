//! Transactional session-end, user-disable, out-of-schedule, and cause-rewrite operations.

use jiff::Timestamp;
use sqlx::SqliteConnection;
use tracing::debug;

use super::SessionLifecycleRepository;
use crate::Result;
use crate::database::repositories::{
    MonitorOutboxTxOps, SessionEventTxOps, SessionTxOps, StreamerTxOps,
};
use crate::domain::StreamerState;
use crate::monitor::MonitorEvent;
use crate::session::events::{SessionEventPayload, TerminalCauseDto};
use crate::streamer::state_store::StateChange;

/// Inputs for [`SessionLifecycleRepository::end`].
#[derive(Debug, Clone)]
pub struct EndSessionInputs {
    pub streamer_id: String,
    pub streamer_name: String,
    /// Explicit session id to end; if `None`, falls back to the active
    /// session for `streamer_id` (if any).
    pub session_id: Option<String>,
    /// Caller observation, retained for diagnosing stale monitor state.
    /// The persisted state inside the transaction decides offline delivery.
    pub state_was_live: bool,
    /// `true` to clear accumulated transient errors on a clean offline obs.
    pub clear_errors: bool,
    /// With `clear_errors`, the failure count at which the streamer's checks
    /// alerted; clearing a run that reached it enqueues
    /// [`MonitorEvent::ChecksRestored`]. `None` never does.
    pub checks_restored_threshold: Option<i32>,
    /// Wire-format cause for the `session_ended` audit row.
    pub cause: crate::session::TerminalCauseDto,
    /// `true` if the session was in `Hysteresis` state when the end
    /// observation arrived.
    pub via_hysteresis: bool,
    pub now: Timestamp,
}

/// Outcome of [`SessionLifecycleRepository::end`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndSessionOutcome {
    pub resolved_session_id: Option<String>,
    pub offline_event_emitted: bool,
}

/// Inputs for [`SessionLifecycleRepository::end_for_out_of_schedule`].
#[derive(Debug, Clone)]
pub struct EndForOutOfScheduleInputs {
    pub streamer_id: String,
    pub streamer_name: String,
    /// Explicit session id to end; if `None`, falls back to the active
    /// session for `streamer_id` (if any).
    pub session_id: Option<String>,
    /// Streamer state before the schedule transition. Preserved on the
    /// `MonitorEvent::StateChanged` outbox event.
    pub old_state: StreamerState,
    pub via_hysteresis: bool,
    pub now: Timestamp,
}

impl SessionLifecycleRepository {
    /// Light "session ended" bundle: close the session row only, without
    /// touching streamer state or the outbox.
    pub async fn end_session_only(
        &self,
        streamer_id: &str,
        session_id: Option<&str>,
        cause: TerminalCauseDto,
        via_hysteresis: bool,
        now: Timestamp,
    ) -> Result<Option<String>> {
        let streamer_id = streamer_id.to_owned();
        let session_id = session_id.map(str::to_owned);
        self.write_transaction("session end_session_only", move |tx| {
            Box::pin(async move {
                Self::end_session_only_in_connection(
                    tx,
                    &streamer_id,
                    session_id.as_deref(),
                    cause,
                    via_hysteresis,
                    now,
                )
                .await
            })
        })
        .await
    }

    async fn end_session_only_in_connection(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        session_id: Option<&str>,
        cause: TerminalCauseDto,
        via_hysteresis: bool,
        now: Timestamp,
    ) -> Result<Option<String>> {
        let resolved = if let Some(id) = session_id {
            (SessionTxOps::end_session(&mut *tx, id, now).await? > 0).then(|| id.to_string())
        } else {
            SessionTxOps::end_active_session(&mut *tx, streamer_id, now).await?
        };

        if let Some(ref id) = resolved {
            let payload = SessionEventPayload::SessionEnded {
                cause,
                via_hysteresis,
            };
            let row = Self::event_row(id, streamer_id, &payload, now)?;
            SessionEventTxOps::insert(&mut *tx, &row).await?;
        }

        Ok(resolved)
    }

    /// Atomic "user disabled" tear-down: closes the active session row and
    /// inserts the `session_ended { cause: user_disabled }` audit row.
    pub async fn end_for_disable(
        &self,
        streamer_id: &str,
        session_id: Option<&str>,
        via_hysteresis: bool,
        now: Timestamp,
    ) -> Result<Option<String>> {
        let streamer_id = streamer_id.to_owned();
        let session_id = session_id.map(str::to_owned);
        self.write_transaction("session end_for_disable", move |tx| {
            Box::pin(async move {
                Self::end_for_disable_in_connection(
                    tx,
                    &streamer_id,
                    session_id.as_deref(),
                    via_hysteresis,
                    now,
                )
                .await
            })
        })
        .await
    }

    async fn end_for_disable_in_connection(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        session_id: Option<&str>,
        via_hysteresis: bool,
        now: Timestamp,
    ) -> Result<Option<String>> {
        let resolved: Option<String> = if let Some(id) = session_id {
            let still_active: Option<bool> =
                sqlx::query_scalar("SELECT end_time IS NULL FROM live_sessions WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            match still_active {
                Some(true) => Some(id.to_string()),
                _ => None,
            }
        } else {
            SessionTxOps::get_active_session_id(&mut *tx, streamer_id).await?
        };

        let Some(sid) = resolved else {
            return Ok(None);
        };

        SessionTxOps::end_session(&mut *tx, &sid, now).await?;

        let payload = SessionEventPayload::SessionEnded {
            cause: TerminalCauseDto::UserDisabled,
            via_hysteresis,
        };
        let row = Self::event_row(&sid, streamer_id, &payload, now)?;
        SessionEventTxOps::insert(&mut *tx, &row).await?;

        Ok(Some(sid))
    }

    /// Atomic "out of schedule" tear-down.
    pub async fn end_for_out_of_schedule(
        &self,
        inputs: EndForOutOfScheduleInputs,
    ) -> Result<Option<String>> {
        self.state_transaction("session end_for_out_of_schedule", move |tx| {
            Box::pin(Self::end_for_out_of_schedule_in_connection(tx, inputs))
        })
        .await
    }

    async fn end_for_out_of_schedule_in_connection(
        tx: &mut SqliteConnection,
        inputs: EndForOutOfScheduleInputs,
    ) -> Result<StateChange<Option<String>>> {
        let mut committed_row = None;
        if Self::inactive_state(&mut *tx, &inputs.streamer_id)
            .await?
            .is_some()
        {
            return Ok(StateChange::row(None, committed_row));
        }

        let resolved: Option<String> = if let Some(ref id) = inputs.session_id {
            let still_active: Option<bool> =
                sqlx::query_scalar("SELECT end_time IS NULL FROM live_sessions WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await?;
            match still_active {
                Some(true) => Some(id.clone()),
                _ => None,
            }
        } else {
            SessionTxOps::get_active_session_id(&mut *tx, &inputs.streamer_id).await?
        };

        if let Some(ref sid) = resolved {
            SessionTxOps::end_session(&mut *tx, sid, inputs.now).await?;

            let payload = SessionEventPayload::SessionEnded {
                cause: TerminalCauseDto::OutOfSchedule,
                via_hysteresis: inputs.via_hysteresis,
            };
            let row = Self::event_row(sid, &inputs.streamer_id, &payload, inputs.now)?;
            SessionEventTxOps::insert(&mut *tx, &row).await?;
        }

        committed_row = StreamerTxOps::update_state_row(
            &mut *tx,
            &inputs.streamer_id,
            &StreamerState::OutOfSchedule.to_string(),
        )
        .await?;

        if inputs.old_state != StreamerState::OutOfSchedule || resolved.is_some() {
            let event = MonitorEvent::StateChanged {
                streamer_id: inputs.streamer_id.clone(),
                streamer_name: inputs.streamer_name.clone(),
                old_state: inputs.old_state,
                new_state: StreamerState::OutOfSchedule,
                reason: Some("out_of_schedule".to_string()),
                timestamp: inputs.now,
            };
            MonitorOutboxTxOps::enqueue_event(&mut *tx, &inputs.streamer_id, &event).await?;
        }

        Ok(StateChange::row(resolved, committed_row))
    }

    /// Reattribute an ambiguous hysteresis-timer end to a later user stop.
    pub async fn rewrite_session_ended_cause(
        &self,
        session_id: &str,
        new_cause: TerminalCauseDto,
    ) -> Result<bool> {
        let session_id = session_id.to_owned();
        self.write_transaction("session rewrite_session_ended_cause", move |tx| {
            Box::pin(async move {
                Self::rewrite_session_ended_cause_in_connection(tx, &session_id, new_cause).await
            })
        })
        .await
    }

    async fn rewrite_session_ended_cause_in_connection(
        tx: &mut SqliteConnection,
        session_id: &str,
        new_cause: TerminalCauseDto,
    ) -> Result<bool> {
        let existing: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, payload FROM session_events
             WHERE session_id = ? AND kind = 'session_ended'
             ORDER BY occurred_at DESC, id DESC LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((row_id, payload_json)) = existing else {
            return Ok(false);
        };

        let mut payload: SessionEventPayload = match payload_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
        {
            Some(p) => p,
            None => {
                return Ok(false);
            }
        };

        match &mut payload {
            SessionEventPayload::SessionEnded {
                cause,
                via_hysteresis: true,
            } if matches!(
                cause,
                TerminalCauseDto::Completed | TerminalCauseDto::Failed { .. }
            ) =>
            {
                *cause = new_cause;
            }
            _ => {
                return Ok(false);
            }
        }

        let new_json = serde_json::to_string(&payload)?;
        sqlx::query("UPDATE session_events SET payload = ? WHERE id = ?")
            .bind(new_json)
            .bind(row_id)
            .execute(&mut *tx)
            .await?;
        Ok(true)
    }

    /// Atomic "session ended" bundle.
    pub async fn end(&self, inputs: EndSessionInputs) -> Result<EndSessionOutcome> {
        self.state_transaction("session end", move |tx| {
            Box::pin(Self::end_in_connection(tx, inputs))
        })
        .await
    }

    async fn end_in_connection(
        tx: &mut SqliteConnection,
        inputs: EndSessionInputs,
    ) -> Result<StateChange<EndSessionOutcome>> {
        let mut committed_row = None;
        if let Some(ref id) = inputs.session_id {
            let belongs_to_streamer: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM live_sessions WHERE id = ? AND streamer_id = ?)",
            )
            .bind(id)
            .bind(&inputs.streamer_id)
            .fetch_one(&mut *tx)
            .await?;
            let current =
                SessionTxOps::get_active_session_id(&mut *tx, &inputs.streamer_id).await?;
            // Session-scoped signals (for example a delayed danmu close) may
            // outlive their recording. They cannot take a successor offline.
            if !belongs_to_streamer || current.as_ref().is_some_and(|current| current != id) {
                return Ok(StateChange::row(
                    EndSessionOutcome {
                        resolved_session_id: None,
                        offline_event_emitted: false,
                    },
                    committed_row,
                ));
            }
        }
        let state = Self::authoritative_state(&mut *tx, &inputs.streamer_id).await?;
        if inputs.state_was_live != (state == Some(StreamerState::Live)) {
            debug!(
                streamer_id = %inputs.streamer_id,
                observed_live = inputs.state_was_live,
                persisted_state = ?state,
                "Offline observation state changed before persistence"
            );
        }

        let resolved_session_id = if let Some(ref id) = inputs.session_id {
            (SessionTxOps::end_session(&mut *tx, id, inputs.now).await? > 0).then(|| id.clone())
        } else {
            SessionTxOps::end_active_session(&mut *tx, &inputs.streamer_id, inputs.now).await?
        };

        let is_active = state.as_ref().is_some_and(StreamerState::is_active);
        let should_emit =
            is_active && (state == Some(StreamerState::Live) || resolved_session_id.is_some());

        if is_active {
            committed_row = StreamerTxOps::set_offline_row(&mut *tx, &inputs.streamer_id).await?;
        }

        if inputs.clear_errors && is_active {
            if let Some(threshold) = inputs.checks_restored_threshold {
                MonitorOutboxTxOps::enqueue_checks_restored(
                    &mut *tx,
                    &inputs.streamer_id,
                    threshold,
                    inputs.now,
                )
                .await?;
            }
            committed_row =
                StreamerTxOps::clear_error_state_row(&mut *tx, &inputs.streamer_id).await?;
        }

        if should_emit {
            let event = MonitorEvent::StreamerOffline {
                streamer_id: inputs.streamer_id.clone(),
                streamer_name: inputs.streamer_name.clone(),
                session_id: resolved_session_id.clone(),
                timestamp: inputs.now,
            };
            MonitorOutboxTxOps::enqueue_event(&mut *tx, &inputs.streamer_id, &event).await?;
        }

        if let Some(ref id) = resolved_session_id {
            let payload = SessionEventPayload::SessionEnded {
                cause: inputs.cause,
                via_hysteresis: inputs.via_hysteresis,
            };
            let row = Self::event_row(id, &inputs.streamer_id, &payload, inputs.now)?;
            SessionEventTxOps::insert(&mut *tx, &row).await?;
        }

        Ok(StateChange::row(
            EndSessionOutcome {
                resolved_session_id,
                offline_event_emitted: should_emit,
            },
            committed_row,
        ))
    }
}
