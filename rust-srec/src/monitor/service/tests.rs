mod credential_checks;
use super::*;

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::config::{ConfigEventBroadcaster, ConfigService};
use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
use crate::database::repositories::{
    MonitorOutboxOps, SessionEventRepository as _, SqlxConfigRepository, SqlxFilterRepository,
    SqlxSessionEventRepository, SqlxSessionRepository, SqlxStreamerRepository,
};
use crate::database::{init_pool_with_size, run_migrations};
use crate::streamer::StreamerManager;

#[test]
fn concurrent_checks_share_results_only_within_one_purpose_and_binding() {
    use crate::credentials::{
        CredentialBinding, CredentialIdentity, CredentialOwner, CredentialSelection,
        ResolvedCredentialPolicy,
    };
    let policy = ResolvedCredentialPolicy::new(
        "platform".into(),
        CredentialOwner::Platform {
            platform_id: "platform".into(),
        },
        CredentialSelection::Pool {
            credential_ids: vec!["a".into(), "b".into()],
            strategy: crate::credentials::PoolStrategy::RoundRobin,
            failover: true,
            max_attempts: 3,
            anonymous_fallback: false,
        },
    )
    .unwrap();
    let binding = CredentialBinding {
        identity: CredentialIdentity::Profile {
            profile_id: "a".into(),
        },
        revision: 1,
        policy: policy.clone(),
        epoch: 1,
    };
    let key = |purpose, session: Option<&str>, binding: Option<&CredentialBinding>| {
        CredentialCheckKey::new(
            "streamer",
            purpose,
            Some(&policy),
            session.map(str::to_owned),
            binding,
        )
    };
    // Unbound discovery, queue-start and the recording's bound poll run at once.
    let discovery = key(CredentialCheckPurpose::Discovery, None, None);
    let queue_start = key(
        CredentialCheckPurpose::QueueStart,
        Some("session"),
        Some(&binding),
    );
    let bound_poll = key(
        CredentialCheckPurpose::Discovery,
        Some("session"),
        Some(&binding),
    );
    let recovery = key(
        CredentialCheckPurpose::Recovery,
        Some("session"),
        Some(&binding),
    );
    assert_eq!(bound_poll.purpose, CredentialCheckPurpose::BoundPoll);
    let keys = [&discovery, &queue_start, &bound_poll, &recovery];
    for (index, left) in keys.iter().enumerate() {
        for right in &keys[index + 1..] {
            assert_ne!(
                left, right,
                "{left:?} must not borrow {right:?}'s account result"
            );
        }
    }
    assert_eq!(
        bound_poll,
        key(
            CredentialCheckPurpose::BoundPoll,
            Some("session"),
            Some(&binding)
        ),
        "two polls of the same pinned account share one extraction"
    );
    for changed in [
        CredentialBinding {
            epoch: 2,
            ..binding.clone()
        },
        CredentialBinding {
            revision: 2,
            ..binding.clone()
        },
    ] {
        assert_ne!(
            bound_poll,
            key(
                CredentialCheckPurpose::BoundPoll,
                Some("session"),
                Some(&changed)
            )
        );
    }
    let other_policy = ResolvedCredentialPolicy::new(
        "platform".into(),
        CredentialOwner::Platform {
            platform_id: "platform".into(),
        },
        CredentialSelection::None,
    )
    .unwrap();
    assert_ne!(
        discovery,
        CredentialCheckKey::new(
            "streamer",
            CredentialCheckPurpose::Discovery,
            Some(&other_policy),
            None,
            None
        )
    );
}

