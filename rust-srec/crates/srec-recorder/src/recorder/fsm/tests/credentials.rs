use std::time::Duration;

use crate::credentials::UnavailableReason;
use crate::downloader::DownloadFailureKind;
use crate::downloader::engine::{DownloadProtocol, EngineType};
use crate::recorder::fsm::credentials::{RECOVERED_MEDIA_TTL, deterministic_retry_delay};
use crate::recorder::fsm::schedule::{STALL_WINDOW, WATCHDOG_ERROR_BACKOFF};
use crate::recorder::fsm::state::{AttemptPhase, FsmInput, MonitorPhase, SessionSlot, StreamerFsm};
use crate::recorder::ports::{
    AttemptStartFailure, DbCommitFailure, DbCommitOp, DbCommitOutcome, ProbeFailure, ProbePurpose,
    ProbeStatus, SessionStartDisposition,
};
use srec_core::domain::{RetryPolicy, StreamerState};

use super::{
    TestClock, base_config, base_snapshot, failed_event, find_db_commit, find_probe,
    find_start_attempt, live_payload, open_running_session, sample_binding, sample_snapshot,
};

#[test]
fn credential_diagnostic_commits_authoritative_epoch_enforces_30s_ttl_and_wakes_from_waiting() {
    // Verify deterministic retry delay jitter scaling and max_retries cap.
    let jitter_policy = RetryPolicy {
        max_retries: 2,
        initial_delay_ms: 4_000,
        max_delay_ms: 10_000,
        backoff_multiplier: 2.0,
        use_jitter: true,
    };
    assert_eq!(
        deterministic_retry_delay(&jitter_policy, 0, 9_000),
        Duration::from_secs(4)
    );
    assert_eq!(
        deterministic_retry_delay(&jitter_policy, 0, 11_000),
        Duration::from_secs(5)
    );
    assert_eq!(
        deterministic_retry_delay(&jitter_policy, 2, 10_000),
        Duration::from_secs(10)
    );

    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.managed_credentials = true;
    cfg.download_retry_policy = RetryPolicy {
        max_retries: 3,
        initial_delay_ms: 2_000,
        max_delay_ms: 30_000,
        backoff_multiplier: 2.0,
        use_jitter: false,
    };

    let binding_v1 = sample_binding("prof-1", 1);
    let (mut fsm, attempt_id) =
        open_running_session(&clock, cfg.clone(), "sess-cred", Some(binding_v1));

    // HTTP 403 triggers credential diagnostic with 2s backoff.
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id,
            event: failed_event(
                "sess-cred",
                "dl-1",
                EngineType::Mesio,
                DownloadProtocol::Hls,
                DownloadFailureKind::HttpClientError { status: 403 },
                "403 forbidden",
            ),
        },
    );
    assert!(find_db_commit(&effects).is_none());
    assert_eq!(
        fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + Duration::from_secs(2))
    );

    clock.advance(Duration::from_secs(2));
    let req = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .clone();
    assert_eq!(req.purpose, ProbePurpose::Recovery);

    // Recovery Live probe commits binding BEFORE launching attempt.
    let rotated = sample_binding("prof-2", 0);
    let mut recovery_payload = live_payload("Live Title");
    recovery_payload.credential_binding = Some(rotated.clone());
    recovery_payload.credential_snapshot = Some(sample_snapshot(rotated));
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: req.probe_id,
            result: Ok(ProbeStatus::Live(recovery_payload)),
        },
    );
    assert!(find_start_attempt(&effects).is_none());
    let (bind_op_id, bind_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        bind_op,
        DbCommitOp::CommitCredentialBinding { .. }
    ));

    // DB returns authoritative epoch = 9 within 30s -> reuses RecoveredMedia.
    clock.advance(Duration::from_secs(5));
    let committed_binding = sample_binding("prof-2", 9);
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: bind_op_id,
            result: Ok(DbCommitOutcome::CredentialBindingCommitted {
                session_id: "sess-cred".into(),
                binding: committed_binding.clone(),
            }),
        },
    );
    let spec = find_start_attempt(&effects).unwrap();
    assert!(spec.using_recovered_media);
    assert!(spec.payload.is_some());
    assert_eq!(spec.credential_binding, Some(committed_binding.clone()));
    let attempt2 = spec.attempt_id;

    // CredentialsUnavailable parks in WaitingCredentials; CredentialMaterialChanged wakes with committed binding.
    fsm.step(
        clock.ctx(),
        FsmInput::AttemptStartFailed {
            attempt_id: attempt2,
            session_id: "sess-cred".into(),
            reason: AttemptStartFailure::CredentialsUnavailable,
        },
    );
    assert!(matches!(
        &fsm.session,
        SessionSlot::Open(open) if matches!(open.attempt, AttemptPhase::WaitingCredentials)
    ));
    let effects = fsm.step(clock.ctx(), FsmInput::CredentialMaterialChanged);
    let resumed_spec = find_start_attempt(&effects).unwrap();
    assert!(!resumed_spec.using_recovered_media);
    assert_eq!(resumed_spec.credential_binding, Some(committed_binding));

    // Verify 30s RecoveredMedia TTL expiry end-to-end through `step()`.
    let mut clock_exp = TestClock::new();
    let (mut fsm_exp, attempt_exp) = open_running_session(
        &clock_exp,
        cfg,
        "sess-ttl",
        Some(sample_binding("prof-1", 1)),
    );
    fsm_exp.step(
        clock_exp.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt_exp,
            event: failed_event(
                "sess-ttl",
                "dl-1",
                EngineType::Mesio,
                DownloadProtocol::Hls,
                DownloadFailureKind::HttpClientError { status: 403 },
                "403",
            ),
        },
    );
    clock_exp.advance(Duration::from_secs(2));
    let req_exp = find_probe(&fsm_exp.step(clock_exp.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let rot_exp = sample_binding("prof-3", 0);
    let mut payload_exp = live_payload("Live");
    payload_exp.credential_binding = Some(rot_exp.clone());
    payload_exp.credential_snapshot = Some(sample_snapshot(rot_exp));
    let (bind_exp_op, _) = find_db_commit(&fsm_exp.step(
        clock_exp.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: req_exp,
            result: Ok(ProbeStatus::Live(payload_exp)),
        },
    ))
    .unwrap();

    // Advance >= 30s before DB returns -> RecoveredMedia expires and is NOT reused.
    clock_exp.advance(RECOVERED_MEDIA_TTL);
    let committed_exp = sample_binding("prof-3", 4);
    let effects = fsm_exp.step(
        clock_exp.ctx(),
        FsmInput::DbCommitted {
            op_id: bind_exp_op,
            result: Ok(DbCommitOutcome::CredentialBindingCommitted {
                session_id: "sess-ttl".into(),
                binding: committed_exp.clone(),
            }),
        },
    );
    let expired_spec = find_start_attempt(&effects).unwrap();
    assert!(!expired_spec.using_recovered_media);
    assert!(expired_spec.payload.is_none());
    assert_eq!(expired_spec.credential_binding, Some(committed_exp));
}

