//! Download attempt lifecycle, epoch validation, rejection handling, and
//! direct/quiet-period terminal transitions.

use std::time::Duration;

use crate::downloader::{
    DownloadFailureKind, DownloadRejectedKind, DownloadStopCause, DownloadTerminalEvent,
};
use crate::monitor::InfraBlockReason;
use crate::session::{EngineKind, TerminalCause};

use super::credentials::take_valid_recovered_media;
use super::schedule;
use super::state::{
    AttemptPhase, BlockedKind, DanmuSessionState, FilteredKind, FsmEffect, MonitorPhase,
    PendingDbKind, SessionSlot, StepContext, StopDownload, StreamerFsm,
};
use super::stop::stop_cause_for_kind;
use crate::recorder::ports::{
    AttemptStartFailure, AttemptStartSpec, DanmuStopKind, DbCommitOp, LiveProbePayload,
};

/// Input bundle for [`StreamerFsm::on_attempt_started`].
pub(super) struct AttemptStartedInput {
    pub attempt_id: u64,
    pub session_id: String,
    pub download_id: String,
    pub engine_type: crate::downloader::engine::EngineType,
    pub protocol: crate::downloader::DownloadProtocol,
}

impl StreamerFsm {
    /// Whether `(session_id, attempt_id)` matches the currently tracked attempt.
    pub(super) fn matches_current_attempt(&self, session_id: &str, attempt_id: u64) -> bool {
        match &self.session {
            SessionSlot::Open(open) if open.session_id == session_id => match open.attempt {
                AttemptPhase::Starting {
                    attempt_id: current,
                    ..
                }
                | AttemptPhase::Running {
                    attempt_id: current,
                    ..
                }
                | AttemptPhase::StoppingAttempt {
                    attempt_id: current,
                    ..
                } => current == attempt_id,
                AttemptPhase::Idle
                | AttemptPhase::WaitingCredentials
                | AttemptPhase::DiagnosingCredentials { .. } => false,
            },
            _ => false,
        }
    }

    /// Whether the active stop still waits for `attempt_id` to end.
    fn stop_awaits_attempt(&self, attempt_id: u64) -> bool {
        matches!(
            &self.monitor,
            MonitorPhase::Stopping(stopping)
                if stopping.download == (StopDownload::Pending { attempt_id })
        )
    }

