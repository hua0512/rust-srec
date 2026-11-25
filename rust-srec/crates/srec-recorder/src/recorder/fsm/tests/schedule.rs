use std::path::PathBuf;
use std::time::Duration;

use jiff::SignedDuration;

use crate::downloader::engine::DownloadProgress;
use crate::downloader::{DownloadStopCause, IoErrorKindSer};
use crate::monitor::{FilterReason, InfraBlockReason};
use crate::recorder::fsm::error::{
    BASE_BACKOFF_SECS, MAX_BACKOFF_SECS, backoff_duration_for_error_count,
    should_emit_checks_failing, should_emit_checks_restored,
};
use crate::recorder::fsm::schedule::{
    SMART_WAKE_BUFFER, STALL_WINDOW, WATCHDOG_ERROR_BACKOFF, jittered_delay,
};
use crate::recorder::fsm::segment::RECOVERY_PROGRESS_MIN_BYTES;
use crate::recorder::fsm::state::{
    AttemptPhase, BlockedKind, FilteredKind, FsmEffect, FsmInput, MonitorPhase, SessionSlot,
    StreamerFsm,
};
use crate::recorder::ports::{
    AttemptStartFailure, DanmuStopKind, DanmuStopResult, DbCommitOp, DbCommitOutcome, ProbeFailure,
    ProbePurpose, ProbeStatus, SessionStartDisposition,
};
use crate::session::TerminalCause;
use srec_core::domain::StreamerState;

use super::{
    TestClock, base_config, base_snapshot, cancelled_event, find_db_commit, find_probe,
    find_start_attempt, live_payload, open_running_session, open_running_session_with_snapshot,
    session_ended_outcome,
};

#[test]
fn out_of_schedule_stops_active_session_with_smart_wake_while_metadata_mismatch_keeps_recording() {
    let clock = TestClock::new();
    let mut cfg = base_config();
    cfg.record_danmu = true;
    let (mut fsm, attempt_id) = open_running_session(&clock, cfg, "sess-sched", None);
    fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStarted {
            session_id: "sess-sched".into(),
        },
    );

    for reason in [FilterReason::TitleMismatch, FilterReason::CategoryMismatch] {
        fsm.step(clock.ctx(), FsmInput::CheckNow);
        let probe = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
            .unwrap()
            .probe_id;
        let effects = fsm.step(
            clock.ctx(),
            FsmInput::ProbeCompleted {
                probe_id: probe,
                result: Ok(ProbeStatus::Filtered {
                    reason,
                    title: "Other Title".into(),
                    category: Some("Chat".into()),
                }),
            },
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, FsmEffect::StopAttempt { .. }))
        );
        assert!(find_db_commit(&effects).is_none());
        assert!(matches!(fsm.monitor, MonitorPhase::LiveWatchdog { .. }));
        assert!(matches!(
            &fsm.session,
            SessionSlot::Open(open) if matches!(open.attempt, AttemptPhase::Running { .. })
        ));
    }

    let next_window = clock.wall + SignedDuration::from_secs(600);
    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let probe = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe,
            result: Ok(ProbeStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: Some(next_window),
                },
                title: "Live".into(),
                category: None,
            }),
        },
    );
    assert!(matches!(fsm.monitor, MonitorPhase::Stopping(_)));
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
        } if session_id == "sess-sched"
    )));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: cancelled_event("sess-sched", DownloadStopCause::OutOfSchedule),
        },
    );
    assert!(find_db_commit(&effects).is_none());

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DanmuCollectionStopped {
            session_id: "sess-sched".into(),
            total_count: 12,
            result: DanmuStopResult::Terminated,
        },
    );
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            cause: TerminalCause::OutOfSchedule,
            update_streamer_state: Some(StreamerState::OutOfSchedule),
            suppress_offline_notification: true,
            ..
        }
    ));

    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-sched"), clock.wall, true)),
        },
    );
    assert!(matches!(
        fsm.monitor,
        MonitorPhase::Filtered {
            reason: FilteredKind::OutOfSchedule { smart_wake: Some(ts) },
            recurring_interval_ms: None,
            ..
        } if ts == next_window
    ));
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + Duration::from_secs(600) + SMART_WAKE_BUFFER)
    );
}

