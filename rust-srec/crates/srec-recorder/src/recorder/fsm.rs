//! Pure synchronous [`StreamerFsm`] driver and thin [`StreamerFsm::step`] dispatcher.

pub mod attempt;
pub mod commit;
pub mod credentials;
pub mod error;
pub mod hysteresis;
pub mod probe;
pub mod schedule;
pub mod segment;
pub mod state;
pub mod stop;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use jiff::Timestamp;

use srec_core::domain::StreamerState;

pub use self::error::ErrorTracker;
pub use self::hysteresis::{OfflineConfirmation, SlidingFailureWindow};
pub use self::segment::SessionSegmentGuard;
pub use self::state::{
    AttemptPhase, BlockedKind, DanmuSessionState, FilteredKind, FsmEffect, FsmInput, InFlightProbe,
    MonitorPhase, OpenSession, PendingDbKind, PendingDbOp, QuietPeriod, RecoveredMedia,
    SessionSlot, StepContext, StopDanmu, StopDbEnd, StopDownload, StopKind, StoppingState,
    StreamerFsm,
};

use self::attempt::AttemptStartedInput;
use self::segment::{SegmentStartedInput, VideoSegmentInput};
use self::state::QueuedDbOp;
use crate::recorder::ports::{
    DbCommitOp, InitialStreamerSnapshot, ResolvedActorConfig, StreamerRuntimeView,
    should_spawn_actor,
};

/// Timers the state machine arms, in the order one `TimerFired` handles them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timer {
    /// End of the `RecentlyEnded` reattribution window.
    RecentlyEndedEviction,
    /// Stop confirmation deadline in `Stopping`.
    StopDeadline,
    /// Backoff before a credential recovery probe.
    CredentialDiagnostic,
    /// Quiet-period backstop, or the retry of its failed `EndSession`.
    QuietPeriod,
    /// The monitor phase's next status probe.
    MonitorCheck,
}

impl Timer {
    const ALL: [Self; 5] = [
        Self::RecentlyEndedEviction,
        Self::StopDeadline,
        Self::CredentialDiagnostic,
        Self::QuietPeriod,
        Self::MonitorCheck,
    ];
}

impl StreamerFsm {
    /// Construct a new [`StreamerFsm`] from a persisted streamer snapshot.
    ///
    /// Persisted [`StreamerState::OutOfSpace`] and other inactive states start
    /// dormant ([`MonitorPhase::Terminated`]); only active states (including
    /// [`StreamerState::TemporalDisabled`]) schedule polling.
    pub fn new(snapshot: InitialStreamerSnapshot, now: Instant, wall_now: Timestamp) -> Self {
        let errors = ErrorTracker::from_persisted(
            snapshot.consecutive_error_count,
            snapshot.disabled_until,
            snapshot.last_error,
        );

        let monitor = match snapshot.state {
            _ if !should_spawn_actor(snapshot.state, snapshot.deleted) => MonitorPhase::Terminated,
            StreamerState::TemporalDisabled => {
                let remaining = errors
                    .remaining_backoff(wall_now)
                    .unwrap_or(std::time::Duration::ZERO);
                MonitorPhase::Blocked {
                    kind: BlockedKind::TemporalDisabled,
                    retry_at: now.checked_add(remaining).unwrap_or(now),
                }
            }
            StreamerState::OutOfSchedule => MonitorPhase::Filtered {
                reason: FilteredKind::OutOfSchedule { smart_wake: None },
                next_check: now,
                recurring_interval_ms: None,
            },
            // The remaining active states: `NotLive`, `Live` and `InspectingLive`.
            _ => schedule::schedule_idle_immediate(now),
        };

        let offline_confirmation =
            OfflineConfirmation::from_live(snapshot.state == StreamerState::Live);

        Self {
            streamer_id: snapshot.streamer_id,
            config: snapshot.config,
            config_revision: snapshot.config_revision,
            monitor,
            offline_confirmation,
            session: SessionSlot::None,
            in_flight_probe: None,
            pending_db: None,
            queued_db: VecDeque::new(),
            deferred_stop: None,
            initial_reconcile_pending: true,
            errors,
            network_failures: SlidingFailureWindow::default(),
            lingering_physical_lease: false,
            batch_live_hint_armed: false,
            draining_segment_guards: HashMap::new(),
            detached_attempts: HashMap::new(),
            next_seq: 1,
        }
    }