#[test]
fn credential_recovery_loop_guards_failure_fallbacks_and_config_wake() {
    let mut clock = TestClock::new();
    let mut cfg = base_config();
    cfg.managed_credentials = true;
    cfg.download_retry_policy = RetryPolicy {
        max_retries: 1,
        initial_delay_ms: 1_000,
        max_delay_ms: 5_000,
        backoff_multiplier: 2.0,
        use_jitter: false,
    };

    // 1. CredentialsChanged: 1st startup exit retries once immediately; 2nd parks in WaitingCredentials;
    //    ConfigUpdated wakes WaitingCredentials into a new attempt.
    let mut fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, cfg.clone()),
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
    let attempt1 = find_start_attempt(&fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: start_op,
            result: Ok(DbCommitOutcome::SessionStarted {
                session_id: "sess-cc".into(),
                started_at: clock.wall,
                disposition: SessionStartDisposition::Created,
                credential_binding: Some(sample_binding("prof-1", 1)),
            }),
        },
    ))
    .unwrap()
    .attempt_id;

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::AttemptStartFailed {
            attempt_id: attempt1,
            session_id: "sess-cc".into(),
            reason: AttemptStartFailure::CredentialsChanged,
        },
    );
    let attempt2 = find_start_attempt(&effects).unwrap().attempt_id;

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::AttemptStartFailed {
            attempt_id: attempt2,
            session_id: "sess-cc".into(),
            reason: AttemptStartFailure::CredentialsChanged,
        },
    );
    assert!(find_start_attempt(&effects).is_none());
    assert!(matches!(
        &fsm.session,
        SessionSlot::Open(open) if matches!(open.attempt, AttemptPhase::WaitingCredentials)
    ));

    let effects = fsm.step(
        clock.ctx(),
        FsmInput::ConfigUpdated {
            revision: 2,
            config: Box::new(cfg.clone()),
        },
    );
    let attempt3 = find_start_attempt(&effects).unwrap().attempt_id;

    // 2. CommitCredentialBinding conflict falls back to QuietPeriod + RecordTransientError, and
    //    diagnostic max_retries=1 prevents a second diagnostic loop on subsequent failure.
    fsm.step(
        clock.ctx(),
        FsmInput::AttemptStarted {
            attempt_id: attempt3,
            session_id: "sess-cc".into(),
            download_id: "dl-3".into(),
            engine_type: EngineType::Mesio,
            protocol: DownloadProtocol::Hls,
        },
    );
    fsm.step(
        clock.ctx(),
        FsmInput::DownloadEnded {
            attempt_id: attempt3,
            event: failed_event(
                "sess-cc",
                "dl-3",
                EngineType::Mesio,
                DownloadProtocol::Hls,
                DownloadFailureKind::HttpClientError { status: 401 },
                "401 unauthorized",
            ),
        },
    );
    clock.advance(Duration::from_secs(1));
    let rec_probe = find_probe(&fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let mut rec_payload = live_payload("Live");
    rec_payload.credential_binding = Some(sample_binding("prof-2", 0));
    let (bind_op, _) = find_db_commit(&fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: rec_probe,
            result: Ok(ProbeStatus::Live(rec_payload)),
        },
    ))
    .unwrap();
    let effects = fsm.step(
        clock.ctx(),
        FsmInput::DbCommitted {
            op_id: bind_op,
            result: Err(DbCommitFailure::CredentialBindingConflict {
                message: "policy changed".into(),
            }),
        },
    );
    let (_, fallback_op) = find_db_commit(&effects).unwrap();
    assert!(matches!(
        fallback_op,
        DbCommitOp::RecordHysteresisEntered { .. }
    ));
    assert!(matches!(
        &fsm.session,
        SessionSlot::Open(open) if open.quiet_period.is_some() && open.diagnostic_attempts == 1
    ));
}

