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

use crate::domain::StreamerState;

pub use self::error::ErrorTracker;
pub use self::hysteresis::{OfflineConfirmation, SlidingFailureWindow};
pub use self::segment::SessionSegmentGuard;
pub use self::state::{
    AttemptPhase, BlockedKind, DanmuSessionState, FilteredKind, FsmEffect, FsmInput, InFlightProbe,
    MonitorPhase, OpenSession, PendingDbKind, PendingDbOp, QuietPeriod, RecoveredMedia,
    SessionSlot, StepContext, StopKind, StoppingState, StreamerFsm,
};

use self::attempt::AttemptStartedInput;
use self::segment::{SegmentStartedInput, VideoSegmentInput};
use self::state::QueuedDbOp;
use crate::recorder::ports::{
    DbCommitOp, InitialStreamerSnapshot, ResolvedActorConfig, StreamerRuntimeView,
    should_spawn_actor,
};

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

        let monitor = if !should_spawn_actor(snapshot.state, snapshot.deleted) {
            MonitorPhase::Terminated
        } else {
            match snapshot.state {
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
                StreamerState::NotLive | StreamerState::Live | StreamerState::InspectingLive => {
                    schedule::schedule_idle_immediate(now)
                }
                _ => MonitorPhase::Terminated,
            }
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
            next_seq: 1,
        }
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
                self.lingering_physical_lease = false;
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

    /// Earliest monotonic instant at which the caller must invoke
    /// `step(ctx, FsmInput::TimerFired)`, or `None` when parked / terminated.
    pub fn next_deadline(&self, now: Instant, wall_now: Timestamp) -> Option<Instant> {
        let mut earliest: Option<Instant> = None;
        let mut consider = |candidate: Instant| {
            earliest = Some(match earliest {
                Some(current) => current.min(candidate),
                None => candidate,
            });
        };

        // Monitor phase timers.
        match &self.monitor {
            MonitorPhase::Stopping(stopping) => {
                if !stopping.deadline_expired {
                    consider(stopping.deadline);
                }
            }
            MonitorPhase::Terminated => {}
            MonitorPhase::Idle {
                next_check: Some(t),
                ..
            } => {
                if self.in_flight_probe.is_none() && self.pending_db.is_none() {
                    consider(*t);
                }
            }
            MonitorPhase::Idle {
                next_check: None, ..
            } => {}
            MonitorPhase::Filtered { next_check, .. }
            | MonitorPhase::RefusedBroadcast { next_check, .. } => {
                if self.in_flight_probe.is_none() && self.pending_db.is_none() {
                    consider(*next_check);
                }
            }
            MonitorPhase::Blocked { retry_at, .. } => {
                if self.in_flight_probe.is_none() && self.pending_db.is_none() {
                    consider(*retry_at);
                }
            }
            MonitorPhase::LiveWatchdog {
                last_activity_at,
                next_filter_hint,
                watchdog_backoff_until,
            } => {
                let diagnostic_busy = matches!(
                    &self.session,
                    SessionSlot::Open(open)
                        if matches!(open.attempt, AttemptPhase::DiagnosingCredentials { .. })
                );
                if self.in_flight_probe.is_none() && self.pending_db.is_none() && !diagnostic_busy {
                    consider(schedule::live_watchdog_deadline(
                        *last_activity_at,
                        *next_filter_hint,
                        *watchdog_backoff_until,
                        self.config.check_interval_ms,
                        now,
                        wall_now,
                    ));
                }
            }
        }

        // Session orthogonal timers (quiet period, diagnostic backoff, 60s eviction).
        match &self.session {
            SessionSlot::Open(open) => {
                if let Some(qp) = &open.quiet_period {
                    if qp.retrying_commit {
                        if let Some(retry_at) = qp.next_retry_at
                            && self.pending_db.is_none()
                        {
                            consider(retry_at);
                        }
                    } else if qp.grace_remaining_checks.is_none() {
                        consider(qp.backstop_deadline);
                    }
                }
                if let AttemptPhase::DiagnosingCredentials {
                    probe_id: None,
                    backoff_until: Some(until),
                    ..
                } = &open.attempt
                    && self.in_flight_probe.is_none()
                {
                    consider(*until);
                }
            }
            SessionSlot::RecentlyEnded {
                reattributable_timer_end: true,
                evict_at,
                ..
            } => {
                consider(*evict_at);
            }
            SessionSlot::None
            | SessionSlot::CommittingStart { .. }
            | SessionSlot::RecentlyEnded {
                reattributable_timer_end: false,
                ..
            } => {}
        }

        earliest
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
                let (active, queued) = match &open.attempt {
                    AttemptPhase::Starting { queued, .. } => (true, *queued),
                    AttemptPhase::Running { .. } | AttemptPhase::StoppingAttempt { .. } => {
                        (true, false)
                    }
                    AttemptPhase::Idle
                    | AttemptPhase::WaitingCredentials
                    | AttemptPhase::DiagnosingCredentials { .. } => (false, false),
                };
                let (collecting, link_down) = match open.danmu {
                    DanmuSessionState::Running { link_down_since } => (true, link_down_since),
                    DanmuSessionState::Starting | DanmuSessionState::Stopping => (true, None),
                    DanmuSessionState::Off => (false, None),
                };
                (
                    Some(open.session_id.clone()),
                    Some(open.started_at),
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
        // 1. Evict expired 60s `RecentlyEnded` window.
        if let SessionSlot::RecentlyEnded { evict_at, .. } = &self.session
            && ctx.now >= *evict_at
        {
            self.session = SessionSlot::None;
        }

        // 2. Stopping deadline check takes precedence over regular polling.
        if matches!(self.monitor, MonitorPhase::Stopping(_)) {
            self.on_stop_deadline_fired(ctx, effects);
            return;
        }

        if matches!(self.monitor, MonitorPhase::Terminated) {
            return;
        }

        // 3. Credential diagnostic backoff timer.
        self.on_credential_diagnostic_timer(ctx, effects);

        // 4. Orthogonal QuietPeriod backstop or 1s failed-commit retry timer.
        self.check_quiet_period_timer(ctx, effects);

        // 5. Monitor schedule timer -> spawn status probe.
        if self.in_flight_probe.is_some() || self.pending_db.is_some() {
            return;
        }
        let (due, is_live_watchdog) = match &self.monitor {
            MonitorPhase::Idle {
                next_check: Some(t),
                ..
            } => (ctx.now >= *t, false),
            MonitorPhase::Idle {
                next_check: None, ..
            } => (false, false),
            MonitorPhase::Filtered { next_check, .. }
            | MonitorPhase::RefusedBroadcast { next_check, .. } => (ctx.now >= *next_check, false),
            MonitorPhase::Blocked { retry_at, .. } => (ctx.now >= *retry_at, false),
            MonitorPhase::LiveWatchdog {
                last_activity_at,
                next_filter_hint,
                watchdog_backoff_until,
            } => {
                let deadline = schedule::live_watchdog_deadline(
                    *last_activity_at,
                    *next_filter_hint,
                    *watchdog_backoff_until,
                    self.config.check_interval_ms,
                    ctx.now,
                    ctx.wall_now,
                );
                (ctx.now >= deadline, true)
            }
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated => (false, false),
        };

        if !due {
            return;
        }

        // Respect persisted `disabled_until` backoff before firing network probes.
        if !is_live_watchdog && self.errors.is_disabled(ctx.wall_now) {
            if let Some(remaining) = self.errors.remaining_backoff(ctx.wall_now) {
                let kind = match &self.monitor {
                    MonitorPhase::Blocked { kind, .. } => *kind,
                    _ => BlockedKind::TemporalDisabled,
                };
                self.monitor = MonitorPhase::Blocked {
                    kind,
                    retry_at: ctx.now.checked_add(remaining).unwrap_or(ctx.now),
                };
            }
            return;
        }

        self.spawn_status_probe(ctx, is_live_watchdog, effects);
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

    fn on_retry_admission(&mut self, ctx: StepContext) {
        if matches!(
            self.monitor,
            MonitorPhase::Stopping(_) | MonitorPhase::Terminated
        ) {
            return;
        }
        let has_active_attempt = matches!(
            &self.session,
            SessionSlot::Open(open)
                if matches!(
                    open.attempt,
                    AttemptPhase::Starting { .. }
                        | AttemptPhase::Running { .. }
                        | AttemptPhase::StoppingAttempt { .. }
                )
        );
        if !has_active_attempt {
            self.monitor = schedule::schedule_idle_immediate(ctx.now);
        }
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