    /// Give actor generation `generation` its own id range. Probe, commit and
    /// attempt ids then never repeat across a streamer's generations, so the
    /// download events of an earlier generation's attempts match no attempt
    /// of this one.
    pub fn for_generation(mut self, generation: u64) -> Self {
        self.next_seq = (generation << 32) + 1;
        self
    }

    /// Process one [`FsmInput`] synchronously and return every side effect to perform.
    pub fn step(&mut self, ctx: StepContext, input: FsmInput) -> Vec<FsmEffect> {
        let mut effects = Vec::new();
        let before_view = self.runtime_view();

        match input {
            FsmInput::TimerFired => self.on_timer_fired(ctx, &mut effects),
            FsmInput::CheckNow => self.on_check_now(ctx),
            FsmInput::BatchLiveHint => self.on_batch_live_hint(ctx),
            FsmInput::RetryAdmission => self.on_retry_admission(ctx),
            FsmInput::ConfigUpdated { revision, config } => {
                self.on_config_updated(ctx, revision, *config, &mut effects);
            }
            FsmInput::ErrorCleared => self.on_error_cleared(ctx),
            FsmInput::CredentialMaterialChanged => {
                self.on_credential_material_changed(ctx, &mut effects);
            }
            FsmInput::ProbeCompleted { probe_id, result } => {
                self.on_probe_completed(ctx, probe_id, result, &mut effects);
            }
            FsmInput::DbCommitted { op_id, result } => {
                self.on_db_committed(ctx, op_id, result, &mut effects);
            }
            FsmInput::AttemptQueued {
                attempt_id,
                session_id,
            } => self.on_attempt_queued(attempt_id, &session_id),
            FsmInput::AttemptBindingCommitted {
                attempt_id,
                session_id,
                binding,
            } => {
                if self.matches_current_attempt(&session_id, attempt_id)
                    && let SessionSlot::Open(open) = &mut self.session
                {
                    open.credential_binding = Some(binding);
                }
            }
            FsmInput::AttemptDequeued {
                attempt_id,
                session_id,
            } => self.on_attempt_dequeued(attempt_id, &session_id),
            FsmInput::AttemptStarted {
                attempt_id,
                session_id,
                download_id,
                engine_type,
                protocol,
            } => self.on_attempt_started(
                ctx,
                AttemptStartedInput {
                    attempt_id,
                    session_id,
                    download_id,
                    engine_type,
                    protocol,
                },
                &mut effects,
            ),
            FsmInput::AttemptStartFailed {
                attempt_id,
                session_id,
                reason,
            } => self.on_attempt_start_failed(ctx, attempt_id, session_id, reason, &mut effects),
            FsmInput::DownloadHeartbeat {
                session_id,
                download_id,
                progress,
            } => self.on_download_heartbeat(
                ctx,
                &session_id,
                &download_id,
                progress.as_ref(),
                &mut effects,
            ),
            FsmInput::SegmentStarted {
                attempt_id,
                session_id,
                segment_index,
                segment_path,
                started_at,
                ..
            } => self.on_segment_started(
                ctx,
                SegmentStartedInput {
                    attempt_id,
                    session_id,
                    segment_index,
                    segment_path,
                    started_at,
                },
                &mut effects,
            ),
            FsmInput::SegmentCompleted {
                attempt_id,
                session_id,
                segment_index,
                segment_path,
                started_at,
                completed_at,
                duration_secs,
                size_bytes,
                effective_size_bytes,
                split_reason_code,
                split_reason_details_json,
                split_by_format_change,
                ..
            } => self.on_segment_completed(
                ctx,
                VideoSegmentInput {
                    attempt_id,
                    session_id,
                    segment_index,
                    segment_path,
                    started_at,
                    completed_at,
                    duration_secs,
                    size_bytes,
                    effective_size_bytes,
                    split_reason_code,
                    split_reason_details_json,
                    split_by_format_change,
                },
                &mut effects,
            ),
            FsmInput::DownloadEnded { attempt_id, event } => {
                self.on_download_ended(ctx, attempt_id, event, &mut effects);
            }
            FsmInput::DanmuCollectionStarted { session_id } => {
                self.on_danmu_collection_started(session_id, &mut effects);
            }
            FsmInput::DanmuSegmentCompleted {
                session_id,
                segment_index,
                output_path,
                message_count,
                size_bytes,
            } => self.on_danmu_segment_completed(
                session_id,
                segment_index,
                output_path,
                message_count,
                size_bytes,
                &mut effects,
            ),
            FsmInput::DanmuControl {
                session_id,
                control,
            } => self.on_danmu_control(ctx, session_id, control, &mut effects),
            FsmInput::DanmuReconnecting { session_id, .. } => {
                self.on_danmu_reconnecting(ctx, &session_id);
            }
            FsmInput::DanmuReconnected { session_id, .. } => {
                self.on_danmu_reconnected(&session_id);
            }
            FsmInput::DanmuCollectionStopped {
                session_id,
                total_count,
                result,
            } => {
                self.on_danmu_collection_stopped(ctx, session_id, total_count, result, &mut effects)
            }
            FsmInput::PhysicalLeaseReleased => {
                // The registry reports this only once no download, startup or
                // danmu collector of the streamer remains: detached attempts
                // have ended, and a stop's physical legs are settled.
                self.lingering_physical_lease = false;
                self.detached_attempts.clear();
                if let MonitorPhase::Stopping(stopping) = &mut self.monitor {
                    stopping.download = StopDownload::Confirmed;
                    stopping.danmu = StopDanmu::Confirmed;
                    self.advance_stopping(ctx, &mut effects);
                } else {
                    self.resume_held_attempt(ctx, &mut effects);
                }
            }
            FsmInput::Disable => self.initiate_stop(ctx, StopKind::Disable, &mut effects),
            FsmInput::CredentialRetire {
                session_id,
                binding,
            } => self.initiate_stop(
                ctx,
                StopKind::CredentialRetire {
                    session_id,
                    binding,
                },
                &mut effects,
            ),
            FsmInput::Retire { bounds } => {
                self.initiate_stop(ctx, StopKind::Retire { bounds }, &mut effects);
            }
            FsmInput::Shutdown => self.initiate_stop(ctx, StopKind::Shutdown, &mut effects),
        }

        let after_view = self.runtime_view();
        if after_view != before_view {
            effects.push(FsmEffect::PublishRuntimeView(after_view));
        }
        effects
    }

