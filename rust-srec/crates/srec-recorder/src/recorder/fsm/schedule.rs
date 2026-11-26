//! Pure scheduling math for polling intervals, ±10% jitter, Live Watchdog
//! cadence, 5-minute stall detection, smart-wake buffers, and batch-hint deadlines.

use std::time::{Duration, Instant};

use jiff::Timestamp;

use super::hysteresis::OfflineConfirmation;
use super::state::{FilteredKind, MonitorPhase, StepContext};
use crate::recorder::ports::ResolvedActorConfig;

/// Download inactivity threshold after which the Live Watchdog forces a
/// re-check and re-emits live startup if the stream is still live.
pub const STALL_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Floor for the periodic Live Watchdog interval when a download is healthy.
pub const WATCHDOG_MINIMUM: Duration = Duration::from_secs(2 * 60 * 60);

/// Minimum re-check backoff after a failed Live Watchdog probe or credential
/// unavailability while live, preventing busy-retry against an expired stall window.
pub const WATCHDOG_ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// Safety buffer added after a time-based filter's `next_available` hint so
/// the actor wakes squarely inside the newly opened schedule window.
pub const SMART_WAKE_BUFFER: Duration = Duration::from_secs(5);

/// Default deadline for `Disable`, `OutOfSchedule`, and `CredentialRetire`
/// physical stop confirmation.
pub const STOP_DEADLINE_DURATION: Duration = Duration::from_secs(20);

/// Compute the effective recurring polling interval in milliseconds.
///
/// Uses the shorter `offline_check_interval_ms` while `was_live` is active
/// (unless in an error state that has already reached `offline_check_count`).
pub fn effective_poll_interval_ms(
    config: &ResolvedActorConfig,
    offline_confirmation: &OfflineConfirmation,
    in_error_state: bool,
    error_count: u32,
) -> u64 {
    let use_short = offline_confirmation.was_live
        && (!in_error_state || error_count < config.offline_check_count);
    if use_short {
        config.offline_check_interval_ms
    } else {
        config.check_interval_ms
    }
}

/// Apply bounded ±10% jitter (`basis_points` clamped to `9000..=11000`) using
/// integer nanosecond arithmetic so sub-10ms intervals keep exact bounds.
pub fn jittered_delay(interval_ms: u64, basis_points: u16) -> Duration {
    let nanos = u128::from(interval_ms) * u128::from(basis_points.clamp(9000, 11000)) * 100;
    Duration::new(
        (nanos / 1_000_000_000) as u64,
        (nanos % 1_000_000_000) as u32,
    )
}

/// Compute the monotonic deadline for an `OutOfSchedule` smart-wake hint, if
/// the hint lies in the future relative to `wall_now`.
pub fn smart_wake_deadline(
    now: Instant,
    wall_now: Timestamp,
    hint: Option<Timestamp>,
) -> Option<Instant> {
    let hint = hint?;
    if hint <= wall_now {
        return None;
    }
    let delay = Duration::try_from(hint.duration_since(wall_now)).unwrap_or(Duration::ZERO);
    now.checked_add(delay.saturating_add(SMART_WAKE_BUFFER))
}

/// Compute the next monotonic wake deadline while in [`MonitorPhase::LiveWatchdog`].
///
/// Precedence:
/// 1. `min(max(WATCHDOG_MINIMUM, check_interval_ms), remaining_stall_window)`
/// 2. Clamped down by `next_filter_hint` when a schedule-end boundary is sooner.
/// 3. Clamped up by `watchdog_backoff_until` so a failed watchdog probe never
///    spins against an already-expired stall window or boundary hint.
pub fn live_watchdog_deadline(
    last_activity_at: Instant,
    next_filter_hint: Option<Timestamp>,
    watchdog_backoff_until: Option<Instant>,
    check_interval_ms: u64,
    now: Instant,
    wall_now: Timestamp,
) -> Instant {
    let watchdog = WATCHDOG_MINIMUM.max(Duration::from_millis(check_interval_ms));
    let stall = STALL_WINDOW.saturating_sub(now.saturating_duration_since(last_activity_at));
    let mut delay = watchdog.min(stall);

    if let Some(hint) = next_filter_hint {
        let until_hint =
            Duration::try_from(hint.duration_since(wall_now)).unwrap_or(Duration::ZERO);
        delay = delay.min(until_hint);
    }

    if let Some(floor) = watchdog_backoff_until {
        delay = delay.max(floor.saturating_duration_since(now));
    }

    now.checked_add(delay).unwrap_or(now)
}

/// Whether download activity has been silent long enough (`>= 5 min`) that a
/// Live Watchdog probe confirming `Live` must force re-emission / recovery.
pub fn is_live_stalled(last_activity_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_activity_at) >= STALL_WINDOW
}

