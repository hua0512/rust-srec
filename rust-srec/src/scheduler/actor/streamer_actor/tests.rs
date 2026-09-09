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
        batch_capable: false,
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
    for batch in [false, true] {
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
        let (mut actor, _handle) = if batch {
            StreamerActor::with_priority_channel(
                "test-streamer".to_string(),
                create_test_metadata_store(),
                create_test_config(),
                CancellationToken::new(),
                checker.clone(),
            )
        } else {
            StreamerActor::new(
                "test-streamer".to_string(),
                create_test_metadata_store(),
                create_test_config(),
                CancellationToken::new(),
                checker.clone(),
            )
        };
        for (pending, remaining) in [(true, 3), (true, 2), (false, 1), (false, 1)] {
            if batch {
                actor
                    .handle_batch_result(BatchDetectionResult {
                        streamer_id: "test-streamer".to_string(),
                        result: CheckResult::success(StreamerState::NotLive),
                        status: LiveStatus::Offline,
                    })
                    .await
                    .unwrap();
            } else {
                actor.perform_check().await.unwrap();
            }
            assert_eq!(actor.initial_status_pending, pending);
            assert_eq!(checker.outcomes.lock().unwrap().len(), remaining);
            assert!(actor.state.next_check.is_some());
        }
    }
}

#[tokio::test]
async fn live_actor_retries_failed_offline_at_grace_threshold() {
    for batch in [false, true] {
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
        let (mut actor, _handle) = if batch {
            StreamerActor::with_priority_channel(
                "test-streamer".to_string(),
                create_test_metadata_store(),
                create_test_config(),
                CancellationToken::new(),
                checker.clone(),
            )
        } else {
            StreamerActor::new(
                "test-streamer".to_string(),
                create_test_metadata_store(),
                create_test_config(),
                CancellationToken::new(),
                checker.clone(),
            )
        };
        actor.state = previous_state;
        assert_eq!(actor.state.streamer_state, StreamerState::Live);
        for (pending, remaining) in [(true, 3), (true, 3), (true, 2), (false, 1), (false, 1)] {
            if batch {
                actor
                    .handle_batch_result(BatchDetectionResult {
                        streamer_id: "test-streamer".to_string(),
                        result: CheckResult::success(StreamerState::NotLive),
                        status: LiveStatus::Offline,
                    })
                    .await
                    .unwrap();
            } else {
                actor.perform_check().await.unwrap();
            }
            assert_eq!(actor.initial_status_pending, pending);
            assert_eq!(checker.outcomes.lock().unwrap().len(), remaining);
        }
    }
}

#[tokio::test]
async fn initial_reconciliation_keeps_known_live_offline_grace_period() {
    for batch in [false, true] {
        let store = create_test_metadata_store();
        Arc::make_mut(&mut store.get_mut("test-streamer").unwrap()).state = StreamerState::Live;
        let checker = Arc::new(SequenceStatusChecker::new(
            (0..3)
                .map(|_| {
                    (
                        CheckResult::success(StreamerState::NotLive),
                        LiveStatus::Offline,
                    )
                })
                .collect(),
            vec![ProcessStatusResult::Applied],
        ));
        let (mut actor, _handle) = StreamerActor::new(
            "test-streamer".to_string(),
            store,
            create_test_config(),
            CancellationToken::new(),
            checker.clone(),
        );
        for remaining in [1, 1, 0] {
            if batch {
                actor
                    .handle_batch_result(BatchDetectionResult {
                        streamer_id: "test-streamer".to_string(),
                        result: CheckResult::success(StreamerState::NotLive),
                        status: LiveStatus::Offline,
                    })
                    .await
                    .unwrap();
            } else {
                actor.perform_check().await.unwrap();
            }
            assert_eq!(checker.outcomes.lock().unwrap().len(), remaining);
        }
        assert!(!actor.initial_status_pending);
    }
}

#[test]
fn test_streamer_actor_new() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        create_noop_checker(),
    );

    assert_eq!(actor.id(), "test-streamer");
    assert_eq!(handle.id(), "test-streamer");
    assert!(!actor.uses_batch_detection());
}

