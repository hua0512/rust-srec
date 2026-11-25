//! Transactional SQL implementations for [`DbCommitOp`].

use std::time::Duration;

use jiff::Timestamp;
use sqlx::SqliteConnection;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::TxCommitPayload;
use crate::Result;
use crate::credentials::{CredentialBinding, CredentialIdentity};
use crate::monitor::InfraBlockReason;
use crate::recorder::fsm::error::{disabled_until_for_error_count, should_emit_checks_failing};
use crate::recorder::ports::{
    DbCommitOp, DbCommitOutcome, OutboxEvent, SessionStartDisposition, StreamOnlineKind,
};
use crate::session::TerminalCause;
use crate::session::events::{SessionEventPayload, TerminalCauseDto};
use crate::streamer::state_store::StateChange;
use srec_core::domain::StreamerState;
use srec_sqlite::models::{SessionEventDbModel, StreamerDbModel};
use srec_sqlite::repositories::{
    MonitorOutboxTxOps, SessionEventTxOps, SessionTxOps, StreamerTxOps, credential_profiles,
};
use srec_sqlite::time::{datetime_to_ms, ms_to_datetime};

const STALE_ACTIVE_REPLACED_REASON: &str = "stale_active_replaced";

async fn enqueue_outbox_event(
    tx: &mut SqliteConnection,
    streamer_id: Option<&str>,
    event: &OutboxEvent,
    created_at_ms: i64,
) -> Result<()> {
    let payload = serde_json::to_string(event)
        .map_err(|e| crate::Error::Other(format!("Failed to serialize outbox event: {}", e)))?;
    Ok(MonitorOutboxTxOps::enqueue_raw(
        tx,
        streamer_id,
        event.event_type(),
        &payload,
        created_at_ms,
    )
    .await?)
}

pub(super) fn commit_op_label(op: &DbCommitOp) -> &'static str {
    match op {
        DbCommitOp::StartOrResumeSession { .. } => "recorder start_or_resume_session",
        DbCommitOp::ResumeHysteresis { .. } => "recorder resume_hysteresis",
        DbCommitOp::EndSession { .. } => "recorder end_session",
        DbCommitOp::EndForCredentialRetirement { .. } => "recorder end_for_credential_retirement",
        DbCommitOp::RewriteEndedCause { .. } => "recorder rewrite_ended_cause",
        DbCommitOp::CommitCredentialBinding { .. } => "recorder commit_credential_binding",
        DbCommitOp::RecordFilteredState { .. } => "recorder record_filtered_state",
        DbCommitOp::RecordFatalError { .. } => "recorder record_fatal_error",
        DbCommitOp::RecordTransientError { .. } => "recorder record_transient_error",
        DbCommitOp::ClearErrors { .. } => "recorder clear_errors",
        DbCommitOp::ClearLastError { .. } => "recorder clear_last_error",
        DbCommitOp::SetInfraBlocked { .. } => "recorder set_infra_blocked",
        DbCommitOp::AddSessionTitle { .. } => "recorder add_session_title",
        DbCommitOp::RecordHysteresisEntered { .. } => "recorder record_hysteresis_entered",
    }
}

