use std::time::Duration;

use jiff::SignedDuration;

use crate::downloader::engine::{DownloadProtocol, EngineType};
use crate::downloader::{DownloadStopCause, EngineEndSignal};
use crate::monitor::FilterReason;
use crate::recorder::fsm::schedule::STOP_DEADLINE_DURATION;
use crate::recorder::fsm::state::{
    AttemptPhase, FsmEffect, FsmInput, MonitorPhase, SessionSlot, StreamerFsm,
};
use crate::recorder::ports::{
    ActorStopReason, AttemptStartFailure, DanmuStopKind, DanmuStopResult, DbCommitFailure,
    DbCommitOp, DbCommitOutcome, INTERACTIVE_RETIREMENT, OBSERVE_RETIREMENT, PipelineNotification,
    ProbeStatus, SessionStartDisposition,
};
use crate::session::TerminalCause;
use srec_core::domain::StreamerState;

use super::{
    TestClock, base_config, base_snapshot, cancelled_event, completed_event, find_db_commit,
    find_probe, find_start_attempt, live_payload, open_running_session, sample_binding,
    session_ended_outcome,
};

#[test]
fn out_of_schedule_and_disable_stop_deadline_timeout_sets_lingering_lease() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg.clone(), "sess-hang", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-hang".into(),
        },
    );

    // OutOfSchedule stop hits 20s deadline while download and danmu fail to confirm.
    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let p = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p,
            result: Ok(ProbeStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: Some(clock.wall + SignedDuration::from_secs(60)),
                },
                title: "Live".into(),
                category: None,
            }),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            attempt_id: id,
            cause: DownloadStopCause::OutOfSchedule,
            ..
        } if *id == attempt_id
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopDanmu {
            session_id,
            reason: DanmuStopKind::SessionEnded,
        } if session_id == "sess-hang"
    )));
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + STOP_DEADLINE_DURATION)
    );

    clock.advance(STOP_DEADLINE_DURATION);
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    assert!(fsm.lingering_physical_lease);
    assert!(!fsm.is_physically_quiescent());
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            cause: TerminalCause::OutOfSchedule,
            ..
        }
    ));
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-hang"), clock.wall, true)),
        },
    );

    // Lingering physical lease blocks launching a download for a new session until PhysicalLeaseReleased.
    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let p2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (start_op2, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p2,
            result: Ok(ProbeStatus::Live(live_payload("Next"))),
        },
    ))
    .unwrap();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op2,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-next".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    );
    assert!(find_start_attempt(&effects).is_none());

    fsm.step(clock.ctx(), FsmInput::PhysicalLeaseReleased);
    assert!(!fsm.lingering_physical_lease);
    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let p3 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p3,
            result: Ok(ProbeStatus::Live(live_payload("Next"))),
        },
    );
    let spec = find_start_attempt(&effects).unwrap();
    assert_eq!(spec.session_id, "sess-next");

    // Disable stop hitting 20s deadline unblocks control plane with UserDisabled + ActorStopped(Disabled)
    // while keeping `is_physically_quiescent() == false`.
    let (mut fsm_dis, _att_dis) = open_running_session(&clock, cfg, "sess-dis-hang", None);
    fsm_dis.step(clock.ctx(), FsmInput::Disable);
    clock.advance(STOP_DEADLINE_DURATION);
    let effects = fsm_dis.step(clock.ctx(), FsmInput::TimerFired);
    let (dis_end_op, dis_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        dis_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::UserDisabled,
            ..
        } if sid == "sess-dis-hang"
    ));
    let effects = fsm_dis.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: dis_end_op,
            result: Ok(session_ended_outcome(
                Some("sess-dis-hang"),
                clock.wall,
                true,
            )),
        },
    );
    assert!(fsm_dis.lingering_physical_lease);
    assert!(!fsm_dis.is_physically_quiescent());
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Disabled
        }
    )));
}

