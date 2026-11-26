//! Orthogonal offline confirmation, non-extending quiet-period backstop,
//! 60-second retroactive disable reattribution, and deterministic Mesio
//! network-failure classification.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use jiff::Timestamp;

use crate::downloader::DownloadFailureKind;
use crate::session::{EngineKind, OfflineSignal, TerminalCause};
use crate::streamer::download_failure_threshold;

use super::state::{FsmEffect, PendingDbKind, QuietPeriod, SessionSlot, StepContext, StreamerFsm};
use crate::recorder::ports::{DanmuStopKind, DbCommitOp, ResolvedActorConfig};

/// Hard upper bound on the derived quiet-period backstop window (5 minutes).
pub const MAX_QUIET_PERIOD_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Retry cadence when committing a quiet-period expiry to SQLite fails transiently.
pub const QUIET_PERIOD_COMMIT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// Retention window for `SessionSlot::RecentlyEnded` (60 seconds).
pub const RECENTLY_ENDED_RETENTION: Duration = Duration::from_secs(60);

/// Result of recording one monitor `Offline` poll against [`OfflineConfirmation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineCheckDecision {
    /// Streamer was not previously live; no grace-period confirmation needed.
    NotPreviouslyLive,
    /// Still accumulating consecutive offline polls inside the grace period.
    GracePeriod { count: u32, remaining: u32 },
    /// Reached or exceeded the configured offline-check threshold.
    ConfirmedOffline { count: u32 },
}

/// Orthogonal monitor-side offline confirmation counter.
///
/// Tracks consecutive offline polls independently of whether an attempt is
/// running or parked in [`QuietPeriod`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfflineConfirmation {
    pub was_live: bool,
    pub consecutive_offline: u32,
}

impl OfflineConfirmation {
    /// Create a fresh confirmation tracker, marking `was_live` when initialized
    /// from a `Live` state.
    pub fn from_live(is_live: bool) -> Self {
        Self {
            was_live: is_live,
            consecutive_offline: 0,
        }
    }

    /// Record that the streamer is confirmed live, clearing any offline count.
    pub fn mark_live(&mut self) {
        self.was_live = true;
        self.consecutive_offline = 0;
    }

    /// Ensure `was_live` is set without resetting an existing offline count.
    pub fn ensure_live(&mut self) {
        if !self.was_live {
            self.mark_live();
        }
    }

    /// Forget prior live status and reset the offline count.
    pub fn reset(&mut self) {
        self.was_live = false;
        self.consecutive_offline = 0;
    }

    /// Record an offline hint contributed by a download terminal (e.g. clean
    /// completion or explicit offline stop) to enter short post-live polling.
    pub fn mark_offline_observed(&mut self) {
        if !self.was_live {
            return;
        }
        self.consecutive_offline = self.consecutive_offline.saturating_add(1);
    }

    /// Advance the orthogonal counter on a monitor `Offline` poll without
    /// resetting `was_live` until the caller's DB commit actually succeeds.
    pub fn observe_monitor_offline(&mut self, threshold: u32) -> OfflineCheckDecision {
        let threshold = threshold.max(1);
        if !self.was_live {
            return OfflineCheckDecision::NotPreviouslyLive;
        }
        self.consecutive_offline = self.consecutive_offline.saturating_add(1);
        if self.consecutive_offline >= threshold {
            OfflineCheckDecision::ConfirmedOffline {
                count: self.consecutive_offline,
            }
        } else {
            OfflineCheckDecision::GracePeriod {
                count: self.consecutive_offline,
                remaining: threshold.saturating_sub(self.consecutive_offline),
            }
        }
    }

    /// Whether an offline confirmation sequence has started (`> 0`) and not yet
    /// reached `threshold`.
    pub fn has_active_sequence(&self, threshold: u32) -> bool {
        let threshold = threshold.max(1);
        self.was_live && self.consecutive_offline > 0 && self.consecutive_offline < threshold
    }

    /// Remaining offline polls needed to confirm offline in an active sequence.
    pub fn remaining_checks(&self, threshold: u32) -> u32 {
        let threshold = threshold.max(1);
        if self.has_active_sequence(threshold) {
            threshold.saturating_sub(self.consecutive_offline)
        } else {
            0
        }
    }
}

/// Derive the quiet-period backstop window `(count + 1) * interval`, capped at
/// [`MAX_QUIET_PERIOD_WINDOW`] (5 minutes).
pub fn quiet_period_window(offline_check_count: u32, offline_check_interval_ms: u64) -> Duration {
    let count = u64::from(offline_check_count.max(1));
    let interval = offline_check_interval_ms.max(1_000);
    let raw = Duration::from_millis(count.saturating_add(1).saturating_mul(interval));
    raw.min(MAX_QUIET_PERIOD_WINDOW)
}