#[tokio::test]
async fn committed_admin_authority_wins_over_checks_waiting_with_stale_cache() {
    use crate::database::committed_writer::{CommitPhase, CommitTestGate};
    use crate::database::repositories::SessionLifecycleRepository;
    use crate::streamer::manager::StreamerUpdateParams;
    for retired in [false, true] {
        let pool = setup_monitor_test_db().await;
        let mut model = StreamerDbModel::new(
            "Authority",
            "https://example.test/authority",
            "platform-huya",
        );
        model.id = "authority".into();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&model)
            .await
            .unwrap();
        let monitor = Arc::new(build_test_monitor(&pool).await);
        let stale = monitor.streamer_manager.get_streamer("authority").unwrap();
        let state = monitor.streamer_manager.committed_state().unwrap();
        let gate = Arc::new(CommitTestGate::default());
        state
            .writer
            .set_commit_gate(CommitPhase::AfterCommit, Some(gate.clone()));
        let manager = monitor.streamer_manager.clone();
        let admin = tokio::spawn(async move {
            if retired {
                manager.mark_deleting("authority").await.map(|_| ())
            } else {
                manager
                    .partial_update_streamer(StreamerUpdateParams {
                        id: "authority".into(),
                        name: None,
                        url: None,
                        platform_config_id: None,
                        template_config_id: None,
                        priority: None,
                        state: Some(StreamerState::Disabled),
                        streamer_specific_config: None,
                    })
                    .await
                    .map(|_| ())
            }
        });
        tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
            .await
            .unwrap();
        assert!(
            monitor
                .streamer_manager
                .get_streamer_snapshot("authority")
                .unwrap()
                .is_active()
        );
        state.writer.set_commit_gate(CommitPhase::AfterCommit, None);
        let lifecycle = SessionLifecycleRepository::new(pool.clone()).with_committed_state(state);
        let mut error_write = Box::pin(monitor.handle_error(&stale, "late check error"));
        let mut live_write =
            Box::pin(lifecycle.mark_streamer_live("authority", jiff::Timestamp::now()));
        // Both operations start while the cache still shows the pre-commit
        // active row, then wait for the administrator's retained writer lease.
        assert!(futures::poll!(&mut error_write).is_pending());
        assert!(futures::poll!(&mut live_write).is_pending());
        gate.release.notify_one();
        admin.await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(2), error_write)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), live_write)
                .await
                .unwrap()
                .unwrap()
                .is_some()
        );
        let row = SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .get_streamer("authority")
            .await
            .unwrap();
        assert_eq!(row.consecutive_error_count, Some(0));
        assert_eq!(row.deleted_at.is_some(), retired);
        assert!(
            !monitor
                .streamer_manager
                .get_streamer_snapshot("authority")
                .unwrap()
                .is_active()
        );
        assert!(outbox_events(&pool).await.is_empty());
        monitor.stop();
    }
}

#[tokio::test]
async fn monitor_and_configuration_share_empty_filter_snapshots_and_invalidation() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "snapshot-owner", StreamerState::NotLive, 0, None).await;
        let monitor = build_test_monitor(&pool).await;
        let empty = monitor.filter_store.get("snapshot-owner").await.unwrap();
        assert!(empty.is_empty());
        let connection = pool.acquire().await.unwrap();
        let repeated = tokio::time::timeout(
            Duration::from_millis(100),
            monitor.filter_store.get("snapshot-owner"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            Arc::ptr_eq(&empty, &repeated),
            "a repeated monitor check must not need the held SQL connection"
        );
        drop(connection);
        let repository = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        assert!(Arc::ptr_eq(
            &monitor.filter_store,
            &monitor.config_service.filter_store_for(repository.clone())
        ));
        repository
            .create_filter(&crate::database::models::FilterDbModel::new(
                "snapshot-owner",
                crate::database::models::FilterType::Keyword,
                r#"{"include":["allowed"],"exclude":[]}"#,
            ))
            .await
            .unwrap();
        monitor
            .config_service
            .notify_streamer_filters_updated("snapshot-owner");
        let updated = monitor.filter_store.get("snapshot-owner").await.unwrap();
        assert_eq!(updated.len(), 1);
        assert!(
            empty.is_empty(),
            "an already running check retains its old immutable snapshot"
        );
        monitor.stop();
        monitor
            ._task_supervisor
            .shutdown(Duration::from_secs(1))
            .await;
    })
    .await
    .expect("monitor snapshot wiring must remain shared and bounded");
}

