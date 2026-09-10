//! Live wake precedence, evaluated with explicit wall and monotonic clocks.
use super::*;

const STALL_WINDOW: Duration = Duration::from_secs(5 * 60);
const WATCHDOG_MINIMUM: Duration = Duration::from_secs(2 * 60 * 60);
const WATCHDOG_ERROR_BACKOFF: Duration = Duration::from_secs(60);

fn wake_delay(
    state: &StreamerActorState,
    interval_ms: u64,
    backoff: Option<Instant>,
    now: Instant,
    wall_now: chrono::DateTime<chrono::Utc>,
) -> Option<Duration> {
    // Explicit checks retain their authority, including retry and immediate
    // deadlines. Only a parked Live actor uses the watchdog decision.
    if let Some(deadline) = state.next_check {
        return Some(deadline.saturating_duration_since(now));
    }
    if state.streamer_state != StreamerState::Live {
        return None;
    }
    let watchdog = WATCHDOG_MINIMUM.max(Duration::from_millis(interval_ms));
    let stall = state
        .last_download_activity_at
        .map_or(STALL_WINDOW, |last| {
            STALL_WINDOW.saturating_sub(now.saturating_duration_since(last))
        });
    let mut delay = watchdog.min(stall);
    if let Some(hint) = state
        .last_check
        .as_ref()
        .and_then(|last| last.next_check_hint)
    {
        let until_hint = hint
            .signed_duration_since(wall_now)
            .to_std()
            .unwrap_or(Duration::ZERO);
        delay = delay.min(until_hint);
    }
    // A failed watchdog must not spin on an already-expired stall or hint.
    if let Some(floor) = backoff {
        delay = delay.max(floor.saturating_duration_since(now));
    }
    Some(delay)
}

fn should_reemit_live(state: &StreamerActorState, next_state: StreamerState, now: Instant) -> bool {
    state.streamer_state == StreamerState::Live
        && state.next_check.is_none()
        && next_state == StreamerState::Live
        && state
            .last_download_activity_at
            .is_none_or(|last| now.saturating_duration_since(last) >= STALL_WINDOW)
}

impl StreamerActor {
    pub(super) fn wake_delay(&self) -> Option<Duration> {
        wake_delay(
            &self.state,
            self.config.check_interval_ms,
            self.live_watchdog_backoff_until,
            Instant::now(),
            chrono::Utc::now(),
        )
    }

    pub(super) fn live_watchdog_error_backoff(&self) -> Duration {
        WATCHDOG_ERROR_BACKOFF
    }

    pub(super) fn is_live_watchdog(&self) -> bool {
        self.state.streamer_state == StreamerState::Live && self.state.next_check.is_none()
    }

    /// Run before refreshing the activity timestamp from a Live result, so a
    /// stalled download is re-emitted through the normal deduplicated startup.
    pub(super) fn force_live_reemit_if_stalled(
        &mut self,
        next_state: StreamerState,
        context: &'static str,
    ) {
        if should_reemit_live(&self.state, next_state, Instant::now()) {
            info!(streamer_id = %self.id, context, "Live watchdog forcing a stalled download recheck");
            self.state.streamer_state = StreamerState::NotLive;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_deadlines_and_parked_states_keep_precedence_over_watchdogs() {
        let now = Instant::now();
        let wall = chrono::Utc::now();
        for state in [
            StreamerState::Live,
            StreamerState::NotLive,
            StreamerState::OutOfSchedule,
        ] {
            let mut runtime = StreamerActorState {
                streamer_state: state,
                ..Default::default()
            };
            for offset in [0, 1, 9000] {
                runtime.next_check = Some(now + Duration::from_secs(offset));
                assert_eq!(
                    wake_delay(
                        &runtime,
                        1000,
                        Some(now + Duration::from_secs(9999)),
                        now,
                        wall
                    ),
                    Some(Duration::from_secs(offset))
                );
            }
            runtime.next_check = None;
            assert_eq!(
                wake_delay(&runtime, 1000, None, now, wall),
                (state == StreamerState::Live).then_some(STALL_WINDOW)
            );
        }
    }

    #[test]
    fn live_stall_hint_and_error_floor_edges_are_clock_controlled() {
        let now = Instant::now();
        let wall = chrono::Utc::now();
        for age in [0, 299, 300, 301] {
            for hint in [-1, 0, 1, 500] {
                for backoff in [None, Some(0), Some(60)] {
                    let mut check = CheckResult::success(StreamerState::Live);
                    check.next_check_hint = Some(wall + chrono::Duration::seconds(hint));
                    let runtime = StreamerActorState {
                        streamer_state: StreamerState::Live,
                        last_download_activity_at: Some(now - Duration::from_secs(age)),
                        last_check: Some(check),
                        ..Default::default()
                    };
                    let expected = (300_u64.saturating_sub(age))
                        .min(hint.max(0) as u64)
                        .max(backoff.unwrap_or(0));
                    assert_eq!(
                        wake_delay(
                            &runtime,
                            1000,
                            backoff.map(|seconds| now + Duration::from_secs(seconds)),
                            now,
                            wall
                        ),
                        Some(Duration::from_secs(expected))
                    );
                    assert_eq!(
                        should_reemit_live(&runtime, StreamerState::Live, now),
                        age >= 300
                    );
                    assert!(!should_reemit_live(&runtime, StreamerState::NotLive, now));
                }
            }
        }
        let runtime = StreamerActorState {
            streamer_state: StreamerState::Live,
            ..Default::default()
        };
        assert!(should_reemit_live(&runtime, StreamerState::Live, now));
        assert_eq!(
            wake_delay(
                &runtime,
                u64::MAX,
                Some(now - Duration::from_secs(1)),
                now,
                wall
            ),
            Some(STALL_WINDOW)
        );
    }
}