    /// Database writes waiting behind the one in flight.
    pub fn queued_db_len(&self) -> usize {
        self.queued_db.len()
    }

    /// Earliest monotonic instant at which the caller must invoke
    /// `step(ctx, FsmInput::TimerFired)`, or `None` when parked / terminated.
    pub fn next_deadline(&self, now: Instant, wall_now: Timestamp) -> Option<Instant> {
        Timer::ALL
            .into_iter()
            .filter_map(|timer| self.timer_deadline(timer, now, wall_now))
            .min()
    }

    /// When `timer` is due, or `None` while it is not armed.
    ///
    /// [`Self::next_deadline`] and [`Self::on_timer_fired`] both read this
    /// table, so a timer is offered only while its handler can act on it.
    /// Every handler either disarms its timer or moves it into the future.
    fn timer_deadline(&self, timer: Timer, now: Instant, wall_now: Timestamp) -> Option<Instant> {
        let monitoring = !matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        );
        match timer {
            Timer::RecentlyEndedEviction => match &self.session {
                SessionSlot::RecentlyEnded { evict_at, .. } => Some(*evict_at),
                _ => None,
            },
            Timer::StopDeadline => match &self.monitor {
                MonitorPhase::Stopping(stopping) if !stopping.deadline_expired => {
                    Some(stopping.deadline)
                }
                _ => None,
            },
            Timer::CredentialDiagnostic => match &self.session {
                SessionSlot::Open(open) if monitoring && self.in_flight_probe.is_none() => {
                    match open.attempt {
                        AttemptPhase::DiagnosingCredentials {
                            probe_id: None,
                            backoff_until,
                            ..
                        } => backoff_until,
                        _ => None,
                    }
                }
                _ => None,
            },
            // The backstop and its commit retry dispatch `EndSession`, so they
            // wait for any in-flight commit. A stop ends the session itself.
            Timer::QuietPeriod => match &self.session {
                SessionSlot::Open(open) if monitoring && self.pending_db.is_none() => {
                    let quiet_period = open.quiet_period.as_ref()?;
                    if quiet_period.retrying_commit {
                        quiet_period.next_retry_at
                    } else if quiet_period.grace_remaining_checks.is_none() {
                        Some(quiet_period.backstop_deadline)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            Timer::MonitorCheck => {
                if self.in_flight_probe.is_some() || self.pending_db.is_some() {
                    return None;
                }
                match &self.monitor {
                    MonitorPhase::Idle { next_check, .. } => *next_check,
                    MonitorPhase::Filtered { next_check, .. }
                    | MonitorPhase::RefusedBroadcast { next_check, .. } => Some(*next_check),
                    MonitorPhase::Blocked { retry_at, .. } => Some(*retry_at),
                    MonitorPhase::LiveWatchdog { .. }
                        if matches!(
                            &self.session,
                            SessionSlot::Open(open)
                                if matches!(open.attempt, AttemptPhase::DiagnosingCredentials { .. })
                        ) =>
                    {
                        None
                    }
                    MonitorPhase::LiveWatchdog {
                        last_activity_at,
                        next_filter_hint,
                        watchdog_backoff_until,
                    } => Some(schedule::live_watchdog_deadline(
                        *last_activity_at,
                        *next_filter_hint,
                        *watchdog_backoff_until,
                        self.config.check_interval_ms,
                        now,
                        wall_now,
                    )),
                    MonitorPhase::Stopping(_) | MonitorPhase::Terminated => None,
                }
            }
        }
    }

    /// Whether every physical resource (probe, DB commit, download attempt,
    /// danmu collector, lingering physical leases, draining guards) is quiescent.
    ///
    /// `Stopping` deadline expiry never forces this predicate to `true`, so
    /// `reap_deleted` and `reap_retired_profiles` remain safely gated.
    pub fn is_physically_quiescent(&self) -> bool {
        if self.lingering_physical_lease
            || self.in_flight_probe.is_some()
            || self.pending_db.is_some()
            || !self.queued_db.is_empty()
            || !self.draining_segment_guards.is_empty()
            || !self.detached_attempts.is_empty()
        {
            return false;
        }
        match &self.session {
            SessionSlot::Open(open) => {
                matches!(
                    open.attempt,
                    AttemptPhase::Idle | AttemptPhase::WaitingCredentials
                ) && matches!(open.danmu, DanmuSessionState::Off)
            }
            SessionSlot::CommittingStart { .. } => false,
            SessionSlot::None | SessionSlot::RecentlyEnded { .. } => true,
        }
    }

    /// Project the current state into a unidirectional read-only [`StreamerRuntimeView`].
    pub fn runtime_view(&self) -> StreamerRuntimeView {
        let monitor_state = match &self.monitor {
            MonitorPhase::Idle { .. } => StreamerState::NotLive,
            MonitorPhase::Filtered { .. } => StreamerState::OutOfSchedule,
            MonitorPhase::RefusedBroadcast { .. } => StreamerState::NotLive,
            MonitorPhase::Blocked {
                kind: BlockedKind::TemporalDisabled,
                ..
            } => StreamerState::TemporalDisabled,
            MonitorPhase::Blocked {
                kind: BlockedKind::OutputRootCooldown,
                ..
            } => StreamerState::OutOfSpace,
            MonitorPhase::LiveWatchdog { .. } => StreamerState::Live,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated => StreamerState::Disabled,
        };

        let (
            active_session_id,
            recording_started_at,
            in_hysteresis,
            attempt_active,
            attempt_queued,
            danmu_collecting,
            danmu_link_down_since,
        ) = match &self.session {
            SessionSlot::Open(open) => {
                let (active, queued, attempt_started_at) = match &open.attempt {
                    AttemptPhase::Starting { queued, .. } => (false, *queued, None),
                    AttemptPhase::Running { started_at, .. } => (true, false, Some(*started_at)),
                    AttemptPhase::StoppingAttempt { .. }
                    | AttemptPhase::Idle
                    | AttemptPhase::WaitingCredentials
                    | AttemptPhase::DiagnosingCredentials { .. } => (false, false, None),
                };
                let (collecting, link_down) = match open.danmu {
                    DanmuSessionState::Running { link_down_since } => (true, link_down_since),
                    DanmuSessionState::Starting | DanmuSessionState::Stopping => (true, None),
                    DanmuSessionState::Off => (false, None),
                };
                (
                    Some(open.session_id.clone()),
                    attempt_started_at,
                    open.quiet_period.is_some(),
                    active,
                    queued,
                    collecting,
                    link_down,
                )
            }
            _ => (None, None, false, false, false, false, None),
        };

        StreamerRuntimeView {
            streamer_id: self.streamer_id.clone(),
            monitor_state,
            active_session_id,
            recording_started_at,
            in_hysteresis,
            attempt_active,
            attempt_queued,
            expects_danmu: self.config.record_danmu,
            danmu_collecting,
            danmu_link_down_since,
            lingering_physical_lease: self.lingering_physical_lease,
            config_revision: self.config_revision,
        }
    }

    pub(super) fn next_id(&mut self) -> u64 {
        let id = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        id
    }

    pub(super) fn enqueue_or_dispatch_db(
        &mut self,
        kind: PendingDbKind,
        op: DbCommitOp,
        effects: &mut Vec<FsmEffect>,
    ) -> u64 {
        let op_id = self.next_id();
        if self.pending_db.is_some() {
            self.queued_db.push_back(QueuedDbOp { op_id, kind, op });
            return op_id;
        }
        self.pending_db = Some(PendingDbOp { op_id, kind });
        effects.push(FsmEffect::CommitDb { op_id, op });
        op_id
    }

    /// Whether a commit that ends `session_id` is in flight or queued.
    pub(super) fn session_end_pending(&self, session_id: &str) -> bool {
        let ends_session = |kind: &PendingDbKind| match kind {
            PendingDbKind::EndSession {
                session_id: Some(ending),
                ..
            }
            | PendingDbKind::EndForCredentialRetirement { session_id: ending } => {
                ending == session_id
            }
            _ => false,
        };
        self.pending_db
            .as_ref()
            .is_some_and(|pending| ends_session(&pending.kind))
            || self
                .queued_db
                .iter()
                .any(|queued| ends_session(&queued.kind))
    }

    pub(super) fn flush_queued_db(&mut self, effects: &mut Vec<FsmEffect>) {
        if self.pending_db.is_some() {
            return;
        }
        if let Some(next) = self.queued_db.pop_front() {
            self.pending_db = Some(PendingDbOp {
                op_id: next.op_id,
                kind: next.kind,
            });
            effects.push(FsmEffect::CommitDb {
                op_id: next.op_id,
                op: next.op,
            });
        }
    }

    fn on_timer_fired(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        // Each handler can disarm timers later in the order (a stop deadline
        // that ends the stop, a recovery probe that occupies the probe slot),
        // so the table is read again before every handler.
        for timer in Timer::ALL {
            let due = self
                .timer_deadline(timer, ctx.now, ctx.wall_now)
                .is_some_and(|deadline| deadline <= ctx.now);
            if !due {
                continue;
            }
            match timer {
                Timer::RecentlyEndedEviction => self.session = SessionSlot::None,
                Timer::StopDeadline => self.on_stop_deadline_fired(ctx, effects),
                Timer::CredentialDiagnostic => self.on_credential_diagnostic_timer(ctx, effects),
                Timer::QuietPeriod => self.on_quiet_period_timer(ctx, effects),
                Timer::MonitorCheck => self.on_monitor_check_due(ctx, effects),
            }
        }
    }

    /// Start the monitor phase's status probe, or wait out a persisted
    /// `disabled_until` backoff first (a live session's watchdog ignores it).
    fn on_monitor_check_due(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        let is_live_watchdog = matches!(self.monitor, MonitorPhase::LiveWatchdog { .. });
        if !is_live_watchdog && let Some(remaining) = self.errors.remaining_backoff(ctx.wall_now) {
            let kind = match &self.monitor {
                MonitorPhase::Blocked { kind, .. } => *kind,
                _ => BlockedKind::TemporalDisabled,
            };
            self.monitor = MonitorPhase::Blocked {
                kind,
                retry_at: ctx.now.checked_add(remaining).unwrap_or(ctx.now),
            };
            return;
        }
        self.spawn_status_probe(effects);
    }

    fn on_check_now(&mut self, ctx: StepContext) {
        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }
        self.batch_live_hint_armed = false;
        match &mut self.monitor {
            MonitorPhase::Idle {
                next_check,
                recurring_interval_ms,
                ..
            } => {
                *next_check = Some(ctx.now);
                *recurring_interval_ms = None;
            }
            MonitorPhase::Filtered {
                next_check,
                recurring_interval_ms,
                ..
            }
            | MonitorPhase::RefusedBroadcast {
                next_check,
                recurring_interval_ms,
            } => {
                *next_check = ctx.now;
                *recurring_interval_ms = None;
            }
            MonitorPhase::Blocked { retry_at, .. } => {
                *retry_at = ctx.now;
            }
            MonitorPhase::LiveWatchdog {
                next_filter_hint,
                watchdog_backoff_until,
                ..
            } => {
                *next_filter_hint = Some(ctx.wall_now);
                *watchdog_backoff_until = None;
            }
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated => {}
        }
    }

    fn on_batch_live_hint(&mut self, ctx: StepContext) {
        if !self.batch_live_hint_armed
            || self.in_flight_probe.is_some()
            || self.pending_db.is_some()
            || self.offline_confirmation.was_live
            || matches!(
                self.session,
                SessionSlot::Open(_) | SessionSlot::CommittingStart { .. }
            )
            || self.errors.has_errors()
            || self.errors.is_disabled(ctx.wall_now)
        {
            return;
        }
        let MonitorPhase::Idle {
            next_check,
            recurring_interval_ms,
            batch_routine_deadline,
        } = &mut self.monitor
        else {
            return;
        };
        if recurring_interval_ms.is_none() {
            return;
        }
        self.batch_live_hint_armed = false;
        if batch_routine_deadline.is_none() {
            *batch_routine_deadline = *next_check;
        }
        *next_check = Some(ctx.now);
        *recurring_interval_ms = None;
    }

    /// The lifecycle lane has room again after refusing a start.
    fn on_retry_admission(&mut self, ctx: StepContext) {
        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }
        if let SessionSlot::Open(open) = &mut self.session {
            match &mut open.attempt {
                // The refusal for this attempt may still be on its way.
                AttemptPhase::Starting {
                    admission_rechecked,
                    ..
                } => {
                    *admission_rechecked = true;
                    return;
                }
                AttemptPhase::Running { .. } | AttemptPhase::StoppingAttempt { .. } => return,
                AttemptPhase::Idle
                | AttemptPhase::WaitingCredentials
                | AttemptPhase::DiagnosingCredentials { .. } => {}
            }
        }
        self.monitor = schedule::schedule_idle_immediate(ctx.now);
    }

