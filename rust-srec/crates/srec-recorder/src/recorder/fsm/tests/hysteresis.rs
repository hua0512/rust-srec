use std::time::Duration;

use crate::downloader::engine::{DownloadProtocol, EngineType};
use crate::downloader::{DownloadFailureKind, DownloadStopCause, EngineEndSignal};
use crate::recorder::fsm::hysteresis::{
    MAX_QUIET_PERIOD_WINDOW, QUIET_PERIOD_COMMIT_RETRY_DELAY, RECENTLY_ENDED_RETENTION,
    SlidingFailureWindow, quiet_period_window,
};
use crate::recorder::fsm::state::{FsmEffect, FsmInput, MonitorPhase, SessionSlot};
use crate::recorder::ports::{
    ActorStopReason, DanmuStopKind, DbCommitFailure, DbCommitOp, DbCommitOutcome,
    PipelineNotification, ProbeFailure, ProbeStatus,
};
use crate::session::{EngineKind, OfflineSignal, TerminalCause, TerminalCauseDto};
use srec_core::domain::StreamerState;

use super::{
    TestClock, base_config, completed_event, enter_quiet_period_ready_for_direct_backstop,
    failed_event, find_db_commit, find_probe, find_start_attempt, live_payload,
    open_running_session, session_ended_outcome,
};

#[test]
fn orthogonal_offline_confirmation_survives_running_attempt_and_resets_on_live() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-1", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-1".into(),
        },
    );

    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let p1 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p1,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, FsmEffect::StopAttempt { .. }))
    );
    assert_eq!(fsm.offline_confirmation.consecutive_offline, 1);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + Duration::from_secs(20))
    );

    clock.advance(Duration::from_secs(20));
    let p2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p2,
            result: Ok(ProbeStatus::Live(live_payload("Live Title"))),
        },
    );
    assert!(find_start_attempt(&effects).is_none());
    assert_eq!(fsm.offline_confirmation.consecutive_offline, 0);
    assert!(matches!(fsm.monitor, MonitorPhase::LiveWatchdog { .. }));

    fsm.step(clock.ctx(), FsmInput::CheckNow);
    for expected_count in 1..=2 {
        let p = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
            .unwrap()
            .probe_id;
        let effects = fsm.step(
            clock.ctx(),
            FsmInput::ProbeCompleted {
                probe_id: p,
                result: Ok(ProbeStatus::Offline {
                    batch_live_hint_armed: false,
                }),
            },
        );
        assert_eq!(fsm.offline_confirmation.consecutive_offline, expected_count);
        assert!(find_db_commit(&effects).is_none());
        clock.advance(Duration::from_secs(20));
    }

    let p3 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p3,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopAttempt {
            attempt_id: id,
            cause: DownloadStopCause::StreamerOffline,
            ..
        } if *id == attempt_id
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::StopDanmu {
            session_id,
            reason: DanmuStopKind::SessionEnded,
        } if session_id == "sess-1"
    )));
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            cause: TerminalCause::StreamerOffline,
            update_streamer_state: Some(StreamerState::NotLive),
            ..
        }
    ));

    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-1"), clock.wall, true)),
        },
    );
    assert!(!fsm.offline_confirmation.was_live);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + Duration::from_secs(60))
    );
}

