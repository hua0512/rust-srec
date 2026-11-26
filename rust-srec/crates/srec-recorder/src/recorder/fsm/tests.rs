mod credentials;
mod hysteresis;
mod invariants;
mod lifecycle;
mod schedule;
mod segment;
mod stopping;

use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use platforms_parser::media::StreamFormat;
use platforms_parser::media::formats::MediaFormat;

use super::state::{FsmEffect, FsmInput, SessionSlot, StepContext, StreamerFsm};
use crate::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner,
    CredentialSelection, CredentialSnapshot, ResolvedCredentialPolicy,
};
use crate::downloader::engine::{DownloadProtocol, EngineType};
use crate::downloader::{
    DownloadFailureKind, DownloadStopCause, DownloadTerminalEvent, EngineEndSignal,
};
use crate::monitor::StreamInfo;
use crate::proxies::ResolvedRoute;
use crate::recorder::ports::{
    AttemptStartSpec, DbCommitOp, DbCommitOutcome, InitialStreamerSnapshot, LiveProbePayload,
    ProbeFailure, ProbeRequest, ProbeStatus, ResolvedActorConfig, SessionStartDisposition,
};
use srec_core::domain::StreamerState;

struct TestClock {
    now: Instant,
    wall: Timestamp,
}

impl TestClock {
    fn new() -> Self {
        Self {
            now: Instant::now(),
            wall: Timestamp::from_second(1_700_000_000).unwrap(),
        }
    }

    fn ctx(&self) -> StepContext {
        StepContext::deterministic(self.now, self.wall)
    }

    fn advance(&mut self, duration: Duration) {
        self.now += duration;
        self.wall += SignedDuration::try_from(duration).unwrap();
    }
}

fn base_config() -> ResolvedActorConfig {
    ResolvedActorConfig {
        streamer_name: "Streamer".into(),
        streamer_url: "https://example.com/room/1".into(),
        platform: "twitch".into(),
        check_interval_ms: 60_000,
        offline_check_interval_ms: 20_000,
        offline_check_count: 3,
        record_danmu: false,
        ..ResolvedActorConfig::default()
    }
}

fn base_snapshot(state: StreamerState, config: ResolvedActorConfig) -> InitialStreamerSnapshot {
    InitialStreamerSnapshot {
        streamer_id: "streamer-1".into(),
        state,
        deleted: false,
        consecutive_error_count: 0,
        disabled_until: None,
        last_error: None,
        config,
        config_revision: 1,
    }
}

fn sample_stream() -> StreamInfo {
    StreamInfo::builder(
        "https://cdn.example.com/live/stream.m3u8?token=secret",
        StreamFormat::Hls,
        MediaFormat::Mp4,
    )
    .quality("1080p")
    .build()
}

fn sample_binding(profile_id: &str, epoch: u64) -> CredentialBinding {
    let policy = ResolvedCredentialPolicy::new(
        "twitch".into(),
        CredentialOwner::Platform {
            platform_id: "twitch".into(),
        },
        CredentialSelection::Fixed {
            credential_id: profile_id.into(),
        },
    )
    .unwrap();
    CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: profile_id.into(),
        },
        revision: 1,
        policy,
        epoch,
    }
}

fn sample_snapshot(binding: CredentialBinding) -> Arc<CredentialSnapshot> {
    Arc::new(CredentialSnapshot {
        binding,
        material: CredentialMaterial {
            cookies: "session=supersecret".into(),
            refresh_token: None,
            access_token: None,
            reauth_config: None,
        },
        route: ResolvedRoute::default(),
    })
}

fn live_payload(title: &str) -> Box<LiveProbePayload> {
    Box::new(LiveProbePayload {
        title: title.into(),
        category: Some("Gaming".into()),
        avatar: None,
        streams: vec![sample_stream()],
        media_headers: None,
        media_extras: None,
        credential_binding: None,
        credential_snapshot: None,
        next_check_hint: None,
    })
}