    /// Settle the stop's download leg once its attempt ended, even when an
    /// earlier end already closed that attempt's session. `idles_open_attempt`
    /// says whether the ended attempt is the open session's current one.
    fn settle_stop_download(
        &mut self,
        ctx: StepContext,
        session_id: &str,
        idles_open_attempt: bool,
        effects: &mut Vec<FsmEffect>,
    ) {
        if idles_open_attempt
            && let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            open.attempt = AttemptPhase::Idle;
        }
        if let MonitorPhase::Stopping(stopping) = &mut self.monitor {
            stopping.download = StopDownload::Confirmed;
        }
        self.advance_stopping(ctx, effects);
    }

    /// Start the download of a live open session that an earlier download
    /// held back, once nothing of that download remains.
    pub(super) fn resume_held_attempt(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        if self.detached_attempts.is_empty()
            && matches!(self.monitor, MonitorPhase::LiveWatchdog { .. })
            && matches!(&self.session, SessionSlot::Open(open) if open.attempt == AttemptPhase::Idle)
        {
            self.start_attempt_for_open_session(ctx, None, false, effects);
        }
    }

    /// Launch a download attempt for the currently open session if permitted.
    pub(super) fn start_attempt_for_open_session(
        &mut self,
        ctx: StepContext,
        payload: Option<LiveProbePayload>,
        from_hysteresis_resume: bool,
        effects: &mut Vec<FsmEffect>,
    ) {
        if self.lingering_physical_lease
            || matches!(
                self.monitor,
                MonitorPhase::Stopping(_) | MonitorPhase::Terminated
            )
        {
            return;
        }

        // A session whose end is committing never records again, and one
        // download at a time records a streamer.
        if !self.detached_attempts.is_empty()
            || matches!(&self.session, SessionSlot::Open(open) if self.session_end_pending(&open.session_id))
        {
            return;
        }

        let attempt_id = self.next_id();
        let spec = match &mut self.session {
            SessionSlot::Open(open) => {
                if open.quiet_period.is_some()
                    || !matches!(
                        open.attempt,
                        AttemptPhase::Idle | AttemptPhase::WaitingCredentials
                    )
                {
                    return;
                }

                let recovered = take_valid_recovered_media(open, ctx.now);
                let (effective_payload, binding, recovered_media_extracted_at) =
                    if let Some(media) = recovered {
                        let recovered_payload = LiveProbePayload {
                            title: open.title.clone(),
                            category: None,
                            avatar: None,
                            streams: media.streams,
                            media_headers: media.media_headers,
                            media_extras: media.media_extras,
                            credential_binding: Some(media.binding.clone()),
                            credential_snapshot: Some(media.snapshot),
                            next_check_hint: None,
                        };
                        (
                            Some(recovered_payload),
                            Some(media.binding),
                            Some(media.extracted_at),
                        )
                    } else {
                        (payload, open.credential_binding.clone(), None)
                    };

                open.attempt = AttemptPhase::Starting {
                    attempt_id,
                    queued: false,
                    credential_binding: binding.clone(),
                    admission_rechecked: false,
                };
                open.segment_guard.download_ended = false;

                AttemptStartSpec {
                    attempt_id,
                    session_id: open.session_id.clone(),
                    streamer_id: self.streamer_id.clone(),
                    streamer_name: self.config.streamer_name.clone(),
                    title: open.title.clone(),
                    payload: effective_payload,
                    credential_binding: binding,
                    from_hysteresis_resume,
                    priority: self.config.priority,
                    recovered_media_extracted_at,
                }
            }
            _ => return,
        };

        effects.push(FsmEffect::StartAttempt(Box::new(spec)));
    }

    /// Handle `AttemptQueued`.
    pub(super) fn on_attempt_queued(&mut self, attempt_id: u64, session_id: &str) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
            && let AttemptPhase::Starting {
                attempt_id: current,
                queued,
                ..
            } = &mut open.attempt
            && *current == attempt_id
        {
            *queued = true;
        }
    }

    /// Handle `AttemptDequeued`.
    pub(super) fn on_attempt_dequeued(&mut self, attempt_id: u64, session_id: &str) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
            && let AttemptPhase::Starting {
                attempt_id: current,
                queued,
                ..
            } = &mut open.attempt
            && *current == attempt_id
        {
            *queued = false;
        }
    }

    /// Handle `AttemptStarted`: transition `Starting -> Running` and mark live activity.
    pub(super) fn on_attempt_started(
        &mut self,
        ctx: StepContext,
        input: AttemptStartedInput,
        effects: &mut Vec<FsmEffect>,
    ) {
        let AttemptStartedInput {
            attempt_id,
            session_id,
            download_id,
            engine_type,
            protocol,
        } = input;
        if let Some(cause) = self.detached_attempts.get(&attempt_id) {
            effects.push(FsmEffect::StopAttempt {
                attempt_id,
                download_id: Some(download_id),
                cause: cause.clone(),
            });
            return;
        }
        if let MonitorPhase::Stopping(stopping) = &self.monitor {
            // Stop arrived while the startup task was finishing; immediately
            // request stop on the newly created engine download.
            let cause = stop_cause_for_kind(&stopping.kind);
            if let SessionSlot::Open(open) = &mut self.session
                && open.session_id == session_id
            {
                open.attempt = AttemptPhase::StoppingAttempt {
                    attempt_id,
                    download_id: Some(download_id.clone()),
                    cause: cause.clone(),
                };
            }
            effects.push(FsmEffect::StopAttempt {
                attempt_id,
                download_id: Some(download_id),
                cause,
            });
            return;
        }

        if !self.matches_current_attempt(&session_id, attempt_id) {
            return;
        }

        // A teardown outside `Stopping` (offline end, fatal probe, danmu stream
        // closed) asked this attempt to stop before its engine existed, so pass
        // the stop on to the engine instead of recording.
        if let SessionSlot::Open(open) = &mut self.session
            && let AttemptPhase::StoppingAttempt {
                download_id: stopping_download,
                cause,
                ..
            } = &mut open.attempt
        {
            if stopping_download.is_none() {
                *stopping_download = Some(download_id.clone());
                effects.push(FsmEffect::StopAttempt {
                    attempt_id,
                    download_id: Some(download_id),
                    cause: cause.clone(),
                });
            }
            return;
        }

        if let SessionSlot::Open(open) = &mut self.session {
            let engine_kind = EngineKind::from_engine_and_protocol(engine_type, protocol);
            open.attempt = AttemptPhase::Running {
                attempt_id,
                download_id,
                started_at: ctx.wall_now,
                engine_kind,
                engine_type,
                protocol,
            };
            open.credentials_changed_retried = false;
            if self.config.record_danmu && matches!(open.danmu, DanmuSessionState::Off) {
                open.danmu = DanmuSessionState::Starting;
                open.segment_guard.danmu_stopped = false;
            }
        }

        self.offline_confirmation.mark_live();
        if let MonitorPhase::LiveWatchdog {
            last_activity_at, ..
        } = &mut self.monitor
        {
            *last_activity_at = ctx.now;
        } else if !matches!(self.monitor, MonitorPhase::Terminated) {
            self.monitor = MonitorPhase::LiveWatchdog {
                last_activity_at: ctx.now,
                next_filter_hint: None,
                watchdog_backoff_until: None,
            };
        }
    }

    /// Handle `AttemptStartFailed` (startup exited before engine start).
    pub(super) fn on_attempt_start_failed(
        &mut self,
        ctx: StepContext,
        attempt_id: u64,
        session_id: String,
        reason: AttemptStartFailure,
        effects: &mut Vec<FsmEffect>,
    ) {
        if self.detached_attempts.remove(&attempt_id).is_some() {
            self.mark_guard_download_ended(&session_id);
            self.resume_held_attempt(ctx, effects);
            return;
        }
        let is_current = self.matches_current_attempt(&session_id, attempt_id);
        if matches!(self.monitor, MonitorPhase::Stopping(_)) {
            if is_current || self.stop_awaits_attempt(attempt_id) {
                self.mark_guard_download_ended(&session_id);
                self.settle_stop_download(ctx, &session_id, is_current, effects);
            }
            return;
        }
        if !is_current {
            return;
        }
        self.mark_guard_download_ended(&session_id);

        match reason {
            AttemptStartFailure::Cancelled | AttemptStartFailure::StaleBinding => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
                }
            }
            AttemptStartFailure::CredentialsChanged => {
                let can_retry = match &mut self.session {
                    SessionSlot::Open(open) => {
                        open.attempt = AttemptPhase::Idle;
                        if !open.credentials_changed_retried {
                            open.credentials_changed_retried = true;
                            true
                        } else {
                            open.attempt = AttemptPhase::WaitingCredentials;
                            false
                        }
                    }
                    _ => false,
                };
                if can_retry {
                    self.start_attempt_for_open_session(ctx, None, false, effects);
                }
            }
            AttemptStartFailure::CredentialsUnavailable => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::WaitingCredentials;
                }
            }
            AttemptStartFailure::RecorderFeedbackBusy => {
                // The lane sends `RetryAdmission` once it has room again. Room
                // that freed while this attempt was starting is not reported
                // twice, so the check runs now.
                let mut rechecked = false;
                if let SessionSlot::Open(open) = &mut self.session {
                    rechecked = matches!(
                        open.attempt,
                        AttemptPhase::Starting {
                            admission_rechecked: true,
                            ..
                        }
                    );
                    open.attempt = AttemptPhase::Idle;
                }
                if rechecked {
                    self.monitor = schedule::schedule_idle_immediate(ctx.now);
                }
            }
            AttemptStartFailure::NotLiveAfterQueue => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
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
            AttemptStartFailure::CircuitBreaker {
                reason,
                retry_after_secs,
            } => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
                }
                self.apply_rejected_download(
                    ctx,
                    session_id,
                    reason,
                    Some(retry_after_secs),
                    DownloadRejectedKind::CircuitBreaker,
                    effects,
                );
            }
            AttemptStartFailure::OutputRootUnavailable {
                path,
                io_kind,
                reason,
                retry_after_secs,
            } => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
                }
                self.apply_rejected_download(
                    ctx,
                    session_id,
                    reason,
                    Some(retry_after_secs),
                    DownloadRejectedKind::OutputRootUnavailable { path, io_kind },
                    effects,
                );
            }
            AttemptStartFailure::StreamerBackoff {
                reason,
                retry_after_secs,
            } => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
                }
                self.apply_rejected_download(
                    ctx,
                    session_id,
                    reason,
                    Some(retry_after_secs),
                    DownloadRejectedKind::StreamerBackoff,
                    effects,
                );
            }
            AttemptStartFailure::Other { message } => {
                if let SessionSlot::Open(open) = &mut self.session {
                    open.attempt = AttemptPhase::Idle;
                }
                self.record_error_and_back_off(ctx, message, effects);
            }
        }
    }

    /// Handle `DownloadEnded` (terminal outcome from a download attempt).
    pub(super) fn on_download_ended(
        &mut self,
        ctx: StepContext,
        attempt_id: u64,
        event: DownloadTerminalEvent,
        effects: &mut Vec<FsmEffect>,
    ) {
        let session_id = event.session_id().to_owned();
        self.mark_guard_download_ended(&session_id);
        if self.detached_attempts.remove(&attempt_id).is_some() {
            self.resume_held_attempt(ctx, effects);
            return;
        }

        // Rejected events may arrive with no active Running attempt if
        // preflight rejected before AttemptStarted, so allow Rejected on the
        // matching open session.
        let owns_session_attempt = self.matches_current_attempt(&session_id, attempt_id)
            || (matches!(event, DownloadTerminalEvent::Rejected { .. })
                && matches!(&self.session, SessionSlot::Open(open) if open.session_id == session_id));

        if matches!(self.monitor, MonitorPhase::Stopping(_)) {
            if owns_session_attempt || self.stop_awaits_attempt(attempt_id) {
                self.settle_stop_download(ctx, &session_id, owns_session_attempt, effects);
            }
            return;
        }
        if !owns_session_attempt {
            return;
        }

        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            open.attempt = AttemptPhase::Idle;
        }

        match event {
            DownloadTerminalEvent::Cancelled { cause, .. } => {
                self.apply_cancelled_download(ctx, &session_id, cause, effects);
            }
            DownloadTerminalEvent::Completed {
                engine_signal,
                stop_cause,
                ..
            } => {
                if let Some(cause) = stop_cause {
                    self.apply_completed_with_stop_cause(ctx, &session_id, cause, effects);
                    return;
                }
                self.offline_confirmation.ensure_live();
                self.offline_confirmation.mark_offline_observed();
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );

                let cause = TerminalCause::Completed;
                if cause.is_authoritative_end_with_signal(Some(engine_signal)) {
                    self.commit_direct_download_end(ctx, session_id, cause, effects);
                } else {
                    self.enter_quiet_period(ctx, &session_id, cause, effects);
                }
            }
            DownloadTerminalEvent::Failed {
                download_id,
                engine_type,
                protocol,
                kind,
                error,
                ..
            } => {
                if kind == DownloadFailureKind::CredentialUnavailable {
                    if let SessionSlot::Open(open) = &mut self.session
                        && open.session_id == session_id
                    {
                        open.attempt = AttemptPhase::WaitingCredentials;
                    }
                    return;
                }
                if kind == DownloadFailureKind::CredentialRecovery {
                    self.start_attempt_for_open_session(ctx, None, false, effects);
                    return;
                }

                // Check credential diagnostic eligibility before error tracking or quiet period.
                if self.try_begin_credential_diagnostic(
                    ctx,
                    &session_id,
                    &download_id,
                    engine_type,
                    kind,
                    &error,
                ) {
                    return;
                }

                self.finish_failed_download(
                    ctx,
                    session_id,
                    EngineKind::from_engine_and_protocol(engine_type, protocol),
                    kind,
                    error,
                    effects,
                );
            }
            DownloadTerminalEvent::Rejected {
                reason,
                retry_after_secs,
                kind,
                ..
            } => {
                self.apply_rejected_download(
                    ctx,
                    session_id,
                    reason,
                    retry_after_secs,
                    kind,
                    effects,
                );
            }
        }
    }

    fn apply_cancelled_download(
        &mut self,
        ctx: StepContext,
        session_id: &str,
        cause: DownloadStopCause,
        effects: &mut Vec<FsmEffect>,
    ) {
        match cause {
            DownloadStopCause::DanmuStreamClosed => {
                self.offline_confirmation.reset();
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );
            }
            DownloadStopCause::OutOfSchedule => {
                self.offline_confirmation.ensure_live();
                self.monitor = schedule::schedule_filtered(
                    FilteredKind::OutOfSchedule { smart_wake: None },
                    &self.config,
                    &self.offline_confirmation,
                    self.errors.error_count_u32(),
                    ctx,
                );
            }
            // Shutdown cancels downloads before it stops actors, so parking
            // keeps this actor from recording again during the drain.
            DownloadStopCause::Shutdown => {
                self.monitor = MonitorPhase::Idle {
                    next_check: None,
                    recurring_interval_ms: None,
                    batch_routine_deadline: None,
                };
            }
            DownloadStopCause::StreamerOffline => {
                self.offline_confirmation.ensure_live();
                self.offline_confirmation.mark_offline_observed();
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );
                self.commit_offline_end(ctx, Some(session_id.to_owned()), effects);
            }
            // A disable's own cancel arrives during its stop. One that arrives
            // later belongs to a stop that already timed out, such as a
            // credential retirement, so monitoring carries on.
            DownloadStopCause::User
            | DownloadStopCause::StreamerDisabled
            | DownloadStopCause::Other(_) => {
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

    fn apply_completed_with_stop_cause(
        &mut self,
        ctx: StepContext,
        session_id: &str,
        cause: DownloadStopCause,
        effects: &mut Vec<FsmEffect>,
    ) {
        self.apply_cancelled_download(ctx, session_id, cause, effects);
    }

    pub(super) fn finish_failed_download(
        &mut self,
        ctx: StepContext,
        session_id: String,
        engine_kind: EngineKind,
        kind: DownloadFailureKind,
        error: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        if kind == DownloadFailureKind::UnsupportedStream {
            self.offline_confirmation.reset();
            self.monitor = schedule::schedule_refused_broadcast(
                &self.config,
                &self.offline_confirmation,
                self.errors.error_count_u32(),
                ctx,
            );
            let cause = TerminalCause::Failed { kind };
            self.commit_direct_download_end(ctx, session_id, cause, effects);
            self.record_transient_error(ctx, error, effects);
            return;
        }

        let cause =
            match self
                .network_failures
                .classify_failure(ctx.now, engine_kind, kind, &self.config)
            {
                Some(signal) => TerminalCause::DefinitiveOffline { signal },
                None => TerminalCause::Failed { kind },
            };

        self.finish_failed_download_with_cause(ctx, session_id, cause, error, effects);
    }

    pub(super) fn finish_failed_download_with_cause(
        &mut self,
        ctx: StepContext,
        session_id: String,
        cause: TerminalCause,
        error: String,
        effects: &mut Vec<FsmEffect>,
    ) {
        self.monitor = schedule::schedule_idle_immediate(ctx.now);

        if cause.is_authoritative_end() {
            self.commit_direct_download_end(ctx, session_id, cause, effects);
        } else {
            self.enter_quiet_period(ctx, &session_id, cause, effects);
        }
        self.record_transient_error(ctx, error, effects);
    }

    fn apply_rejected_download(
        &mut self,
        ctx: StepContext,
        session_id: String,
        reason: String,
        retry_after_secs: Option<u64>,
        kind: DownloadRejectedKind,
        effects: &mut Vec<FsmEffect>,
    ) {
        let retry_secs = retry_after_secs.unwrap_or(60);
        let retry_at = ctx
            .now
            .checked_add(Duration::from_secs(retry_secs))
            .unwrap_or(ctx.now);

        let (blocked_kind, infra_block) = match kind {
            DownloadRejectedKind::CircuitBreaker => (
                BlockedKind::TemporalDisabled,
                Some(InfraBlockReason::CircuitBreaker {
                    retry_after_secs: retry_secs,
                }),
            ),
            DownloadRejectedKind::OutputRootUnavailable { path, io_kind } => (
                BlockedKind::OutputRootCooldown,
                Some(InfraBlockReason::OutputRootUnavailable {
                    path,
                    io_kind,
                    retry_after_secs: retry_secs,
                }),
            ),
            DownloadRejectedKind::StreamerBackoff => (BlockedKind::TemporalDisabled, None),
        };

        if infra_block.is_some() {
            let wall_until = ctx.wall_now
                + jiff::SignedDuration::from_secs(retry_secs.min(i64::MAX as u64) as i64);
            self.errors
                .apply_infra_backoff(wall_until, Some(reason.clone()));
        }

        self.monitor = MonitorPhase::Blocked {
            kind: blocked_kind,
            retry_at,
        };

        let cause = TerminalCause::Rejected { reason };
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            Self::stop_open_danmu(open, DanmuStopKind::SessionEnded, effects);
        }

        self.enqueue_or_dispatch_db(
            PendingDbKind::EndSession {
                session_id: Some(session_id.clone()),
                cause: cause.clone(),
                via_hysteresis: false,
                from_quiet_period_timer_id: None,
                sets_offline: false,
                sets_out_of_schedule: false,
                clears_errors: false,
                for_stop: false,
            },
            DbCommitOp::EndSession {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                session_id: Some(session_id),
                cause,
                via_hysteresis: false,
                update_streamer_state: None,
                clear_errors: false,
                checks_restored_threshold: None,
                infra_block,
                suppress_offline_notification: false,
                now: ctx.wall_now,
            },
            effects,
        );
    }

    pub(super) fn commit_direct_download_end(
        &mut self,
        ctx: StepContext,
        session_id: String,
        cause: TerminalCause,
        effects: &mut Vec<FsmEffect>,
    ) {
        let via_hysteresis = match &mut self.session {
            SessionSlot::Open(open) if open.session_id == session_id => {
                Self::stop_open_danmu(open, DanmuStopKind::SessionEnded, effects);
                open.quiet_period.take().is_some()
            }
            _ => false,
        };

        self.enqueue_or_dispatch_db(
            PendingDbKind::EndSession {
                session_id: Some(session_id.clone()),
                cause: cause.clone(),
                via_hysteresis,
                from_quiet_period_timer_id: None,
                sets_offline: false,
                sets_out_of_schedule: false,
                clears_errors: false,
                for_stop: false,
            },
            DbCommitOp::EndSession {
                streamer_id: self.streamer_id.clone(),
                streamer_name: self.config.streamer_name.clone(),
                session_id: Some(session_id),
                cause,
                via_hysteresis,
                update_streamer_state: None,
                clear_errors: false,
                checks_restored_threshold: None,
                infra_block: None,
                suppress_offline_notification: false,
                now: ctx.wall_now,
            },
            effects,
        );
    }
}