#[test]
fn test_streamer_actor_with_platform() {
    let metadata_store = create_test_metadata_store();
    let mut config = create_test_config();
    config.batch_capable = true;
    let token = CancellationToken::new();
    let (platform_tx, _platform_rx) = mpsc::channel::<PlatformMessage>(10);

    let (actor, _handle) = StreamerActor::with_priority_and_platform(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        platform_tx,
        create_noop_checker(),
    );

    assert!(actor.uses_batch_detection());
}

#[tokio::test]
async fn test_streamer_actor_get_state() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token.clone(),
        create_noop_checker(),
    );

    // Spawn actor
    let actor_task = tokio::spawn(async move { actor.run().await });

    // Query state
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    handle
        .send(StreamerMessage::GetState(reply_tx))
        .await
        .unwrap();

    let state = reply_rx.await.unwrap();
    assert_eq!(state.streamer_state, StreamerState::NotLive);
    assert_eq!(state.hysteresis.offline_count(), 0);

    // Stop actor
    handle.send(StreamerMessage::Stop).await.unwrap();
    let result = actor_task.await.unwrap();
    assert!(matches!(result, Ok(ActorOutcome::Stopped)));
}

#[tokio::test]
async fn test_streamer_actor_config_update() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token.clone(),
        create_noop_checker(),
    );

    // Spawn actor
    let actor_task = tokio::spawn(async move { actor.run().await });

    // Send config update
    let new_config = StreamerConfig {
        check_interval_ms: 5000,
        offline_check_interval_ms: 2000,
        offline_check_count: 5,
        priority: Priority::High,
        batch_capable: false,
    };
    handle
        .send(StreamerMessage::ConfigUpdate(new_config))
        .await
        .unwrap();

    // Give time for processing
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Query state to verify config was applied
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    handle
        .send(StreamerMessage::GetState(reply_tx))
        .await
        .unwrap();
    let _state = reply_rx.await.unwrap();

    // Stop actor
    handle.send(StreamerMessage::Stop).await.unwrap();
    let result = actor_task.await.unwrap();
    assert!(matches!(result, Ok(ActorOutcome::Stopped)));
}

#[tokio::test]
async fn test_streamer_actor_cancellation() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token.clone(),
        create_noop_checker(),
    );

    // Spawn actor
    let actor_task = tokio::spawn(async move { actor.run().await });

    // Cancel
    token.cancel();

    let result = actor_task.await.unwrap();
    assert!(matches!(result, Ok(ActorOutcome::Cancelled)));
}

#[tokio::test]
async fn test_streamer_actor_batch_result() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token.clone(),
        create_noop_checker(),
    );

    // Spawn actor
    let actor_task = tokio::spawn(async move { actor.run().await });

    // Send batch result
    let batch_result = BatchDetectionResult {
        streamer_id: "test-streamer".to_string(),
        result: CheckResult::success(StreamerState::Live),
        status: crate::monitor::LiveStatus::Live {
            title: "Test Stream".to_string(),
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
    };
    handle
        .send(StreamerMessage::BatchResult(Box::new(batch_result)))
        .await
        .unwrap();

    // Give time for processing
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Query state
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    handle
        .send(StreamerMessage::GetState(reply_tx))
        .await
        .unwrap();
    let state = reply_rx.await.unwrap();

    // State should be updated to Live
    assert_eq!(state.streamer_state, StreamerState::Live);

    // Stop actor
    handle.send(StreamerMessage::Stop).await.unwrap();
    let result = actor_task.await.unwrap();
    assert!(matches!(result, Ok(ActorOutcome::Stopped)));
}

#[test]
fn test_actor_error_display() {
    let err = ActorError::recoverable("test error");
    assert_eq!(err.to_string(), "test error");
    assert!(err.recoverable);

    let err = ActorError::fatal("fatal error");
    assert_eq!(err.to_string(), "fatal error");
    assert!(!err.recoverable);
}

#[test]
fn test_actor_outcome() {
    assert_eq!(ActorOutcome::Stopped, ActorOutcome::Stopped);
    assert_ne!(ActorOutcome::Stopped, ActorOutcome::Cancelled);
}

