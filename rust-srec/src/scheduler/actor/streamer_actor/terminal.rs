//! Download-end decisions are independent of actor mailbox and persistence.
use super::*;
use crate::monitor::InfraBlockReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HysteresisAction {
    Preserve,
    Reset,
    EnsureLive,
    ObserveOffline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckSchedule {
    Normal,
    Immediate,
    Park,
    Retry(u64),
}

#[derive(Debug)]
enum TerminalEffect {
    None,
    Offline,
    Infrastructure(InfraBlockReason),
}

#[derive(Debug)]
struct TerminalDecision {
    state: StreamerState,
    hysteresis: HysteresisAction,
    schedule: CheckSchedule,
    effect: TerminalEffect,
}

fn decide_terminal(reason: DownloadEndPolicy) -> TerminalDecision {
    use CheckSchedule::*;
    use HysteresisAction::*;
    let (state, hysteresis, schedule, effect) = match reason {
        DownloadEndPolicy::Stopped(DownloadStopCause::DanmuStreamClosed) => {
            (StreamerState::NotLive, Reset, Normal, TerminalEffect::None)
        }
        DownloadEndPolicy::OutOfSchedule
        | DownloadEndPolicy::Stopped(DownloadStopCause::OutOfSchedule) => (
            StreamerState::OutOfSchedule,
            EnsureLive,
            Normal,
            TerminalEffect::None,
        ),
        DownloadEndPolicy::Completed => (
            StreamerState::NotLive,
            ObserveOffline,
            Normal,
            TerminalEffect::None,
        ),
        DownloadEndPolicy::Stopped(
            DownloadStopCause::Shutdown | DownloadStopCause::StreamerDisabled,
        ) => (StreamerState::NotLive, Preserve, Park, TerminalEffect::None),
        DownloadEndPolicy::StreamerOffline
        | DownloadEndPolicy::Stopped(DownloadStopCause::StreamerOffline) => (
            StreamerState::NotLive,
            ObserveOffline,
            Normal,
            TerminalEffect::Offline,
        ),
        DownloadEndPolicy::NetworkError(_) | DownloadEndPolicy::SegmentFailed(_) => (
            StreamerState::NotLive,
            Preserve,
            Immediate,
            TerminalEffect::None,
        ),
        // A wire-level stop request is not evidence that the platform went offline
        // or an instruction to persist Cancelled and permanently remove this actor.
        DownloadEndPolicy::Other(_)
        | DownloadEndPolicy::Stopped(DownloadStopCause::Other(_) | DownloadStopCause::User) => (
            StreamerState::NotLive,
            Preserve,
            Normal,
            TerminalEffect::None,
        ),
        DownloadEndPolicy::CircuitBreakerBlocked {
            retry_after_secs, ..
        } => (
            StreamerState::TemporalDisabled,
            Preserve,
            Retry(retry_after_secs),
            TerminalEffect::Infrastructure(InfraBlockReason::CircuitBreaker { retry_after_secs }),
        ),
        DownloadEndPolicy::OutputRootBlocked {
            path,
            io_kind,
            retry_after_secs,
            ..
        } => (
            StreamerState::OutOfSpace,
            Preserve,
            Retry(retry_after_secs),
            TerminalEffect::Infrastructure(InfraBlockReason::OutputRootUnavailable {
                path,
                io_kind,
                retry_after_secs,
            }),
        ),
        DownloadEndPolicy::StreamerBackoffBlocked {
            retry_after_secs, ..
        } => (
            StreamerState::TemporalDisabled,
            Preserve,
            Retry(retry_after_secs),
            TerminalEffect::None,
        ),
    };
    TerminalDecision {
        state,
        hysteresis,
        schedule,
        effect,
    }
}

impl StreamerActor {
    pub(super) fn schedule_blocked_retry(&mut self, state: StreamerState, seconds: u64) {
        self.state.streamer_state = state;
        self.state
            .set_next_check(Instant::now().checked_add(Duration::from_secs(seconds)));
    }

    pub(super) async fn handle_download_ended(
        &mut self,
        reason: DownloadEndPolicy,
    ) -> Result<(), ActorError> {
        info!(streamer_id = %self.id, ?reason, "Download ended; applying actor policy");
        let decision = decide_terminal(reason);
        let error_count = self.get_error_count();
        self.state.last_download_activity_at = None;
        // Persistence remains before scheduling and feedback acknowledgement. The
        // terminal lifecycle owner already handles all verification-only endings.
        if !matches!(decision.effect, TerminalEffect::None) {
            let metadata = self
                .get_metadata()
                .ok_or_else(|| ActorError::fatal("Streamer removed from metadata store"))?;
            let result = match decision.effect {
                TerminalEffect::Offline => self
                    .status_checker
                    .process_status(&metadata, LiveStatus::Offline)
                    .await
                    .map(|_| ()),
                TerminalEffect::Infrastructure(reason) => {
                    self.status_checker
                        .set_infra_blocked(&metadata, reason)
                        .await
                }
                TerminalEffect::None => Ok(()),
            };
            if let Err(error) = result {
                warn!(streamer_id = %self.id, %error, "Failed to apply download-end persistence effect");
            }
        }
        self.state.streamer_state = decision.state;
        match decision.hysteresis {
            HysteresisAction::Preserve => {}
            HysteresisAction::Reset => self.state.hysteresis.reset(),
            HysteresisAction::EnsureLive | HysteresisAction::ObserveOffline => {
                if !self.state.hysteresis.was_live() {
                    self.state.hysteresis.mark_live();
                }
                if decision.hysteresis == HysteresisAction::ObserveOffline {
                    self.state.hysteresis.mark_offline_observed();
                }
            }
        }
        match decision.schedule {
            CheckSchedule::Normal => self.state.schedule_next_check(&self.config, error_count),
            CheckSchedule::Immediate => self.state.schedule_immediate_check(),
            CheckSchedule::Park => self.state.set_next_check(None),
            CheckSchedule::Retry(seconds) => self.schedule_blocked_retry(decision.state, seconds),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_terminal_policies_select_explicit_effects_and_scheduling() {
        use CheckSchedule::*;
        use HysteresisAction::*;
        let cases = [
            (
                DownloadEndPolicy::StreamerOffline,
                StreamerState::NotLive,
                ObserveOffline,
                Normal,
                "offline",
            ),
            (
                DownloadEndPolicy::Completed,
                StreamerState::NotLive,
                ObserveOffline,
                Normal,
                "none",
            ),
            (
                DownloadEndPolicy::OutOfSchedule,
                StreamerState::OutOfSchedule,
                EnsureLive,
                Normal,
                "none",
            ),
            (
                DownloadEndPolicy::NetworkError("network".into()),
                StreamerState::NotLive,
                Preserve,
                Immediate,
                "none",
            ),
            (
                DownloadEndPolicy::SegmentFailed("segment".into()),
                StreamerState::NotLive,
                Preserve,
                Immediate,
                "none",
            ),
            (
                DownloadEndPolicy::Other("unknown".into()),
                StreamerState::NotLive,
                Preserve,
                Normal,
                "none",
            ),
            (
                DownloadEndPolicy::CircuitBreakerBlocked {
                    reason: "circuit".into(),
                    retry_after_secs: 13,
                    session_id: "session".into(),
                },
                StreamerState::TemporalDisabled,
                Preserve,
                Retry(13),
                "circuit",
            ),
            (
                DownloadEndPolicy::OutputRootBlocked {
                    path: "recordings".into(),
                    io_kind: crate::downloader::IoErrorKindSer::PermissionDenied,
                    retry_after_secs: 17,
                    session_id: "session".into(),
                },
                StreamerState::OutOfSpace,
                Preserve,
                Retry(17),
                "output",
            ),
            (
                DownloadEndPolicy::StreamerBackoffBlocked {
                    reason: "backoff".into(),
                    retry_after_secs: 19,
                    session_id: "session".into(),
                },
                StreamerState::TemporalDisabled,
                Preserve,
                Retry(19),
                "none",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::User),
                StreamerState::NotLive,
                Preserve,
                Normal,
                "none",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::Shutdown),
                StreamerState::NotLive,
                Preserve,
                Park,
                "none",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::StreamerDisabled),
                StreamerState::NotLive,
                Preserve,
                Park,
                "none",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::StreamerOffline),
                StreamerState::NotLive,
                ObserveOffline,
                Normal,
                "offline",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::OutOfSchedule),
                StreamerState::OutOfSchedule,
                EnsureLive,
                Normal,
                "none",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::DanmuStreamClosed),
                StreamerState::NotLive,
                Reset,
                Normal,
                "none",
            ),
            (
                DownloadEndPolicy::Stopped(DownloadStopCause::Other("other".into())),
                StreamerState::NotLive,
                Preserve,
                Normal,
                "none",
            ),
        ];
        for (policy, state, hysteresis, schedule, expected_effect) in cases {
            let decision = decide_terminal(policy);
            assert_eq!(
                (decision.state, decision.hysteresis, decision.schedule),
                (state, hysteresis, schedule)
            );
            let effect = match decision.effect {
                TerminalEffect::None => "none",
                TerminalEffect::Offline => "offline",
                TerminalEffect::Infrastructure(InfraBlockReason::CircuitBreaker {
                    retry_after_secs,
                }) => {
                    assert_eq!(retry_after_secs, 13);
                    "circuit"
                }
                TerminalEffect::Infrastructure(InfraBlockReason::OutputRootUnavailable {
                    path,
                    io_kind,
                    retry_after_secs,
                }) => {
                    assert_eq!(path, std::path::PathBuf::from("recordings"));
                    assert_eq!(io_kind, crate::downloader::IoErrorKindSer::PermissionDenied);
                    assert_eq!(retry_after_secs, 17);
                    "output"
                }
            };
            assert_eq!(effect, expected_effect);
        }
    }
}