/// Arm a new non-extending [`QuietPeriod`].
pub fn arm_quiet_period(
    timer_id: u64,
    cause: TerminalCause,
    now: Instant,
    wall_now: Timestamp,
    config: &ResolvedActorConfig,
) -> (QuietPeriod, Timestamp) {
    let window = quiet_period_window(config.offline_check_count, config.offline_check_interval_ms);
    let backstop_deadline = now.checked_add(window).unwrap_or(now);
    let resume_deadline = wall_now
        + jiff::SignedDuration::try_from(window).unwrap_or(jiff::SignedDuration::from_secs(90));
    let quiet_period = QuietPeriod {
        timer_id,
        cause,
        entered_at: wall_now,
        backstop_deadline,
        grace_remaining_checks: None,
        retrying_commit: false,
        next_retry_at: None,
    };
    (quiet_period, resume_deadline)
}

/// Action decided when the quiet-period backstop deadline elapses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackstopExpiryAction {
    /// Freeze remaining checks (`>= 1`) while an active confirmation sequence
    /// or in-flight probe completes within its existing bound.
    FreezeBoundedGrace { remaining_checks: u32 },
    /// No confirmation sequence or probe is active; commit the original ambiguous cause now.
    CommitNow,
}

/// Decide whether an elapsed `backstop_deadline` should immediately commit the
/// quiet-period cause or freeze a bounded number of checks for in-flight probing.
///
/// When the monitor is parked in `Blocked` (e.g. `TemporalDisabled` backoff up to
/// 1 hour) and no probe is currently in flight, do not freeze the backstop timer
/// indefinitely — commit the quiet-period end now.
pub fn evaluate_backstop_expiry(
    offline_confirmation: &OfflineConfirmation,
    has_in_flight_probe: bool,
    monitor_blocked: bool,
    offline_check_count: u32,
) -> BackstopExpiryAction {
    if !monitor_blocked && offline_confirmation.has_active_sequence(offline_check_count) {
        let remaining = offline_confirmation
            .remaining_checks(offline_check_count)
            .max(1);
        BackstopExpiryAction::FreezeBoundedGrace {
            remaining_checks: remaining,
        }
    } else if has_in_flight_probe {
        BackstopExpiryAction::FreezeBoundedGrace {
            remaining_checks: 1,
        }
    } else {
        BackstopExpiryAction::CommitNow
    }
}

/// Whether an ended session is eligible for retroactive `UserDisabled`
/// reattribution within the 60-second `RecentlyEnded` window.
///
/// Only ambiguous timer-ended sessions (`via_hysteresis == true` with
/// `Completed` or `Failed`) qualify; authoritative causes are always preserved.
pub fn is_reattributable_timer_end(cause: &TerminalCause, via_hysteresis: bool) -> bool {
    via_hysteresis
        && matches!(
            cause,
            TerminalCause::Completed | TerminalCause::Failed { .. }
        )
}

/// Pure deterministic sliding window classifying consecutive Mesio `Network`
/// failures into [`OfflineSignal::ConsecutiveFailures`].
///
/// Matches `session::classifier::OfflineClassifier` rules without global locks
/// or wall-clock calls:
/// - Only Mesio engines (`MesioHls`, `MesioFlv`, `MesioDash`, `MesioUnknown`)
///   and [`DownloadFailureKind::Network`] accumulate.
/// - HTTP 404, ffmpeg/streamlink subprocess exits, and infrastructure errors
///   never classify as definitive offline.
/// - Window = `max(offline_check_count, 1) * max(offline_check_interval_ms, 1000)`.
/// - Threshold = `max(offline_check_count, 2)` (preserving mid-stream RST safety).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlidingFailureWindow {
    failures: VecDeque<Instant>,
}