#[test]
fn test_streamer_actor_with_priority_channel() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (actor, handle) = StreamerActor::with_priority_channel(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        create_noop_checker(),
    );

    assert_eq!(actor.id(), "test-streamer");
    assert_eq!(handle.id(), "test-streamer");
    // Actor should have priority mailbox
    assert!(actor.priority_mailbox.is_some());
}

#[test]
fn test_streamer_actor_with_priority_and_platform() {
    let metadata_store = create_test_metadata_store();
    let mut config = create_test_config();
    config.batch_capable = true;
    let token = CancellationToken::new();
    let (platform_tx, _platform_rx) = mpsc::channel::<PlatformMessage>(10);

    let (actor, _handle) = StreamerActor::with_priority_and_platform(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        platform_tx,
        create_noop_checker(),
    );

    assert!(actor.uses_batch_detection());
    assert!(actor.priority_mailbox.is_some());
}

#[tokio::test]
async fn test_priority_channel_stop_message() {
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

    // Send stop via priority channel
    handle.send_priority(StreamerMessage::Stop).await.unwrap();

    let result = actor_task.await.unwrap();
    assert!(matches!(result, Ok(ActorOutcome::Stopped)));
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
        let batch_result = BatchDetectionResult {
            streamer_id: "test-streamer".to_string(),
            result: CheckResult::success(StreamerState::NotLive),
            status: crate::monitor::LiveStatus::Offline,
        };
        handle
            .send(StreamerMessage::BatchResult(Box::new(batch_result)))
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

#[tokio::test]
async fn test_priority_channel_get_state() {
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

    // Query state via priority channel
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    handle
        .send_priority(StreamerMessage::GetState(reply_tx))
        .await
        .unwrap();

    let state = reply_rx.await.unwrap();
    assert_eq!(state.streamer_state, StreamerState::NotLive);

    // Stop actor
    handle.send_priority(StreamerMessage::Stop).await.unwrap();
    let result = actor_task.await.unwrap();
    assert!(matches!(result, Ok(ActorOutcome::Stopped)));
}

#[tokio::test]
async fn test_streamer_actor_resume_on_download_end() {
    let metadata_store = create_test_metadata_store();
    let config = StreamerConfig::default();
    let token = CancellationToken::new();

    // Create actor with live state (checks paused)
    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config.clone(),
        token,
        create_noop_checker(),
    );
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live(); // Set was_live since we were live
    actor.state.next_check = None;

    // Verify initially paused
    assert!(actor.state.next_check.is_none());

    // Simulate download ended (streamer offline)
    let result = actor
        .handle_download_ended(super::super::messages::DownloadEndPolicy::StreamerOffline)
        .await;
    assert!(result.is_ok());

    // Verify state changed and check scheduled
    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.next_check.is_some());
    // Hysteresis stays "recently live" so short offline polling is used
    assert!(actor.state.hysteresis.was_live());
    assert_eq!(actor.state.hysteresis.offline_count(), 1);
    let until = actor.state.time_until_next_check().unwrap();
    assert_eq!(
        actor.state.recurring_interval_ms,
        Some(config.offline_check_interval_ms)
    );
    assert!(until <= Duration::from_millis(config.offline_check_interval_ms * 11 / 10));

    // Reset and test error case
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;

    // Simulate download failed (network error)
    let result = actor
        .handle_download_ended(super::super::messages::DownloadEndPolicy::NetworkError(
            "timeout".into(),
        ))
        .await;
    assert!(result.is_ok());

    // Verify state changed (assumed not live for safety) and check scheduled immediately
    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.next_check.is_some());
    assert!(actor.state.is_check_due()); // Network errors trigger immediate check

    // Reset and test user cancellation
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;

    // A wire user stop resumes verification without inventing an Offline write
    // or a permanent monitoring-stop request.
    actor
        .handle_download_ended(DownloadEndPolicy::Stopped(DownloadStopCause::User))
        .await
        .unwrap();
    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.next_check.is_some());
    assert!(actor.state.hysteresis.was_live());

    // Reset and test unknown reason
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;

    // Simulate download ended with unknown reason
    let result = actor
        .handle_download_ended(super::super::messages::DownloadEndPolicy::Other(
            "unknown".into(),
        ))
        .await;
    assert!(result.is_ok());

    // Verify state changed but hysteresis preserved (let checks verify)
    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.next_check.is_some());
    // Hysteresis should be preserved - let checks determine actual state
    assert!(actor.state.hysteresis.was_live());
}