#[test]
fn quiet_period_backstop_cap_1s_retry_and_live_preemption() {
    assert_eq!(quiet_period_window(20, 60_000), MAX_QUIET_PERIOD_WINDOW);

    let mut clock = TestClock::new();
    let (mut fsm, timer_id_1, _backstop) =
        enter_quiet_period_ready_for_direct_backstop(&mut clock, "sess-qp");

    // Advance straight to 45s backstop deadline -> commits original Failed(Io) cause.
    clock.advance(Duration::from_secs(45));
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    let (end_op_1, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::Failed {
                kind: DownloadFailureKind::Io,
            },
            via_hysteresis: true,
            ..
        } if sid == "sess-qp"
    ));

    // Transient DB failure retains the exact same timer_id and schedules 1s retry.
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_1,
            result: Err(DbCommitFailure::Transient {
                message: "io timeout".into(),
            }),
        },
    );
    match &fsm.session {
        SessionSlot::Open(open) => {
            let qp = open.quiet_period.as_ref().unwrap();
            assert_eq!(qp.timer_id, timer_id_1);
            assert!(qp.retrying_commit);
        }
        other => panic!("expected Open session retained, got {other:?}"),
    }
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + QUIET_PERIOD_COMMIT_RETRY_DELAY)
    );

    // At +1s retry dispatches the exact same EndSession again.
    clock.advance(QUIET_PERIOD_COMMIT_RETRY_DELAY);
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    let (end_op_2, retry_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        retry_op,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::Failed {
                kind: DownloadFailureKind::Io,
            },
            via_hysteresis: true,
            ..
        } if sid == "sess-qp"
    ));
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_2,
            result: Err(DbCommitFailure::Transient {
                message: "still busy".into(),
            }),
        },
    );

    // Live check arriving before the next 1s retry preempts with ResumeHysteresis.
    fsm.step(clock.ctx(), FsmInput::ErrorCleared);
    let live_probe = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: live_probe,
            result: Ok(ProbeStatus::Live(live_payload("Back Online"))),
        },
    );
    let (resume_op_id, resume_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(resume_op, DbCommitOp::ResumeHysteresis { .. }));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: resume_op_id,
            result: Ok(DbCommitOutcome::HysteresisResumed {
                session_id: "sess-qp".into(),
                started_at: clock.wall,
            }),
        },
    );
    let spec = find_start_attempt(&effects).unwrap();
    assert!(spec.from_hysteresis_resume);
    assert_eq!(spec.session_id, "sess-qp");
    assert!(matches!(
        &fsm.session,
        SessionSlot::Open(open) if open.quiet_period.is_none()
    ));
}

