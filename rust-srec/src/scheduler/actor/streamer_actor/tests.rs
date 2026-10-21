use super::*;

#[tokio::test]
async fn config_messages_preserve_recurring_and_authoritative_retry_deadlines() {
    let config = create_test_config();
    let (mut actor, _) = StreamerActor::new(
        "test-streamer".to_owned(),
        create_test_metadata_store(),
        config.clone(),
        CancellationToken::new(),
        create_noop_checker(),
    );
    actor.state.schedule_next_check(&config, 0);
    let recurring = actor.state.next_check;
    actor
        .handle_config_update(StreamerConfig {
            priority: Priority::High,
            ..config.clone()
        })
        .await
        .unwrap();
    assert_eq!(actor.state.next_check, recurring);
    actor.schedule_blocked_retry(StreamerState::TemporalDisabled, 30);
    let retry = actor.state.next_check;
    actor
        .handle_config_update(StreamerConfig {
            check_interval_ms: 1,
            ..config
        })
        .await
        .unwrap();
    assert_eq!(
        actor.state.next_check, retry,
        "a shorter configured interval cannot advance an admission cooldown"
    );
}
use crate::domain::Priority;
use crate::monitor::{ProcessStatusResult, ProcessStatusSuppression};
use crate::scheduler::actor::monitor_adapter::{CheckError, NoOpStatusChecker};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

struct UnavailableCredentialChecker {
    errors: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl StatusChecker for UnavailableCredentialChecker {
    async fn check_status(
        &self,
        _streamer: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        Err(
            crate::Error::CredentialUnavailable(crate::credentials::CredentialUnavailable {
                reason: crate::credentials::UnavailableReason::LoginRequired,
                policy_generation: "fixture".into(),
            })
            .into(),
        )
    }
    async fn process_status(
        &self,
        _streamer: &StreamerMetadata,
        _status: LiveStatus,
    ) -> Result<ProcessStatusResult, CheckError> {
        panic!("unavailability is not an offline status")
    }
    async fn handle_error(
        &self,
        _streamer: &StreamerMetadata,
        _error: &str,
    ) -> Result<(), CheckError> {
        self.errors
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn set_infra_blocked(
        &self,
        _streamer: &StreamerMetadata,
        _reason: crate::monitor::InfraBlockReason,
    ) -> Result<(), CheckError> {
        panic!("credentials must not disable the streamer")
    }
}

#[tokio::test]
async fn repeated_credential_exhaustion_preserves_live_state_and_bypasses_error_circuit() {
    let checker = Arc::new(UnavailableCredentialChecker {
        errors: std::sync::atomic::AtomicUsize::new(0),
    });
    let (mut actor, _) = StreamerActor::new(
        "test-streamer".into(),
        create_test_metadata_store(),
        create_test_config(),
        CancellationToken::new(),
        checker.clone(),
    );
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    for _ in 0..12 {
        actor.perform_check().await.unwrap();
    }
    assert_eq!(actor.state.streamer_state, StreamerState::Live);
    assert!(actor.state.hysteresis.was_live());
    assert_eq!(checker.errors.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(actor.get_error_count(), 0);
    assert!(actor.live_watchdog_backoff_until.is_some());
}

fn create_test_metadata() -> StreamerMetadata {
    StreamerMetadata {
        id: "test-streamer".to_string(),
        name: "Test Streamer".to_string(),
        url: "https://twitch.tv/test".to_string(),
        platform_config_id: "twitch".to_string(),
        template_config_id: None,
        state: StreamerState::NotLive,
        priority: Priority::Normal,
        avatar_url: None,
        consecutive_error_count: 0,
        disabled_until: None,
        last_live_time: None,
        last_error: None,
        streamer_specific_config: None,
        offline_check_count: 3,
        offline_check_delay_ms: 20_000,
        created_at: chrono::Utc::now(),
        deleted_at: None,
        updated_at: chrono::Utc::now(),
    }
}

fn create_test_metadata_store() -> Arc<DashMap<String, Arc<StreamerMetadata>>> {
    let store = Arc::new(DashMap::new());
    let metadata = create_test_metadata();
    store.insert(metadata.id.clone(), Arc::new(metadata));
    store
}

fn create_test_config() -> StreamerConfig {
    StreamerConfig {
        check_interval_ms: 1000, // 1 second for tests
        offline_check_interval_ms: 500,
        offline_check_count: 3,
        priority: Priority::Normal,
    }
}

fn create_noop_checker() -> Arc<dyn StatusChecker> {
    Arc::new(NoOpStatusChecker)
}

#[derive(Debug)]
struct SequenceStatusChecker {
    checks: Mutex<VecDeque<(CheckResult, LiveStatus)>>,
    outcomes: Mutex<VecDeque<Result<ProcessStatusResult, CheckError>>>,
}

impl SequenceStatusChecker {
    fn new(checks: Vec<(CheckResult, LiveStatus)>, outcomes: Vec<ProcessStatusResult>) -> Self {
        Self {
            checks: Mutex::new(VecDeque::from(checks)),
            outcomes: Mutex::new(outcomes.into_iter().map(Ok).collect()),
        }
    }
}

#[async_trait]
impl StatusChecker for SequenceStatusChecker {
    async fn check_status(
        &self,
        _streamer: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        self.checks
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| CheckError::transient("missing check result"))
    }

    async fn process_status(
        &self,
        _streamer: &StreamerMetadata,
        _status: LiveStatus,
    ) -> Result<ProcessStatusResult, CheckError> {
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(ProcessStatusResult::Applied))
    }

    async fn handle_error(
        &self,
        _streamer: &StreamerMetadata,
        _error: &str,
    ) -> Result<(), CheckError> {
        Ok(())
    }

    async fn set_infra_blocked(
        &self,
        _streamer: &StreamerMetadata,
        _reason: crate::monitor::InfraBlockReason,
    ) -> Result<(), CheckError> {
        Ok(())
    }
}

#[tokio::test]
async fn initial_offline_reconciliation_retries_until_applied_then_suppresses_duplicates() {
    let checker = Arc::new(SequenceStatusChecker::new(
        (0..4)
            .map(|_| {
                (
                    CheckResult::success(StreamerState::NotLive),
                    LiveStatus::Offline,
                )
            })
            .collect(),
        vec![
            ProcessStatusResult::Suppressed(ProcessStatusSuppression::TemporarilyDisabled {
                retry_after: Some(Duration::from_secs(1)),
            }),
            ProcessStatusResult::Applied,
            ProcessStatusResult::Applied,
        ],
    ));
    checker
        .outcomes
        .lock()
        .unwrap()
        .push_front(Err(CheckError::transient("database busy")));
    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        create_test_metadata_store(),
        create_test_config(),
        CancellationToken::new(),
        checker.clone(),
    );
    // Each row is `(initial_status_pending, process_status outcomes left)`
    // after one offline check: the failed write and the suppression keep the
    // reconciliation pending, `Applied` settles it, and a further offline
    // result is not reported again.
    for (pending, remaining) in [(true, 3), (true, 2), (false, 1), (false, 1)] {
        actor.perform_check().await.unwrap();
        assert_eq!(actor.initial_status_pending, pending);
        assert_eq!(checker.outcomes.lock().unwrap().len(), remaining);
        assert!(actor.state.next_check.is_some());
    }
}

