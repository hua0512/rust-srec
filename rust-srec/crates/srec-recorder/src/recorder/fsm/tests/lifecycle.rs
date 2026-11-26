use std::time::Duration;

use crate::danmu::DanmuControlEvent;
use crate::downloader::engine::{DownloadProtocol, EngineType};
use crate::downloader::{DownloadFailureKind, EngineEndSignal};
use crate::recorder::fsm::hysteresis::QUIET_PERIOD_COMMIT_RETRY_DELAY;
use crate::recorder::fsm::state::{
    AttemptPhase, BlockedKind, FsmEffect, FsmInput, MonitorPhase, SessionSlot, StreamerFsm,
};
use crate::recorder::ports::{
    ActorStopReason, AttemptStartFailure, DanmuStopKind, DbCommitFailure, DbCommitOp,
    DbCommitOutcome, PipelineNotification, ProbeStatus, SessionStartDisposition,
};
use crate::session::{OfflineSignal, TerminalCause};
use srec_core::domain::StreamerState;
use srec_core::domain::streamer::FatalErrorType;

use super::{
    TestClock, base_config, base_snapshot, completed_event,
    enter_quiet_period_ready_for_direct_backstop, failed_event, find_db_commit, find_probe,
    find_start_attempt, live_payload, open_running_session, reach_starting_attempt, sample_binding,
    session_ended_outcome,
};

#[test]
fn committing_start_gates_attempt_on_authoritative_db_commit_and_uses_db_epoch() {
    let clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );

    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    let probe_id = find_probe(&effects).unwrap().probe_id;

    let mut payload = live_payload("Restarted Stream");
    payload.credential_binding = Some(sample_binding("prof-1", 0));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(payload)),
        },
    );
    assert!(find_start_attempt(&effects).is_none());
    assert!(matches!(fsm.session, SessionSlot::CommittingStart { .. }));
    let (op_id, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(op, DbCommitOp::StartOrResumeSession { .. }));

    let committed_binding = sample_binding("prof-1", 7);
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-reused".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::ReusedActive,
                credential_binding: Some(committed_binding.clone()),
            }),
        },
    );

    assert!(!fsm.initial_reconcile_pending);
    assert!(matches!(fsm.monitor, MonitorPhase::LiveWatchdog { .. }));
    let spec = find_start_attempt(&effects).unwrap();
    assert_eq!(spec.session_id, "sess-reused");
    assert_eq!(spec.credential_binding, Some(committed_binding));
    assert!(!spec.from_hysteresis_resume);
}

#[test]
fn committing_start_transient_db_failure_rolls_back_and_retries_immediately() {
    let clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );

    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(live_payload("Live"))),
        },
    );
    let (op_id, _) = find_db_commit(&effects).unwrap();

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Err(DbCommitFailure::Transient {
                message: "sqlite busy".into(),
            }),
        },
    );

    assert!(find_start_attempt(&effects).is_none());
    assert!(matches!(fsm.session, SessionSlot::None));
    assert!(fsm.initial_reconcile_pending);
    assert!(matches!(fsm.monitor, MonitorPhase::Idle { .. }));
    assert_eq!(fsm.next_deadline(clock.now, clock.wall), Some(clock.now));
}

#[test]
fn committing_start_suppressed_inactive_rolls_back_without_clearing_reconcile() {
    let clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );

    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(live_payload("Live"))),
        },
    );
    let (op_id, _) = find_db_commit(&effects).unwrap();

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::SessionStartSuppressed {
                state: StreamerState::TemporalDisabled,
                retry_after: Some(Duration::from_secs(120)),
            }),
        },
    );

    assert!(find_start_attempt(&effects).is_none());
    assert!(matches!(fsm.session, SessionSlot::None));
    assert!(fsm.initial_reconcile_pending);
    assert_eq!(
        fsm.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::TemporalDisabled,
            retry_at: clock.now + Duration::from_secs(120),
        }
    );
}

#[test]
fn disable_during_in_flight_committing_start_waits_for_session_id_then_ends_it() {
    let clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );

    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(live_payload("Live"))),
        },
    );
    let (start_op_id, _) = find_db_commit(&effects).unwrap();

    let effects = fsm.step(clock.ctx(), FsmInput::Disable);
    assert!(matches!(fsm.monitor, MonitorPhase::Stopping(_)));
    assert!(find_db_commit(&effects).is_none());

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-42".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    );
    assert!(find_start_attempt(&effects).is_none());
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::UserDisabled,
            ..
        } if sid == "sess-42"
    ));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-42"), clock.wall, true)),
        },
    );
    assert!(matches!(fsm.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Disabled
        }
    )));
}