/// `DownloadEndPolicy::Completed` (engine clean EOF without platform
/// authority) must update local scheduling state to the post-live
/// short-polling cadence — same as `StreamerOffline` — but must NOT
/// emit a `process_status(Offline)` push (the lifecycle's hysteresis
/// is the authoritative absorber for this case; a duplicate offline
/// emit would race and bypass it).
///
/// We can't directly assert "no push" here without a tracking
/// `StatusChecker`, but we can lock in the local-state contract:
/// the post-live cadence assertions identical to
/// `test_streamer_actor_resume_on_download_end` for the
/// `StreamerOffline` path. If a future change re-routes `Completed`
/// through a different scheduling branch, this test fails and points
/// the maintainer at the contract.
#[tokio::test]
async fn test_streamer_actor_completed_uses_post_live_cadence() {
    let metadata_store = create_test_metadata_store();
    let config = StreamerConfig::default();
    let token = CancellationToken::new();

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config.clone(),
        token,
        create_noop_checker(),
    );
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;

    let result = actor
        .handle_download_ended(super::super::messages::DownloadEndPolicy::Completed)
        .await;
    assert!(result.is_ok());

    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.next_check.is_some());
    // Same post-live cadence as `StreamerOffline`: was_live preserved,
    // offline-observed counter incremented, next check arrives within
    // the jittered offline polling window.
    assert!(actor.state.hysteresis.was_live());
    assert_eq!(actor.state.hysteresis.offline_count(), 1);
    let until = actor.state.time_until_next_check().unwrap();
    assert!(
        until <= Duration::from_millis(config.offline_check_interval_ms * 11 / 10),
        "Completed must schedule next check within the post-live offline window"
    );
}