    fn on_config_updated(
        &mut self,
        ctx: StepContext,
        revision: u64,
        config: ResolvedActorConfig,
        effects: &mut Vec<FsmEffect>,
    ) {
        // Monotonic configuration revision fence: ignore stale async resolutions.
        if revision <= self.config_revision {
            return;
        }
        self.config_revision = revision;
        self.config = config;
        self.batch_live_hint_armed = false;

        schedule::reschedule_for_config(
            &mut self.monitor,
            &self.config,
            &self.offline_confirmation,
            self.errors.error_count_u32(),
            ctx,
        );

        let waiting_creds = matches!(
            &self.session,
            SessionSlot::Open(open) if matches!(open.attempt, AttemptPhase::WaitingCredentials)
        );
        if waiting_creds {
            self.start_attempt_for_open_session(ctx, None, false, effects);
        }
    }

    fn on_error_cleared(&mut self, ctx: StepContext) {
        self.errors.clear_all(self.config.offline_check_count);
        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }
        match self.monitor {
            MonitorPhase::Blocked { .. } | MonitorPhase::Idle { .. } => {
                self.monitor = schedule::schedule_idle_immediate(ctx.now);
            }
            _ => self.on_check_now(ctx),
        }
    }

    fn on_credential_material_changed(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }
        let waiting_creds = matches!(
            &self.session,
            SessionSlot::Open(open) if matches!(open.attempt, AttemptPhase::WaitingCredentials)
        );
        if waiting_creds {
            self.start_attempt_for_open_session(ctx, None, false, effects);
        } else {
            self.on_check_now(ctx);
        }
    }
}