#[test]
fn persisted_out_of_space_is_dormant_while_temporal_disabled_blocks_and_runtime_output_root_cools_down()
 {
    let clock = TestClock::new();

    let dormant = StreamerFsm::new(
        base_snapshot(StreamerState::OutOfSpace, base_config()),
        clock.now,
        clock.wall,
    );
    assert!(matches!(dormant.monitor, MonitorPhase::Terminated));
    assert_eq!(dormant.next_deadline(clock.now, clock.wall), None);

    let mut deleted_snap = base_snapshot(StreamerState::Live, base_config());
    deleted_snap.deleted = true;
    assert!(matches!(
        StreamerFsm::new(deleted_snap, clock.now, clock.wall).monitor,
        MonitorPhase::Terminated
    ));

    let mut td_snap = base_snapshot(StreamerState::TemporalDisabled, base_config());
    td_snap.disabled_until = Some(clock.wall + SignedDuration::from_secs(90));
    let td_fsm = StreamerFsm::new(td_snap, clock.now, clock.wall);
    assert_eq!(
        td_fsm.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::TemporalDisabled,
            retry_at: clock.now + Duration::from_secs(90),
        }
    );

    // Runtime OutputRootUnavailable -> Blocked(OutputRootCooldown), infra_block persisted, error count untouched.
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );
    let probe = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (start_op, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe,
            result: Ok(ProbeStatus::Live(live_payload("Live"))),
        },
    ))
    .unwrap();
    let attempt_id = find_start_attempt(&fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-disk".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: None,
            }),
        },
    ))
    .unwrap()
    .attempt_id;

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::AttemptStartFailed {
            attempt_id,
            session_id: "sess-disk".into(),
            reason: AttemptStartFailure::OutputRootUnavailable {
                path: PathBuf::from("/mnt/rec"),
                io_kind: IoErrorKindSer::StorageFull,
                reason: "disk full".into(),
                retry_after_secs: 180,
            },
        },
    );
    assert_eq!(
        fsm.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::OutputRootCooldown,
            retry_at: clock.now + Duration::from_secs(180),
        }
    );
    assert!(fsm.errors.is_disabled(clock.wall));
    assert_eq!(fsm.errors.consecutive_error_count, 0);
    let (end_op_id, end_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        end_op,
        DbCommitOp::EndSession {
            infra_block: Some(InfraBlockReason::OutputRootUnavailable {
                retry_after_secs: 180,
                ..
            }),
            ..
        }
    ));
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: end_op_id,
            result: Ok(session_ended_outcome(Some("sess-disk"), clock.wall, true)),
        },
    );

    fsm.step(clock.ctx(), FsmInput::ErrorCleared);
    assert!(!fsm.errors.is_disabled(clock.wall));
    assert!(matches!(fsm.monitor, MonitorPhase::Idle { .. }));
    assert_eq!(fsm.next_deadline(clock.now, clock.wall), Some(clock.now));
}