#[test]
fn quiet_period_backstop_freezes_bounded_grace_for_in_flight_probe_and_confirmation() {
    // Case A: probe is in flight when backstop expires (no active offline confirmation) -> 1 grace slot.
    let mut clock_a = TestClock::new();
    let (mut fsm_a, _, _) = enter_quiet_period_ready_for_direct_backstop(&mut clock_a, "sess-a");
    clock_a.advance(Duration::from_secs(44));
    fsm_a.step(clock_a.ctx(), FsmInput::ErrorCleared);
    let probe_a = find_probe(&fsm_a.step(clock_a.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;

    clock_a.advance(Duration::from_secs(1)); // hits 45s backstop with probe_a in flight
    let effects = fsm_a.step(clock_a.ctx(), FsmInput::TimerFired);
    assert!(find_db_commit(&effects).is_none());
    match &fsm_a.session {
        SessionSlot::Open(open) => {
            assert_eq!(
                open.quiet_period.as_ref().unwrap().grace_remaining_checks,
                Some(1)
            );
        }
        other => panic!("expected Open, got {other:?}"),
    }

    // Probe returns Offline (1 of 2): exhausts the 1 frozen grace slot -> commits original Failed(Io).
    let effects = fsm_a.step(
        clock_a.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe_a,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    let (_, end_op_a) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op_a,
        DbCommitOp::EndSession {
            cause: TerminalCause::Failed {
                kind: DownloadFailureKind::Io,
            },
            via_hysteresis: true,
            ..
        }
    ));

    // Case B: active offline confirmation sequence (1 of 3) at backstop freezes 2 grace slots;
    // a transient probe failure during grace ends immediately with original ambiguous Completed cause.
    let mut clock_b = TestClock::new();
    let (mut fsm_b, attempt_b) = open_running_session(&clock_b, base_config(), "sess-b", None);
    let effects = fsm_b.step(
        clock_b.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_b,
            event: completed_event("sess-b", EngineEndSignal::CleanDisconnect),
        },
    );
    let (hyst_b, _) = find_db_commit(&effects).unwrap();
    fsm_b.step(
        clock_b.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_b,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    );
    // A slow probe launched at T+20s finishes transiently at T+65s (`err_count = 1`,
    // monitor stays `Idle` scheduled for T+85s rather than entering `Blocked`),
    // leaving an active confirmation sequence (1 of 3) when backstop fires at T+80s.
    clock_b.advance(Duration::from_secs(20));
    let p = find_probe(&fsm_b.step(clock_b.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    clock_b.advance(Duration::from_secs(45));
    let effects = fsm_b.step(
        clock_b.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p,
            result: Err(ProbeFailure::Transient {
                message: "slow-err-1".into(),
            }),
        },
    );
    let (err_op, _) = find_db_commit(&effects).unwrap();
    fsm_b.step(
        clock_b.ctx(),
        FsmInput::DbCommitted {
            op_id: err_op,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 1,
                disabled_until: None,
            }),
        },
    );

    // At T+80s, backstop fires while monitor is `Idle` (next check T+85s) ->
    // freezes 2 remaining checks (3 - 1 = 2).
    clock_b.advance(Duration::from_secs(15));
    let effects = fsm_b.step(clock_b.ctx(), FsmInput::TimerFired);
    assert!(find_db_commit(&effects).is_none());
    match &fsm_b.session {
        SessionSlot::Open(open) => {
            assert_eq!(
                open.quiet_period.as_ref().unwrap().grace_remaining_checks,
                Some(2)
            );
        }
        other => panic!("expected Open, got {other:?}"),
    }

    // At T+85s, the scheduled confirmation probe fails transiently -> commits original Completed immediately.
    clock_b.advance(Duration::from_secs(5));
    let probe_b = find_probe(&fsm_b.step(clock_b.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm_b.step(
        clock_b.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe_b,
            result: Err(ProbeFailure::Transient {
                message: "timeout".into(),
            }),
        },
    );
    let (_, end_op_b) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op_b,
        DbCommitOp::EndSession {
            cause: TerminalCause::Completed,
            via_hysteresis: true,
            ..
        }
    ));

    // Case C: When monitor entered `Blocked(TemporalDisabled)` with no in-flight probe,
    // backstop expiry must commit immediately rather than freezing until backoff clears.
    let mut clock_c = TestClock::new();
    let (mut fsm_c, attempt_c) = open_running_session(&clock_c, base_config(), "sess-c", None);
    let effects = fsm_c.step(
        clock_c.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_c,
            event: completed_event("sess-c", EngineEndSignal::CleanDisconnect),
        },
    );
    let (hyst_c, _) = find_db_commit(&effects).unwrap();
    fsm_c.step(
        clock_c.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_c,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    );
    for (step_idx, err_count) in [(1, 1), (2, 2), (3, 3)] {
        clock_c.advance(Duration::from_secs(20));
        let p = find_probe(&fsm_c.step(clock_c.ctx(), FsmInput::TimerFired))
            .unwrap()
            .probe_id;
        let effects = fsm_c.step(
            clock_c.ctx(),
            FsmInput::ProbeCompleted {
                probe_id: p,
                result: Err(ProbeFailure::Transient {
                    message: format!("err-{step_idx}"),
                }),
            },
        );
        let (err_op, _) = find_db_commit(&effects).unwrap();
        let disabled_until =
            (err_count >= 3).then(|| clock_c.wall + jiff::SignedDuration::from_secs(3600));
        fsm_c.step(
            clock_c.ctx(),
            FsmInput::DbCommitted {
                op_id: err_op,
                result: Ok(DbCommitOutcome::TransientErrorRecorded {
                    consecutive_error_count: err_count,
                    disabled_until,
                }),
            },
        );
    }
    clock_c.advance(Duration::from_secs(20));
    let effects = fsm_c.step(clock_c.ctx(), FsmInput::TimerFired);
    let (_, end_op_c) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op_c,
        DbCommitOp::EndSession {
            cause: TerminalCause::Completed,
            via_hysteresis: true,
            ..
        }
    ));
}

