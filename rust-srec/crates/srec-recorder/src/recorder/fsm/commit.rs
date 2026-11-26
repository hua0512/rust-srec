//! Authoritative database commit reconciliation (`StartOrResumeSession`,
//! `ResumeHysteresis`, `EndSession`, credential binding commits, and error
//! persistence).

use std::collections::HashSet;

use crate::session::TerminalCause;
use srec_core::domain::StreamerState;

use super::hysteresis;
use super::schedule;
use super::segment::SessionSegmentGuard;
use super::state::{
    AttemptPhase, BlockedKind, DanmuSessionState, FsmEffect, MonitorPhase, OpenSession,
    PendingDbKind, RecoveredMedia, SessionSlot, StepContext, StopDbEnd, StreamerFsm,
};
use crate::recorder::ports::{
    ActorStopReason, DbCommitFailure, DbCommitOutcome, PipelineNotification,
};

impl StreamerFsm {
    pub(super) fn on_db_committed(
        &mut self,
        ctx: StepContext,
        op_id: u64,
        result: std::result::Result<DbCommitOutcome, DbCommitFailure>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let Some(pending) = self.pending_db.take() else {
            return;
        };
        if pending.op_id != op_id {
            self.pending_db = Some(pending);
            return;
        }

        match pending.kind {
            PendingDbKind::StartOrResume => {
                self.on_start_or_resume_committed(ctx, op_id, result, effects);
            }
            PendingDbKind::ResumeHysteresis => {
                self.on_resume_hysteresis_committed(ctx, op_id, result, effects);
            }
            PendingDbKind::EndSession {
                session_id,
                cause,
                via_hysteresis,
                from_quiet_period_timer_id,
                sets_offline,
                sets_out_of_schedule,
                clears_errors,
                for_stop,
            } => {
                self.on_end_session_committed(
                    ctx,
                    session_id,
                    cause,
                    via_hysteresis,
                    from_quiet_period_timer_id,
                    sets_offline,
                    sets_out_of_schedule,
                    clears_errors,
                    for_stop,
                    result,
                    effects,
                );
            }
            PendingDbKind::EndForCredentialRetirement { session_id } => {
                if let Ok(DbCommitOutcome::CredentialRetirementEnded { ended, .. }) = result {
                    // Always clear the matching OpenSession slot even when `ended == false`
                    // (e.g. `ActorRegistry::settle_credential_retirement` committed the DB
                    // end first), while gating `SessionEnded` pipeline notification on
                    // `ended == true` so only the winning committer emits it once.
                    let matches_open = matches!(
                        &self.session,
                        SessionSlot::Open(open) if open.session_id == session_id
                    );
                    if matches_open
                        && let SessionSlot::Open(open) =
                            std::mem::replace(&mut self.session, SessionSlot::None)
                    {
                        self.release_open_session(open, effects);
                    }
                    if ended {
                        self.emit_session_ended_pipeline(
                            session_id,
                            TerminalCause::UserDisabled,
                            effects,
                        );
                    }
                    if let MonitorPhase::Stopping(stopping) = &mut self.monitor {
                        stopping.db_end = StopDbEnd::Settled { failed: false };
                    }
                    self.advance_stopping(ctx, effects);
                } else {
                    // DB failure on credential retirement: leave session open in DB,
                    // exit Stopping so reaper retries safely.
                    self.monitor = schedule::schedule_idle(
                        &self.config,
                        &self.offline_confirmation,
                        false,
                        self.errors.error_count_u32(),
                        None,
                        ctx,
                    );
                }
            }
            PendingDbKind::RewriteEndedCause { for_stop } => {
                let ok = matches!(result, Ok(DbCommitOutcome::EndedCauseRewritten { .. }));
                if for_stop {
                    if let MonitorPhase::Stopping(stopping) = &mut self.monitor {
                        stopping.db_end = StopDbEnd::Settled { failed: !ok };
                    }
                    self.advance_stopping(ctx, effects);
                }
            }
            PendingDbKind::CommitCredentialBinding {
                session_id,
                staged_media,
                original_cause,
                original_error,
            } => match result {
                Ok(DbCommitOutcome::CredentialBindingCommitted { binding, .. }) => {
                    let applied = match &mut self.session {
                        SessionSlot::Open(open) if open.session_id == session_id => {
                            open.credential_binding = Some(binding.clone());
                            open.recovered_media = staged_media.map(|media| RecoveredMedia {
                                binding: binding.clone(),
                                snapshot: media.snapshot,
                                streams: media.streams,
                                media_headers: media.media_headers,
                                media_extras: media.media_extras,
                                extracted_at: media.extracted_at,
                            });
                            open.attempt = AttemptPhase::Idle;
                            true
                        }
                        _ => false,
                    };
                    if applied {
                        self.start_attempt_for_open_session(ctx, None, false, effects);
                    }
                }
                Err(_) | Ok(_) => {
                    if let SessionSlot::Open(open) = &mut self.session
                        && open.session_id == session_id
                    {
                        open.attempt = AttemptPhase::Idle;
                    }
                    self.finish_failed_download_with_cause(
                        ctx,
                        session_id,
                        original_cause,
                        original_error,
                        effects,
                    );
                }
            },
            PendingDbKind::RecordTransientError { error_message } => {
                if let Ok(DbCommitOutcome::TransientErrorRecorded {
                    consecutive_error_count,
                    disabled_until,
                    ..
                }) = result
                {
                    self.errors.apply_committed_error(
                        consecutive_error_count,
                        disabled_until,
                        Some(error_message),
                    );
                    if let Some(remaining) = self.errors.remaining_backoff(ctx.wall_now)
                        && !matches!(
                            self.monitor,
                            MonitorPhase::Stopping(_)
                                | MonitorPhase::Terminated
                                | MonitorPhase::LiveWatchdog { .. }
                        )
                    {
                        self.monitor = MonitorPhase::Blocked {
                            kind: BlockedKind::TemporalDisabled,
                            retry_at: ctx.now.checked_add(remaining).unwrap_or(ctx.now),
                        };
                    }
                }
            }
            PendingDbKind::ClearErrors => {
                if result.is_ok() {
                    self.errors.clear_all(self.config.offline_check_count);
                }
            }
            PendingDbKind::ClearLastError => {
                if result.is_ok() {
                    self.errors.clear_last_error();
                }
            }
            PendingDbKind::RecordFatalError { new_state } => {
                if let Ok(DbCommitOutcome::FatalErrorRecorded { session_ended, .. }) = result {
                    if let SessionSlot::Open(open) =
                        std::mem::replace(&mut self.session, SessionSlot::None)
                    {
                        self.release_open_session(open, effects);
                    }
                    if let Some(sid) = session_ended {
                        self.emit_session_ended_pipeline(
                            sid,
                            TerminalCause::StreamerOffline,
                            effects,
                        );
                    }
                    // The fatal state is final and its commit ended the open
                    // session, so writes queued behind it no longer apply.
                    self.queued_db.clear();
                    self.monitor = MonitorPhase::Terminated;
                    effects.push(FsmEffect::ActorStopped {
                        reason: ActorStopReason::FatalError { state: new_state },
                    });
                } else {
                    self.monitor = schedule::schedule_idle(
                        &self.config,
                        &self.offline_confirmation,
                        false,
                        self.errors.error_count_u32(),
                        None,
                        ctx,
                    );
                }
            }
            PendingDbKind::RecordFilteredState
            | PendingDbKind::AddSessionTitle
            | PendingDbKind::RecordHysteresisEntered => {}
        }

        if matches!(self.monitor, MonitorPhase::Stopping(_)) {
            self.advance_stopping(ctx, effects);
        }
        self.flush_queued_db(effects);
    }