impl SlidingFailureWindow {
    /// Record a terminal failure and return `Some(OfflineSignal::ConsecutiveFailures)`
    /// if the sliding-window threshold is reached.
    pub fn classify_failure(
        &mut self,
        now: Instant,
        engine_kind: EngineKind,
        failure: DownloadFailureKind,
        config: &ResolvedActorConfig,
    ) -> Option<OfflineSignal> {
        if !engine_kind.is_mesio() || failure != DownloadFailureKind::Network {
            return None;
        }

        let count = config.offline_check_count.max(1);
        let interval = config.offline_check_interval_ms.max(1_000);
        let window = Duration::from_millis(u64::from(count).saturating_mul(interval));
        let threshold = usize::try_from(download_failure_threshold(count)).unwrap_or(usize::MAX);

        while let Some(&front) = self.failures.front() {
            if now.saturating_duration_since(front) >= window {
                self.failures.pop_front();
            } else {
                break;
            }
        }

        self.failures.push_back(now);
        if self.failures.len() >= threshold {
            self.failures.clear();
            Some(OfflineSignal::ConsecutiveFailures(
                u32::try_from(threshold).unwrap_or(u32::MAX),
            ))
        } else {
            None
        }
    }

    /// Clear accumulated network failures after a non-discarded segment completes.
    pub fn note_successful_segment(&mut self) {
        self.failures.clear();
    }

    /// Purge all failure history (e.g. on disable, retirement, or session end).
    pub fn clear(&mut self) {
        self.failures.clear();
    }

    /// Number of currently tracked failures inside the window.
    pub fn len(&self) -> usize {
        self.failures.len()
    }

    /// Whether no failures are currently tracked.
    pub fn is_empty(&self) -> bool {
        self.failures.is_empty()
    }
}

impl StreamerFsm {
    pub(super) fn enter_quiet_period(
        &mut self,
        ctx: StepContext,
        session_id: &str,
        cause: TerminalCause,
        effects: &mut Vec<FsmEffect>,
    ) {
        let already_armed = matches!(
            &self.session,
            SessionSlot::Open(open) if open.session_id == session_id && open.quiet_period.is_some()
        );
        if already_armed {
            // Idempotent: repeated ambiguous terminals must never extend backstop_deadline.
            return;
        }

        let timer_id = self.next_id();
        let (qp, resume_deadline) =
            arm_quiet_period(timer_id, cause.clone(), ctx.now, ctx.wall_now, &self.config);
        let open_session_id = if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            open.quiet_period = Some(qp);
            Some(open.session_id.clone())
        } else {
            None
        };
        if let Some(sid) = open_session_id {
            self.enqueue_or_dispatch_db(
                PendingDbKind::RecordHysteresisEntered,
                DbCommitOp::RecordHysteresisEntered {
                    streamer_id: self.streamer_id.clone(),
                    session_id: sid,
                    cause: (&cause).into(),
                    resume_deadline,
                    occurred_at: ctx.wall_now,
                },
                effects,
            );
        }
    }

    /// Handle a due quiet-period timer: retry the failed `EndSession`, or act
    /// on the elapsed backstop.
    pub(super) fn on_quiet_period_timer(&mut self, ctx: StepContext, effects: &mut Vec<FsmEffect>) {
        let SessionSlot::Open(open) = &mut self.session else {
            return;
        };
        let Some(qp) = &mut open.quiet_period else {
            return;
        };
        let session_id = open.session_id.clone();
        let (timer_id, cause) = (qp.timer_id, qp.cause.clone());

        if qp.retrying_commit {
            qp.next_retry_at = None;
            self.dispatch_quiet_period_end(ctx, session_id, timer_id, cause, effects);
            return;
        }

        let monitor_blocked = matches!(self.monitor, super::state::MonitorPhase::Blocked { .. });
        match evaluate_backstop_expiry(
            &self.offline_confirmation,
            self.in_flight_probe.is_some(),
            monitor_blocked,
            self.config.offline_check_count,
        ) {
            BackstopExpiryAction::FreezeBoundedGrace { remaining_checks } => {
                if let SessionSlot::Open(open) = &mut self.session
                    && let Some(qp) = &mut open.quiet_period
                {
                    qp.grace_remaining_checks = Some(remaining_checks);
                }
            }
            BackstopExpiryAction::CommitNow => {
                self.dispatch_quiet_period_end(ctx, session_id, timer_id, cause, effects);
            }
        }
    }

    pub(super) fn dispatch_quiet_period_end(
        &mut self,
        ctx: StepContext,
        session_id: String,
        timer_id: u64,
        cause: TerminalCause,
        effects: &mut Vec<FsmEffect>,
    ) {
        if let SessionSlot::Open(open) = &mut self.session
            && open.session_id == session_id
        {
            Self::stop_open_danmu(open, DanmuStopKind::SessionEnded, effects);
        }

        self.enqueue_or_dispatch_db(
            PendingDbKind::EndSession {
                session_id: Some(session_id.clone()),
                cause: cause.clone(),
                via_hysteresis: true,
                from_quiet_period_timer_id: Some(timer_id),
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
                via_hysteresis: true,
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