pub(super) async fn execute_commit_op(
    tx: &mut SqliteConnection,
    op: DbCommitOp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    match op {
        DbCommitOp::StartOrResumeSession {
            streamer_id,
            streamer_name,
            title,
            category,
            current_avatar,
            new_avatar,
            credential_binding,
            now,
        } => {
            start_or_resume_session(
                tx,
                StartOrResumeParams {
                    streamer_id,
                    streamer_name,
                    title,
                    category,
                    current_avatar,
                    new_avatar,
                    credential_binding,
                    now,
                },
            )
            .await
        }
        DbCommitOp::ResumeHysteresis {
            streamer_id,
            streamer_name,
            session_id,
            title,
            category,
            hysteresis_duration_secs,
            now,
        } => {
            resume_hysteresis(
                tx,
                ResumeHysteresisParams {
                    streamer_id,
                    streamer_name,
                    session_id,
                    title,
                    category,
                    hysteresis_duration_secs,
                    now,
                },
            )
            .await
        }
        DbCommitOp::EndSession {
            streamer_id,
            streamer_name,
            session_id,
            cause,
            via_hysteresis,
            update_streamer_state,
            clear_errors,
            checks_restored_threshold,
            infra_block,
            suppress_offline_notification,
            now,
        } => {
            end_session(
                tx,
                EndSessionParams {
                    streamer_id,
                    streamer_name,
                    session_id,
                    cause,
                    via_hysteresis,
                    update_streamer_state,
                    clear_errors,
                    checks_restored_threshold,
                    infra_block,
                    suppress_offline_notification,
                    now,
                },
            )
            .await
        }
        DbCommitOp::EndForCredentialRetirement {
            streamer_id,
            session_id,
            binding,
            via_hysteresis,
            now,
            ..
        } => {
            end_for_credential_retirement(
                tx,
                &streamer_id,
                &session_id,
                &binding,
                via_hysteresis,
                now,
            )
            .await
        }
        DbCommitOp::RewriteEndedCause {
            session_id,
            new_cause,
        } => rewrite_ended_cause(tx, &session_id, new_cause).await,
        DbCommitOp::CommitCredentialBinding {
            streamer_id,
            session_id,
            binding,
        } => commit_credential_binding(tx, &streamer_id, &session_id, &binding).await,
        DbCommitOp::RecordFilteredState {
            streamer_id,
            new_state,
            ..
        } => record_filtered_state(tx, &streamer_id, new_state).await,
        DbCommitOp::RecordFatalError {
            streamer_id,
            streamer_name,
            session_id,
            error_type,
            message,
            new_state,
            now,
        } => {
            record_fatal_error(
                tx,
                &streamer_id,
                &streamer_name,
                session_id.as_deref(),
                error_type,
                &message,
                new_state,
                now,
            )
            .await
        }
        DbCommitOp::RecordTransientError {
            streamer_id,
            streamer_name,
            platform,
            error,
            offline_check_count,
            now,
        } => {
            record_transient_error(
                tx,
                &streamer_id,
                &streamer_name,
                &platform,
                &error,
                offline_check_count,
                now,
            )
            .await
        }
        DbCommitOp::ClearErrors {
            streamer_id,
            streamer_name,
            checks_restored_threshold,
            set_state,
            now,
        } => {
            clear_errors(
                tx,
                &streamer_id,
                &streamer_name,
                checks_restored_threshold,
                set_state,
                now,
            )
            .await
        }
        DbCommitOp::ClearLastError { streamer_id } => clear_last_error(tx, &streamer_id).await,
        DbCommitOp::SetInfraBlocked {
            streamer_id,
            reason,
            now,
        } => set_infra_blocked(tx, &streamer_id, &reason, now).await,
        DbCommitOp::AddSessionTitle {
            session_id,
            title,
            now,
        } => add_session_title(tx, &session_id, &title, now).await,
        DbCommitOp::RecordHysteresisEntered {
            streamer_id,
            session_id,
            cause,
            resume_deadline,
            occurred_at,
        } => {
            record_hysteresis_entered(
                tx,
                &streamer_id,
                &session_id,
                cause,
                resume_deadline,
                occurred_at,
            )
            .await
        }
    }
}

struct StartOrResumeParams {
    streamer_id: String,
    streamer_name: String,
    title: String,
    category: Option<String>,
    current_avatar: Option<String>,
    new_avatar: Option<String>,
    credential_binding: Option<CredentialBinding>,
    now: Timestamp,
}

struct ResumeHysteresisParams {
    streamer_id: String,
    streamer_name: String,
    session_id: String,
    title: String,
    category: Option<String>,
    hysteresis_duration_secs: u64,
    now: Timestamp,
}

struct EndSessionParams {
    streamer_id: String,
    streamer_name: String,
    session_id: Option<String>,
    cause: TerminalCause,
    via_hysteresis: bool,
    update_streamer_state: Option<StreamerState>,
    clear_errors: bool,
    checks_restored_threshold: Option<i32>,
    infra_block: Option<InfraBlockReason>,
    suppress_offline_notification: bool,
    now: Timestamp,
}

