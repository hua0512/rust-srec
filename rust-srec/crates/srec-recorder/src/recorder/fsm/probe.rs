//! Status probe dispatch and outcome handling (`Live`, `Offline`, `Filtered`,
//! `Fatal`, and probe failures).

use std::time::Instant;

use crate::monitor::FilterReason;
use crate::session::TerminalCause;
use crate::streamer::download_failure_threshold;
use srec_core::domain::StreamerState;
use srec_core::domain::streamer::FatalErrorType;

use super::hysteresis::OfflineCheckDecision;
use super::schedule;
use super::state::{
    AttemptPhase, BlockedKind, FilteredKind, FsmEffect, InFlightProbe, MonitorPhase, OpenSession,
    PendingDbKind, SessionSlot, StepContext, StopKind, StreamerFsm,
};
use super::stop::request_stop_open_attempt;
use crate::recorder::ports::{
    DanmuStopKind, DbCommitOp, LiveProbePayload, ProbeFailure, ProbePurpose, ProbeRequest,
    ProbeStatus,
};

impl StreamerFsm {
    pub(super) fn spawn_status_probe(&mut self, effects: &mut Vec<FsmEffect>) {
        self.batch_live_hint_armed = false;
        let (session_id, credential_binding, purpose) = match &self.session {
            SessionSlot::Open(open) => (
                Some(open.session_id.clone()),
                open.credential_binding.clone(),
                ProbePurpose::BoundPoll,
            ),
            _ => (None, None, ProbePurpose::Discovery),
        };
        let probe_id = self.next_id();
        self.in_flight_probe = Some(InFlightProbe { probe_id, purpose });
        effects.push(FsmEffect::SpawnProbe(ProbeRequest {
            probe_id,
            streamer_id: self.streamer_id.clone(),
            purpose,
            session_id,
            credential_binding,
        }));
    }

    pub(super) fn on_probe_completed(
        &mut self,
        ctx: StepContext,
        probe_id: u64,
        result: std::result::Result<ProbeStatus, ProbeFailure>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let Some(probe) = self.in_flight_probe.take() else {
            return;
        };
        if probe.probe_id != probe_id {
            self.in_flight_probe = Some(probe);
            return;
        }

        if probe.purpose == ProbePurpose::Recovery {
            self.on_recovery_probe_completed(ctx, probe_id, result, effects);
            return;
        }

        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }

        let saved_routine = match &mut self.monitor {
            MonitorPhase::Idle {
                batch_routine_deadline,
                ..
            } => batch_routine_deadline.take(),
            _ => None,
        };