    fn on_start_or_resume_committed(
        &mut self,
        ctx: StepContext,
        op_id: u64,
        result: std::result::Result<DbCommitOutcome, DbCommitFailure>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let (probe_payload, prior_open) =
            match std::mem::replace(&mut self.session, SessionSlot::None) {
                SessionSlot::CommittingStart {
                    op_id: slot_op,
                    probe_payload,
                    prior_open,
                    ..
                } if slot_op == op_id => (probe_payload, prior_open),
                other => {
                    self.session = other;
                    return;
                }
            };

        let deferred_stop = self.deferred_stop.take();

        match result {
            Ok(DbCommitOutcome::SessionStarted {
                session_id,
                started_at,
                disposition: _,
                credential_binding,
            }) => {
                self.initial_reconcile_pending = false;
                if let Some(new_avatar) = probe_payload.avatar.as_ref().filter(|a| !a.is_empty()) {
                    self.config.avatar_url = Some(new_avatar.clone());
                }

                let open = match prior_open {
                    Some(mut existing) if existing.session_id == session_id => {
                        existing.title = probe_payload.title.clone();
                        existing.credential_binding = credential_binding;
                        existing.quiet_period = None;
                        existing
                    }
                    other => {
                        if let Some(old) = other {
                            self.release_open_session(old, effects);
                        }
                        Box::new(OpenSession {
                            session_id: session_id.clone(),
                            started_at,
                            title: probe_payload.title.clone(),
                            credential_binding,
                            recovered_media: None,
                            diagnostic_attempts: 0,
                            diagnosed_download_ids: HashSet::new(),
                            attempt: AttemptPhase::Idle,
                            danmu: DanmuSessionState::Off,
                            quiet_period: None,
                            segment_guard: SessionSegmentGuard::new(session_id),
                            credentials_changed_retried: false,
                        })
                    }
                };
                self.session = SessionSlot::Open(open);

                // If a stop arrived while `StartOrResumeSession` was in flight,
                // never start a download — immediately advance the stop so the
                // just-created authoritative session is closed cleanly in SQLite.
                if deferred_stop.is_some() || matches!(self.monitor, MonitorPhase::Stopping(_)) {
                    self.advance_stopping(ctx, effects);
                    return;
                }

                self.offline_confirmation.mark_live();
                self.monitor = MonitorPhase::LiveWatchdog {
                    last_activity_at: ctx.now,
                    next_filter_hint: probe_payload.next_check_hint,
                    watchdog_backoff_until: None,
                };
                self.start_attempt_for_open_session(ctx, Some(*probe_payload), false, effects);
            }
            Ok(DbCommitOutcome::SessionStartSuppressed { state, retry_after }) => {
                // Keep `initial_reconcile_pending` unchanged and roll back session slot.
                self.session = match prior_open {
                    Some(open) => SessionSlot::Open(open),
                    None => SessionSlot::None,
                };
                if deferred_stop.is_some() || matches!(self.monitor, MonitorPhase::Stopping(_)) {
                    self.advance_stopping(ctx, effects);
                    return;
                }
                self.apply_start_suppression(ctx, state, retry_after);
            }
            Err(_) | Ok(_) => {
                // DB commit failed: roll back the session slot and poll again at
                // the normal cadence, so a failing database costs one probe and
                // one commit per poll interval.
                self.session = match prior_open {
                    Some(open) => SessionSlot::Open(open),
                    None => SessionSlot::None,
                };
                if deferred_stop.is_some() || matches!(self.monitor, MonitorPhase::Stopping(_)) {
                    self.advance_stopping(ctx, effects);
                    return;
                }
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );
            }
        }
    }

    fn on_resume_hysteresis_committed(
        &mut self,
        ctx: StepContext,
        op_id: u64,
        result: std::result::Result<DbCommitOutcome, DbCommitFailure>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let (probe_payload, prior_open) =
            match std::mem::replace(&mut self.session, SessionSlot::None) {
                SessionSlot::CommittingStart {
                    op_id: slot_op,
                    probe_payload,
                    prior_open,
                    ..
                } if slot_op == op_id => (probe_payload, prior_open),
                other => {
                    self.session = other;
                    return;
                }
            };

        let deferred_stop = self.deferred_stop.take();

        match result {
            Ok(DbCommitOutcome::HysteresisResumed { .. }) => {
                self.initial_reconcile_pending = false;
                if let Some(mut open) = prior_open {
                    open.title = probe_payload.title.clone();
                    open.quiet_period = None;
                    if let Some(binding) = probe_payload.credential_binding.clone() {
                        open.credential_binding = Some(binding);
                    }
                    self.session = SessionSlot::Open(open);
                }

                if deferred_stop.is_some() || matches!(self.monitor, MonitorPhase::Stopping(_)) {
                    self.advance_stopping(ctx, effects);
                    return;
                }

                self.offline_confirmation.mark_live();
                self.monitor = MonitorPhase::LiveWatchdog {
                    last_activity_at: ctx.now,
                    next_filter_hint: probe_payload.next_check_hint,
                    watchdog_backoff_until: None,
                };
                self.start_attempt_for_open_session(ctx, Some(*probe_payload), true, effects);
            }
            Ok(DbCommitOutcome::HysteresisResumeSuppressed { state, retry_after }) => {
                // Keep quiet_period armed on inactive suppression so disable teardown
                // or the quiet-period timer closes the session normally.
                if let Some(open) = prior_open {
                    self.session = SessionSlot::Open(open);
                }
                if deferred_stop.is_some() || matches!(self.monitor, MonitorPhase::Stopping(_)) {
                    self.advance_stopping(ctx, effects);
                    return;
                }
                self.apply_start_suppression(ctx, state, retry_after);
            }
            Err(_) | Ok(_) => {
                // Restore the open session with its quiet period intact and poll
                // again at the normal cadence.
                if let Some(open) = prior_open {
                    self.session = SessionSlot::Open(open);
                }
                if deferred_stop.is_some() || matches!(self.monitor, MonitorPhase::Stopping(_)) {
                    self.advance_stopping(ctx, effects);
                    return;
                }
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );
            }
        }
    }

    fn apply_start_suppression(
        &mut self,
        ctx: StepContext,
        state: StreamerState,
        retry_after: Option<std::time::Duration>,
    ) {
        let delay = retry_after
            .unwrap_or_else(|| std::time::Duration::from_millis(self.config.check_interval_ms));
        let retry_at = ctx.now.checked_add(delay).unwrap_or(ctx.now);
        if state == StreamerState::TemporalDisabled {
            self.monitor = MonitorPhase::Blocked {
                kind: BlockedKind::TemporalDisabled,
                retry_at,
            };
        } else if state == StreamerState::OutOfSpace {
            self.monitor = MonitorPhase::Blocked {
                kind: BlockedKind::OutputRootCooldown,
                retry_at,
            };
        } else {
            self.monitor = MonitorPhase::Idle {
                next_check: Some(retry_at),
                recurring_interval_ms: None,
                batch_routine_deadline: None,
            };
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "reconciles all commit metadata fields explicitly"
    )]
    fn on_end_session_committed(
        &mut self,
        ctx: StepContext,
        expected_session_id: Option<String>,
        cause: TerminalCause,
        via_hysteresis: bool,
        from_quiet_period_timer_id: Option<u64>,
        sets_offline: bool,
        sets_out_of_schedule: bool,
        clears_errors: bool,
        for_stop: bool,
        result: std::result::Result<DbCommitOutcome, DbCommitFailure>,
        effects: &mut Vec<FsmEffect>,
    ) {
        match result {
            Ok(DbCommitOutcome::SessionEnded {
                session_id,
                transitioned_from_open,
                ..
            }) => {
                if sets_offline {
                    let was_live = self.offline_confirmation.was_live;
                    self.initial_reconcile_pending = false;
                    self.offline_confirmation.reset();
                    // Only a recurring poll moves to the offline cadence; an
                    // explicit check, such as the one a live result queues
                    // while this end commits, keeps its deadline.
                    if was_live
                        && matches!(
                            self.monitor,
                            MonitorPhase::Idle {
                                recurring_interval_ms: Some(_),
                                ..
                            }
                        )
                    {
                        self.monitor = schedule::schedule_idle(
                            &self.config,
                            &self.offline_confirmation,
                            false,
                            self.errors.error_count_u32(),
                            None,
                            ctx,
                        );
                    }
                }
                if sets_out_of_schedule {
                    self.initial_reconcile_pending = false;
                }
                if clears_errors {
                    self.errors.clear_all(self.config.offline_check_count);
                }

                let closed_id = session_id.or(expected_session_id);
                let closed_matches_open = match (&self.session, &closed_id) {
                    (SessionSlot::Open(open), Some(id)) => open.session_id == *id,
                    (SessionSlot::Open(_), None) => true,
                    _ => false,
                };
                let closed_matches_queued_resume = matches!(
                    (&self.session, &closed_id),
                    (
                        SessionSlot::CommittingStart {
                            from_hysteresis: true,
                            prior_open: Some(open),
                            ..
                        },
                        Some(id)
                    ) if open.session_id == *id
                );

                if closed_matches_open
                    && let SessionSlot::Open(open) =
                        std::mem::replace(&mut self.session, SessionSlot::None)
                {
                    let sid = open.session_id.clone();
                    self.release_open_session(open, effects);
                    if hysteresis::is_reattributable_timer_end(&cause, via_hysteresis) {
                        self.session = SessionSlot::RecentlyEnded {
                            session_id: sid,
                            evict_at: ctx
                                .now
                                .checked_add(hysteresis::RECENTLY_ENDED_RETENTION)
                                .unwrap_or(ctx.now),
                        };
                    }
                } else if closed_matches_queued_resume
                    && let SessionSlot::CommittingStart {
                        op_id: queued_resume_op_id,
                        probe_payload,
                        prior_open: Some(open),
                        ..
                    } = std::mem::replace(&mut self.session, SessionSlot::None)
                {
                    self.queued_db.retain(|op| op.op_id != queued_resume_op_id);
                    self.release_open_session(open, effects);
                    if transitioned_from_open && let Some(sid) = closed_id.clone() {
                        self.emit_session_ended_pipeline(sid, cause, effects);
                    }
                    self.begin_start_or_resume(ctx, probe_payload, None, effects);
                    return;
                }

                if transitioned_from_open && let Some(sid) = closed_id {
                    self.emit_session_ended_pipeline(sid, cause, effects);
                }

                if for_stop && let MonitorPhase::Stopping(stopping) = &mut self.monitor {
                    stopping.db_end = StopDbEnd::Settled { failed: false };
                }
            }
            Err(_) | Ok(_) => {
                // Failed QuietPeriod expiry commit: keep the exact same OpenSession
                // and timer_id and schedule a 1-second retry.
                if let Some(timer_id) = from_quiet_period_timer_id {
                    if let SessionSlot::Open(open) = &mut self.session
                        && let Some(qp) = &mut open.quiet_period
                        && qp.timer_id == timer_id
                    {
                        qp.retrying_commit = true;
                        qp.next_retry_at = ctx
                            .now
                            .checked_add(hysteresis::QUIET_PERIOD_COMMIT_RETRY_DELAY)
                            .or(Some(ctx.now));
                    }
                    return;
                }

                if for_stop {
                    // For Disable/OutOfSchedule/Retire, unblock the control plane
                    // while marking the end failed so Retire reports quiescent = false.
                    if let MonitorPhase::Stopping(stopping) = &mut self.monitor {
                        stopping.db_end = StopDbEnd::Settled { failed: true };
                    }
                }
            }
        }
    }

    /// Emit `SessionEnded` pipeline notification after an authoritative DB end commit.
    pub(super) fn emit_session_ended_pipeline(
        &self,
        session_id: String,
        cause: TerminalCause,
        effects: &mut Vec<FsmEffect>,
    ) {
        let should_run_session_complete = cause.should_run_session_complete_pipeline();
        effects.push(FsmEffect::NotifyPipeline(
            PipelineNotification::SessionEnded {
                session_id,
                streamer_id: self.streamer_id.clone(),
                cause: cause.as_str().to_owned(),
                should_run_session_complete,
            },
        ));
    }
}