#[test]
fn recently_ended_60s_window_reattributes_ambiguous_timer_end_only() {
    // Part 1: Ambiguous hysteresis end -> Disable within 60s rewrites cause without re-emitting SessionEnded.
    let mut clock1 = TestClock::new();
    let (mut fsm1, _, _) = enter_quiet_period_ready_for_direct_backstop(&mut clock1, "sess-ambig");
    clock1.advance(Duration::from_secs(45));
    let (end_op1, _) = find_db_commit(&fsm1.step(clock1.ctx(), FsmInput::TimerFired)).unwrap();
    let effects = fsm1.step(
        clock1.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op1,
            result: Ok(session_ended_outcome(Some("sess-ambig"), clock1.wall, true)),
        },
    );
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { .. })
    )));

    clock1.advance(Duration::from_secs(30));
    let effects = fsm1.step(clock1.ctx(), FsmInput::Disable);
    let (rewrite_op_id, rewrite_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        rewrite_op,
        DbCommitOp::RewriteEndedCause {
            session_id,
            new_cause: TerminalCauseDto::UserDisabled,
        } if session_id == "sess-ambig"
    ));
    let effects = fsm1.step(
        clock1.ctx(),
        FsmInput::DbCommitted {
            op_id: rewrite_op_id,
            result: Ok(DbCommitOutcome::EndedCauseRewritten {
                session_id: "sess-ambig".into(),
                updated: true,
            }),
        },
    );
    assert!(!effects.iter().any(|e| matches!(
        e,
        FsmEffect::NotifyPipeline(PipelineNotification::SessionEnded { .. })
    )));
    assert!(matches!(fsm1.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Disabled
        }
    )));

    // Part 2: Disable racing at `now >= evict_at` BEFORE TimerFired must NOT rewrite (tests `ctx.now < *evict_at`).
    let mut clock2 = TestClock::new();
    let (mut fsm2, _, _) = enter_quiet_period_ready_for_direct_backstop(&mut clock2, "sess-race60");
    clock2.advance(Duration::from_secs(45));
    let (end_op2, _) = find_db_commit(&fsm2.step(clock2.ctx(), FsmInput::TimerFired)).unwrap();
    fsm2.step(
        clock2.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op2,
            result: Ok(session_ended_outcome(
                Some("sess-race60"),
                clock2.wall,
                true,
            )),
        },
    );
    clock2.advance(RECENTLY_ENDED_RETENTION);
    let effects = fsm2.step(clock2.ctx(), FsmInput::Disable);
    assert!(find_db_commit(&effects).is_none());
    assert!(matches!(fsm2.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Disabled
        }
    )));

    // Part 3: Authoritative HLS ENDLIST end is never retained for reattribution on Disable.
    let mut clock3 = TestClock::new();
    let (mut fsm3, attempt3) = open_running_session(&clock3, base_config(), "sess-hls", None);
    let effects3 = fsm3.step(
        clock3.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt3,
            event: completed_event("sess-hls", EngineEndSignal::HlsEndlist),
        },
    );
    let (end_op3, end_db3) = find_db_commit(&effects3).unwrap();
    assert!(matches!(
        end_db3,
        DbCommitOp::EndSession {
            cause: TerminalCause::Completed,
            via_hysteresis: false,
            ..
        }
    ));
    fsm3.step(
        clock3.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op3,
            result: Ok(session_ended_outcome(Some("sess-hls"), clock3.wall, true)),
        },
    );
    clock3.advance(Duration::from_secs(5));
    let effects = fsm3.step(clock3.ctx(), FsmInput::Disable);
    assert!(find_db_commit(&effects).is_none());
    assert!(matches!(fsm3.monitor, MonitorPhase::Terminated));
    assert!(effects.iter().any(|e| matches!(
        e,
        FsmEffect::ActorStopped {
            reason: ActorStopReason::Disabled
        }
    )));
}