async fn start_suppression(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    now_ms: i64,
) -> Result<Option<(StreamerState, Option<Duration>)>> {
    let row: Option<(String, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT state, deleted_at, disabled_until FROM streamers WHERE id = ?")
            .bind(streamer_id)
            .fetch_optional(tx)
            .await?;

    let Some((raw_state, deleted_at, disabled_until)) = row else {
        return Ok(Some((StreamerState::Disabled, None)));
    };
    if deleted_at.is_some() {
        return Ok(Some((StreamerState::Disabled, None)));
    }
    let state = StreamerState::parse(&raw_state).unwrap_or(StreamerState::Disabled);
    if !state.is_active() {
        return Ok(Some((state, None)));
    }
    let remaining_backoff = disabled_until
        .filter(|&until_ms| until_ms > now_ms)
        .map(|until_ms| Duration::from_millis(u64::try_from(until_ms - now_ms).unwrap_or(0)));
    if state == StreamerState::OutOfSpace {
        return Ok(Some((StreamerState::OutOfSpace, remaining_backoff)));
    }
    if let Some(remaining) = remaining_backoff {
        return Ok(Some((StreamerState::TemporalDisabled, Some(remaining))));
    }
    Ok(None)
}

async fn authoritative_state(
    tx: &mut SqliteConnection,
    streamer_id: &str,
) -> Result<Option<StreamerState>> {
    let row: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT state, deleted_at FROM streamers WHERE id = ?")
            .bind(streamer_id)
            .fetch_optional(tx)
            .await?;
    Ok(row.and_then(|(state, deleted_at)| {
        if deleted_at.is_some() {
            Some(StreamerState::Disabled)
        } else {
            StreamerState::parse(&state)
        }
    }))
}

fn event_row(
    session_id: &str,
    streamer_id: &str,
    payload: &SessionEventPayload,
    occurred_at_ms: i64,
) -> Result<SessionEventDbModel> {
    let encoded_payload = serde_json::to_string(payload)?;
    Ok(SessionEventDbModel {
        id: 0,
        session_id: session_id.to_owned(),
        streamer_id: streamer_id.to_owned(),
        kind: payload.kind().as_str().to_owned(),
        occurred_at: occurred_at_ms,
        payload: Some(encoded_payload),
    })
}

fn suppresses_offline_notification(cause: &TerminalCause) -> bool {
    matches!(
        cause,
        TerminalCause::UserDisabled | TerminalCause::OutOfSchedule
    ) || matches!(
        cause,
        TerminalCause::Rejected { reason } if reason == STALE_ACTIVE_REPLACED_REASON
    )
}

async fn start_or_resume_session(
    tx: &mut SqliteConnection,
    params: StartOrResumeParams,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let now_ms = datetime_to_ms(params.now);
    let exact_now = ms_to_datetime(now_ms);

    if let Some((state, retry_after)) =
        start_suppression(&mut *tx, &params.streamer_id, now_ms).await?
    {
        info!(
            streamer_id = %params.streamer_id,
            %state,
            "start_or_resume_session suppressed by database guard"
        );
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::SessionStartSuppressed { state, retry_after },
        ));
    }

    let last = SessionTxOps::get_last_session(&mut *tx, &params.streamer_id).await?;
    let keep_id = last
        .as_ref()
        .filter(|session| session.end_time.is_none())
        .map(|session| session.id.as_str());

    // Self-heal stale open rows without emitting StreamOffline notifications.
    let cleaned = SessionTxOps::end_all_active_for_streamer(
        &mut *tx,
        &params.streamer_id,
        keep_id,
        exact_now,
    )
    .await?;
    if !cleaned.is_empty() {
        warn!(
            streamer_id = %params.streamer_id,
            kept = keep_id.unwrap_or("<none>"),
            count = cleaned.len(),
            "Closed stale active session rows before start_or_resume"
        );
    }
    for stale_id in &cleaned {
        let payload = SessionEventPayload::SessionEnded {
            cause: TerminalCauseDto::Rejected {
                reason: STALE_ACTIVE_REPLACED_REASON.to_owned(),
            },
            via_hysteresis: false,
        };
        let row = event_row(stale_id, &params.streamer_id, &payload, now_ms)?;
        SessionEventTxOps::insert(&mut *tx, &row).await?;
    }

    let (session_id, session_started_at, disposition, online_kind) = match last {
        Some(session) if session.end_time.is_none() => {
            debug!(session_id = %session.id, "Reusing active recording session");
            SessionTxOps::update_titles(
                &mut *tx,
                &session.id,
                session.titles.as_deref(),
                &params.title,
                exact_now,
            )
            .await?;
            (
                session.id,
                ms_to_datetime(session.start_time),
                SessionStartDisposition::ReusedActive,
                StreamOnlineKind::ReusedActive,
            )
        }
        _ => {
            let new_id = Uuid::new_v4().to_string();
            SessionTxOps::create_session(
                &mut *tx,
                &new_id,
                &params.streamer_id,
                &params.streamer_name,
                exact_now,
                &params.title,
            )
            .await?;
            let payload = SessionEventPayload::SessionStarted {
                from_hysteresis: false,
                title: Some(params.title.clone()),
            };
            let row = event_row(&new_id, &params.streamer_id, &payload, now_ms)?;
            SessionEventTxOps::insert(&mut *tx, &row).await?;
            info!(session_id = %new_id, streamer_id = %params.streamer_id, "Created new recording session");
            (
                new_id,
                exact_now,
                SessionStartDisposition::Created,
                StreamOnlineKind::Created,
            )
        }
    };

    let mut committed_row =
        StreamerTxOps::set_live_row(&mut *tx, &params.streamer_id, exact_now).await?;
    if let Some(ref new_avatar) = params.new_avatar
        && !new_avatar.is_empty()
        && params.new_avatar != params.current_avatar
    {
        committed_row =
            StreamerTxOps::update_avatar_row(&mut *tx, &params.streamer_id, new_avatar).await?;
    }

    let credential_binding = if let Some(binding) = params.credential_binding {
        Some(
            credential_profiles::commit_session_binding(
                &mut *tx,
                &session_id,
                &params.streamer_id,
                &binding,
                false,
            )
            .await?,
        )
    } else {
        None
    };

    let outbox_event = OutboxEvent::StreamOnline {
        streamer_id: params.streamer_id.clone(),
        streamer_name: params.streamer_name,
        session_id: session_id.clone(),
        title: params.title,
        category: params.category,
        started_at_ms: now_ms,
        kind: online_kind,
    };
    enqueue_outbox_event(&mut *tx, Some(&params.streamer_id), &outbox_event, now_ms).await?;

    Ok(TxCommitPayload {
        value: DbCommitOutcome::SessionStarted {
            session_id,
            started_at: session_started_at,
            disposition,
            credential_binding,
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: true,
    })
}

