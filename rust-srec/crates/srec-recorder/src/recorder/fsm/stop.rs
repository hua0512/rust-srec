//! Stop contracts and shutdown convergence (`Disable` / `OutOfSchedule`,
//! strict `CredentialRetire`, `Retire { bounds }`, and `Shutdown`).

use crate::downloader::DownloadStopCause;
use crate::session::{TerminalCause, TerminalCauseDto};
use srec_core::domain::StreamerState;

use super::schedule;
use super::state::{
    AttemptPhase, FilteredKind, FsmEffect, MonitorPhase, OpenSession, PendingDbKind, SessionSlot,
    StepContext, StopDanmu, StopDbEnd, StopDownload, StopKind, StoppingState, StreamerFsm,
};
use crate::recorder::ports::{ActorStopReason, DanmuStopKind, DbCommitOp};

impl StreamerFsm {
    /// Initiate one of the stop contracts (`Disable`, `OutOfSchedule`,
    /// `CredentialRetire`, `Retire`, `Shutdown`).
    pub(super) fn initiate_stop(
        &mut self,
        ctx: StepContext,
        kind: StopKind,
        effects: &mut Vec<FsmEffect>,
    ) {
        if matches!(self.monitor, MonitorPhase::Terminated) {
            return;
        }

        // Strict CredentialRetire guard: must match the exact open `(session_id, binding)`.
        if let StopKind::CredentialRetire {
            session_id,
            binding,
        } = &kind
        {
            let matches_captured = matches!(
                &self.session,
                SessionSlot::Open(open)
                    if open.session_id == *session_id
                        && open.credential_binding.as_ref() == Some(binding)
            );
            if !matches_captured {
                return;
            }
        }

        // Stronger stop intents (Disable / Retire / Shutdown) preempt weaker ones.
        if let MonitorPhase::Stopping(current) = &self.monitor
            && stop_priority(&kind) <= stop_priority(&current.kind)
        {
            return;
        }

        if let Some(probe) = self.in_flight_probe.take() {
            effects.push(FsmEffect::CancelProbe {
                probe_id: probe.probe_id,
            });
        }

        // If a session start/resume commit was still queued (never dispatched to
        // SQLite), roll back the session slot before clearing `queued_db` so the
        // stop never waits for a commit that will not run.
        let queued_start_not_dispatched = match &self.session {
            SessionSlot::CommittingStart { op_id, .. } => {
                self.pending_db.as_ref().is_none_or(|p| p.op_id != *op_id)
            }
            _ => false,
        };
        if queued_start_not_dispatched
            && let SessionSlot::CommittingStart { prior_open, .. } =
                std::mem::replace(&mut self.session, SessionSlot::None)
        {
            self.session = match prior_open {
                Some(open) => SessionSlot::Open(open),
                None => SessionSlot::None,
            };
        }

        // Drop non-urgent queued DB writes so the stop's final DB op runs next.
        self.queued_db.clear();

        if matches!(kind, StopKind::Disable | StopKind::Retire { .. }) {
            self.network_failures.clear();
        }

        let stop_cause = stop_cause_for_kind(&kind);
        let danmu_stop_kind = if matches!(kind, StopKind::Shutdown) {
            DanmuStopKind::ServiceShutdown
        } else {
            DanmuStopKind::SessionEnded
        };

        let (download, danmu) = match &mut self.session {
            SessionSlot::Open(open)
            | SessionSlot::CommittingStart {
                prior_open: Some(open),
                ..
            } => {
                let download = match request_stop_open_attempt(open, stop_cause, effects) {
                    Some(attempt_id) => StopDownload::Pending { attempt_id },
                    None => StopDownload::Confirmed,
                };
                let danmu = if Self::stop_open_danmu(open, danmu_stop_kind, effects) {
                    StopDanmu::Pending {
                        session_id: open.session_id.clone(),
                    }
                } else {
                    StopDanmu::Confirmed
                };
                (download, danmu)
            }
            SessionSlot::CommittingStart {
                prior_open: None, ..
            }
            | SessionSlot::None
            | SessionSlot::RecentlyEnded { .. } => (StopDownload::Confirmed, StopDanmu::Confirmed),
        };

        let deadline = match &kind {
            StopKind::Retire { bounds } => {
                // `OBSERVE_RETIREMENT` sets `bounds.download = 0` so the *caller*
                // returns without blocking; the background actor still waits up
                // to `STOP_DEADLINE_DURATION` for the stop it requested so the
                // engine can flush its final segment before `EndSession` commits.
                let wait = if bounds.download.is_zero() {
                    schedule::STOP_DEADLINE_DURATION
                } else {
                    bounds.download
                };
                ctx.now.checked_add(wait).unwrap_or(ctx.now)
            }
            StopKind::Disable
            | StopKind::OutOfSchedule { .. }
            | StopKind::CredentialRetire { .. }
            | StopKind::Shutdown => ctx
                .now
                .checked_add(schedule::STOP_DEADLINE_DURATION)
                .unwrap_or(ctx.now),
        };

        let is_committing_start = matches!(self.session, SessionSlot::CommittingStart { .. });
        if is_committing_start {
            self.deferred_stop = Some(kind.clone());
        }

        self.monitor = MonitorPhase::Stopping(StoppingState {
            kind,
            deadline,
            deadline_expired: false,
            download,
            danmu,
            db_end: StopDbEnd::NotDispatched,
        });

        self.advance_stopping(ctx, effects);
    }