#[test]
fn queued_resume_hysteresis_preserves_op_id_handles_suppression_and_disable_without_deadlock() {
    let clock = TestClock::new();

    // Case A: queued ResumeHysteresis preserves pre-allocated op_id; suppression keeps quiet_period armed.
    let (mut fsm_a, attempt_a) = open_running_session(&clock, base_config(), "sess-q", None);
    fsm_a.step(clock.ctx(), FsmInput::CheckNow);
    let probe_a = find_probe(&fsm_a.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_a,
            event: completed_event("sess-q", EngineEndSignal::CleanDisconnect),
        },
    );
    let (hyst_a_op, hyst_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        hyst_op,
        DbCommitOp::RecordHysteresisEntered { .. }
    ));

    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe_a,
            result: Ok(ProbeStatus::Live(live_payload("Resumed Title"))),
        },
    );
    assert!(find_db_commit(&effects).is_none());
    let queued_op_id = match &fsm_a.session {
        SessionSlot::CommittingStart { op_id, .. } => *op_id,
        other => panic!("expected CommittingStart, got {other:?}"),
    };
    assert_ne!(queued_op_id, 0);

    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_a_op,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    );
    let (flushed_op_id, flushed_op) = find_db_commit(&effects).unwrap();
    assert_eq!(flushed_op_id, queued_op_id);
    assert!(matches!(flushed_op, DbCommitOp::ResumeHysteresis { .. }));

    // Suppression on ResumeHysteresis must restore prior_open with quiet_period still armed.
    fsm_a.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: flushed_op_id,
            result: Ok(DbCommitOutcome::HysteresisResumeSuppressed {
                state: StreamerState::TemporalDisabled,
                retry_after: Some(Duration::from_secs(30)),
            }),
        },
    );
    assert!(matches!(
        &fsm_a.session,
        SessionSlot::Open(open) if open.session_id == "sess-q" && open.quiet_period.is_some()
    ));
    assert!(matches!(
        fsm_a.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::TemporalDisabled,
            ..
        }
    ));

    // Retry live check -> ResumeHysteresis succeeds and starts attempt with from_hysteresis_resume = true.
    fsm_a.step(clock.ctx(), FsmInput::CheckNow);
    let probe_a2 = find_probe(&fsm_a.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe_a2,
            result: Ok(ProbeStatus::Live(live_payload("Resumed Title 2"))),
        },
    );
    let (resume_op, _) = find_db_commit(&effects).unwrap();
    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: resume_op,
            result: Ok(DbCommitOutcome::HysteresisResumed {
                session_id: "sess-q".into(),
                started_at: clock.wall,
            }),
        },
    );
    assert!(find_start_attempt(&effects).unwrap().from_hysteresis_resume);

    // Case B: Disable arrives while ResumeHysteresis is still queued behind RecordHysteresisEntered.
    let (mut fsm_b, attempt_b) = open_running_session(&clock, base_config(), "sess-1", None);
    fsm_b.step(clock.ctx(), FsmInput::CheckNow);
    let probe_b = find_probe(&fsm_b.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm_b.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_b,
            event: completed_event("sess-1", EngineEndSignal::CleanDisconnect),
        },
    );
    let (hyst_b_op, _) = find_db_commit(&effects).unwrap();
    fsm_b.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe_b,
            result: Ok(ProbeStatus::Live(live_payload("Resumed While Pending"))),
        },
    );
    assert!(matches!(fsm_b.session, SessionSlot::CommittingStart { .. }));

    fsm_b.step(clock.ctx(), FsmInput::Disable);
    assert!(matches!(fsm_b.session, SessionSlot::Open(_)));
    let effects = fsm_b.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_b_op,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    );
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::UserDisabled,
            via_hysteresis: true,
            ..
        } if sid == "sess-1"
    ));
    let effects = fsm_b.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-1"), clock.wall, true)),
        },
    );
    assert!(matches!(fsm_b.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Disabled
        }
    )));
}