async fn resume_hysteresis(
    tx: &mut SqliteConnection,
    params: ResumeHysteresisParams,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let now_ms = datetime_to_ms(params.now);
    let exact_now = ms_to_datetime(now_ms);

    if let Some((state, retry_after)) =
        start_suppression(&mut *tx, &params.streamer_id, now_ms).await?
    {
        info!(
            streamer_id = %params.streamer_id,
            session_id = %params.session_id,
            %state,
            "resume_hysteresis suppressed by database guard"
        );
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::HysteresisResumeSuppressed { state, retry_after },
        ));
    }

    let active_row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT start_time, titles FROM live_sessions \
         WHERE id = ? AND streamer_id = ? AND end_time IS NULL",
    )
    .bind(&params.session_id)
    .bind(&params.streamer_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((start_time_ms, current_titles)) = active_row else {
        return Err(crate::Error::Conflict(format!(
            "session {} is no longer active for hysteresis resume",
            params.session_id
        )));
    };

    let committed_row =
        StreamerTxOps::set_live_row(&mut *tx, &params.streamer_id, exact_now).await?;
    SessionTxOps::update_titles(
        &mut *tx,
        &params.session_id,
        current_titles.as_deref(),
        &params.title,
        exact_now,
    )
    .await?;

    let resumed_payload = SessionEventPayload::SessionResumed {
        hysteresis_duration_secs: params.hysteresis_duration_secs,
    };
    let resumed_row = event_row(
        &params.session_id,
        &params.streamer_id,
        &resumed_payload,
        now_ms,
    )?;
    SessionEventTxOps::insert(&mut *tx, &resumed_row).await?;

    let started_payload = SessionEventPayload::SessionStarted {
        from_hysteresis: true,
        title: Some(params.title.clone()),
    };
    let started_row = event_row(
        &params.session_id,
        &params.streamer_id,
        &started_payload,
        now_ms,
    )?;
    SessionEventTxOps::insert(&mut *tx, &started_row).await?;

    let outbox_event = OutboxEvent::StreamOnline {
        streamer_id: params.streamer_id.clone(),
        streamer_name: params.streamer_name,
        session_id: params.session_id.clone(),
        title: params.title,
        category: params.category,
        started_at_ms: now_ms,
        kind: StreamOnlineKind::HysteresisResumed,
    };
    enqueue_outbox_event(&mut *tx, Some(&params.streamer_id), &outbox_event, now_ms).await?;

    Ok(TxCommitPayload {
        value: DbCommitOutcome::HysteresisResumed {
            session_id: params.session_id,
            started_at: ms_to_datetime(start_time_ms),
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: true,
    })
}

async fn apply_infra_block_in_tx(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    reason: &InfraBlockReason,
    exact_now: Timestamp,
) -> Result<(Option<StreamerDbModel>, StreamerState, Timestamp)> {
    let disabled_until = exact_now
        + jiff::SignedDuration::from_secs(reason.retry_after_secs().min(i64::MAX as u64) as i64);
    let target_state = reason.target_state();
    let last_error_override = reason.last_error_override();

    let mut committed_row =
        StreamerTxOps::set_disabled_until_row(&mut *tx, streamer_id, Some(disabled_until)).await?;
    if target_state != StreamerState::TemporalDisabled {
        committed_row =
            StreamerTxOps::update_state_row(&mut *tx, streamer_id, &target_state.to_string())
                .await?;
    }
    if let Some(msg) = last_error_override {
        committed_row =
            StreamerTxOps::update_last_error_row(&mut *tx, streamer_id, Some(&msg)).await?;
    }
    Ok((committed_row, target_state, disabled_until))
}

async fn enqueue_checks_restored_if_due(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    fallback_name: &str,
    threshold: i32,
    now_ms: i64,
) -> Result<bool> {
    let row: Option<(String, Option<i32>)> =
        sqlx::query_as("SELECT name, consecutive_error_count FROM streamers WHERE id = ?")
            .bind(streamer_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((db_name, Some(count))) = row
        && count >= threshold
    {
        let streamer_name = if db_name.is_empty() {
            fallback_name.to_owned()
        } else {
            db_name
        };
        let event = OutboxEvent::ChecksRestored {
            streamer_id: streamer_id.to_owned(),
            streamer_name,
            occurred_at_ms: now_ms,
        };
        enqueue_outbox_event(&mut *tx, Some(streamer_id), &event, now_ms).await?;
        return Ok(true);
    }
    Ok(false)
}

async fn end_session(
    tx: &mut SqliteConnection,
    params: EndSessionParams,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let now_ms = datetime_to_ms(params.now);
    let exact_now = ms_to_datetime(now_ms);
    let mut committed_row: Option<StreamerDbModel> = None;
    let mut outbox_enqueued = false;

    if let Some(ref id) = params.session_id {
        let belongs_to_streamer: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM live_sessions WHERE id = ? AND streamer_id = ?)",
        )
        .bind(id)
        .bind(&params.streamer_id)
        .fetch_one(&mut *tx)
        .await?;
        let current = SessionTxOps::get_active_session_id(&mut *tx, &params.streamer_id).await?;
        if !belongs_to_streamer || current.as_deref().is_some_and(|cur| cur != id.as_str()) {
            return Ok(TxCommitPayload::plain(DbCommitOutcome::SessionEnded {
                session_id: None,
                started_at: None,
                ended_at: exact_now,
                transitioned_from_open: false,
                offline_event_emitted: false,
                checks_restored_emitted: false,
            }));
        }
    }

    let open_session: Option<(String, i64)> = if let Some(ref id) = params.session_id {
        sqlx::query_as(
            "SELECT id, start_time FROM live_sessions \
             WHERE id = ? AND streamer_id = ? AND end_time IS NULL",
        )
        .bind(id)
        .bind(&params.streamer_id)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, start_time FROM live_sessions \
             WHERE streamer_id = ? AND end_time IS NULL \
             ORDER BY start_time DESC LIMIT 1",
        )
        .bind(&params.streamer_id)
        .fetch_optional(&mut *tx)
        .await?
    };

    let (resolved_session_id, started_at_ms, transitioned_from_open) =
        if let Some((sid, start_ms)) = open_session {
            let affected = SessionTxOps::end_session(&mut *tx, &sid, exact_now).await?;
            if affected > 0 {
                let payload = SessionEventPayload::SessionEnded {
                    cause: (&params.cause).into(),
                    via_hysteresis: params.via_hysteresis,
                };
                let row = event_row(&sid, &params.streamer_id, &payload, now_ms)?;
                SessionEventTxOps::insert(&mut *tx, &row).await?;
                (Some(sid), Some(start_ms), true)
            } else {
                (None, None, false)
            }
        } else {
            (None, None, false)
        };

    let state = authoritative_state(&mut *tx, &params.streamer_id).await?;
    let is_active = state.is_some_and(|s| s.is_active());

    if let Some(target_state) = params.update_streamer_state
        && is_active
    {
        committed_row = if target_state == StreamerState::NotLive {
            StreamerTxOps::set_offline_row(&mut *tx, &params.streamer_id).await?
        } else {
            StreamerTxOps::update_state_row(
                &mut *tx,
                &params.streamer_id,
                &target_state.to_string(),
            )
            .await?
        };
    }

    let mut checks_restored_emitted = false;
    if params.clear_errors && is_active {
        if let Some(threshold) = params.checks_restored_threshold
            && enqueue_checks_restored_if_due(
                &mut *tx,
                &params.streamer_id,
                &params.streamer_name,
                threshold,
                now_ms,
            )
            .await?
        {
            checks_restored_emitted = true;
            outbox_enqueued = true;
        }
        committed_row = StreamerTxOps::clear_error_state_row(&mut *tx, &params.streamer_id).await?;
    }

    if let Some(ref reason) = params.infra_block
        && StreamerTxOps::monitor_may_write(&mut *tx, &params.streamer_id).await?
    {
        let (row, _, _) =
            apply_infra_block_in_tx(&mut *tx, &params.streamer_id, reason, exact_now).await?;
        committed_row = row;
    }

    // Emit StreamOffline ONLY when `live_sessions.end_time` transitioned
    // NULL -> Some and the cause is not suppressed.
    let mut offline_event_emitted = false;
    if transitioned_from_open
        && !params.suppress_offline_notification
        && !suppresses_offline_notification(&params.cause)
        && let Some(ref sid) = resolved_session_id
    {
        let duration_secs =
            started_at_ms.map(|start_ms| (now_ms - start_ms).max(0) as f64 / 1000.0);
        let event = OutboxEvent::StreamOffline {
            streamer_id: params.streamer_id.clone(),
            streamer_name: params.streamer_name,
            session_id: sid.clone(),
            duration_secs,
            ended_at_ms: now_ms,
        };
        enqueue_outbox_event(&mut *tx, Some(&params.streamer_id), &event, now_ms).await?;
        offline_event_emitted = true;
        outbox_enqueued = true;
    }

    Ok(TxCommitPayload {
        value: DbCommitOutcome::SessionEnded {
            session_id: resolved_session_id,
            started_at: started_at_ms.map(ms_to_datetime),
            ended_at: exact_now,
            transitioned_from_open,
            offline_event_emitted,
            checks_restored_emitted,
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued,
    })
}