#[test]
fn config_revision_fence_and_config_rescheduling_rules() {
    // Jitter clamps out-of-range basis points strictly to ±10%.
    assert_eq!(jittered_delay(10_000, 0), Duration::from_secs(9));
    assert_eq!(jittered_delay(10_000, u16::MAX), Duration::from_secs(11));

    let mut clock = TestClock::new();
    let mut snap = base_snapshot(StreamerState::NotLive, base_config());
    snap.config_revision = 5;
    let mut fsm = StreamerFsm::new(snap, clock.now, clock.wall);

    let probe = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (op, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: probe,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    ))
    .unwrap();
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: op,
            result: Ok(session_ended_outcome(None, clock.wall, false)),
        },
    );
    let initial_deadline = clock.now + Duration::from_secs(60);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(initial_deadline)
    );

    // Stale revision <= 5 is ignored.
    let mut stale_cfg = base_config();
    stale_cfg.check_interval_ms = 5_000;
    fsm.step(
        clock.ctx(),
        FsmInput::ConfigUpdated {
            revision: 5,
            config: Box::new(stale_cfg),
        },
    );
    assert_eq!(fsm.config_revision, 5);
    assert_eq!(fsm.config.check_interval_ms, 60_000);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(initial_deadline)
    );

    // Slower interval (revision 6) does NOT postpone earlier scheduled poll.
    clock.advance(Duration::from_secs(10));
    let mut slower_cfg = base_config();
    slower_cfg.check_interval_ms = 120_000;
    fsm.step(
        clock.ctx(),
        FsmInput::ConfigUpdated {
            revision: 6,
            config: Box::new(slower_cfg),
        },
    );
    assert_eq!(fsm.config_revision, 6);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(initial_deadline)
    );

    // Faster interval (revision 7) brings next_check forward.
    let mut faster_cfg = base_config();
    faster_cfg.check_interval_ms = 10_000;
    fsm.step(
        clock.ctx(),
        FsmInput::ConfigUpdated {
            revision: 7,
            config: Box::new(faster_cfg),
        },
    );
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + Duration::from_secs(10))
    );

    // Smart-wake OutOfSchedule (reached through public step) is NOT overridden by faster config.
    clock.advance(Duration::from_secs(10));
    let p_oos = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let smart_target = clock.wall + SignedDuration::from_secs(500);
    let (oos_op, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p_oos,
            result: Ok(ProbeStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: Some(smart_target),
                },
                title: "Filtered".into(),
                category: None,
            }),
        },
    ))
    .unwrap();
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: oos_op,
            result: Ok(session_ended_outcome(None, clock.wall, false)),
        },
    );
    let smart_deadline = clock.now + Duration::from_secs(500) + SMART_WAKE_BUFFER;
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(smart_deadline)
    );

    let mut very_fast = base_config();
    very_fast.check_interval_ms = 2_000;
    fsm.step(
        clock.ctx(),
        FsmInput::ConfigUpdated {
            revision: 8,
            config: Box::new(very_fast.clone()),
        },
    );
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(smart_deadline)
    );

    // Blocked TemporalDisabled (reached via constructor) is NOT overridden by faster config.
    let mut blocked_snap = base_snapshot(StreamerState::TemporalDisabled, base_config());
    blocked_snap.config_revision = 8;
    blocked_snap.disabled_until = Some(clock.wall + SignedDuration::from_secs(300));
    let mut blocked_fsm = StreamerFsm::new(blocked_snap, clock.now, clock.wall);
    let blocked_deadline = clock.now + Duration::from_secs(300);
    blocked_fsm.step(
        clock.ctx(),
        FsmInput::ConfigUpdated {
            revision: 9,
            config: Box::new(very_fast),
        },
    );
    assert_eq!(
        blocked_fsm.next_deadline(clock.now, clock.wall),
        Some(blocked_deadline)
    );
}

#[test]
fn batch_live_hint_wakes_early_and_preserves_routine_deadline_when_offline() {
    let mut clock = TestClock::new();
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );

    let p1 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (op1, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p1,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: true,
            }),
        },
    ))
    .unwrap();
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: op1,
            result: Ok(session_ended_outcome(None, clock.wall, false)),
        },
    );
    let routine_deadline = clock.now + Duration::from_secs(60);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(routine_deadline)
    );

    clock.advance(Duration::from_secs(10));
    fsm.step(clock.ctx(), FsmInput::BatchLiveHint);
    assert_eq!(fsm.next_deadline(clock.now, clock.wall), Some(clock.now));

    let p2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p2,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: true,
            }),
        },
    );
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(routine_deadline)
    );
}

#[test]
fn live_watchdog_activity_refresh_stall_recommit_filter_hint_and_failure_backoff() {
    let mut clock = TestClock::new();
    let (mut fsm, _attempt_id) = open_running_session(&clock, base_config(), "sess-wd", None);

    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + STALL_WINDOW)
    );

    // Mismatched heartbeat must NOT extend last_activity_at; matching heartbeat DOES.
    clock.advance(Duration::from_secs(120));
    fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-wd".into(),
            download_id: "wrong-dl".into(),
            progress: None,
        },
    );
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + STALL_WINDOW - Duration::from_secs(120))
    );

    fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-wd".into(),
            download_id: "dl-1".into(),
            progress: None,
        },
    );
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + STALL_WINDOW)
    );

    // Advance 5 minutes with no activity -> watchdog spawns probe with `is_live_watchdog == true`.
    clock.advance(STALL_WINDOW);
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    let req = find_probe(&effects).unwrap();
    assert!(req.is_live_watchdog);
    assert_eq!(req.purpose, ProbePurpose::BoundPoll);

    // Watchdog transient failure sets 60s backoff floor without incrementing ErrorTracker or writing DB.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: req.probe_id,
            result: Err(ProbeFailure::Transient {
                message: "network blip".into(),
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());
    assert_eq!(fsm.errors.consecutive_error_count, 0);
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + WATCHDOG_ERROR_BACKOFF)
    );

    // After 60s floor, watchdog probe succeeds Live while stalled -> forces StartOrResume re-commit.
    clock.advance(WATCHDOG_ERROR_BACKOFF);
    let req2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let mut payload = live_payload("Still Live");
    payload.next_check_hint = Some(clock.wall + SignedDuration::from_secs(90));
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: req2,
            result: Ok(ProbeStatus::Live(payload)),
        },
    );
    let (op_id, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(op, DbCommitOp::StartOrResumeSession { .. }));

    // Existing Running attempt must NOT be duplicated when stalled SessionStarted commits.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-wd".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::ReusedActive,
                credential_binding: None,
            }),
        },
    );
    assert!(find_start_attempt(&effects).is_none());
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + Duration::from_secs(90))
    );
}