#[test]
fn queued_resume_hysteresis_invalidated_when_in_flight_end_session_commits() {
    let mut clock = TestClock::new();
    let (mut fsm, _timer_id, _backstop) =
        enter_quiet_period_ready_for_direct_backstop(&mut clock, "sess-race");

    // Wake error backoff right before backstop so a BoundPoll probe is in flight when EndSession fires.
    clock.advance(Duration::from_secs(44));
    fsm.step(clock.ctx(), FsmInput::ErrorCleared);
    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuControl {
            session_id: "sess-race".into(),
            control: DanmuControlEvent::StreamClosed {
                message: Some("closed".into()),
                action: None,
            },
        },
    );
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::DefinitiveOffline {
                signal: OfflineSignal::DanmuStreamClosed,
            },
            ..
        } if sid == "sess-race"
    ));

    // Re-arm a QuietPeriod scenario where EndSession from backstop is in flight while a Live probe completes.
    let mut clock2 = TestClock::new();
    let (mut fsm2, _, _) = enter_quiet_period_ready_for_direct_backstop(&mut clock2, "sess-qrace");
    clock2.advance(Duration::from_secs(45));
    let (end_op2, _) = find_db_commit(&fsm2.step(clock2.ctx(), FsmInput::TimerFired)).unwrap();
    fsm2.step(
        clock2.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op2,
            result: Err(DbCommitFailure::Transient {
                message: "busy".into(),
            }),
        },
    );
    // Spawn a probe via ErrorCleared before the 1s retry fires.
    fsm2.step(clock2.ctx(), FsmInput::ErrorCleared);
    let in_flight_probe = find_probe(&fsm2.step(clock2.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;

    // Advance 1s -> QuietPeriod retry dispatches EndSession while `in_flight_probe` is STILL running.
    clock2.advance(QUIET_PERIOD_COMMIT_RETRY_DELAY);
    let effects = fsm2.step(clock2.ctx(), FsmInput::TimerFired);
    let (retry_end_op, retry_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        retry_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            via_hysteresis: true,
            ..
        } if sid == "sess-qrace"
    ));

    // While `retry_end_op` is in flight, `in_flight_probe` completes Live -> queues `ResumeHysteresis`.
    let effects = fsm2.step(
        clock2.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: in_flight_probe,
            result: Ok(ProbeStatus::Live(live_payload("Brand New Stream"))),
        },
    );
    assert!(find_db_commit(&effects).is_none());
    assert!(matches!(
        fsm2.session,
        SessionSlot::CommittingStart {
            from_hysteresis: true,
            ..
        }
    ));

    // `retry_end_op` commits SessionEnded -> drops stale queued ResumeHysteresis, emits SessionEnded
    // for "sess-qrace", and immediately dispatches a fresh StartOrResumeSession instead.
    let effects = fsm2.step(
        clock2.ctx(),
        FsmInput::DbCommitted {
            op_id: retry_end_op,
            result: Ok(session_ended_outcome(Some("sess-qrace"), clock2.wall, true)),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { session_id, .. })
            if session_id == "sess-qrace"
    )));
    let (fresh_op_id, fresh_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(fresh_op, DbCommitOp::StartOrResumeSession { .. }));

    // Fresh session commits cleanly and launches download for "sess-new".
    let effects = fsm2.step(
        clock2.ctx(),
        FsmInput::DbCommitted {
            op_id: fresh_op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-new".into(),
                started_at: clock2.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    );
    let spec = find_start_attempt(&effects).unwrap();
    assert_eq!(spec.session_id, "sess-new");
    assert!(!spec.from_hysteresis_resume);

    // Cleanly ignore late probe on first fsm after StreamClosed ended it.
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-race"), clock.wall, true)),
        },
    );
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());
}

#[test]
fn cold_start_offline_reconciles_until_committed_then_suppresses_redundant_writes() {
    let mut clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );

    let probe1 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe1,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    let (op1, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op,
        DbCommitOp::EndSession {
            update_streamer_state: Some(StreamerState::NotLive),
            ..
        }
    ));

    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: op1,
            result: Err(DbCommitFailure::Transient {
                message: "db locked".into(),
            }),
        },
    );
    assert!(fsm.initial_reconcile_pending);

    clock.advance(Duration::from_secs(60));
    let probe2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe2,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    let (op2, _) = find_db_commit(&effects).unwrap();
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: op2,
            result: Ok(session_ended_outcome(None, clock.wall, false)),
        },
    );
    assert!(!fsm.initial_reconcile_pending);

    clock.advance(Duration::from_secs(60));
    let probe3 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe3,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());
}