async fn end_for_credential_retirement(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    session_id: &str,
    binding: &CredentialBinding,
    via_hysteresis: bool,
    now: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let CredentialIdentity::Profile { profile_id } = &binding.identity else {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::CredentialRetirementEnded {
                session_id: session_id.to_owned(),
                ended: false,
            },
        ));
    };

    let retiring_matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(\
            SELECT 1 FROM live_sessions s \
            JOIN retirement_credential_profiles r \
              ON r.profile_id = json_extract(s.credential_binding, '$.identity.profile_id') \
            WHERE s.id = ? \
              AND s.streamer_id = ? \
              AND s.end_time IS NULL \
              AND r.profile_id = ? \
              AND json_extract(s.credential_binding, '$.epoch') = ?\
        )",
    )
    .bind(session_id)
    .bind(streamer_id)
    .bind(profile_id)
    .bind(binding.epoch as i64)
    .fetch_one(&mut *tx)
    .await?;

    if !retiring_matches {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::CredentialRetirementEnded {
                session_id: session_id.to_owned(),
                ended: false,
            },
        ));
    }

    let now_ms = datetime_to_ms(now);
    let exact_now = ms_to_datetime(now_ms);
    let affected = SessionTxOps::end_session(&mut *tx, session_id, exact_now).await?;
    let ended = affected > 0;
    if ended {
        let payload = SessionEventPayload::SessionEnded {
            cause: TerminalCauseDto::UserDisabled,
            via_hysteresis,
        };
        let row = event_row(session_id, streamer_id, &payload, now_ms)?;
        SessionEventTxOps::insert(&mut *tx, &row).await?;
    }

    Ok(TxCommitPayload::plain(
        DbCommitOutcome::CredentialRetirementEnded {
            session_id: session_id.to_owned(),
            ended,
        },
    ))
}