async fn setup_monitor_test_db() -> SqlitePool {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

async fn build_test_monitor(
    pool: &SqlitePool,
) -> StreamMonitor<SqlxStreamerRepository, SqlxSessionRepository, SqlxConfigRepository> {
    let session_lifecycle = Arc::new(crate::session::SessionLifecycle::with_default_capacity(
        Arc::new(crate::database::repositories::SessionLifecycleRepository::new(pool.clone())),
        Arc::new(crate::session::OfflineClassifier::new()),
    ));

    build_test_monitor_with_lifecycle(pool, session_lifecycle).await
}

async fn build_test_monitor_with_lifecycle(
    pool: &SqlitePool,
    session_lifecycle: Arc<crate::session::SessionLifecycle>,
) -> StreamMonitor<SqlxStreamerRepository, SqlxSessionRepository, SqlxConfigRepository> {
    let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
    let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
    let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
    let streamer_manager = Arc::new(StreamerManager::new(
        streamer_repo.clone(),
        ConfigEventBroadcaster::new(),
    ));
    streamer_manager.hydrate().await.unwrap();
    let config_service = Arc::new(ConfigService::new(config_repo, streamer_repo));

    StreamMonitor::new(
        streamer_manager,
        filter_repo,
        session_repo,
        config_service,
        pool.clone(),
        session_lifecycle,
    )
    .unwrap()
}

async fn insert_streamer(
    pool: &SqlitePool,
    id: &str,
    state: StreamerState,
    consecutive_errors: i32,
    disabled_until: Option<i64>,
) {
    let mut streamer = StreamerDbModel::new(
        "Test Streamer",
        format!("https://example.com/{id}"),
        "platform-twitch",
    );
    streamer.id = id.to_string();
    streamer.state = state.to_string();
    streamer.consecutive_error_count = Some(consecutive_errors);
    streamer.disabled_until = disabled_until;
    streamer.last_error = Some("boom".to_string());

    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&streamer)
        .await
        .unwrap();
}

async fn insert_active_session(pool: &SqlitePool, session_id: &str, streamer_id: &str) {
    let mut session = LiveSessionDbModel::new(streamer_id);
    session.id = session_id.to_string();

    SqlxSessionRepository::new(pool.clone(), pool.clone())
        .create_session(&session)
        .await
        .unwrap();
}

async fn get_session(pool: &SqlitePool, session_id: &str) -> LiveSessionDbModel {
    SqlxSessionRepository::new(pool.clone(), pool.clone())
        .get_session(session_id)
        .await
        .unwrap()
}

async fn get_active_session_for_streamer(
    pool: &SqlitePool,
    streamer_id: &str,
) -> Option<LiveSessionDbModel> {
    SqlxSessionRepository::new(pool.clone(), pool.clone())
        .get_active_session_for_streamer(streamer_id)
        .await
        .unwrap()
}