/// Build a recurring [`MonitorPhase::Idle`] state scheduled at the effective
/// jittered interval, preserving an earlier `batch_routine_deadline` when one
/// was saved and is still ahead of `now`.
pub fn schedule_idle(
    config: &ResolvedActorConfig,
    offline_confirmation: &OfflineConfirmation,
    in_error_state: bool,
    error_count: u32,
    saved_routine_deadline: Option<Instant>,
    ctx: StepContext,
) -> MonitorPhase {
    let interval_ms =
        effective_poll_interval_ms(config, offline_confirmation, in_error_state, error_count);
    let mut next_check = ctx
        .now
        .checked_add(jittered_delay(interval_ms, ctx.jitter_bps));
    if let Some(routine) = saved_routine_deadline
        && routine > ctx.now
        && next_check.is_some_and(|next| next > routine)
    {
        next_check = Some(routine);
    }
    MonitorPhase::Idle {
        next_check,
        recurring_interval_ms: Some(interval_ms),
        batch_routine_deadline: None,
    }
}

/// Build an immediate-check [`MonitorPhase::Idle`] state (non-recurring, so
/// config updates will not override it).
pub fn schedule_idle_immediate(now: Instant) -> MonitorPhase {
    MonitorPhase::Idle {
        next_check: Some(now),
        recurring_interval_ms: None,
        batch_routine_deadline: None,
    }
}

/// Build a [`MonitorPhase::Filtered`] state using smart-wake when `OutOfSchedule`
/// supplies a future `next_available` timestamp, otherwise a recurring jittered poll.
pub fn schedule_filtered(
    reason: FilteredKind,
    config: &ResolvedActorConfig,
    offline_confirmation: &OfflineConfirmation,
    error_count: u32,
    ctx: StepContext,
) -> MonitorPhase {
    if let FilteredKind::OutOfSchedule { smart_wake } = &reason
        && let Some(deadline) = smart_wake_deadline(ctx.now, ctx.wall_now, *smart_wake)
    {
        return MonitorPhase::Filtered {
            reason,
            next_check: deadline,
            recurring_interval_ms: None,
        };
    }

    let interval_ms = effective_poll_interval_ms(config, offline_confirmation, false, error_count);
    let next_check = ctx
        .now
        .checked_add(jittered_delay(interval_ms, ctx.jitter_bps))
        .unwrap_or(ctx.now);
    MonitorPhase::Filtered {
        reason,
        next_check,
        recurring_interval_ms: Some(interval_ms),
    }
}

/// Build a [`MonitorPhase::RefusedBroadcast`] state scheduled on normal cadence.
pub fn schedule_refused_broadcast(
    config: &ResolvedActorConfig,
    offline_confirmation: &OfflineConfirmation,
    error_count: u32,
    ctx: StepContext,
) -> MonitorPhase {
    let interval_ms = effective_poll_interval_ms(config, offline_confirmation, false, error_count);
    let next_check = ctx
        .now
        .checked_add(jittered_delay(interval_ms, ctx.jitter_bps))
        .unwrap_or(ctx.now);
    MonitorPhase::RefusedBroadcast {
        next_check,
        recurring_interval_ms: Some(interval_ms),
    }
}

/// Apply a configuration update to an existing [`MonitorPhase`].
///
/// Only recurring interval polls are updated; smart-wake hints, admission/error
/// cooldowns, explicit immediate deadlines, and `LiveWatchdog` retain authority.
/// A faster cadence brings `next_check` forward; a slower cadence takes effect
/// only after the already-scheduled poll fires.
pub fn reschedule_for_config(
    monitor: &mut MonitorPhase,
    config: &ResolvedActorConfig,
    offline_confirmation: &OfflineConfirmation,
    error_count: u32,
    ctx: StepContext,
) {
    let (next_check_opt, recurring) = match monitor {
        MonitorPhase::Idle {
            next_check,
            recurring_interval_ms,
            ..
        } => (next_check, recurring_interval_ms),
        MonitorPhase::Filtered {
            next_check,
            recurring_interval_ms,
            ..
        }
        | MonitorPhase::RefusedBroadcast {
            next_check,
            recurring_interval_ms,
        } => {
            let Some(previous_interval) = *recurring_interval_ms else {
                return;
            };
            let interval_ms =
                effective_poll_interval_ms(config, offline_confirmation, false, error_count);
            if previous_interval == interval_ms || *next_check <= ctx.now {
                return;
            }
            if let Some(candidate) = ctx
                .now
                .checked_add(jittered_delay(interval_ms, ctx.jitter_bps))
            {
                *next_check = (*next_check).min(candidate);
            }
            *recurring_interval_ms = Some(interval_ms);
            return;
        }
        MonitorPhase::Blocked { .. }
        | MonitorPhase::LiveWatchdog { .. }
        | MonitorPhase::Stopping(_)
        | MonitorPhase::Terminated => return,
    };

    let Some(previous_interval) = *recurring else {
        return;
    };
    let interval_ms = effective_poll_interval_ms(config, offline_confirmation, false, error_count);
    if previous_interval == interval_ms
        || next_check_opt.is_some_and(|deadline| deadline <= ctx.now)
    {
        return;
    }
    let candidate = ctx
        .now
        .checked_add(jittered_delay(interval_ms, ctx.jitter_bps));
    *next_check_opt = match (*next_check_opt, candidate) {
        (Some(existing), Some(candidate)) => Some(existing.min(candidate)),
        (existing, candidate) => existing.or(candidate),
    };
    *recurring = Some(interval_ms);
}