#[tokio::test]
async fn test_perform_check_suppressed_live_does_not_leave_actor_stuck_live() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let checker: Arc<dyn StatusChecker> = Arc::new(SequenceStatusChecker::new(
        vec![(
            CheckResult::success(StreamerState::Live),
            LiveStatus::Live {
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

    actor.perform_check().await.unwrap();

    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(actor.state.last_download_activity_at.is_none());
    assert!(actor.state.next_check.is_some());
    assert!(!actor.state.hysteresis.was_live());

    let until = actor.state.time_until_next_check().unwrap();
    assert!(until <= Duration::from_secs(30));
}

#[tokio::test]
async fn test_perform_check_recovers_after_suppressed_live_when_backoff_expires() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let checker: Arc<dyn StatusChecker> = Arc::new(SequenceStatusChecker::new(
        vec![
            (
                CheckResult::success(StreamerState::Live),
                LiveStatus::Live {
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
            ),
            (
                CheckResult::success(StreamerState::Live),
                LiveStatus::Live {
                    title: "Recovered Live".to_string(),
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
            ),
        ],
        vec![
            ProcessStatusResult::Suppressed(ProcessStatusSuppression::TemporarilyDisabled {
                retry_after: Some(Duration::from_secs(1)),
            }),
            ProcessStatusResult::Applied,
        ],
    ));

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        checker,
    );

    actor.perform_check().await.unwrap();
    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(!actor.state.hysteresis.was_live());

    actor.perform_check().await.unwrap();

    assert_eq!(actor.state.streamer_state, StreamerState::Live);
    assert!(actor.state.last_download_activity_at.is_some());
    assert!(actor.state.next_check.is_none());
    assert!(actor.state.hysteresis.was_live());
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

/// `StreamerBackoffBlocked` (container refused the download start because
/// the streamer is inside its `disabled_until` error backoff) must move
/// the actor out of Live and schedule a re-check at backoff expiry —
/// otherwise the actor sits in watchdog mode where `(Live, Live)`
/// suppression never lets `process_status` restart the download.
#[tokio::test]
async fn test_download_ended_streamer_backoff_blocked_schedules_expiry_check() {
    let metadata_store = create_test_metadata_store();
    let config = create_test_config();
    let token = CancellationToken::new();

    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        create_noop_checker(),
    );

    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.next_check = None;
    actor.state.last_download_activity_at = Some(Instant::now());

    actor
        .handle_download_ended(
            super::super::messages::DownloadEndPolicy::StreamerBackoffBlocked {
                reason: "streamer temporarily disabled (error backoff)".to_string(),
                retry_after_secs: 30,
                session_id: "session-1".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(actor.state.streamer_state, StreamerState::TemporalDisabled);
    assert!(actor.state.last_download_activity_at.is_none());
    let until = actor.state.time_until_next_check().unwrap();
    assert!(until <= Duration::from_secs(30));
    assert!(until > Duration::from_secs(20));
    // TemporalDisabled → Live is an emitting transition in
    // `HysteresisState::should_emit`, so the expiry check re-drives the
    // download through the normal monitor path.
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

/// `process_status` must NOT be called on the `Stopped(DanmuStreamClosed)`
/// path — the container's danmu observer is the canonical owner of the
/// offline emission for this signal. Calling `process_status` here too
/// would race the observer at the `streamer.state == Live` gate inside
/// `handle_offline_with_session`, ending whichever session the actor's
/// snapshot saw as active (frequently a different `session_id` than
/// the one the observer is closing).
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
async fn danmu_stream_closed_does_not_call_process_status() {
    let metadata_store = create_test_metadata_store();
    let config = StreamerConfig::default();
    let token = CancellationToken::new();
    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        Arc::new(AssertNotCalledStatusChecker),
    );
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();

    // No panic ⇒ the actor did NOT route through `process_status`.
    let result = actor
        .handle_download_ended(super::super::messages::DownloadEndPolicy::Stopped(
            DownloadStopCause::DanmuStreamClosed,
        ))
        .await;
    assert!(
        result.is_ok(),
        "actor must complete cleanly without process_status"
    );
}

#[tokio::test]
async fn danmu_stream_closed_resets_hysteresis_and_reschedules() {
    let metadata_store = create_test_metadata_store();
    let config = StreamerConfig::default();
    let token = CancellationToken::new();
    let (mut actor, _handle) = StreamerActor::new(
        "test-streamer".to_string(),
        metadata_store,
        config,
        token,
        create_noop_checker(),
    );
    actor.state.streamer_state = StreamerState::Live;
    actor.state.hysteresis.mark_live();
    actor.state.hysteresis.mark_offline_observed();
    assert!(
        actor.state.hysteresis.was_live(),
        "fixture: hysteresis records the prior live state"
    );
    actor.state.next_check = None;

    actor
        .handle_download_ended(super::super::messages::DownloadEndPolicy::Stopped(
            DownloadStopCause::DanmuStreamClosed,
        ))
        .await
        .unwrap();

    // The arm flips local in-memory state to NotLive, fully resets
    // hysteresis (was_live becomes false → schedule_next_check uses
    // the longer normal interval), and arms a next-check timer.
    assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
    assert!(
        !actor.state.hysteresis.was_live(),
        "DanmuStreamClosed is authoritative; hysteresis must be fully reset"
    );
    assert!(actor.state.next_check.is_some());
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

#[test]
fn actor_metadata_snapshots_share_configuration_without_holding_cache_guards() {
    let store = create_test_metadata_store();
    let (actor, _) = StreamerActor::new(
        "test-streamer".into(),
        store.clone(),
        create_test_config(),
        CancellationToken::new(),
        create_noop_checker(),
    );
    let first = actor.get_metadata().unwrap();
    let second = actor.get_metadata().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    Arc::make_mut(&mut store.get_mut("test-streamer").unwrap()).name = "replacement".into();
    let replacement = actor.get_metadata().unwrap();
    assert!(!Arc::ptr_eq(&first, &replacement));
    assert_eq!(replacement.name, "replacement");
    assert_ne!(first.name, replacement.name);
    store.remove("test-streamer");
    assert!(actor.get_metadata().is_none());
    assert_eq!(first.name, second.name);
}