#[test]
fn sliding_failure_window_promotes_mesio_network_only_and_ignores_discarded_segments() {
    let mut clock = TestClock::new();
    let cfg = base_config();
    let mut window = SlidingFailureWindow::default();

    // HTTP 404 and FFmpeg Network never accumulate or classify.
    for _ in 0..5 {
        assert!(
            window
                .classify_failure(
                    clock.now,
                    EngineKind::MesioHls,
                    DownloadFailureKind::HttpClientError { status: 404 },
                    &cfg,
                )
                .is_none()
        );
        assert!(
            window
                .classify_failure(
                    clock.now,
                    EngineKind::Ffmpeg,
                    DownloadFailureKind::Network,
                    &cfg,
                )
                .is_none()
        );
    }
    assert!(window.is_empty());

    // Old failure outside 60s window expires before reaching threshold 3.
    assert!(
        window
            .classify_failure(
                clock.now,
                EngineKind::MesioFlv,
                DownloadFailureKind::Network,
                &cfg
            )
            .is_none()
    );
    clock.advance(Duration::from_secs(61));
    assert!(
        window
            .classify_failure(
                clock.now,
                EngineKind::MesioFlv,
                DownloadFailureKind::Network,
                &cfg
            )
            .is_none()
    );
    clock.advance(Duration::from_secs(1));
    assert!(
        window
            .classify_failure(
                clock.now,
                EngineKind::MesioFlv,
                DownloadFailureKind::Network,
                &cfg
            )
            .is_none()
    );
    clock.advance(Duration::from_secs(1));
    assert_eq!(
        window.classify_failure(
            clock.now,
            EngineKind::MesioFlv,
            DownloadFailureKind::Network,
            &cfg
        ),
        Some(OfflineSignal::ConsecutiveFailures(3))
    );

    // End-to-end via public `step()` with offline_check_count = 1 (threshold floors at 2):
    // 1) First Mesio Network failure enters QuietPeriod (count = 1).
    // 2) Resume -> DISCARDED small segment must NOT clear `network_failures` -> 2nd Network failure promotes!
    let mut small_cfg = base_config();
    small_cfg.offline_check_count = 1;
    small_cfg.min_segment_size_bytes = 1_000_000;
    let (mut fsm, attempt1) = open_running_session(&clock, small_cfg, "sess-net", None);

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt1,
            event: failed_event(
                "sess-net",
                "dl-1",
                EngineType::Mesio,
                DownloadProtocol::Flv,
                DownloadFailureKind::Network,
                "reset-1",
            ),
        },
    );
    let (hyst_op, op1) = find_db_commit(&effects).unwrap();
    assert!(matches!(op1, DbCommitOp::RecordHysteresisEntered { .. }));
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_op,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
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

    // Resume via Live probe -> new attempt starts -> emits a DISCARDED segment (< min_segment_size_bytes).
    clock.advance(Duration::from_secs(2));
    let p_live = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p_live,
            result: Ok(ProbeStatus::Live(live_payload("Resumed"))),
        },
    );
    let (res_op, _) = find_db_commit(&effects).unwrap();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: res_op,
            result: Ok(DbCommitOutcome::HysteresisResumed {
                session_id: "sess-net".into(),
                started_at: clock.wall,
            }),
        },
    );
    let attempt2 = find_start_attempt(&effects).unwrap().attempt_id;
    fsm.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id: attempt2,
            session_id: "sess-net".into(),
            download_id: "dl-2".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
        },
    );
    // Discarded tiny segment must NOT reset network_failures.
    fsm.step(
        clock.ctx(),
        FsmInput::SegmentCompleted {
            attempt_id: attempt2,
            session_id: "sess-net".into(),
            download_id: "dl-2".into(),
            segment_index: 0,
            segment_path: "/rec/tiny.flv".into(),
            started_at: Some(clock.wall),
            completed_at: clock.wall,
            duration_secs: 0.5,
            size_bytes: 1000,
            effective_size_bytes: 1000,
            split_reason_code: None,
            split_reason_details_json: None,
            split_by_format_change: false,
        },
    );
    assert_eq!(fsm.network_failures.len(), 1);

    // Second Mesio Network failure within window promotes directly to DefinitiveOffline(ConsecutiveFailures(2)).
    clock.advance(Duration::from_secs(2));
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt2,
            event: failed_event(
                "sess-net",
                "dl-2",
                EngineType::Mesio,
                DownloadProtocol::Flv,
                DownloadFailureKind::Network,
                "reset-2",
            ),
        },
    );
    let (_, op2) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        op2,
        DbCommitOp::EndSession {
            session_id: Some(sid),
            cause: TerminalCause::DefinitiveOffline {
                signal: OfflineSignal::ConsecutiveFailures(2),
            },
            ..
        } if sid == "sess-net"
    ));

    // Contrast: a KEPT (non-discarded) segment resets `network_failures` so a subsequent failure stays in QuietPeriod.
    let mut keep_cfg = base_config();
    keep_cfg.offline_check_count = 1;
    keep_cfg.min_segment_size_bytes = 1_000_000;
    let (mut fsm_k, attempt_k1) = open_running_session(&clock, keep_cfg, "sess-keep", None);
    let (hyst_k, _) = find_db_commit(&fsm_k.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_k1,
            event: failed_event(
                "sess-keep",
                "dl-1",
                EngineType::Mesio,
                DownloadProtocol::Flv,
                DownloadFailureKind::Network,
                "reset-1",
            ),
        },
    ))
    .unwrap();
    let (err_k, _) = find_db_commit(&fsm_k.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: hyst_k,
            result: Ok(DbCommitOutcome::HysteresisEnteredRecorded),
        },
    ))
    .unwrap();
    fsm_k.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: err_k,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 1,
                disabled_until: None,
            }),
        },
    );
    let p_k = find_probe(&fsm_k.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (res_k, _) = find_db_commit(&fsm_k.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p_k,
            result: Ok(ProbeStatus::Live(live_payload("Resumed"))),
        },
    ))
    .unwrap();
    let attempt_k2 = find_start_attempt(&fsm_k.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: res_k,
            result: Ok(DbCommitOutcome::HysteresisResumed {
                session_id: "sess-keep".into(),
                started_at: clock.wall,
            }),
        },
    ))
    .unwrap()
    .attempt_id;
    fsm_k.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id: attempt_k2,
            session_id: "sess-keep".into(),
            download_id: "dl-2".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Flv,
        },
    );
    // Kept segment clears network_failures.
    let effects = fsm_k.step(
        clock.ctx(),
        FsmInput::SegmentCompleted {
            attempt_id: attempt_k2,
            session_id: "sess-keep".into(),
            download_id: "dl-2".into(),
            segment_index: 0,
            segment_path: "/rec/full.flv".into(),
            started_at: Some(clock.wall),
            completed_at: clock.wall,
            duration_secs: 60.0,
            size_bytes: 5_000_000,
            effective_size_bytes: 5_000_000,
            split_reason_code: None,
            split_reason_details_json: None,
            split_by_format_change: false,
        },
    );
    if let Some((clr_op, _)) = find_db_commit(&effects) {
        fsm_k.step(
            clock.ctx(),
            FsmInput::DbCommitted {
                op_id: clr_op,
                result: Ok(DbCommitOutcome::ErrorsCleared),
            },
        );
    }
    assert!(fsm_k.network_failures.is_empty());
    let effects = fsm_k.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_k2,
            event: failed_event(
                "sess-keep",
                "dl-2",
                EngineType::Mesio,
                DownloadProtocol::Flv,
                DownloadFailureKind::Network,
                "reset-after-good-seg",
            ),
        },
    );
    let (_, op_k2) = find_db_commit(&effects).unwrap();
    assert!(matches!(op_k2, DbCommitOp::RecordHysteresisEntered { .. }));
}