#[test]
fn refused_broadcast_suppresses_live_until_offline_reconciles() {
    let mut clock = TestClock::new();
    let (mut fsm, attempt_id) = open_running_session(&clock, base_config(), "sess-drm", None);

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: failed_event(
                "sess-drm",
                "dl-1",
                EngineType::Mesio,
                DownloadProtocol::Hls,
                DownloadFailureKind::UnsupportedStream,
                "DRM protected",
            ),
        },
    );
    assert!(matches!(fsm.monitor, MonitorPhase::RefusedBroadcast { .. }));
    let (end_op, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::Failed {
                kind: DownloadFailureKind::UnsupportedStream,
            },
            ..
        } if sid == "sess-drm"
    ));
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op,
            result: Ok(session_ended_outcome(Some("sess-drm"), clock.wall, true)),
        },
    );
    let (err_op, _) = find_db_commit(&effects).unwrap();
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: err_op,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 1,
                disabled_until: None,
            }),
        },
    );

    // Subsequent Live probe during same broadcast is suppressed (no StartOrResumeSession).
    clock.advance(Duration::from_secs(60));
    let p_live = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p_live,
            result: Ok(ProbeStatus::Live(live_payload("Same DRM Broadcast"))),
        },
    );
    assert!(find_db_commit(&effects).is_none());
    assert!(matches!(fsm.monitor, MonitorPhase::RefusedBroadcast { .. }));

    // Once Offline is observed, RefusedBroadcast clears and forces DB reconciliation.
    clock.advance(Duration::from_secs(60));
    let p_off = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p_off,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    assert!(matches!(fsm.monitor, MonitorPhase::Idle { .. }));
    let (_, reconcile_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        reconcile_op,
        DbCommitOp::EndSession {
            update_streamer_state: Some(StreamerState::NotLive),
            clear_errors: true,
            ..
        }
    ));
}

#[test]
fn fatal_probe_stops_active_session_terminates_and_cold_start_disable_closes_db_row() {
    let clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-fatal", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-fatal".into(),
        },
    );

    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let p = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p,
            result: Ok(ProbeStatus::Fatal {
                kind: FatalErrorType::NotFound,
                message: "room removed".into(),
            }),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt { attempt_id: id, .. } if *id == attempt_id
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopDanmu {
            session_id,
            reason: DanmuStopKind::SessionEnded,
        } if session_id == "sess-fatal"
    )));
    let (fatal_op, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op,
        DbCommitOp::RecordFatalError {
            session_id: Some(sid),
            new_state: StreamerState::NotFound,
            ..
        } if sid == "sess-fatal"
    ));

    // Transient DB failure on RecordFatalError schedules immediate retry rather than terminating prematurely.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: fatal_op,
            result: Err(DbCommitFailure::Transient {
                message: "db busy".into(),
            }),
        },
    );
    assert!(!matches!(fsm.monitor, MonitorPhase::Terminated));
    assert!(
        effects.is_empty()
            || !effects
                .iter()
                .any(|e| matches!(e, FsmEffect::ActorStopped { .. }))
    );
    assert_eq!(fsm.next_deadline(clock.now, clock.wall), Some(clock.now));

    // Retry succeeds -> emits SessionEnded + ActorStopped(FatalError { NotFound }).
    let p2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (fatal_op2, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p2,
            result: Ok(ProbeStatus::Fatal {
                kind: FatalErrorType::NotFound,
                message: "room removed".into(),
            }),
        },
    ))
    .unwrap();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: fatal_op2,
            result: Ok(DbCommitOutcome::FatalErrorRecorded {
                session_ended: Some("sess-fatal".into()),
                new_state: StreamerState::NotFound,
            }),
        },
    );
    assert!(matches!(fsm.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { session_id, .. })
            if session_id == "sess-fatal"
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::FatalError {
                state: StreamerState::NotFound,
            }
        }
    )));

    // Cold-start Disable when SessionSlot::None and initial_reconcile_pending == true still closes any DB row.
    let mut cold = StreamerFsm::new(
        base_snapshot(StreamerState::Live, base_config()),
        clock.now,
        clock.wall,
    );
    let effects = cold.step(clock.ctx(), FsmInput::Disable);
    let (_, cold_end) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        cold_end,
        DbCommitOp::EndSession {
            session_id: None,
            cause: TerminalCause::UserDisabled,
            suppress_offline_notification: true,
            ..
        }
    ));
}