#[tokio::test]
async fn live_actor_retries_failed_offline_at_grace_threshold() {
    let mut previous_metadata = create_test_metadata();
    previous_metadata.state = StreamerState::Live;
    let previous_state = StreamerActorState::from_metadata(&previous_metadata);
    let checker = Arc::new(SequenceStatusChecker::new(
        (0..5)
            .map(|_| {
                (
                    CheckResult::success(StreamerState::NotLive),
                    LiveStatus::Offline,
                )
            })
            .collect(),
        vec![ProcessStatusResult::Applied, ProcessStatusResult::Applied],
    ));
    checker
        .outcomes
        .lock()
        .unwrap()
        .push_front(Err(CheckError::transient("database busy")));
    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        create_test_metadata_store(),
        create_test_config(),
        CancellationToken::new(),
        checker.clone(),
    );
    actor.state = previous_state;
    assert_eq!(actor.state.streamer_state, StreamerState::Live);
    // Each row is `(initial_status_pending, process_status outcomes left)`
    // after one offline check: the first two stay inside the offline grace
    // count without reporting, the third reports and fails, the fourth
    // reports again and is applied, and the fifth is not reported.
    for (pending, remaining) in [(true, 3), (true, 3), (true, 2), (false, 1), (false, 1)] {
        actor.perform_check().await.unwrap();
        assert_eq!(actor.initial_status_pending, pending);
        assert_eq!(checker.outcomes.lock().unwrap().len(), remaining);
    }
}