#[test]
fn credential_retire_requires_exact_match_and_never_commits_on_unconfirmed_or_timeout() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let binding = sample_binding("prof-retire", 3);
    let (mut fsm, attempt_id) =
        open_running_session(&clock, cfg.clone(), "sess-ret", Some(binding.clone()));
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-ret".into(),
        },
    );

    // Mismatched epoch or session_id is ignored completely.
    assert!(
        fsm.step(
            clock.ctx(),
            FsmInput::CredentialRetire {
                session_id: "sess-ret".into(),
                binding: sample_binding("prof-retire", 4),
            },
        )
        .is_empty()
    );
    assert!(
        fsm.step(
            clock.ctx(),
            FsmInput::CredentialRetire {
                session_id: "other-sess".into(),
                binding: binding.clone(),
            },
        )
        .is_empty()
    );

    // Case 1: DanmuStopResult::Unconfirmed -> NEVER commits EndForCredentialRetirement.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::CredentialRetire {
            session_id: "sess-ret".into(),
            binding: binding.clone(),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            attempt_id: id,
            cause: DownloadStopCause::StreamerDisabled,
            ..
        } if *id == attempt_id
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopDanmu {
            session_id,
            reason: DanmuStopKind::SessionEnded,
        } if session_id == "sess-ret"
    )));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-ret".into(),
            total_count: 0,
            result: DanmuStopResult::Unconfirmed {
                error: "timed out".into(),
            },
        },
    );
    assert!(find_db_commit(&effects).is_none());
    assert!(fsm.lingering_physical_lease);
    assert!(matches!(fsm.session, SessionSlot::Open(_)));
    assert!(matches!(fsm.monitor, MonitorPhase::Idle { .. }));

    // Case 2: 20s deadline timeout without download confirmation -> NEVER commits EndForCredentialRetirement.
    let (mut fsm_to, _) =
        open_running_session(&clock, base_config(), "sess-ret-to", Some(binding.clone()));
    fsm_to.step(
        clock.ctx(),
        FsmInput::CredentialRetire {
            session_id: "sess-ret-to".into(),
            binding: binding.clone(),
        },
    );
    clock.advance(STOP_DEADLINE_DURATION);
    let effects = fsm_to.step(clock.ctx(), FsmInput::TimerFired);
    assert!(find_db_commit(&effects).is_none());
    assert!(fsm_to.lingering_physical_lease);
    assert!(matches!(fsm_to.session, SessionSlot::Open(_)));
    assert!(matches!(fsm_to.monitor, MonitorPhase::Idle { .. }));

    // Case 3: Both download and danmu confirm before 20s -> commits EndForCredentialRetirement and stays Idle.
    let (mut fsm_ok, attempt_ok) =
        open_running_session(&clock, cfg, "sess-ret-ok", Some(binding.clone()));
    fsm_ok.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-ret-ok".into(),
        },
    );
    fsm_ok.step(
        clock.ctx(),
        FsmInput::CredentialRetire {
            session_id: "sess-ret-ok".into(),
            binding: binding.clone(),
        },
    );
    let effects = fsm_ok.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_ok,
            event: cancelled_event("sess-ret-ok", DownloadStopCause::StreamerDisabled),
        },
    );
    assert!(find_db_commit(&effects).is_none());

    clock.advance(Duration::from_secs(1));
    let effects = fsm_ok.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-ret-ok".into(),
            total_count: 5,
            result: DanmuStopResult::Terminated,
        },
    );
    let (retire_op_id, retire_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        retire_op,
        DbCommitOp::EndForCredentialRetirement {
            session_id,
            binding: b,
            ..
        } if session_id == "sess-ret-ok" && *b == binding
    ));

    let effects = fsm_ok.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: retire_op_id,
            result: Ok(DbCommitOutcome::CredentialRetirementEnded {
                session_id: "sess-ret-ok".into(),
                ended: true,
            }),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { session_id, .. })
            if session_id == "sess-ret-ok"
    )));
    assert!(matches!(fsm_ok.session, SessionSlot::None));
    assert!(matches!(fsm_ok.monitor, MonitorPhase::Idle { .. }));
}