#[test]
fn runtime_view_reports_attempt_started_at_only_while_running() {
    let mut clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );
    let p = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (start_op, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p,
            result: Ok(ProbeStatus::Live(live_payload("Live"))),
        },
    ))
    .unwrap();
    let session_start_wall = clock.wall;
    let attempt_id = find_start_attempt(&fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-view".into(),
                started_at: session_start_wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    ))
    .unwrap()
    .attempt_id;

    let starting_view = fsm.runtime_view();
    assert!(!starting_view.attempt_active);
    assert_eq!(starting_view.recording_started_at, None);

    clock.advance(Duration::from_secs(90));
    fsm.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id,
            session_id: "sess-view".into(),
            download_id: "dl-view".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Hls,
        },
    );
    let running_view = fsm.runtime_view();
    assert!(running_view.attempt_active);
    assert_eq!(running_view.recording_started_at, Some(clock.wall));
    assert_ne!(running_view.recording_started_at, Some(session_start_wall));

    // StoppingAttempt / Idle clear `attempt_active` and `recording_started_at`.
    fsm.step(clock.ctx(), FsmInput::Disable);
    let stopping_view = fsm.runtime_view();
    assert!(!stopping_view.attempt_active);
    assert_eq!(stopping_view.recording_started_at, None);
}

#[test]
#[ignore = "bug: attempt ids restart at the same sequence in every actor generation, so a \
            restarted generation adopts the previous generation's download as its own"]
fn known_bug_restarted_generation_does_not_adopt_the_previous_generations_attempt() {
    let clock = TestClock::new();
    let (_previous, previous_attempt) = reach_starting_attempt(&clock, base_config());
    let (mut current, current_attempt) = reach_starting_attempt(&clock, base_config());
    assert_ne!(previous_attempt, current_attempt);

    // The previous generation's surviving startup task starts its download.
    current.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id: previous_attempt,
            session_id: "sess-1".into(),
            download_id: "dl-previous".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
        },
    );
    match &current.session {
        SessionSlot::Open(open) => assert!(matches!(
            open.attempt,
            AttemptPhase::Starting { attempt_id, .. } if attempt_id == current_attempt
        )),
        other => panic!("expected Open, got {other:?}"),
    }
}

#[test]
#[ignore = "bug: an attempt that starts after a teardown outside Stopping (danmu StreamClosed, \
            offline end, fatal probe) is promoted to Running and never stopped"]
fn known_bug_attempt_started_after_a_teardown_is_stopped() {
    let clock = TestClock::new();
    let (mut fsm, attempt_id) = reach_starting_attempt(&clock, base_config());
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuControl {
            session_id: "sess-1".into(),
            control: DanmuControlEvent::StreamClosed {
                message: None,
                action: None,
            },
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            download_id: None,
            ..
        }
    )));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id,
            session_id: "sess-1".into(),
            download_id: "dl-late".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt { attempt_id: id, download_id: Some(download), .. }
            if *id == attempt_id && download == "dl-late"
    )));
    if let SessionSlot::Open(open) = &fsm.session {
        assert!(!matches!(open.attempt, AttemptPhase::Running { .. }));
    }
}

#[test]
#[ignore = "bug: a startup refused because the previous download is still active reports \
            Cancelled and waits for the 5-minute stall probe instead of the lease release"]
fn known_bug_startup_refused_by_an_active_download_retries_once_it_is_released() {
    let clock = TestClock::new();
    let (mut fsm, attempt_id) = reach_starting_attempt(&clock, base_config());
    fsm.step(
        clock.ctx(),
        FsmInput::AttemptStartFailed {
            attempt_id,
            session_id: "sess-1".into(),
            reason: AttemptStartFailure::Cancelled,
        },
    );
    let effects = fsm.step(clock.ctx(), FsmInput::PhysicalLeaseReleased);
    assert!(find_start_attempt(&effects).is_some());
}