#[test]
fn non_watchdog_transient_probe_failure_enters_backoff_gates_check_now_and_ignores_stale_ids() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.offline_check_count = 2; // threshold = 2
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, cfg),
        clock.now,
        clock.wall,
    );

    let p1 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;

    // Stale probe_id is ignored without clearing in_flight_probe.
    assert!(
        fsm.step(
            clock.ctx(),
            FsmInput::ProbeCompleted {
                probe_id: p1 + 99,
                result: Ok(ProbeStatus::Offline {
                    batch_live_hint_armed: false,
                }),
            },
        )
        .is_empty()
    );
    assert_eq!(fsm.in_flight_probe.as_ref().map(|p| p.probe_id), Some(p1));

    // Failure 1 -> RecordTransientError; stale op_id is ignored without clearing pending_db.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p1,
            result: Err(ProbeFailure::Transient {
                message: "dns timeout".into(),
            }),
        },
    );
    let (op1, db1) = find_db_commit(&effects).unwrap();
    assert!(matches!(db1, DbCommitOp::RecordTransientError { .. }));
    assert!(
        fsm.step(
            clock.ctx(),
            FsmInput::DbCommitted {
                op_id: op1 + 99,
                result: Ok(DbCommitOutcome::LastErrorCleared),
            },
        )
        .is_empty()
    );
    assert_eq!(fsm.pending_db.as_ref().map(|p| p.op_id), Some(op1));

    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: op1,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 1,
                disabled_until: None,
                checks_failing_emitted: false,
            }),
        },
    );

    // Failure 2 crosses threshold -> Blocked { TemporalDisabled, now + 60s }.
    clock.advance(Duration::from_secs(60));
    let p2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let (op2, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p2,
            result: Err(ProbeFailure::Transient {
                message: "dns timeout 2".into(),
            }),
        },
    ))
    .unwrap();
    let until = clock.wall + SignedDuration::from_secs(60);
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: op2,
            result: Ok(DbCommitOutcome::TransientErrorRecorded {
                consecutive_error_count: 2,
                disabled_until: Some(until),
                checks_failing_emitted: true,
            }),
        },
    );
    assert_eq!(
        fsm.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::TemporalDisabled,
            retry_at: clock.now + Duration::from_secs(60),
        }
    );

    // CheckNow while `errors.is_disabled(wall_now)` is still true must NOT fire a network probe;
    // on_timer_fired re-arms Blocked until `disabled_until` expires or ErrorCleared arrives.
    clock.advance(Duration::from_secs(10));
    fsm.step(clock.ctx(), FsmInput::CheckNow);
    let effects = fsm.step(clock.ctx(), FsmInput::TimerFired);
    assert!(find_probe(&effects).is_none());
    assert_eq!(
        fsm.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::TemporalDisabled,
            retry_at: clock.now + Duration::from_secs(50),
        }
    );
}