fn completed_event(session_id: &str, engine_signal: EngineEndSignal) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Completed {
        download_id: "dl-1".into(),
        streamer_id: "streamer-1".into(),
        streamer_name: "Streamer".into(),
        session_id: session_id.into(),
        total_bytes: 1024,
        total_duration_secs: 10.0,
        total_segments: 1,
        file_path: None,
        engine_signal,
        stop_cause: None,
    }
}

fn failed_event(
    session_id: &str,
    download_id: &str,
    engine_type: EngineType,
    protocol: DownloadProtocol,
    kind: DownloadFailureKind,
    error: &str,
) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Failed {
        download_id: download_id.into(),
        streamer_id: "streamer-1".into(),
        streamer_name: "Streamer".into(),
        session_id: session_id.into(),
        engine_type,
        protocol,
        kind,
        error: error.into(),
        recoverable: true,
    }
}

fn cancelled_event(session_id: &str, cause: DownloadStopCause) -> DownloadTerminalEvent {
    DownloadTerminalEvent::Cancelled {
        download_id: "dl-1".into(),
        streamer_id: "streamer-1".into(),
        streamer_name: "Streamer".into(),
        session_id: session_id.into(),
        cause,
    }
}

fn session_ended_outcome(
    session_id: Option<&str>,
    wall: Timestamp,
    transitioned_from_open: bool,
) -> DbCommitOutcome {
    DbCommitOutcome::SessionEnded {
        session_id: session_id.map(str::to_owned),
        started_at: transitioned_from_open.then_some(wall),
        ended_at: wall,
        transitioned_from_open,
    }
}

fn find_probe(effects: &[FsmEffect]) -> Option<&ProbeRequest> {
    let mut probes = effects.iter().filter_map(|effect| match effect {
        FsmEffect::SpawnProbe(req) => Some(req),
        _ => None,
    });
    let first = probes.next();
    assert!(
        probes.next().is_none(),
        "at most one SpawnProbe may be emitted per step"
    );
    first
}

fn find_db_commit(effects: &[FsmEffect]) -> Option<(u64, &DbCommitOp)> {
    let mut commits = effects.iter().filter_map(|effect| match effect {
        FsmEffect::CommitDb { op_id, op } => Some((*op_id, op)),
        _ => None,
    });
    let first = commits.next();
    assert!(
        commits.next().is_none(),
        "at most one CommitDb may be dispatched in flight per step"
    );
    first
}

fn find_start_attempt(effects: &[FsmEffect]) -> Option<&AttemptStartSpec> {
    effects.iter().find_map(|effect| match effect {
        FsmEffect::StartAttempt(spec) => Some(spec.as_ref()),
        _ => None,
    })
}

fn open_running_session_with_snapshot(
    clock: &TestClock,
    snapshot: InitialStreamerSnapshot,
    session_id: &str,
    binding: Option<CredentialBinding>,
) -> (StreamerFsm, u64) {
    let mut fsm = StreamerFsm::new(snapshot, clock.now, clock.wall);
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    let probe_id = find_probe(&effects).unwrap().probe_id;

    let mut payload = live_payload("Live Title");
    payload.credential_binding = binding.clone();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(payload)),
        },
    );
    let (op_id, _) = find_db_commit(&effects).unwrap();

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: session_id.into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: binding,
            }),
        },
    );
    let attempt_id = find_start_attempt(&effects).unwrap().attempt_id;

    fsm.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id,
            session_id: session_id.into(),
            download_id: "dl-1".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
        },
    );
    (fsm, attempt_id)
}

/// Probe live and commit a new session, stopping at the `StartAttempt` the
/// commit emits.
fn reach_starting_attempt(clock: &TestClock, config: ResolvedActorConfig) -> (StreamerFsm, u64) {
    let fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, config),
        clock.now,
        clock.wall,
    );
    reach_starting_attempt_from(fsm, clock)
}