#[test]
fn credential_unavailable_and_source_changed_bypass_error_tracking_in_idle_and_watchdog() {
    let mut clock = TestClock::new();

    // In Idle (non-watchdog), CredentialUnavailable and CredentialSourceChanged must NOT increment
    // errors or emit RecordTransientError (unlike ProbeFailure::Transient).
    let mut idle_fsm = StreamerFsm::new(
        base_snapshot(StreamerState::NotLive, base_config()),
        clock.now,
        clock.wall,
    );
    for failure in [
        ProbeFailure::CredentialUnavailable {
            reason: UnavailableReason::ProfilesDisabled,
            policy_generation: "gen-1".into(),
        },
        ProbeFailure::CredentialSourceChanged,
    ] {
        let p = find_probe(&idle_fsm.step(clock.ctx(), FsmInput::TimerFired))
            .unwrap()
            .probe_id;
        let effects = idle_fsm.step(
            clock.ctx(),
            FsmInput::ProbeCompleted {
                probe_id: p,
                result: Err(failure),
            },
        );
        assert!(find_db_commit(&effects).is_none());
        assert_eq!(idle_fsm.errors.consecutive_error_count, 0);
        assert!(matches!(idle_fsm.monitor, MonitorPhase::Idle { .. }));
        clock.advance(Duration::from_secs(60));
    }

    // In LiveWatchdog, CredentialUnavailable sets 60s watchdog floor without tearing down LiveWatchdog.
    let (mut wd_fsm, _) = open_running_session(&clock, base_config(), "sess-cu", None);
    clock.advance(STALL_WINDOW);
    let req = find_probe(&wd_fsm.step(clock.ctx(), FsmInput::TimerFired))
        .unwrap()
        .probe_id;
    let effects = wd_fsm.step(
        clock.ctx(),
        FsmInput::ProbeCompleted {
            probe_id: req,
            result: Err(ProbeFailure::CredentialUnavailable {
                reason: UnavailableReason::ProfilesDisabled,
                policy_generation: "gen-1".into(),
            }),
        },
    );
    assert!(find_db_commit(&effects).is_none());
    assert_eq!(wd_fsm.errors.consecutive_error_count, 0);
    assert!(matches!(wd_fsm.monitor, MonitorPhase::LiveWatchdog { .. }));
    assert_eq!(
        wd_fsm.next_deadline(clock.now, clock.wall),
        Some(clock.now + WATCHDOG_ERROR_BACKOFF)
    );
}