#[tokio::test]
async fn test_priority_channel_processes_before_normal() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, handle) = StreamerActor::with_priority_channel(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token.clone(),
        create_noop_checker(),
    );

    // Spawn actor
    let actor_task = tokio::spawn(async move { actor.run().await });

    // Send multiple normal messages first
    for _ in 0..5 {
        handle
            .send(StreamerMessage::ConfigUpdate(create_test_config()))
            .await
            .unwrap();
    }

    // Send stop via priority channel - should be processed promptly
    handle.send_priority(StreamerMessage::Stop).await.unwrap();

    // Actor should stop quickly despite pending normal messages
    let result = tokio::time::timeout(Duration::from_millis(500), actor_task)
        .await
        .unwrap()
        .unwrap();

    assert!(matches!(result, Ok(ActorOutcome::Stopped)));
}

/// A live-watchdog check that still sees Live while download heartbeats
/// have been silent for the stall window must NOT be swallowed by the
/// `(Live, Live)` hysteresis suppression — it must call `process_status`
/// so the monitor can re-drive the download (a dead session otherwise
/// stays unrecovered: no download → no DownloadEnded → no transition).
#[tokio::test]
async fn test_live_watchdog_stall_forces_live_reemit() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let checker = Arc::new(SequenceStatusChecker::new(
        vec![(
            CheckResult::success(StreamerState::Live),
            LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Stalled Live".to_string(),
                category: None,
                started_at: None,
                viewer_count: None,
                avatar: None,
                streams: vec![],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            },
        )],
        vec![ProcessStatusResult::Applied],
    ));

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        checker.clone() as Arc<dyn StatusChecker>,
    );

    // Wedged shape: Live with no scheduled check (watchdog mode) and no
    // download heartbeat ever observed.
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;
    actor.state.last_download_activity_at = None;

    actor.perform_check().await.unwrap();

    // process_status consumed its queued outcome — the Live result was
    // re-emitted instead of suppressed.
    assert!(
        checker.outcomes.lock().unwrap().is_empty(),
        "stalled live watchdog must call process_status"
    );
    assert_eq!(actor.state.streamer_state, StreamerState::Live);
    assert!(actor.state.last_download_activity_at.is_some());
    assert!(actor.state.hysteresis.was_live());
}

/// The reconciliation must not fire while a download is healthy: fresh
/// heartbeats mean the `(Live, Live)` suppression is doing its intended
/// job (avoiding redundant monitor writes on the 2h watchdog).
#[tokio::test]
async fn test_live_watchdog_with_fresh_heartbeats_keeps_suppression() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let checker = Arc::new(SequenceStatusChecker::new(
        vec![(
            CheckResult::success(StreamerState::Live),
            LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Healthy Live".to_string(),
                category: None,
                started_at: None,
                viewer_count: None,
                avatar: None,
                streams: vec![],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            },
        )],
        vec![ProcessStatusResult::Applied],
    ));

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        checker.clone() as Arc<dyn StatusChecker>,
    );

    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;
    // Heartbeat just arrived — download is alive.
    actor.state.last_download_activity_at = Some(Instant::now());

    actor.perform_check().await.unwrap();

    assert_eq!(
        checker.outcomes.lock().unwrap().len(),
        1,
        "healthy live watchdog must keep the (Live, Live) suppression"
    );
    assert_eq!(actor.state.streamer_state, StreamerState::Live);
    assert!(actor.state.next_check.is_none());
}

#[tokio::test]
async fn test_suppressed_live_restores_notlive_grace_hysteresis_context() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let checker: Arc<dyn StatusChecker> = Arc::new(SequenceStatusChecker::new(
        vec![(
            CheckResult::success(StreamerState::Live),
            LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Suppressed Live".to_string(),
                category: None,
                started_at: None,
                viewer_count: None,
                avatar: None,
                streams: vec![],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            },
        )],
        vec![ProcessStatusResult::Suppressed(
            ProcessStatusSuppression::TemporarilyDisabled {
                retry_after: Some(Duration::from_secs(30)),
            },
        )],
    ));

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config.clone(),
        token,
        checker,
    );

    actor.state.streamer_state = StreamerState::NotLive;
    actor.state.hysteresis.mark_live();
    actor.state.hysteresis.mark_offline_observed();
    let original_offline_count = actor.state.hysteresis.offline_count();
    actor.state.last_check = Some(CheckResult {
        state: StreamerState::NotLive,
        stream_url: None,
        title: Some("Previous offline".to_string()),
        checked_at: chrono::Utc::now(),
        error: None,
        next_check_hint: None,
    });

    actor.perform_check().await.unwrap();

    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.hysteresis.was_live());
    assert_eq!(
        actor.state.hysteresis.offline_count(),
        original_offline_count
    );
    assert_eq!(
        actor
            .state
            .last_check
            .as_ref()
            .and_then(|check| check.title.as_deref()),
        Some("Previous offline")
    );
    let until = actor.state.time_until_next_check().unwrap();
    assert!(until <= Duration::from_secs(30));
}