#[tokio::test]
async fn required_monitor_delivery_is_acknowledged_before_outbox_commit() {
    let pool = setup_monitor_test_db().await;
    insert_streamer(&pool, "required-delivery", StreamerState::NotLive, 0, None).await;
    let event = MonitorEvent::StateChanged {
        streamer_id: "required-delivery".to_string(),
        streamer_name: "Required Delivery".to_string(),
        old_state: StreamerState::NotLive,
        new_state: StreamerState::Live,
        reason: None,
        timestamp: jiff::Timestamp::now(),
    };
    let payload = serde_json::to_string(&event).unwrap();
    sqlx::query(
        "INSERT INTO monitor_event_outbox (streamer_id, event_type, payload, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind("required-delivery")
    .bind("StateChanged")
    .bind(payload)
    .bind(crate::database::time::now_ms())
    .execute(&pool)
    .await
    .unwrap();

    let broadcaster = MonitorEventBroadcaster::new();
    let cancellation = CancellationToken::new();
    let (sender, mut receiver) = mpsc::channel(1);
    let flush_pool = pool.clone();
    let flush_broadcaster = broadcaster.clone();
    let flush_cancellation = cancellation.clone();
    let flush = tokio::spawn(async move {
        flush_outbox_once(
            &flush_pool,
            &flush_broadcaster,
            Some(&sender),
            &flush_cancellation,
        )
        .await
    });

    let delivery = receiver.recv().await.expect("required event should arrive");
    assert_eq!(
        MonitorOutboxOps::fetch_undelivered(&pool, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    delivery
        .acknowledgement
        .send(())
        .expect("outbox publisher should await acknowledgement");

    let result = flush.await.unwrap().unwrap();
    assert_eq!(result.fetched, 1);
    assert!(!result.needs_wait);
    assert!(
        MonitorOutboxOps::fetch_undelivered(&pool, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn session_event_count_for_streamer(pool: &SqlitePool, streamer_id: &str) -> usize {
    SqlxSessionEventRepository::new(pool.clone(), pool.clone())
        .list_for_streamer(streamer_id)
        .await
        .unwrap()
        .len()
}

async fn outbox_events(pool: &SqlitePool) -> Vec<MonitorEvent> {
    MonitorOutboxOps::fetch_undelivered(pool, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| serde_json::from_str(&entry.payload).expect("outbox event deserialises"))
        .collect()
}

fn outbox_event_type(event: &MonitorEvent) -> &'static str {
    match event {
        MonitorEvent::StreamerLive { .. } => "StreamerLive",
        MonitorEvent::StreamerOffline { .. } => "StreamerOffline",
        MonitorEvent::FatalError { .. } => "FatalError",
        MonitorEvent::TransientError { .. } => "TransientError",
        MonitorEvent::ChecksFailing { .. } => "ChecksFailing",
        MonitorEvent::ChecksRestored { .. } => "ChecksRestored",
        MonitorEvent::StateChanged { .. } => "StateChanged",
    }
}

/// The `ChecksFailing` / `ChecksRestored` kinds ever enqueued, oldest
/// first. Delivered rows count too: the monitor's outbox publisher runs
/// alongside the test and may deliver an event before it is read.
async fn check_alerts(pool: &SqlitePool) -> Vec<&'static str> {
    sqlx::query_scalar::<_, String>("SELECT payload FROM monitor_event_outbox ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|payload| serde_json::from_str(payload).unwrap())
        .map(|event: MonitorEvent| outbox_event_type(&event))
        .filter(|kind| kind.starts_with("Checks"))
        .collect()
}

#[tokio::test]
async fn check_failure_alert_fires_once_per_run_and_recovery_follows_it() {
    let pool = setup_monitor_test_db().await;
    insert_streamer(&pool, "flaky", StreamerState::NotLive, 0, None).await;
    // A failing run cleared from a state the session end does not own.
    insert_streamer(&pool, "flaky-oos", StreamerState::OutOfSchedule, 100, None).await;
    let monitor = build_test_monitor(&pool).await;
    let current = |id| monitor.streamer_manager.get_streamer(id).unwrap();
    let threshold = download_failure_threshold(current("flaky").offline_check_count);

    // A run that stops short of the threshold alerts neither way.
    for _ in 1..threshold {
        monitor
            .handle_error(&current("flaky"), "boom")
            .await
            .unwrap();
    }
    monitor.handle_offline(&current("flaky")).await.unwrap();
    assert_eq!(current("flaky").consecutive_error_count, 0);
    assert!(check_alerts(&pool).await.is_empty());

    // The failure that reaches the threshold alerts, and clearing that
    // run reports the recovery once, even when two offline observations
    // race to clear it.
    for _ in 1..threshold {
        monitor
            .handle_error(&current("flaky"), "boom")
            .await
            .unwrap();
    }
    assert!(check_alerts(&pool).await.is_empty());
    monitor
        .handle_error(&current("flaky"), "boom")
        .await
        .unwrap();
    assert_eq!(check_alerts(&pool).await, ["ChecksFailing"]);
    let stale = current("flaky");
    monitor.handle_offline(&stale).await.unwrap();
    monitor.handle_offline(&stale).await.unwrap();
    assert_eq!(
        check_alerts(&pool).await,
        ["ChecksFailing", "ChecksRestored"]
    );

    // Failures past the threshold add nothing, and a recording making
    // progress also reports the recovery.
    for _ in 0..=threshold {
        monitor
            .handle_error(&current("flaky"), "boom")
            .await
            .unwrap();
    }
    monitor
        .streamer_manager
        .record_success("flaky", true)
        .await
        .unwrap();
    assert_eq!(
        check_alerts(&pool).await,
        [
            "ChecksFailing",
            "ChecksRestored",
            "ChecksFailing",
            "ChecksRestored"
        ]
    );

    monitor.handle_offline(&current("flaky-oos")).await.unwrap();
    assert_eq!(current("flaky-oos").consecutive_error_count, 0);
    assert_eq!(check_alerts(&pool).await[4..], ["ChecksRestored"]);

    monitor.stop();
}

#[tokio::test]
async fn test_out_of_schedule_recheck_without_active_session_skips_lifecycle_write() {
    let pool = setup_monitor_test_db().await;
    insert_streamer(
        &pool,
        "streamer-oos-settled",
        StreamerState::OutOfSchedule,
        0,
        None,
    )
    .await;

    let monitor = build_test_monitor(&pool).await;
    let mut rx = monitor.session_lifecycle.subscribe();
    let already_out = monitor
        .streamer_manager
        .get_streamer("streamer-oos-settled")
        .unwrap();
    let before_session_events =
        session_event_count_for_streamer(&pool, "streamer-oos-settled").await;
    let before_outbox_events = outbox_events(&pool).await.len();

    let outcome = monitor
        .process_status(
            &already_out,
            LiveStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: None,
                },
                title: "Still Out".to_string(),
                category: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome, ProcessStatusResult::Applied);
    assert!(
        rx.try_recv().is_err(),
        "settled recheck must not emit a lifecycle transition"
    );
    assert!(
        get_active_session_for_streamer(&pool, "streamer-oos-settled")
            .await
            .is_none()
    );
    assert_eq!(
        session_event_count_for_streamer(&pool, "streamer-oos-settled").await,
        before_session_events,
        "settled recheck must not write duplicate session_events rows"
    );
    assert_eq!(
        outbox_events(&pool).await.len(),
        before_outbox_events,
        "settled recheck must not enqueue duplicate outbox events"
    );

    monitor.stop();
}

#[tokio::test]
async fn test_out_of_schedule_recheck_closes_cold_start_active_session_when_state_matches() {
    let pool = setup_monitor_test_db().await;
    insert_streamer(
        &pool,
        "streamer-oos-cold-start",
        StreamerState::OutOfSchedule,
        0,
        None,
    )
    .await;
    insert_active_session(&pool, "session-oos-cold-start", "streamer-oos-cold-start").await;

    let monitor = build_test_monitor(&pool).await;
    let mut rx = monitor.session_lifecycle.subscribe();
    let already_out = monitor
        .streamer_manager
        .get_streamer("streamer-oos-cold-start")
        .unwrap();

    let outcome = monitor
        .process_status(
            &already_out,
            LiveStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: None,
                },
                title: "Still Out".to_string(),
                category: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome, ProcessStatusResult::Applied);
    assert!(
        get_session(&pool, "session-oos-cold-start")
            .await
            .end_time
            .is_some()
    );

    match rx.recv().await.unwrap() {
        crate::session::SessionTransition::Ended {
            session_id, cause, ..
        } => {
            assert_eq!(session_id, "session-oos-cold-start");
            assert_eq!(cause, crate::session::TerminalCause::OutOfSchedule);
        }
        other => panic!("expected Ended, got {other:?}"),
    }

    let events = outbox_events(&pool).await;
    let event_types: Vec<&str> = events.iter().map(outbox_event_type).collect();
    assert_eq!(event_types, vec!["StateChanged"]);

    monitor.stop();
}

#[tokio::test]
async fn test_process_status_live_is_suppressed_while_temporarily_disabled() {
    let pool = setup_monitor_test_db().await;
    insert_streamer(
        &pool,
        "streamer-live-suppressed",
        StreamerState::TemporalDisabled,
        3,
        Some(jiff::Timestamp::now().as_millisecond() + 60_000),
    )
    .await;

    let monitor = build_test_monitor(&pool).await;
    let streamer = monitor
        .streamer_manager
        .get_streamer("streamer-live-suppressed")
        .unwrap();

    let outcome = monitor
        .process_status(
            &streamer,
            LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Suppressed Live".to_string(),
                category: None,
                avatar: None,
                started_at: None,
                viewer_count: None,
                streams: vec![platforms_parser::media::StreamInfo {
                    url: "https://example.com/stream.m3u8".to_string(),
                    stream_format: platforms_parser::media::StreamFormat::Flv,
                    media_format: platforms_parser::media::formats::MediaFormat::Flv,
                    quality: "best".to_string(),
                    bitrate: 5_000_000,
                    priority: 1,
                    extras: None,
                    codec: "h264".to_string(),
                    fps: 30.0,
                    is_headers_needed: false,
                    is_audio_only: false,
                    format_opt_in: false,
                }],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            },
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        ProcessStatusResult::Suppressed(ProcessStatusSuppression::TemporarilyDisabled {
            retry_after: Some(_)
        })
    ));

    assert!(
        get_active_session_for_streamer(&pool, "streamer-live-suppressed")
            .await
            .is_none()
    );

    assert!(outbox_events(&pool).await.is_empty());

    monitor.stop();
}

#[tokio::test]
async fn test_process_status_live_is_suppressed_for_user_disabled_streamer() {
    let pool = setup_monitor_test_db().await;
    insert_streamer(
        &pool,
        "streamer-user-disabled",
        StreamerState::Disabled,
        0,
        None,
    )
    .await;

    let monitor = build_test_monitor(&pool).await;
    let streamer = monitor
        .streamer_manager
        .get_streamer("streamer-user-disabled")
        .unwrap();

    let outcome = monitor
        .process_status(
            &streamer,
            LiveStatus::Live {
                credential_binding: None,
                credential_snapshot: None,
                title: "Should Stay Disabled".to_string(),
                category: None,
                avatar: None,
                started_at: None,
                viewer_count: None,
                streams: vec![platforms_parser::media::StreamInfo {
                    url: "https://example.com/stream.flv".to_string(),
                    stream_format: platforms_parser::media::StreamFormat::Flv,
                    media_format: platforms_parser::media::formats::MediaFormat::Flv,
                    quality: "best".to_string(),
                    bitrate: 5_000_000,
                    priority: 1,
                    extras: None,
                    codec: "h264".to_string(),
                    fps: 30.0,
                    is_headers_needed: false,
                    is_audio_only: false,
                    format_opt_in: false,
                }],
                media_headers: None,
                media_extras: None,
                next_check_hint: None,
                candidates: vec![],
            },
        )
        .await
        .unwrap();

    assert_eq!(
        outcome,
        ProcessStatusResult::Suppressed(ProcessStatusSuppression::Disabled)
    );

    assert!(outbox_events(&pool).await.is_empty());

    monitor.stop();
}