/// [`reach_starting_attempt`] for an already constructed `fsm`.
fn reach_starting_attempt_from(mut fsm: StreamerFsm, clock: &TestClock) -> (StreamerFsm, u64) {
    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(live_payload("Live Title"))),
        },
    );
    let (op_id, _) = find_db_commit(&effects).unwrap();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-1".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    );
    let attempt_id = find_start_attempt(&effects).unwrap().attempt_id;
    (fsm, attempt_id)
}

/// Assert that the next wake-up is at least the jitter floor of the shortest
/// configured poll interval away, so a failure never re-probes immediately.
#[track_caller]
fn assert_waits_a_poll_interval(fsm: &StreamerFsm, clock: &TestClock) {
    let shortest = fsm
        .config
        .offline_check_interval_ms
        .min(fsm.config.check_interval_ms);
    let floor = clock.now + super::schedule::jittered_delay(shortest, 9_000);
    let deadline = fsm.next_deadline(clock.now, clock.wall);
    assert!(
        deadline.is_some_and(|deadline| deadline >= floor),
        "next deadline {:?} is sooner than {:?} from now",
        deadline.map(|deadline| deadline.saturating_duration_since(clock.now)),
        floor - clock.now
    );
}

fn open_running_session(
    clock: &TestClock,
    config: ResolvedActorConfig,
    session_id: &str,
    binding: Option<CredentialBinding>,
) -> (StreamerFsm, u64) {
    open_running_session_with_snapshot(
        clock,
        base_snapshot(StreamerState::NotLive, config),
        session_id,
        binding,
    )
}

/// Drive a running session into an expired QuietPeriod via `Failed(Io)` + backoff
/// so `offline_confirmation.consecutive_offline` remains 0 and backstop commits
/// cleanly through public `step()` calls without white-box field mutation.
fn enter_quiet_period_ready_for_direct_backstop(
    clock: &mut TestClock,
    session_id: &str,
) -> (StreamerFsm, u64, Instant) {
    let mut cfg = base_config();
    cfg.offline_check_count = 2; // backstop = (2 + 1) * 15s = 45s (< 60s error backoff); threshold = 2
    cfg.offline_check_interval_ms = 15_000;
    let (mut fsm, attempt_id) = open_running_session(clock, cfg, session_id, None);

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: failed_event(
                session_id,
                "dl-1",
                EngineType::Mesio,
                DownloadProtocol::Flv,
                DownloadFailureKind::Io,
                "disk hiccup",
            ),
        },
    );
    let (hyst_op, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(op, DbCommitOp::RecordHysteresisEntered { .. }));
    let (timer_id, backstop) = match &fsm.session {
        SessionSlot::Open(open) => {
            let qp = open.quiet_period.as_ref().unwrap();
            (qp.timer_id, qp.backstop_deadline)
        }
        other => panic!("expected Open session, got {other:?}"),
    };
    assert_eq!(backstop, clock.now + Duration::from_secs(45));

    // Commit RecordHysteresisEntered -> flushes queued RecordTransientError (count 1).
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_op,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    );
    let (err1_op, err1) = find_db_commit(&effects).unwrap();
    assert!(matches!(err1, DbCommitOp::RecordTransientError { .. }));
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: err1_op,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 1,
                disabled_until: None,
            }),
        },
    );

    // Immediate probe fails transiently -> hits error threshold 2 -> enters 60s Blocked backoff
    // so no probe runs before `backstop` (45s) and `consecutive_offline` stays 0.
    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Err(ProbeFailure::Transient {
                message: "upstream 502".into(),
            }),
        },
    );
    let (err2_op, _) = find_db_commit(&effects).unwrap();
    let backoff_until = clock.wall + SignedDuration::from_secs(60);
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: err2_op,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 2,
                disabled_until: Some(backoff_until),
            }),
        },
    );

    (fsm, timer_id, backstop)
}