#[tokio::test]
async fn test_suppressed_live_restores_out_of_schedule_smart_wake_context() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let checker: Arc<dyn StatusChecker> = Arc::new(SequenceStatusChecker::new(
        vec![(
            CheckResult::success(StreamerState::Live),
            LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Suppressed Live".to_string(),
                category: None,
                started_at: None,
                viewer_count: None,
                avatar: None,
                streams: vec![],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            },
        )],
        vec![ProcessStatusResult::Suppressed(
            ProcessStatusSuppression::TemporarilyDisabled {
                retry_after: Some(Duration::from_secs(30)),
            },
        )],
    ));

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        checker,
    );

    let smart_wake_hint = chrono::Utc::now() + chrono::Duration::minutes(15);
    actor.state.streamer_state = StreamerState::OutOfSchedule;
    actor.state.hysteresis.mark_live();
    actor.state.last_check = Some(CheckResult {
        state: StreamerState::OutOfSchedule,
        stream_url: None,
        title: Some("Out of schedule".to_string()),
        checked_at: chrono::Utc::now(),
        error: None,
        next_check_hint: Some(smart_wake_hint),
    });

    actor.perform_check().await.unwrap();

    assert_eq!(actor.state.streamer_state, StreamerState::OutOfSchedule);
    assert!(actor.state.hysteresis.was_live());
    assert_eq!(
        actor
            .state
            .last_check
            .as_ref()
            .and_then(|check| check.next_check_hint),
        Some(smart_wake_hint)
    );
    let until = actor.state.time_until_next_check().unwrap();
    assert!(until <= Duration::from_secs(30));
}

/// Panics on any call, for paths that must not reach the monitor at all.
#[derive(Debug, Default)]
struct AssertNotCalledStatusChecker;

#[async_trait]
impl StatusChecker for AssertNotCalledStatusChecker {
    async fn check_status(
        &self,
        _streamer: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        panic!("AssertNotCalledStatusChecker::check_status must not be called");
    }

    async fn process_status(
        &self,
        _streamer: &StreamerMetadata,
        _status: LiveStatus,
    ) -> Result<ProcessStatusResult, CheckError> {
        panic!("AssertNotCalledStatusChecker::process_status must not be called");
    }

    async fn handle_error(
        &self,
        _streamer: &StreamerMetadata,
        _error: &str,
    ) -> Result<(), CheckError> {
        panic!("AssertNotCalledStatusChecker::handle_error must not be called");
    }

    async fn set_infra_blocked(
        &self,
        _streamer: &StreamerMetadata,
        _reason: crate::monitor::InfraBlockReason,
    ) -> Result<(), CheckError> {
        panic!("AssertNotCalledStatusChecker::set_infra_blocked must not be called");
    }
}

#[tokio::test]
async fn fatal_timer_and_mailbox_errors_stop_gracefully() {
    for trigger in ["timer", "normal", "priority", "late-priority"] {
        let metadata = create_test_metadata_store();
        let (mut actor, handle) = StreamerActor::with_priority_channel(
            "test-streamer".to_owned(),
            metadata.clone(),
            create_test_config(),
            CancellationToken::new(),
            Arc::new(AssertNotCalledStatusChecker),
        );
        actor.state.next_check = Some(Instant::now() + Duration::from_secs(3600));
        let message = StreamerMessage::DownloadEnded(DownloadEndPolicy::StreamerOffline);
        if trigger != "late-priority" {
            metadata.remove("test-streamer");
        }
        match trigger {
            "timer" => actor.state.next_check = Some(Instant::now()),
            "normal" => handle.send(message).await.unwrap(),
            "priority" => handle.send_priority(message).await.unwrap(),
            _ => {}
        }
        let task = tokio::spawn(actor.run());
        if trigger == "late-priority" {
            let (reply, received) = tokio::sync::oneshot::channel();
            handle.send(StreamerMessage::GetState(reply)).await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), received)
                .await
                .unwrap()
                .unwrap();
            metadata.remove("test-streamer");
            handle
                .send_priority(StreamerMessage::DownloadEnded(
                    DownloadEndPolicy::StreamerOffline,
                ))
                .await
                .unwrap();
        }
        let outcome = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.unwrap(), ActorOutcome::Stopped, "{trigger}");
    }
}