#[test]
fn retire_quiescence_distinguishes_clean_drain_from_deadline_expiry_and_db_failure() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;

    // 1. Clean drain within INTERACTIVE_RETIREMENT -> Retired { quiescent: true }.
    let (mut fsm_clean, attempt1) = open_running_session(&clock, cfg, "sess-r1", None);
    fsm_clean.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-r1".into(),
        },
    );
    fsm_clean.step(
        clock.ctx(),
        FsmInput::Retire {
            bounds: INTERACTIVE_RETIREMENT,
        },
    );
    fsm_clean.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt1,
            event: cancelled_event("sess-r1", DownloadStopCause::StreamerDisabled),
        },
    );
    let effects = fsm_clean.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-r1".into(),
            total_count: 1,
            result: DanmuStopResult::Terminated,
        },
    );
    let (end_op1, op1) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op1,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::UserDisabled,
            suppress_offline_notification: true,
            ..
        } if sid == "sess-r1"
    ));
    let effects = fsm_clean.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op1,
            result: Ok(session_ended_outcome(Some("sess-r1"), clock.wall, true)),
        },
    );
    assert!(fsm_clean.is_physically_quiescent());
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Retired { quiescent: true }
        }
    )));

    // 2. OBSERVE_RETIREMENT while download hangs past the actor stop deadline ->
    //    Retired { quiescent: false } WITHOUT prematurely committing EndSession / SessionEnded.
    let (mut fsm_obs, _attempt2) = open_running_session(&clock, base_config(), "sess-r2", None);
    fsm_obs.step(
        clock.ctx(),
        FsmInput::Retire {
            bounds: OBSERVE_RETIREMENT,
        },
    );
    // Zero-wait caller budget must not expire the background actor immediately.
    let early = fsm_obs.step(clock.ctx(), FsmInput::TimerFired);
    assert!(early.is_empty());
    clock.advance(STOP_DEADLINE_DURATION);
    let effects = fsm_obs.step(clock.ctx(), FsmInput::TimerFired);
    assert!(
        find_db_commit(&effects).is_none(),
        "zero-wait Retire must not commit EndSession while download is still running"
    );
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { .. })
        )),
        "zero-wait Retire must not notify pipeline SessionEnded while download is still running"
    );
    assert!(!fsm_obs.is_physically_quiescent());
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Retired { quiescent: false }
        }
    )));

    // 3. Physical exit confirmed (is_physically_quiescent() == true), but DB EndSession fails -> quiescent: false.
    let (mut fsm_dbfail, attempt3) = open_running_session(&clock, base_config(), "sess-r3", None);
    fsm_dbfail.step(
        clock.ctx(),
        FsmInput::Retire {
            bounds: INTERACTIVE_RETIREMENT,
        },
    );
    let effects = fsm_dbfail.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt3,
            event: cancelled_event("sess-r3", DownloadStopCause::StreamerDisabled),
        },
    );
    let (end_op3, op3) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op3,
        DbCommitOp::EndSession {
            cause: TerminalCause::UserDisabled,
            suppress_offline_notification: true,
            ..
        }
    ));
    let effects = fsm_dbfail.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op3,
            result: Err(DbCommitFailure::Transient {
                message: "db error".into(),
            }),
        },
    );
    assert!(fsm_dbfail.is_physically_quiescent());
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Retired { quiescent: false }
        }
    )));
}

#[test]
fn shutdown_preserves_open_session_and_stronger_stop_preempts_weaker_stop() {
    let clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-sd", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-sd".into(),
        },
    );

    // Start weaker OutOfSchedule stop first, then preempt with Shutdown.
    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let p = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p,
            result: Ok(ProbeStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: None,
                },
                title: "Live".into(),
                category: None,
            }),
        },
    );

    let effects = fsm.step(clock.ctx(), FsmInput::Shutdown);
    // Weaker Disable after Shutdown must be ignored.
    assert!(fsm.step(clock.ctx(), FsmInput::Disable).is_empty());
    assert!(find_db_commit(&effects).is_none());

    fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: cancelled_event("sess-sd", DownloadStopCause::Shutdown),
        },
    );
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-sd".into(),
            total_count: 3,
            result: DanmuStopResult::Terminated,
        },
    );
    // Shutdown must NEVER emit EndSession (open session survives for restart-continue).
    assert!(find_db_commit(&effects).is_none());
    assert!(matches!(fsm.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Shutdown
        }
    )));

    // Direct Shutdown on fresh running session emits ServiceShutdown danmu stop to keep checkpoints.
    let mut cfg2 = base_config();
    cfg2.record_danmu = true;
    let (mut fsm2, att2) = open_running_session(&clock, cfg2, "sess-sd2", None);
    fsm2.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-sd2".into(),
        },
    );
    let effects = fsm2.step(clock.ctx(), FsmInput::Shutdown);
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            attempt_id: id,
            cause: DownloadStopCause::Shutdown,
            ..
        } if *id == att2
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopDanmu {
            session_id,
            reason: DanmuStopKind::ServiceShutdown,
        } if session_id == "sess-sd2"
    )));
}

