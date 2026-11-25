//! Consecutive check/download error tracking, exponential backoff, and
//! `ChecksFailing` / `ChecksRestored` threshold transitions.

use std::time::Duration;

use jiff::Timestamp;

use crate::streamer::download_failure_threshold;

/// Base backoff duration once the error threshold is reached (doubles per step).
pub const BASE_BACKOFF_SECS: u64 = 60;

/// Maximum backoff duration (1 hour).
pub const MAX_BACKOFF_SECS: u64 = 3600;

/// Compute the exponential backoff duration for `error_count`, or `None` while
/// below the derived threshold `max(offline_check_count, 2)`.
pub fn backoff_duration_for_error_count(
    error_count: i32,
    offline_check_count: u32,
) -> Option<Duration> {
    let threshold = download_failure_threshold(offline_check_count);
    if error_count < threshold {
        return None;
    }

    let cap_exponent = (MAX_BACKOFF_SECS / BASE_BACKOFF_SECS).ilog2() + 1;
    let exponent = (error_count.saturating_sub(threshold).max(0) as u32).min(cap_exponent);
    let secs = (BASE_BACKOFF_SECS.saturating_mul(2u64.pow(exponent))).min(MAX_BACKOFF_SECS);
    Some(Duration::from_secs(secs))
}

/// Compute the wall-clock `disabled_until` deadline for `error_count`, if
/// exponential backoff applies.
pub fn disabled_until_for_error_count(
    error_count: i32,
    offline_check_count: u32,
    wall_now: Timestamp,
) -> Option<Timestamp> {
    let duration = backoff_duration_for_error_count(error_count, offline_check_count)?;
    let signed = jiff::SignedDuration::from_secs(duration.as_secs() as i64);
    Some(wall_now + signed)
}

/// Returns `true` on the exact failure count that crosses the alert threshold,
/// ensuring `ChecksFailing` is emitted once per run of failures.
pub fn should_emit_checks_failing(new_error_count: i32, offline_check_count: u32) -> bool {
    new_error_count == download_failure_threshold(offline_check_count)
}

/// Returns `true` when clearing a failure run that previously crossed the
/// `ChecksFailing` threshold.
pub fn should_emit_checks_restored(previous_error_count: i32, offline_check_count: u32) -> bool {
    previous_error_count >= download_failure_threshold(offline_check_count)
}

/// Outcome of recording one local transient error transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorTransition {
    pub consecutive_error_count: i32,
    pub disabled_until: Option<Timestamp>,
    pub checks_failing_crossed: bool,
}

/// Per-streamer transient error and backoff tracker synchronized with committed DB state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorTracker {
    pub consecutive_error_count: i32,
    pub disabled_until: Option<Timestamp>,
    pub last_error: Option<String>,
}

impl ErrorTracker {
    /// Build an error tracker from persisted streamer columns.
    pub fn from_persisted(
        consecutive_error_count: i32,
        disabled_until: Option<Timestamp>,
        last_error: Option<String>,
    ) -> Self {
        Self {
            consecutive_error_count,
            disabled_until,
            last_error,
        }
    }

    /// Whether the streamer is currently inside an active error/infra backoff window.
    pub fn is_disabled(&self, wall_now: Timestamp) -> bool {
        self.disabled_until.is_some_and(|until| wall_now < until)
    }

    /// Remaining duration of `disabled_until`, or `None` when not in backoff.
    pub fn remaining_backoff(&self, wall_now: Timestamp) -> Option<Duration> {
        let until = self.disabled_until?;
        if until <= wall_now {
            return None;
        }
        Duration::try_from(until.duration_since(wall_now)).ok()
    }

    /// Whether error count or backoff timestamp is set (used by offline-check recovery).
    pub fn has_errors(&self) -> bool {
        self.consecutive_error_count > 0 || self.disabled_until.is_some()
    }

    /// Whether any transient error field (including `last_error`) is set.
    pub fn has_transient_error_state(&self) -> bool {
        self.has_errors() || self.last_error.is_some()
    }

    /// Whether `last_error` is populated.
    pub fn has_last_error(&self) -> bool {
        self.last_error.is_some()
    }

    /// Current non-negative error count for scheduling cadence checks.
    pub fn error_count_u32(&self) -> u32 {
        u32::try_from(self.consecutive_error_count).unwrap_or(0)
    }

    /// Preview/apply one transient error increment against the derived threshold.
    pub fn record_local_error(
        &mut self,
        error: String,
        offline_check_count: u32,
        wall_now: Timestamp,
    ) -> ErrorTransition {
        let next_count = self.consecutive_error_count.saturating_add(1);
        let disabled_until =
            disabled_until_for_error_count(next_count, offline_check_count, wall_now);
        let checks_failing_crossed = should_emit_checks_failing(next_count, offline_check_count);
        self.consecutive_error_count = next_count;
        self.disabled_until = disabled_until;
        self.last_error = Some(error);
        ErrorTransition {
            consecutive_error_count: next_count,
            disabled_until,
            checks_failing_crossed,
        }
    }

    /// Reconcile local tracker fields with an authoritative committed DB result.
    pub fn apply_committed_error(
        &mut self,
        consecutive_error_count: i32,
        disabled_until: Option<Timestamp>,
        last_error: Option<String>,
    ) {
        self.consecutive_error_count = consecutive_error_count;
        self.disabled_until = disabled_until;
        if let Some(error) = last_error {
            self.last_error = Some(error);
        }
    }

    /// Apply an infrastructure backoff (`disabled_until`) without incrementing error count.
    pub fn apply_infra_backoff(&mut self, disabled_until: Timestamp, last_error: Option<String>) {
        self.disabled_until = Some(disabled_until);
        if let Some(error) = last_error {
            self.last_error = Some(error);
        }
    }

    /// Clear error count, backoff deadline, and `last_error`.
    /// Returns `true` if the cleared failure run had reached the `ChecksFailing` threshold.
    pub fn clear_all(&mut self, offline_check_count: u32) -> bool {
        let restored =
            should_emit_checks_restored(self.consecutive_error_count, offline_check_count);
        self.consecutive_error_count = 0;
        self.disabled_until = None;
        self.last_error = None;
        restored
    }

    /// Clear only `last_error`, preserving count and backoff.
    pub fn clear_last_error(&mut self) {
        self.last_error = None;
    }
}