#[tokio::test]
async fn orchestration_stops_and_unknown_causes_never_publish_offline() {
    for cause in [
        DownloadStopCause::User,
        DownloadStopCause::Shutdown,
        DownloadStopCause::StreamerDisabled,
        DownloadStopCause::Other("internal stop".to_owned()),
    ] {
        let park = matches!(
            cause,
            DownloadStopCause::Shutdown | DownloadStopCause::StreamerDisabled
        );
        let (mut actor, _handle) = StreamerActor::new(
            "test-streamer".to_owned(),
            create_test_metadata_store(),
            create_test_config(),
            CancellationToken::new(),
            Arc::new(AssertNotCalledStatusChecker),
        );
        actor.state.streamer_state = StreamerState::Live;
        actor.state.hysteresis.mark_live();
        actor.state.last_download_activity_at = Some(Instant::now());
        actor
            .handle_download_ended(DownloadEndPolicy::Stopped(cause))
            .await
            .unwrap();
        assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
        assert!(
            actor.state.hysteresis.was_live(),
            "an orchestration stop is not an offline observation"
        );
        assert_eq!(actor.state.hysteresis.offline_count(), 0);
        assert!(actor.state.last_download_activity_at.is_none());
        assert_eq!(actor.state.next_check.is_none(), park);
    }
}