async fn rewrite_ended_cause(
    tx: &mut SqliteConnection,
    session_id: &str,
    new_cause: TerminalCauseDto,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let existing: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id, payload FROM session_events \
         WHERE session_id = ? AND kind = 'session_ended' \
         ORDER BY occurred_at DESC, id DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((row_id, payload_json)) = existing else {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::EndedCauseRewritten {
                session_id: session_id.to_owned(),
                updated: false,
            },
        ));
    };

    let Some(mut payload) = payload_json
        .as_deref()
        .map(serde_json::from_str::<SessionEventPayload>)
        .transpose()?
    else {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::EndedCauseRewritten {
                session_id: session_id.to_owned(),
                updated: false,
            },
        ));
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
            return Ok(TxCommitPayload::plain(
                DbCommitOutcome::EndedCauseRewritten {
                    session_id: session_id.to_owned(),
                    updated: false,
                },
            ));
        }
    }

    let new_json = serde_json::to_string(&payload)?;
    sqlx::query("UPDATE session_events SET payload = ? WHERE id = ?")
        .bind(new_json)
        .bind(row_id)
        .execute(&mut *tx)
        .await?;

    Ok(TxCommitPayload::plain(
        DbCommitOutcome::EndedCauseRewritten {
            session_id: session_id.to_owned(),
            updated: true,
        },
    ))
}