#[test]
fn startup_events_racing_with_stopping_immediately_stop_or_confirm_download() {
    let clock = TestClock::new();

    // Case A: AttemptStarted arrives AFTER Disable entered Stopping -> immediately issues StopAttempt with download_id.
    let mut fsm_a = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );
    let p = find_probe(&fsm_a.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (start_op, _) = find_db_commit(&fsm_a.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p,
            result: Ok(ProbeStatus::Live(live_payload("Live"))),
        },
    ))
    .unwrap();
    let attempt_id = find_start_attempt(&fsm_a.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-race-start".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    ))
    .unwrap()
    .attempt_id;

    fsm_a.step(clock.ctx(), FsmInput::Disable);
    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id,
            session_id: "sess-race-start".into(),
            download_id: "dl-late".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Hls,
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            attempt_id: id,
            download_id: Some(dl),
            cause: DownloadStopCause::StreamerDisabled,
        } if *id == attempt_id && dl == "dl-late"
    )));
    assert!(matches!(
        &fsm_a.session,
        SessionSlot::Open(open) if matches!(open.attempt, AttemptPhase::StoppingAttempt { .. })
    ));

    // Case B: AttemptStartFailed arrives during Stopping -> confirms download exit and dispatches EndSession.
    let effects = fsm_a.step(
        clock.ctx(),
        FsmInput::AttemptStartFailed {
            attempt_id,
            session_id: "sess-race-start".into(),
            reason: AttemptStartFailure::Cancelled,
        },
    );
    let (_, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::UserDisabled,
            ..
        } if sid == "sess-race-start"
    ));
}

#[test]
fn credential_retire_clears_open_session_when_registry_committed_first() {
    let clock = TestClock::new();
    let binding = sample_binding("prof-race", 2);
    let (mut fsm, attempt_id) = open_running_session(
        &clock,
        base_config(),
        "sess-cred-race",
        Some(binding.clone()),
    );

    fsm.step(
        clock.ctx(),
        FsmInput::CredentialRetire {
            session_id: "sess-cred-race".into(),
            binding: binding.clone(),
        },
    );
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: cancelled_event("sess-cred-race", DownloadStopCause::StreamerDisabled),
        },
    );
    let (op_id, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op,
        DbCommitOp::EndForCredentialRetirement { session_id, .. } if session_id == "sess-cred-race"
    ));

    // Simulate `ActorRegistry::settle_credential_retirement` winning the DB end first.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::CredentialRetirementEnded {
                session_id: "sess-cred-race".into(),
                ended: false,
            }),
        },
    );
    assert!(
        matches!(fsm.session, SessionSlot::None),
        "FSM must clear the ended SessionSlot::Open even when registry won the commit"
    );
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { .. })
        )),
        "losing committer must not emit a duplicate SessionEnded pipeline notification"
    );
}

#[test]
#[ignore = "bug: next_deadline offers the elapsed quiet-period backstop while Stopping, where \
            TimerFired ignores it, so the actor loop spins until the stop settles"]
fn known_bug_disable_after_an_elapsed_quiet_backstop_waits_for_the_stop_deadline() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    cfg.offline_check_count = 3;
    cfg.offline_check_interval_ms = 20_000; // backstop window = 80 s
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-1", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-1".into(),
        },
    );
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: completed_event("sess-1", EngineEndSignal::CleanDisconnect),
        },
    );
    let (op_id, _) = find_db_commit(&effects).expect("hysteresis audit commit");
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    );

    // Disable 5 s before the backstop; the danmu stop stays unconfirmed.
    clock.advance(Duration::from_secs(75));
    let stop_deadline = clock.now + STOP_DEADLINE_DURATION;
    fsm.step(clock.ctx(), FsmInput::Disable);
    assert!(matches!(fsm.monitor, MonitorPhase::Stopping(_)));

    // The backstop has elapsed; the stop deadline is the next wake-up.
    clock.advance(Duration::from_secs(6));
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(stop_deadline)
    );

    // At the stop deadline the final EndSession goes out, and nothing is due
    // again while it is in flight.
    clock.advance(stop_deadline - clock.now);
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    let (_, op) = find_db_commit(&effects).expect("EndSession after the stop deadline");
    assert!(matches!(op, DbCommitOp::EndSession { .. }));
    assert!(
        fsm.next_deadline(clock.now, clock.wall)
            .is_none_or(|deadline| deadline > clock.now)
    );
}

#[test]
#[ignore = "bug: Shutdown emits ActorStopped while an EndSession commit is in flight, so its \
            SessionEnded pipeline notification is never sent"]