#[test]
fn error_tracker_backoff_cap_threshold_floor_and_download_recovery_rules() {
    let clock = TestClock::new();

    // Threshold = max(offline_check_count, 2): offline_check_count = 1 still floors at 2.
    assert_eq!(backoff_duration_for_error_count(1, 1), None);
    assert_eq!(
        backoff_duration_for_error_count(2, 1),
        Some(Duration::from_secs(BASE_BACKOFF_SECS))
    );
    assert_eq!(
        backoff_duration_for_error_count(3, 1),
        Some(Duration::from_secs(BASE_BACKOFF_SECS * 2))
    );
    assert_eq!(
        backoff_duration_for_error_count(i32::MAX, 1),
        Some(Duration::from_secs(MAX_BACKOFF_SECS))
    );
    assert!(!should_emit_checks_failing(1, 1));
    assert!(should_emit_checks_failing(2, 1));
    assert!(!should_emit_checks_failing(3, 1));
    assert!(!should_emit_checks_restored(1, 1));
    assert!(should_emit_checks_restored(2, 1));

    // Seed prior error state through InitialStreamerSnapshot (proving StartOrResume preserves it until recovery).
    let mut snap = base_snapshot(StreamerState::NotLive, base_config());
    snap.consecutive_error_count = 2;
    snap.last_error = Some("previous error".into());
    let (mut fsm, attempt_id) = open_running_session_with_snapshot(&clock, snap, "sess-err", None);
    assert_eq!(fsm.errors.consecutive_error_count, 2);

    // SegmentStarted clears last_error only (consecutive_error_count stays 2).
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::SegmentStarted {
            attempt_id,
            session_id: "sess-err".into(),
            download_id: "dl-1".into(),
            segment_index: 0,
            segment_path: "/rec/seg0.flv".into(),
            started_at: clock.wall,
        },
    );
    let (clear_last_op, op) = find_db_commit(&effects).unwrap();
    assert!(matches!(op, DbCommitOp::ClearLastError { .. }));
    assert_eq!(fsm.errors.consecutive_error_count, 2);
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: clear_last_op,
            result: Ok(DbCommitOutcome::LastErrorCleared),
        },
    );

    // Below 8 MiB or zero speed does NOT clear errors.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-err".into(),
            download_id: "dl-1".into(),
            progress: Some(DownloadProgress {
                bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES - 1,
                speed_bytes_per_sec: 100_000,
                ..DownloadProgress::default()
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-err".into(),
            download_id: "dl-1".into(),
            progress: Some(DownloadProgress {
                bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES,
                speed_bytes_per_sec: 0,
                ..DownloadProgress::default()
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());

    // Sustained >= 8 MiB with speed > 0 clears full error state with threshold = 3 and set_state = Some(Live).
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-err".into(),
            download_id: "dl-1".into(),
            progress: Some(DownloadProgress {
                bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES,
                speed_bytes_per_sec: 500_000,
                ..DownloadProgress::default()
            }),
        },
    );
    let (clear_op_id, clear_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        clear_op,
        DbCommitOp::ClearErrors {
            checks_restored_threshold: 3,
            set_state: Some(StreamerState::Live),
            ..
        }
    ));
    assert_eq!(fsm.errors.consecutive_error_count, 2);

    // A second recovery signal while `ClearErrors` is still in flight is deduplicated.
    let dup_effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-err".into(),
            download_id: "dl-1".into(),
            progress: Some(DownloadProgress {
                bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES * 2,
                speed_bytes_per_sec: 500_000,
                ..DownloadProgress::default()
            }),
        },
    );
    assert!(find_db_commit(&dup_effects).is_none());

    // A transient commit failure keeps error state intact so a later recovery retries.
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: clear_op_id,
            result: Err(crate::recorder::ports::DbCommitFailure::Transient {
                message: "sqlite busy".into(),
            }),
        },
    );
    assert_eq!(fsm.errors.consecutive_error_count, 2);

    let retry_effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadHeartbeat {
            session_id: "sess-err".into(),
            download_id: "dl-1".into(),
            progress: Some(DownloadProgress {
                bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES * 3,
                speed_bytes_per_sec: 500_000,
                ..DownloadProgress::default()
            }),
        },
    );
    let (retry_op_id, retry_op) = find_db_commit(&retry_effects).unwrap();
    assert!(matches!(retry_op, DbCommitOp::ClearErrors { .. }));

    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: retry_op_id,
            result: Ok(DbCommitOutcome::ErrorsCleared {
                checks_restored_emitted: false,
            }),
        },
    );
    assert_eq!(fsm.errors.consecutive_error_count, 0);
}

#[test]
fn out_of_space_suppression_preserves_cooldown_and_offline_does_not_overwrite() {
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
    fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op,
            result: Ok(DbCommitOutcome::SessionStartSuppressed {
                state: StreamerState::OutOfSpace,
                retry_after: Some(Duration::from_secs(30)),
            }),
        },
    );
    assert_eq!(
        fsm.monitor,
        MonitorPhase::Blocked {
            kind: BlockedKind::OutputRootCooldown,
            retry_at: clock.now + Duration::from_secs(30),
        }
    );

    // When cooldown expires and the streamer is now offline (with initial reconcile
    // already cleared), an offline probe must not dispatch `EndSession` to overwrite
    // `OutOfSpace` with `NotLive`.
    fsm.initial_reconcile_pending = false;
    clock.advance(Duration::from_secs(30));
    let p2 = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: p2,
            result: Ok(ProbeStatus::Offline {
                batch_live_hint_armed: false,
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());
}