async fn commit_credential_binding(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    session_id: &str,
    binding: &CredentialBinding,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let committed =
        credential_profiles::commit_session_binding(tx, session_id, streamer_id, binding, true)
            .await?;
    Ok(TxCommitPayload::plain(
        DbCommitOutcome::CredentialBindingCommitted {
            session_id: session_id.to_owned(),
            binding: committed,
        },
    ))
}

async fn record_filtered_state(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    new_state: StreamerState,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    if !StreamerTxOps::monitor_may_write(&mut *tx, streamer_id).await? {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::FilteredStateRecorded,
        ));
    }
    let committed_row =
        StreamerTxOps::update_state_row(&mut *tx, streamer_id, &new_state.to_string()).await?;
    Ok(TxCommitPayload {
        value: DbCommitOutcome::FilteredStateRecorded,
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: false,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "transactional fatal error persistence binds every explicit domain input"
)]
async fn record_fatal_error(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    streamer_name: &str,
    session_id: Option<&str>,
    error_type: srec_core::domain::streamer::FatalErrorType,
    message: &str,
    new_state: StreamerState,
    now: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    if !StreamerTxOps::monitor_may_write(&mut *tx, streamer_id).await? {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::FatalErrorRecorded {
                session_ended: None,
                new_state,
            },
        ));
    }

    let now_ms = datetime_to_ms(now);
    let exact_now = ms_to_datetime(now_ms);

    let open_session: Option<(String, i64)> = if let Some(sid) = session_id {
        sqlx::query_as(
            "SELECT id, start_time FROM live_sessions \
             WHERE id = ? AND streamer_id = ? AND end_time IS NULL",
        )
        .bind(sid)
        .bind(streamer_id)
        .fetch_optional(&mut *tx)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, start_time FROM live_sessions \
             WHERE streamer_id = ? AND end_time IS NULL \
             ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(&mut *tx)
        .await?
    };

    let session_ended = if let Some((sid, _start_ms)) = open_session {
        if SessionTxOps::end_session(&mut *tx, &sid, exact_now).await? > 0 {
            let ended_payload = SessionEventPayload::SessionEnded {
                cause: TerminalCauseDto::StreamerOffline,
                via_hysteresis: false,
            };
            let row = event_row(&sid, streamer_id, &ended_payload, now_ms)?;
            SessionEventTxOps::insert(&mut *tx, &row).await?;
            Some(sid)
        } else {
            None
        }
    } else {
        None
    };

    let committed_row =
        StreamerTxOps::set_fatal_error_row(&mut *tx, streamer_id, &new_state.to_string(), message)
            .await?;

    let fatal_event = OutboxEvent::FatalError {
        streamer_id: streamer_id.to_owned(),
        streamer_name: streamer_name.to_owned(),
        error_type,
        message: message.to_owned(),
        new_state,
        occurred_at_ms: now_ms,
    };
    enqueue_outbox_event(&mut *tx, Some(streamer_id), &fatal_event, now_ms).await?;

    Ok(TxCommitPayload {
        value: DbCommitOutcome::FatalErrorRecorded {
            session_ended,
            new_state,
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: true,
    })
}

async fn record_transient_error(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    streamer_name: &str,
    platform: &str,
    error: &str,
    offline_check_count: u32,
    now: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    if !StreamerTxOps::monitor_may_write(&mut *tx, streamer_id).await? {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 0,
                disabled_until: None,
                checks_failing_emitted: false,
            },
        ));
    }

    let now_ms = datetime_to_ms(now);
    let exact_now = ms_to_datetime(now_ms);

    let incremented = StreamerTxOps::increment_error_row(&mut *tx, streamer_id, error).await?;
    let new_error_count = incremented.consecutive_error_count.unwrap_or(0);
    let disabled_until =
        disabled_until_for_error_count(new_error_count, offline_check_count, exact_now);
    let committed_row =
        StreamerTxOps::set_disabled_until_row(&mut *tx, streamer_id, disabled_until).await?;

    let checks_failing_emitted = should_emit_checks_failing(new_error_count, offline_check_count);
    if checks_failing_emitted {
        let event = OutboxEvent::ChecksFailing {
            streamer_id: streamer_id.to_owned(),
            streamer_name: streamer_name.to_owned(),
            platform: platform.to_owned(),
            last_error: error.to_owned(),
            consecutive_errors: new_error_count,
            occurred_at_ms: now_ms,
        };
        enqueue_outbox_event(&mut *tx, Some(streamer_id), &event, now_ms).await?;
    }

    Ok(TxCommitPayload {
        value: DbCommitOutcome::TransientErrorRecorded {
            consecutive_error_count: new_error_count,
            disabled_until,
            checks_failing_emitted,
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: checks_failing_emitted,
    })
}