fn known_bug_shutdown_waits_for_an_in_flight_end_session() {
    let clock = TestClock::new();
    let (mut fsm, attempt_id) = open_running_session(&clock, base_config(), "sess-1", None);
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: completed_event("sess-1", EngineEndSignal::HlsEndlist),
        },
    );
    let (end_op, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(op, DbCommitOp::EndSession { .. }));

    let effects = fsm.step(clock.ctx(), FsmInput::Shutdown);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, FsmEffect::ActorStopped { .. })),
        "the actor must not stop while EndSession is in flight"
    );

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op,
            result: Ok(session_ended_outcome(Some("sess-1"), clock.wall, true)),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { session_id, .. })
            if session_id == "sess-1"
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Shutdown
        }
    )));
}

#[test]
#[ignore = "bug: after a credential retirement times out, the late Cancelled(StreamerDisabled) \
            terminal parks the monitor with no next check, so the streamer is never checked again"]
fn known_bug_credential_retire_that_times_out_keeps_monitoring_after_the_late_cancel() {
    let mut clock = TestClock::new();
    let binding = sample_binding("p1", 1);
    let mut cfg = base_config();
    cfg.managed_credentials = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-1", Some(binding.clone()));
    fsm.step(
        clock.ctx(),
        FsmInput::CredentialRetire {
            session_id: "sess-1".into(),
            binding,
        },
    );
    clock.advance(STOP_DEADLINE_DURATION + Duration::from_secs(1));
    fsm.step(clock.ctx(), FsmInput::TimerFired);
    assert!(matches!(fsm.session, SessionSlot::Open(_)));

    // The engine exits with the stop cause the retirement requested.
    fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: cancelled_event("sess-1", DownloadStopCause::StreamerDisabled),
        },
    );
    fsm.step(clock.ctx(), FsmInput::PhysicalLeaseReleased);

    let next = fsm
        .next_deadline(clock.now, clock.wall)
        .expect("the streamer is still checked");
    assert!(next <= clock.now + Duration::from_millis(fsm.config.check_interval_ms) * 2);
}

#[test]
#[ignore = "bug: a stop whose download exits after an offline EndSession closed the session \
            ignores that exit, so the deadline sets lingering_physical_lease after its release \
            and no later session starts a download"]
fn known_bug_stop_confirmed_after_the_session_ended_does_not_block_later_downloads() {
    let mut clock = TestClock::new();
    let binding = sample_binding("p1", 1);
    let mut cfg = base_config();
    cfg.managed_credentials = true;
    cfg.offline_check_count = 1;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-1", Some(binding.clone()));

    // A watchdog probe confirms offline: StopAttempt plus EndSession in flight.
    clock.advance(Duration::from_secs(301));
    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    let (end_op, _) = find_db_commit(&effects).expect("offline EndSession");

    // Credential retirement for the still-open session arrives meanwhile.
    fsm.step(
        clock.ctx(),
        FsmInput::CredentialRetire {
            session_id: "sess-1".into(),
            binding,
        },
    );
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op,
            result: Ok(session_ended_outcome(Some("sess-1"), clock.wall, true)),
        },
    );
    fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: cancelled_event("sess-1", DownloadStopCause::StreamerOffline),
        },
    );
    fsm.step(clock.ctx(), FsmInput::PhysicalLeaseReleased);
    clock.advance(STOP_DEADLINE_DURATION + Duration::from_secs(1));
    fsm.step(clock.ctx(), FsmInput::TimerFired);
    assert!(!fsm.lingering_physical_lease);

    // The streamer goes live again later and recording starts.
    clock.advance(Duration::from_secs(3600));
    let probe_id = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id,
            result: Ok(ProbeStatus::Live(live_payload("t"))),
        },
    );
    let (op_id, _) = find_db_commit(&effects).unwrap();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-2".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    );
    assert!(find_start_attempt(&effects).is_some());
}

#[test]
#[ignore = "bug: DanmuCollectionStopped from any session confirms the current stop's danmu leg"]
fn known_bug_danmu_stop_from_another_session_does_not_confirm_the_stop() {
    let clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, _attempt_id) = open_running_session(&clock, cfg, "sess-2", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-2".into(),
        },
    );
    fsm.step(clock.ctx(), FsmInput::Disable);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-old".into(),
            total_count: 0,
            result: DanmuStopResult::Terminated,
        },
    );
    match &fsm.monitor {
        MonitorPhase::Stopping(stopping) => assert!(!stopping.danmu_confirmed),
        other => panic!("expected Stopping, got {other:?}"),
    }
}