        match result {
            Ok(status) => {
                if let MonitorPhase::LiveWatchdog {
                    watchdog_backoff_until,
                    ..
                } = &mut self.monitor
                {
                    *watchdog_backoff_until = None;
                }
                self.handle_probe_status(ctx, status, saved_routine, effects);
            }
            Err(failure) => {
                self.handle_probe_failure(ctx, failure, effects);
            }
        }
    }

    fn handle_probe_status(
        &mut self,
        ctx: StepContext,
        status: ProbeStatus,
        saved_routine: Option<Instant>,
        effects: &mut Vec<FsmEffect>,
    ) {
        match status {
            ProbeStatus::Live(payload) => self.handle_live_probe(ctx, payload, effects),
            ProbeStatus::Offline {
                batch_live_hint_armed,
            } => self.handle_offline_probe(ctx, batch_live_hint_armed, saved_routine, effects),
            ProbeStatus::Filtered { reason, .. } => {
                self.handle_filtered_probe(ctx, reason, effects);
            }
            ProbeStatus::Fatal { kind, message } => {
                self.handle_fatal_probe(ctx, kind, message, effects);
            }
        }
    }

    fn handle_live_probe(
        &mut self,
        ctx: StepContext,
        payload: Box<LiveProbePayload>,
        effects: &mut Vec<FsmEffect>,
    ) {
        if matches!(self.monitor, MonitorPhase::RefusedBroadcast { .. }) {
            // Same unrecordable broadcast is still live; suppress re-emission.
            self.monitor = schedule::schedule_refused_broadcast(
                &self.config,
                &self.offline_confirmation,
                self.errors.error_count_u32(),
                ctx,
            );
            return;
        }

        // Case 1: Open session parked in QuietPeriod -> resume via two-phase DB commit.
        if matches!(&self.session, SessionSlot::Open(open) if open.quiet_period.is_some()) {
            let SessionSlot::Open(open) = std::mem::replace(&mut self.session, SessionSlot::None)
            else {
                return;
            };
            let hysteresis_duration_secs = open
                .quiet_period
                .as_ref()
                .map(|qp| {
                    u64::try_from(ctx.wall_now.duration_since(qp.entered_at).as_secs().max(0))
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            let session_id = open.session_id.clone();
            let op = DbCommitOp::ResumeHysteresis {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                session_id,
                title: payload.title.clone(),
                category: payload.category.clone(),
                hysteresis_duration_secs,
                now: ctx.wall_now,
            };
            let op_id = self.enqueue_or_dispatch_db(PendingDbKind::ResumeHysteresis, op, effects);
            self.session = SessionSlot::CommittingStart {
                op_id,
                probe_payload: payload,
                from_hysteresis: true,
                prior_open: Some(open),
            };
            return;
        }

        // Case 2: Open session already recording (not in quiet period).
        if let SessionSlot::Open(open) = &self.session
            && self.session_end_pending(&open.session_id)
        {
            // The session is ending, so it must not record again. Check once
            // more as soon as the end lands, to open the next session.
            self.monitor = schedule::schedule_idle_immediate(ctx.now);
            return;
        }
        if matches!(&self.session, SessionSlot::Open(_)) {
            let stalled = matches!(
                &self.monitor,
                MonitorPhase::LiveWatchdog { last_activity_at, .. }
                    if schedule::is_live_stalled(*last_activity_at, ctx.now)
            );
            let needs_attempt = matches!(
                &self.session,
                SessionSlot::Open(open)
                    if matches!(
                        open.attempt,
                        AttemptPhase::Idle | AttemptPhase::WaitingCredentials
                    )
            );

            if stalled {
                // Watchdog detected 5-minute stall: force re-commit & recovery startup.
                let SessionSlot::Open(open) =
                    std::mem::replace(&mut self.session, SessionSlot::None)
                else {
                    return;
                };
                self.begin_start_or_resume(ctx, payload, Some(open), effects);
                return;
            }

            if let SessionSlot::Open(open) = &mut self.session {
                let title_changed = !payload.title.is_empty() && open.title != payload.title;
                if title_changed {
                    open.title = payload.title.clone();
                    let sid = open.session_id.clone();
                    let title = payload.title.clone();
                    self.enqueue_or_dispatch_db(
                        PendingDbKind::AddSessionTitle,
                        DbCommitOp::AddSessionTitle {
                            session_id: sid,
                            title,
                            now: ctx.wall_now,
                        },
                        effects,
                    );
                }
            }

            self.offline_confirmation.mark_live();
            self.monitor = MonitorPhase::LiveWatchdog {
                last_activity_at: ctx.now,
                next_filter_hint: payload.next_check_hint,
                watchdog_backoff_until: None,
            };

            if needs_attempt {
                self.start_attempt_for_open_session(ctx, Some(*payload), false, effects);
            }
            return;
        }

        // Case 3: No open session -> two-phase `CommittingStart` handshake.
        self.begin_start_or_resume(ctx, payload, None, effects);
    }

    pub(super) fn begin_start_or_resume(
        &mut self,
        ctx: StepContext,
        payload: Box<LiveProbePayload>,
        prior_open: Option<Box<OpenSession>>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let op = DbCommitOp::StartOrResumeSession {
            streamer_id: self.streamer_id.clone(),
            streamer_name: self.config.streamer_name.clone(),
            title: payload.title.clone(),
            category: payload.category.clone(),
            current_avatar: self.config.avatar_url.clone(),
            new_avatar: payload.avatar.clone(),
            credential_binding: payload.credential_binding.clone(),
            now: ctx.wall_now,
        };
        let op_id = self.enqueue_or_dispatch_db(PendingDbKind::StartOrResume, op, effects);
        self.session = SessionSlot::CommittingStart {
            op_id,
            probe_payload: payload,
            from_hysteresis: false,
            prior_open,
        };
    }

    fn handle_offline_probe(
        &mut self,
        ctx: StepContext,
        batch_live_hint_armed: bool,
        saved_routine: Option<Instant>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let refused_broadcast_ended = matches!(self.monitor, MonitorPhase::RefusedBroadcast { .. });
        let was_filtered_or_blocked = matches!(
            self.monitor,
            MonitorPhase::Filtered { .. }
                | MonitorPhase::Blocked {
                    kind: BlockedKind::TemporalDisabled,
                    ..
                }
        );

        let decision = self
            .offline_confirmation
            .observe_monitor_offline(self.config.offline_check_count);

        self.monitor = schedule::schedule_idle(
            &self.config,
            &self.offline_confirmation,
            false,
            self.errors.error_count_u32(),
            saved_routine,
            ctx,
        );
        self.batch_live_hint_armed = batch_live_hint_armed && !self.offline_confirmation.was_live;

        match decision {
            OfflineCheckDecision::ConfirmedOffline { .. } => {
                let open_session_id = match &self.session {
                    SessionSlot::Open(open) => Some(open.session_id.clone()),
                    _ => None,
                };
                self.commit_offline_end(ctx, open_session_id, effects);
            }
            OfflineCheckDecision::GracePeriod { .. } => {
                // If QuietPeriod already froze bounded grace checks, consume one slot.
                let quiet_exhausted = match &mut self.session {
                    SessionSlot::Open(open) => match &mut open.quiet_period {
                        Some(qp) => {
                            if let Some(rem) = &mut qp.grace_remaining_checks {
                                *rem = rem.saturating_sub(1);
                                (*rem == 0).then(|| {
                                    (open.session_id.clone(), qp.timer_id, qp.cause.clone())
                                })
                            } else {
                                None
                            }
                        }
                        None => None,
                    },
                    _ => None,
                };
                if let Some((session_id, timer_id, cause)) = quiet_exhausted {
                    self.dispatch_quiet_period_end(ctx, session_id, timer_id, cause, effects);
                }
            }
            OfflineCheckDecision::NotPreviouslyLive => {
                let must_reconcile = self.initial_reconcile_pending
                    || refused_broadcast_ended
                    || was_filtered_or_blocked
                    || self.errors.has_errors();
                if must_reconcile {
                    let open_session_id = match &self.session {
                        SessionSlot::Open(open) => Some(open.session_id.clone()),
                        _ => None,
                    };
                    self.commit_offline_end(ctx, open_session_id, effects);
                }
            }
        }
    }

    pub(super) fn commit_offline_end(
        &mut self,
        ctx: StepContext,
        session_id: Option<String>,
        effects: &mut Vec<FsmEffect>,
    ) {
        let mut via_hysteresis = false;
        if let Some(sid) = session_id.as_deref()
            && let SessionSlot::Open(open) = &mut self.session
            && open.session_id == sid
        {
            request_stop_open_attempt(
                open,
                crate::downloader::DownloadStopCause::StreamerOffline,
                effects,
            );
            Self::stop_open_danmu(open, DanmuStopKind::SessionEnded, effects);
            via_hysteresis = open.quiet_period.take().is_some();
        }

        let clear_errors = self.errors.has_errors();
        let threshold = download_failure_threshold(self.config.offline_check_count);

        self.enqueue_or_dispatch_db(
            PendingDbKind::EndSession {
                session_id: session_id.clone(),
                cause: TerminalCause::StreamerOffline,
                via_hysteresis,
                from_quiet_period_timer_id: None,
                sets_offline: true,
                sets_out_of_schedule: false,
                clears_errors: clear_errors,
                for_stop: false,
            },
            DbCommitOp::EndSession {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                session_id,
                cause: TerminalCause::StreamerOffline,
                via_hysteresis,
                update_streamer_state: Some(StreamerState::NotLive),
                clear_errors,
                checks_restored_threshold: Some(threshold),
                infra_block: None,
                suppress_offline_notification: false,
                now: ctx.wall_now,
            },
            effects,
        );
    }

    fn handle_filtered_probe(
        &mut self,
        ctx: StepContext,
        reason: FilterReason,
        effects: &mut Vec<FsmEffect>,
    ) {
        match reason {
            FilterReason::OutOfSchedule { next_available } => {
                // Active recording/open session: stop download + danmu + end session.
                if matches!(
                    self.session,
                    SessionSlot::Open(_) | SessionSlot::CommittingStart { .. }
                ) {
                    self.initiate_stop(
                        ctx,
                        StopKind::OutOfSchedule {
                            smart_wake: next_available,
                        },
                        effects,
                    );
                    return;
                }

                let already_out_of_schedule = matches!(
                    self.monitor,
                    MonitorPhase::Filtered {
                        reason: FilteredKind::OutOfSchedule { .. },
                        ..
                    }
                );
                self.monitor = schedule::schedule_filtered(
                    FilteredKind::OutOfSchedule {
                        smart_wake: next_available,
                    },
                    &self.config,
                    &self.offline_confirmation,
                    self.errors.error_count_u32(),
                    ctx,
                );

                if already_out_of_schedule && !self.initial_reconcile_pending {
                    return;
                }

                self.enqueue_or_dispatch_db(
                    PendingDbKind::EndSession {
                        session_id: None,
                        cause: TerminalCause::OutOfSchedule,
                        via_hysteresis: false,
                        from_quiet_period_timer_id: None,
                        sets_offline: false,
                        sets_out_of_schedule: true,
                        clears_errors: false,
                        for_stop: false,
                    },
                    DbCommitOp::EndSession {
                        streamer_id: self.streamer_id.clone(),
                        streamer_name: self.config.streamer_name.clone(),
                        session_id: None,
                        cause: TerminalCause::OutOfSchedule,
                        via_hysteresis: false,
                        update_streamer_state: Some(StreamerState::OutOfSchedule),
                        clear_errors: false,
                        checks_restored_threshold: None,
                        infra_block: None,
                        suppress_offline_notification: true,
                        now: ctx.wall_now,
                    },
                    effects,
                );
            }
            FilterReason::TitleMismatch | FilterReason::CategoryMismatch => {
                // Title/category mismatch NEVER stops an already-open recording session.
                if matches!(self.session, SessionSlot::Open(_)) {
                    if let MonitorPhase::LiveWatchdog {
                        last_activity_at,
                        next_filter_hint,
                        watchdog_backoff_until,
                    } = &mut self.monitor
                    {
                        *last_activity_at = ctx.now;
                        *watchdog_backoff_until = None;
                        // This probe served a boundary or check-now hint that has
                        // passed; keeping it would make every later probe due at once.
                        if next_filter_hint.is_some_and(|hint| hint <= ctx.wall_now) {
                            *next_filter_hint = None;
                        }
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
                    return;
                }

                let already_metadata_filtered = matches!(
                    self.monitor,
                    MonitorPhase::Filtered {
                        reason: FilteredKind::MetadataMismatch,
                        ..
                    }
                );
                let old_state = self.runtime_view().monitor_state;
                self.monitor = schedule::schedule_filtered(
                    FilteredKind::MetadataMismatch,
                    &self.config,
                    &self.offline_confirmation,
                    self.errors.error_count_u32(),
                    ctx,
                );

                if already_metadata_filtered {
                    return;
                }

                let reason_label = match reason {
                    FilterReason::TitleMismatch => "title_mismatch",
                    FilterReason::CategoryMismatch => "category_mismatch",
                    FilterReason::OutOfSchedule { .. } => "out_of_schedule",
                };
                self.enqueue_or_dispatch_db(
                    PendingDbKind::RecordFilteredState,
                    DbCommitOp::RecordFilteredState {
                        streamer_id: self.streamer_id.clone(),
                        streamer_name: self.config.streamer_name.clone(),
                        old_state,
                        new_state: StreamerState::OutOfSchedule,
                        reason: reason_label.to_owned(),
                        now: ctx.wall_now,
                    },
                    effects,
                );
            }
        }
    }

    fn handle_fatal_probe(
        &mut self,
        ctx: StepContext,
        kind: FatalErrorType,
        message: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        let new_state = match kind {
            FatalErrorType::NotFound => StreamerState::NotFound,
            _ => StreamerState::FatalError,
        };
        let session_id = match &mut self.session {
            SessionSlot::Open(open) => {
                let sid = open.session_id.clone();
                Self::stop_open_danmu(open, DanmuStopKind::SessionEnded, effects);
                open.quiet_period = None;
                Some(sid)
            }
            _ => None,
        };
        if let Some(sid) = session_id.as_deref()
            && let SessionSlot::Open(open) = &mut self.session
            && open.session_id == sid
        {
            request_stop_open_attempt(
                open,
                crate::downloader::DownloadStopCause::StreamerOffline,
                effects,
            );
        }

        self.enqueue_or_dispatch_db(
            PendingDbKind::RecordFatalError { new_state },
            DbCommitOp::RecordFatalError {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                session_id,
                error_type: kind,
                message,
                new_state,
                now: ctx.wall_now,
            },
            effects,
        );
    }

    fn handle_probe_failure(
        &mut self,
        ctx: StepContext,
        failure: ProbeFailure,
        effects: &mut Vec<FsmEffect>,
    ) {
        // If QuietPeriod already froze bounded grace checks, any probe failure
        // ends the bounded grace and commits the original ambiguous cause.
        let grace_failure_end = match &self.session {
            SessionSlot::Open(open) => open.quiet_period.as_ref().and_then(|qp| {
                qp.grace_remaining_checks
                    .map(|_| (open.session_id.clone(), qp.timer_id, qp.cause.clone()))
            }),
            _ => None,
        };
        if let Some((session_id, timer_id, cause)) = grace_failure_end {
            self.dispatch_quiet_period_end(ctx, session_id, timer_id, cause, effects);
        }

        match failure {
            ProbeFailure::CredentialUnavailable { .. } | ProbeFailure::CredentialSourceChanged => {
                // Credential exhaustion never increments error count or tears down LiveWatchdog.
                if let MonitorPhase::LiveWatchdog {
                    watchdog_backoff_until,
                    ..
                } = &mut self.monitor
                {
                    *watchdog_backoff_until = ctx
                        .now
                        .checked_add(schedule::WATCHDOG_ERROR_BACKOFF)
                        .or(Some(ctx.now));
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
            ProbeFailure::Transient { message } | ProbeFailure::Permanent { message } => {
                // Live watchdog failures stay side-effect free: no error count increment,
                // no disabled_until, only a 60s re-check floor. A watchdog probe whose
                // download ended while it was in flight counts like any other check.
                if let MonitorPhase::LiveWatchdog {
                    watchdog_backoff_until,
                    ..
                } = &mut self.monitor
                {
                    *watchdog_backoff_until = ctx
                        .now
                        .checked_add(schedule::WATCHDOG_ERROR_BACKOFF)
                        .or(Some(ctx.now));
                    return;
                }

                self.record_error_and_back_off(ctx, message, effects);
            }
        }
    }

    /// Count a failure, persist it, and wait out the resulting backoff:
    /// `Blocked` while `disabled_until` is ahead, otherwise the error-state
    /// poll cadence.
    pub(super) fn record_error_and_back_off(
        &mut self,
        ctx: StepContext,
        message: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        self.errors.record_local_error(
            message.clone(),
            self.config.offline_check_count,
            ctx.wall_now,
        );
        self.monitor = match self.errors.remaining_backoff(ctx.wall_now) {
            Some(remaining) => MonitorPhase::Blocked {
                kind: BlockedKind::TemporalDisabled,
                retry_at: ctx.now.checked_add(remaining).unwrap_or(ctx.now),
            },
            None => schedule::schedule_idle(
                &self.config,
                &self.offline_confirmation,
                true,
                self.errors.error_count_u32(),
                None,
                ctx,
            ),
        };
        self.record_transient_error_db_only(ctx, message, effects);
    }

    pub(super) fn record_transient_error(
        &mut self,
        ctx: StepContext,
        message: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        self.errors.record_local_error(
            message.clone(),
            self.config.offline_check_count,
            ctx.wall_now,
        );
        self.record_transient_error_db_only(ctx, message, effects);
    }

    fn record_transient_error_db_only(
        &mut self,
        ctx: StepContext,
        message: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        self.enqueue_or_dispatch_db(
            PendingDbKind::RecordTransientError {
                error_message: message.clone(),
            },
            DbCommitOp::RecordTransientError {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                platform: self.config.platform.clone(),
                error: message,
                offline_check_count: self.config.offline_check_count,
                now: ctx.wall_now,
            },
            effects,
        );
    }
}