async fn clear_errors(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    streamer_name: &str,
    checks_restored_threshold: i32,
    set_state: Option<StreamerState>,
    now: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    if !StreamerTxOps::monitor_may_write(&mut *tx, streamer_id).await? {
        return Ok(TxCommitPayload::plain(DbCommitOutcome::ErrorsCleared {
            checks_restored_emitted: false,
        }));
    }

    let now_ms = datetime_to_ms(now);
    let exact_now = ms_to_datetime(now_ms);

    let checks_restored_emitted = enqueue_checks_restored_if_due(
        &mut *tx,
        streamer_id,
        streamer_name,
        checks_restored_threshold,
        now_ms,
    )
    .await?;

    let mut committed_row = StreamerTxOps::clear_error_state_row(&mut *tx, streamer_id).await?;
    if let Some(target_state) = set_state {
        committed_row = if target_state == StreamerState::Live {
            StreamerTxOps::set_live_row(&mut *tx, streamer_id, exact_now).await?
        } else {
            StreamerTxOps::update_state_row(&mut *tx, streamer_id, &target_state.to_string())
                .await?
        };
    }

    Ok(TxCommitPayload {
        value: DbCommitOutcome::ErrorsCleared {
            checks_restored_emitted,
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: checks_restored_emitted,
    })
}

async fn clear_last_error(
    tx: &mut SqliteConnection,
    streamer_id: &str,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    if !StreamerTxOps::monitor_may_write(&mut *tx, streamer_id).await? {
        return Ok(TxCommitPayload::plain(DbCommitOutcome::LastErrorCleared));
    }
    let committed_row = StreamerTxOps::update_last_error_row(&mut *tx, streamer_id, None).await?;
    Ok(TxCommitPayload {
        value: DbCommitOutcome::LastErrorCleared,
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: false,
    })
}

async fn set_infra_blocked(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    reason: &InfraBlockReason,
    now: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let now_ms = datetime_to_ms(now);
    let exact_now = ms_to_datetime(now_ms);
    let target_state = reason.target_state();
    let disabled_until = exact_now
        + jiff::SignedDuration::from_secs(reason.retry_after_secs().min(i64::MAX as u64) as i64);

    if !StreamerTxOps::monitor_may_write(&mut *tx, streamer_id).await? {
        return Ok(TxCommitPayload::plain(
            DbCommitOutcome::InfraBlockedRecorded {
                state: target_state,
                disabled_until,
            },
        ));
    }

    let (committed_row, state, disabled_until) =
        apply_infra_block_in_tx(&mut *tx, streamer_id, reason, exact_now).await?;
    Ok(TxCommitPayload {
        value: DbCommitOutcome::InfraBlockedRecorded {
            state,
            disabled_until,
        },
        state_change: StateChange::row((), committed_row),
        outbox_enqueued: false,
    })
}

async fn add_session_title(
    tx: &mut SqliteConnection,
    session_id: &str,
    title: &str,
    now: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let now_ms = datetime_to_ms(now);
    let exact_now = ms_to_datetime(now_ms);
    let titles: Option<Option<String>> =
        sqlx::query_scalar("SELECT titles FROM live_sessions WHERE id = ? AND end_time IS NULL")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
    let changed = match titles {
        Some(existing) => {
            SessionTxOps::update_titles(tx, session_id, existing.as_deref(), title, exact_now)
                .await?
        }
        None => false,
    };
    Ok(TxCommitPayload::plain(DbCommitOutcome::SessionTitleAdded {
        changed,
    }))
}

async fn record_hysteresis_entered(
    tx: &mut SqliteConnection,
    streamer_id: &str,
    session_id: &str,
    cause: TerminalCauseDto,
    resume_deadline: Timestamp,
    occurred_at: Timestamp,
) -> Result<TxCommitPayload<DbCommitOutcome>> {
    let occurred_at_ms = datetime_to_ms(occurred_at);
    let payload = SessionEventPayload::HysteresisEntered {
        cause,
        resume_deadline,
    };
    let row = event_row(session_id, streamer_id, &payload, occurred_at_ms)?;
    SessionEventTxOps::insert(&mut *tx, &row).await?;
    Ok(TxCommitPayload::plain(
        DbCommitOutcome::HysteresisEnteredRecorded,
    ))
}