#[tokio::test]
async fn authoritative_offline_still_emits_exactly_one_monitor_signal() {
    for reason in [
        DownloadEndPolicy::StreamerOffline,
        DownloadEndPolicy::Stopped(DownloadStopCause::StreamerOffline),
    ] {
        let checker = Arc::new(SequenceStatusChecker::new(
            vec![],
            vec![ProcessStatusResult::Applied, ProcessStatusResult::Applied],
        ));
        let (mut actor, _handle) = StreamerActor::new(
            "test-streamer".to_owned(),
            create_test_metadata_store(),
            create_test_config(),
            CancellationToken::new(),
            checker.clone(),
        );
        actor.state.streamer_state = StreamerState::Live;
        actor.state.hysteresis.mark_live();
        actor.handle_download_ended(reason).await.unwrap();
        assert_eq!(
            checker.outcomes.lock().unwrap().len(),
            1,
            "offline must reach process_status exactly once"
        );
        assert_eq!(actor.state.hysteresis.offline_count(), 1);
        assert!(actor.state.next_check.is_some());
    }
}
#[tokio::test]
async fn reliable_feedback_maps_all_terminal_and_rejection_categories() {
    use crate::downloader::engine::EngineType;
    use crate::downloader::{
        DownloadFailureKind, DownloadManagerEvent, DownloadProtocol, DownloadRejectedKind,
        DownloadTerminalEvent, EngineEndSignal,
    };
    use crate::scheduler::feedback::FeedbackDisposition;
    let completed = |cause| DownloadTerminalEvent::Completed {
        download_id: "d".to_owned(),
        streamer_id: "test-streamer".to_owned(),
        streamer_name: "Test".to_owned(),
        session_id: "s".to_owned(),
        total_bytes: 1,
        total_duration_secs: 1.0,
        total_segments: 1,
        file_path: None,
        engine_signal: EngineEndSignal::CleanDisconnect,
        stop_cause: cause,
    };
    let rejected = |kind| DownloadTerminalEvent::Rejected {
        streamer_id: "test-streamer".to_owned(),
        streamer_name: "Test".to_owned(),
        session_id: "s".to_owned(),
        reason: "fixture".to_owned(),
        retry_after_secs: Some(60),
        kind,
    };
    let mut cases = vec![
        (completed(None), StreamerState::NotLive, true),
        (
            completed(Some(DownloadStopCause::OutOfSchedule)),
            StreamerState::OutOfSchedule,
            true,
        ),
        (
            completed(Some(DownloadStopCause::Shutdown)),
            StreamerState::NotLive,
            false,
        ),
        (
            rejected(DownloadRejectedKind::CircuitBreaker),
            StreamerState::TemporalDisabled,
            true,
        ),
        (
            rejected(DownloadRejectedKind::OutputRootUnavailable {
                path: "/recordings".into(),
                io_kind: crate::downloader::IoErrorKindSer::StorageFull,
            }),
            StreamerState::OutOfSpace,
            true,
        ),
        (
            rejected(DownloadRejectedKind::StreamerBackoff),
            StreamerState::TemporalDisabled,
            true,
        ),
        (
            DownloadTerminalEvent::Failed {
                download_id: "d".to_owned(),
                streamer_id: "test-streamer".to_owned(),
                streamer_name: "Test".to_owned(),
                session_id: "s".to_owned(),
                engine_type: EngineType::Ffmpeg,
                protocol: DownloadProtocol::Flv,
                kind: DownloadFailureKind::Other,
                error: "fixture".to_owned(),
                recoverable: true,
            },
            StreamerState::NotLive,
            true,
        ),
    ];
    for cause in [
        DownloadStopCause::Shutdown,
        DownloadStopCause::StreamerDisabled,
        DownloadStopCause::StreamerOffline,
        DownloadStopCause::DanmuStreamClosed,
        DownloadStopCause::OutOfSchedule,
        DownloadStopCause::Other("fixture".to_owned()),
        DownloadStopCause::User,
    ] {
        let scheduled = !matches!(
            cause,
            DownloadStopCause::Shutdown | DownloadStopCause::StreamerDisabled
        );
        let expected = if matches!(cause, DownloadStopCause::OutOfSchedule) {
            StreamerState::OutOfSchedule
        } else {
            StreamerState::NotLive
        };
        cases.push((
            DownloadTerminalEvent::Cancelled {
                download_id: "d".to_owned(),
                streamer_id: "test-streamer".to_owned(),
                streamer_name: "Test".to_owned(),
                session_id: "s".to_owned(),
                cause,
            },
            expected,
            scheduled,
        ));
    }
    for (terminal, expected, scheduled) in cases {
        let (mut actor, _handle) = StreamerActor::new(
            "test-streamer".to_owned(),
            create_test_metadata_store(),
            create_test_config(),
            CancellationToken::new(),
            create_noop_checker(),
        );
        assert_eq!(
            actor
                .apply_lifecycle_feedback(1, DownloadManagerEvent::Terminal(terminal))
                .await
                .unwrap(),
            FeedbackDisposition::Applied
        );
        assert_eq!(actor.state.streamer_state, expected);
        assert_eq!(actor.state.next_check.is_some(), scheduled);
    }
}

#[derive(Default)]
struct PolicyChecker {
    effects: std::sync::Mutex<Vec<String>>,
    fail_effects: bool,
    fail_check: bool,
}