    /// Handle stop deadline expiry during [`MonitorPhase::Stopping`].
    pub(super) fn on_stop_deadline_fired(
        &mut self,
        ctx: StepContext,
        effects: &mut Vec<FsmEffect>,
    ) {
        let MonitorPhase::Stopping(stopping) = &mut self.monitor else {
            return;
        };
        stopping.deadline_expired = true;
        if !stopping.physical_confirmed() {
            self.lingering_physical_lease = true;
        }
        self.advance_stopping(ctx, effects);
    }

    /// Advance the active [`StoppingState`] machine whenever physical resources
    /// confirm, a DB op completes, or the stop deadline elapses.
    pub(super) fn advance_stopping(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        // Wait for an in-flight `CommittingStart` to finish so we learn the
        // authoritative `session_id` before deciding what to end in SQLite.
        if matches!(self.session, SessionSlot::CommittingStart { .. }) {
            return;
        }

        let stopping = match &self.monitor {
            MonitorPhase::Stopping(s) => s.clone(),
            _ => return,
        };

        // Shutdown never closes the open DB session (restart can resume it).
        if matches!(stopping.kind, StopKind::Shutdown) {
            if stopping.physical_confirmed() || stopping.deadline_expired {
                self.monitor = MonitorPhase::Terminated;
                effects.push(FsmEffect::ActorStopped {
                    reason: ActorStopReason::Shutdown,
                });
            }
            return;
        }

        // Strict CredentialRetire contract: if physical download or danmu did
        // not confirm termination within the deadline (or returned Unconfirmed),
        // NEVER commit EndSession / EndForCredentialRetirement.
        if let StopKind::CredentialRetire {
            session_id,
            binding,
        } = &stopping.kind
        {
            if stopping.danmu == StopDanmu::Unconfirmed
                || (stopping.deadline_expired && !stopping.physical_confirmed())
            {
                self.lingering_physical_lease = true;
                self.monitor = schedule::schedule_idle(
                    &self.config,
                    &self.offline_confirmation,
                    false,
                    self.errors.error_count_u32(),
                    None,
                    ctx,
                );
                return;
            }
            if !stopping.physical_confirmed() {
                return;
            }
            if stopping.db_end == StopDbEnd::NotDispatched {
                if self.pending_db.is_some() {
                    return;
                }
                // The captured session already ended another way (an offline
                // end that was in flight, say), so there is nothing to retire.
                let Some(via_hysteresis) = (match &self.session {
                    SessionSlot::Open(open) if open.session_id == *session_id => {
                        Some(open.quiet_period.is_some())
                    }
                    _ => None,
                }) else {
                    self.monitor = schedule::schedule_idle(
                        &self.config,
                        &self.offline_confirmation,
                        false,
                        self.errors.error_count_u32(),
                        None,
                        ctx,
                    );
                    return;
                };
                if let MonitorPhase::Stopping(s) = &mut self.monitor {
                    s.db_end = StopDbEnd::InFlight;
                }
                self.enqueue_or_dispatch_db(
                    PendingDbKind::EndForCredentialRetirement {
                        session_id: session_id.clone(),
                    },
                    DbCommitOp::EndForCredentialRetirement {
                        streamer_id: self.streamer_id.clone(),
                        streamer_name: self.config.streamer_name.clone(),
                        session_id: session_id.clone(),
                        binding: binding.clone(),
                        via_hysteresis,
                        now: ctx.wall_now,
                    },
                    effects,
                );
            } else if matches!(stopping.db_end, StopDbEnd::Settled { .. }) {
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

        // For Retire: if the stop deadline elapses while download or danmu are
        // still active (including zero-wait `OBSERVE_RETIREMENT`), exit as
        // non-quiescent WITHOUT prematurely committing `EndSession` /
        // `PipelineNotification::SessionEnded`. The reaper will close the open
        // session once physical owners actually settle.
        if matches!(stopping.kind, StopKind::Retire { .. })
            && stopping.deadline_expired
            && !stopping.physical_confirmed()
        {
            self.monitor = MonitorPhase::Terminated;
            effects.push(FsmEffect::ActorStopped {
                reason: ActorStopReason::Retired { quiescent: false },
            });
            return;
        }

        // For Disable / OutOfSchedule / Retire: wait until physical confirmed OR deadline expired.
        let ready_for_db = stopping.physical_confirmed() || stopping.deadline_expired;
        if !ready_for_db {
            return;
        }

        if stopping.db_end == StopDbEnd::NotDispatched {
            if self.pending_db.is_some() {
                return;
            }
            match &stopping.kind {
                StopKind::Disable | StopKind::Retire { .. } => {
                    match &self.session {
                        SessionSlot::Open(open) => {
                            let session_id = open.session_id.clone();
                            let via_hysteresis = open.quiet_period.is_some();
                            if let MonitorPhase::Stopping(s) = &mut self.monitor {
                                s.db_end = StopDbEnd::InFlight;
                            }
                            self.enqueue_or_dispatch_db(
                                PendingDbKind::EndSession {
                                    session_id: Some(session_id.clone()),
                                    cause: TerminalCause::UserDisabled,
                                    via_hysteresis,
                                    from_quiet_period_timer_id: None,
                                    sets_offline: false,
                                    sets_out_of_schedule: false,
                                    clears_errors: false,
                                    for_stop: true,
                                },
                                DbCommitOp::EndSession {
                                    streamer_id: self.streamer_id.clone(),
                                    streamer_name: self.config.streamer_name.clone(),
                                    session_id: Some(session_id),
                                    cause: TerminalCause::UserDisabled,
                                    via_hysteresis,
                                    update_streamer_state: None,
                                    clear_errors: false,
                                    checks_restored_threshold: None,
                                    infra_block: None,
                                    suppress_offline_notification: true,
                                    now: ctx.wall_now,
                                },
                                effects,
                            );
                            return;
                        }
                        SessionSlot::RecentlyEnded {
                            session_id,
                            evict_at,
                        } if ctx.now < *evict_at => {
                            let session_id = session_id.clone();
                            if let MonitorPhase::Stopping(s) = &mut self.monitor {
                                s.db_end = StopDbEnd::InFlight;
                            }
                            self.enqueue_or_dispatch_db(
                                PendingDbKind::RewriteEndedCause { for_stop: true },
                                DbCommitOp::RewriteEndedCause {
                                    session_id,
                                    new_cause: TerminalCauseDto::UserDisabled,
                                },
                                effects,
                            );
                            return;
                        }
                        SessionSlot::None | SessionSlot::RecentlyEnded { .. }
                            if self.initial_reconcile_pending =>
                        {
                            // Cold-start safety: DB may still hold an active session row.
                            if let MonitorPhase::Stopping(s) = &mut self.monitor {
                                s.db_end = StopDbEnd::InFlight;
                            }
                            self.enqueue_or_dispatch_db(
                                PendingDbKind::EndSession {
                                    session_id: None,
                                    cause: TerminalCause::UserDisabled,
                                    via_hysteresis: false,
                                    from_quiet_period_timer_id: None,
                                    sets_offline: false,
                                    sets_out_of_schedule: false,
                                    clears_errors: false,
                                    for_stop: true,
                                },
                                DbCommitOp::EndSession {
                                    streamer_id: self.streamer_id.clone(),
                                    streamer_name: self.config.streamer_name.clone(),
                                    session_id: None,
                                    cause: TerminalCause::UserDisabled,
                                    via_hysteresis: false,
                                    update_streamer_state: None,
                                    clear_errors: false,
                                    checks_restored_threshold: None,
                                    infra_block: None,
                                    suppress_offline_notification: true,
                                    now: ctx.wall_now,
                                },
                                effects,
                            );
                            return;
                        }
                        SessionSlot::None
                        | SessionSlot::RecentlyEnded { .. }
                        | SessionSlot::CommittingStart { .. } => {
                            if let MonitorPhase::Stopping(s) = &mut self.monitor {
                                s.db_end = StopDbEnd::Settled { failed: false };
                            }
                        }
                    }
                }
                StopKind::OutOfSchedule { .. } => {
                    let (session_id, via_hysteresis) = match &self.session {
                        SessionSlot::Open(open) => {
                            (Some(open.session_id.clone()), open.quiet_period.is_some())
                        }
                        _ => (None, false),
                    };
                    if let MonitorPhase::Stopping(s) = &mut self.monitor {
                        s.db_end = StopDbEnd::InFlight;
                    }
                    self.enqueue_or_dispatch_db(
                        PendingDbKind::EndSession {
                            session_id: session_id.clone(),
                            cause: TerminalCause::OutOfSchedule,
                            via_hysteresis,
                            from_quiet_period_timer_id: None,
                            sets_offline: false,
                            sets_out_of_schedule: true,
                            clears_errors: false,
                            for_stop: true,
                        },
                        DbCommitOp::EndSession {
                            streamer_id: self.streamer_id.clone(),
                            streamer_name: self.config.streamer_name.clone(),
                            session_id,
                            cause: TerminalCause::OutOfSchedule,
                            via_hysteresis,
                            update_streamer_state: Some(StreamerState::OutOfSchedule),
                            clear_errors: false,
                            checks_restored_threshold: None,
                            infra_block: None,
                            suppress_offline_notification: true,
                            now: ctx.wall_now,
                        },
                        effects,
                    );
                    return;
                }
                StopKind::CredentialRetire { .. } | StopKind::Shutdown => return,
            }
        }

        // Finalize once DB step is settled.
        let finalized = match &self.monitor {
            MonitorPhase::Stopping(s) if matches!(s.db_end, StopDbEnd::Settled { .. }) => s.clone(),
            _ => return,
        };

        match finalized.kind {
            StopKind::Disable => {
                self.monitor = MonitorPhase::Terminated;
                effects.push(FsmEffect::ActorStopped {
                    reason: ActorStopReason::Disabled,
                });
            }
            StopKind::OutOfSchedule { smart_wake } => {
                self.offline_confirmation.ensure_live();
                self.monitor = schedule::schedule_filtered(
                    FilteredKind::OutOfSchedule { smart_wake },
                    &self.config,
                    &self.offline_confirmation,
                    self.errors.error_count_u32(),
                    ctx,
                );
            }
            StopKind::Retire { .. } => {
                let quiescent = finalized.physical_confirmed()
                    && finalized.db_end == (StopDbEnd::Settled { failed: false })
                    && self.is_physically_quiescent();
                self.monitor = MonitorPhase::Terminated;
                effects.push(FsmEffect::ActorStopped {
                    reason: ActorStopReason::Retired { quiescent },
                });
            }
            StopKind::CredentialRetire { .. } | StopKind::Shutdown => {}
        }
    }
}

/// Ask the open session's attempt to stop with `cause`, returning the attempt
/// the caller now waits for. An attempt already stopping keeps its first
/// request; a credential diagnosis has no download left to stop.
pub(super) fn request_stop_open_attempt(
    open: &mut OpenSession,
    cause: DownloadStopCause,
    effects: &mut Vec<FsmEffect>,
) -> Option<u64> {
    let (attempt_id, download_id) = match &open.attempt {
        AttemptPhase::Starting { attempt_id, .. } => (*attempt_id, None),
        AttemptPhase::Running {
            attempt_id,
            download_id,
            ..
        } => (*attempt_id, Some(download_id.clone())),
        AttemptPhase::StoppingAttempt { attempt_id, .. } => return Some(*attempt_id),
        AttemptPhase::DiagnosingCredentials { .. } => {
            open.attempt = AttemptPhase::Idle;
            return None;
        }
        AttemptPhase::Idle | AttemptPhase::WaitingCredentials => return None,
    };
    open.attempt = AttemptPhase::StoppingAttempt {
        attempt_id,
        download_id: download_id.clone(),
        cause: cause.clone(),
    };
    effects.push(FsmEffect::StopAttempt {
        attempt_id,
        download_id,
        cause,
    });
    Some(attempt_id)
}

pub(super) fn stop_cause_for_kind(kind: &StopKind) -> DownloadStopCause {
    match kind {
        StopKind::Disable | StopKind::Retire { .. } | StopKind::CredentialRetire { .. } => {
            DownloadStopCause::StreamerDisabled
        }
        StopKind::OutOfSchedule { .. } => DownloadStopCause::OutOfSchedule,
        StopKind::Shutdown => DownloadStopCause::Shutdown,
    }
}

fn stop_priority(kind: &StopKind) -> u8 {
    match kind {
        StopKind::OutOfSchedule { .. } => 1,
        StopKind::CredentialRetire { .. } => 2,
        StopKind::Disable => 3,
        StopKind::Retire { .. } => 4,
        StopKind::Shutdown => 5,
    }
}