#[async_trait::async_trait]
impl StatusChecker for PolicyChecker {
    async fn check_status(
        &self,
        _: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), super::super::monitor_adapter::CheckError> {
        if self.fail_check {
            Err(super::super::monitor_adapter::CheckError::transient(
                "watchdog fixture",
            ))
        } else {
            Ok((
                CheckResult::success(StreamerState::NotLive),
                LiveStatus::Offline,
            ))
        }
    }
    async fn process_status(
        &self,
        _: &StreamerMetadata,
        status: LiveStatus,
    ) -> Result<ProcessStatusResult, super::super::monitor_adapter::CheckError> {
        assert!(matches!(status, LiveStatus::Offline));
        self.effects.lock().unwrap().push("offline".into());
        if self.fail_effects {
            Err(super::super::monitor_adapter::CheckError::transient(
                "persist fixture",
            ))
        } else {
            Ok(ProcessStatusResult::Applied)
        }
    }
    async fn handle_error(
        &self,
        _: &StreamerMetadata,
        _: &str,
    ) -> Result<(), super::super::monitor_adapter::CheckError> {
        self.effects.lock().unwrap().push("error".into());
        Ok(())
    }
    async fn set_infra_blocked(
        &self,
        _: &StreamerMetadata,
        reason: crate::monitor::InfraBlockReason,
    ) -> Result<(), super::super::monitor_adapter::CheckError> {
        let message = match reason {
            crate::monitor::InfraBlockReason::CircuitBreaker { retry_after_secs } => {
                format!("circuit:{retry_after_secs}")
            }
            crate::monitor::InfraBlockReason::OutputRootUnavailable {
                path,
                io_kind,
                retry_after_secs,
            } => format!(
                "output:{}:{}:{retry_after_secs}",
                path.display(),
                io_kind.as_str()
            ),
        };
        self.effects.lock().unwrap().push(message);
        if self.fail_effects {
            Err(super::super::monitor_adapter::CheckError::transient(
                "persist fixture",
            ))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn terminal_effects_preserve_persistence_and_runtime_contracts_across_prior_states() {
    for previous in [
        StreamerState::Live,
        StreamerState::NotLive,
        StreamerState::OutOfSchedule,
    ] {
        for failed in [false, true] {
            for output in [false, true] {
                let checker = Arc::new(PolicyChecker {
                    fail_effects: failed,
                    ..Default::default()
                });
                let (mut actor, _) = StreamerActor::new(
                    "test-streamer".into(),
                    create_test_metadata_store(),
                    create_test_config(),
                    CancellationToken::new(),
                    checker.clone(),
                );
                actor.state.streamer_state = previous;
                actor.state.last_download_activity_at = Some(Instant::now());
                actor.state.hysteresis.mark_live();
                actor.state.hysteresis.mark_offline_observed();
                let policy = if output {
                    DownloadEndPolicy::OutputRootBlocked {
                        path: "recordings".into(),
                        io_kind: crate::downloader::IoErrorKindSer::PermissionDenied,
                        retry_after_secs: 17,
                        session_id: "session".into(),
                    }
                } else {
                    DownloadEndPolicy::CircuitBreakerBlocked {
                        reason: "circuit".into(),
                        retry_after_secs: 13,
                        session_id: "session".into(),
                    }
                };
                let before = Instant::now();
                actor.handle_download_ended(policy).await.unwrap();
                assert_eq!(
                    actor.state.streamer_state,
                    if output {
                        StreamerState::OutOfSpace
                    } else {
                        StreamerState::TemporalDisabled
                    }
                );
                assert_eq!(actor.state.hysteresis.offline_count(), 1);
                assert!(actor.state.last_download_activity_at.is_none());
                assert!(actor.state.recurring_interval_ms.is_none());
                assert!(
                    actor.state.next_check.unwrap()
                        >= before + Duration::from_secs(if output { 17 } else { 13 })
                );
                assert_eq!(
                    *checker.effects.lock().unwrap(),
                    vec![if output {
                        "output:recordings:permission_denied:17".to_owned()
                    } else {
                        "circuit:13".to_owned()
                    }]
                );
            }
        }
    }
}

#[tokio::test]
async fn live_watchdog_failure_keeps_error_writes_and_normal_backoff_out_of_the_live_path() {
    let checker = Arc::new(PolicyChecker {
        fail_check: true,
        ..Default::default()
    });
    let metadata = create_test_metadata_store();
    let (mut actor, _) = StreamerActor::new(
        "test-streamer".into(),
        metadata.clone(),
        create_test_config(),
        CancellationToken::new(),
        checker.clone(),
    );
    actor.state.streamer_state = StreamerState::Live;
    actor.state.next_check = None;
    actor.state.last_download_activity_at = Some(Instant::now() - Duration::from_secs(301));
    let before = Instant::now();
    let error = actor.perform_check().await.unwrap_err();
    assert!(error.recoverable);
    assert_eq!(actor.state.streamer_state, StreamerState::Live);
    assert!(actor.state.next_check.is_none());
    assert!(actor.live_watchdog_backoff_until.unwrap() >= before + Duration::from_secs(60));
    assert!(checker.effects.lock().unwrap().is_empty());
    assert_eq!(
        metadata
            .get("test-streamer")
            .unwrap()
            .consecutive_error_count,
        0
    );
    assert!(actor.wake_delay().unwrap() > Duration::from_secs(59));

    // A successful watchdog observation clears its previous error floor.
    actor.status_checker = Arc::new(PolicyChecker::default());
    actor.perform_check().await.unwrap();
    assert!(actor.live_watchdog_backoff_until.is_none());

    actor.status_checker = checker.clone();
    actor.state.streamer_state = StreamerState::NotLive;
    actor.state.next_check = Some(Instant::now());
    assert!(actor.perform_check().await.unwrap_err().recoverable);
    assert_eq!(*checker.effects.lock().unwrap(), vec!["error"]);
    assert_eq!(actor.state.streamer_state, StreamerState::Error);
}
