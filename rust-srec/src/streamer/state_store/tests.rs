use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use super::*;
use crate::database::committed_writer::{CommitPhase, CommitTestGate};
use crate::database::repositories::{
    SessionLifecycleRepository, SqlxStreamerRepository, StartSessionInputs, StreamerRepository,
    StreamerTxOps,
};
use crate::domain::StreamerState;
use crate::streamer::{StreamerManager, manager::StreamerUpdateParams};
use crate::utils::task_supervisor::TaskSupervisor;

struct Fixture {
    pool: sqlx::SqlitePool,
    writer: Arc<CommittedWriter>,
    store: Arc<CommittedStreamerState>,
    repository: Arc<SqlxStreamerRepository>,
    manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    supervisor: Arc<TaskSupervisor>,
}

async fn fixture() -> Fixture {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let supervisor = Arc::new(TaskSupervisor::new());
    let writer = Arc::new(CommittedWriter::new(pool.clone(), supervisor.clone()).unwrap());
    let store = Arc::new(CommittedStreamerState::new(
        writer.clone(),
        ConfigEventBroadcaster::new(),
    ));
    let repository = Arc::new(
        SqlxStreamerRepository::new(pool.clone(), pool.clone()).with_committed_state(store.clone()),
    );
    let manager = Arc::new(StreamerManager::new(
        repository.clone(),
        store.cache.broadcaster.clone(),
    ));
    let mut row =
        StreamerDbModel::new("Original", "https://example.test/original", "platform-huya");
    row.id = "state-test".into();
    row.streamer_specific_config =
        Some(r#"{"cookies":"old","extension":{"retained":true}}"#.into());
    row.created_at = 1_788_784_496_123;
    repository.create_streamer(&row).await.unwrap();
    Fixture {
        pool,
        writer,
        store,
        repository,
        manager,
        supervisor,
    }
}

fn patch() -> StreamerUpdateParams {
    StreamerUpdateParams {
        id: "state-test".into(),
        name: None,
        url: None,
        platform_config_id: None,
        template_config_id: None,
        priority: None,
        state: None,
        streamer_specific_config: None,
    }
}

async fn change_state(store: Arc<CommittedStreamerState>, state: &'static str) -> Result<()> {
    store
        .transaction(
            "test state transition",
            StatePublication::StateOnly,
            move |tx| {
                Box::pin(async move {
                    let row = StreamerTxOps::update_state_row(tx, "state-test", state).await?;
                    Ok(StateChange::row((), row))
                })
            },
        )
        .await
}

async fn reached(gate: &Notify) {
    tokio::time::timeout(Duration::from_secs(2), gate.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn writer_lease_orders_publication_and_preserves_current_runtime_configuration() {
    let f = fixture().await;
    let original = f.manager.get_streamer_snapshot("state-test").unwrap();
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    let gate = Arc::new(CommitTestGate::default());
    f.writer
        .set_commit_gate(CommitPhase::AfterCommit, Some(gate.clone()));
    let first = tokio::spawn(change_state(f.store.clone(), "LIVE"));
    reached(&gate.started).await;
    assert!(
        f.pool.try_acquire().is_none(),
        "committed writer retains the lease through publication"
    );
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    let mut merged = crate::config::MergedConfig::builder().build();
    merged.offline_check_count = 17;
    merged.offline_check_delay_ms = 1234;
    f.manager.apply_resolved_config("state-test", &merged);
    f.writer.set_commit_gate(CommitPhase::AfterCommit, None);
    let second = tokio::spawn(change_state(f.store.clone(), "OUT_OF_SCHEDULE"));
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let current = f.manager.get_streamer_snapshot("state-test").unwrap();
    assert_eq!(current.state, StreamerState::OutOfSchedule);
    assert_eq!(
        (current.offline_check_count, current.offline_check_delay_ms),
        (17, 1234)
    );
    assert_eq!(
        original.state,
        StreamerState::NotLive,
        "previous readers retain their immutable snapshot"
    );
    assert_eq!(
        f.repository.get_streamer("state-test").await.unwrap().state,
        "OUT_OF_SCHEDULE"
    );
}

#[tokio::test]
async fn cancellation_before_commit_rolls_back_without_cache_or_events() {
    let f = fixture().await;
    let original = f.manager.get_streamer_snapshot("state-test").unwrap();
    let mut events = f.store.cache.broadcaster.subscribe();
    let written = Arc::new(Notify::new());
    let notify = written.clone();
    let store = f.store.clone();
    let caller = tokio::spawn(async move {
        store
            .transaction(
                "cancel prepared state",
                StatePublication::StateOnly,
                move |tx| {
                    Box::pin(async move {
                        let row = StreamerTxOps::update_state_row(tx, "state-test", "LIVE").await?;
                        notify.notify_one();
                        std::future::pending::<()>().await;
                        Ok(StateChange::row((), row))
                    })
                },
            )
            .await
    });
    reached(&written).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let connection = tokio::time::timeout(Duration::from_secs(2), f.pool.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(connection);
    assert_eq!(
        f.repository.get_streamer("state-test").await.unwrap().state,
        "NOT_LIVE"
    );
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    assert!(events.try_recv().is_err());
    assert!(f.supervisor.shutdown(Duration::from_secs(1)).await);
}

#[tokio::test]
async fn cancellation_inside_commit_still_publishes_the_committed_row() {
    for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
        let f = fixture().await;
        let mut events = f.store.cache.broadcaster.subscribe();
        let gate = Arc::new(CommitTestGate::default());
        f.writer.set_commit_gate(phase, Some(gate.clone()));
        let caller = tokio::spawn(change_state(f.store.clone(), "LIVE"));
        reached(&gate.started).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        gate.release.notify_one();
        let connection = tokio::time::timeout(Duration::from_secs(2), f.pool.acquire())
            .await
            .unwrap()
            .unwrap();
        drop(connection);
        assert_eq!(
            f.repository.get_streamer("state-test").await.unwrap().state,
            "LIVE"
        );
        assert_eq!(
            f.manager.get_streamer_snapshot("state-test").unwrap().state,
            StreamerState::Live
        );
        assert!(matches!(
            events.try_recv().unwrap(),
            ConfigUpdateEvent::StreamerStateSyncedFromDb {
                is_active: true,
                ..
            }
        ));
        assert!(events.try_recv().is_err());
        assert!(f.supervisor.shutdown(Duration::from_secs(1)).await);
    }
}

#[tokio::test]
async fn rejected_url_update_preserves_both_indexes_and_success_replaces_only_its_owner() {
    let f = fixture().await;
    let other = StreamerDbModel::new("Other", "https://example.test/taken", "platform-huya");
    f.repository.create_streamer(&other).await.unwrap();
    let original = f.manager.get_streamer_snapshot("state-test").unwrap();
    let mut rejected = patch();
    rejected.url = Some(other.url.clone());
    assert!(f.manager.partial_update_streamer(rejected).await.is_err());
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    assert_eq!(
        f.manager.get_streamer_by_url(&original.url).unwrap().id,
        original.id
    );
    assert_eq!(
        f.manager.get_streamer_by_url(&other.url).unwrap().id,
        other.id
    );
    let mut accepted = patch();
    accepted.url = Some("https://example.test/renamed".into());
    f.manager.partial_update_streamer(accepted).await.unwrap();
    assert!(f.manager.get_streamer_by_url(&original.url).is_none());
    assert_eq!(
        f.manager
            .get_streamer_by_url("https://example.test/RENAMED")
            .unwrap()
            .id,
        "state-test"
    );
    assert_eq!(
        f.manager.get_streamer_by_url(&other.url).unwrap().id,
        other.id
    );
}

#[tokio::test]
async fn admin_patch_uses_committed_credentials_and_counters_instead_of_its_old_cache() {
    use crate::credentials::{
        CredentialScope, CredentialSource, CredentialStore, RefreshedCredentials,
    };
    let f = fixture().await;
    f.repository
        .increment_error_count("state-test")
        .await
        .unwrap();
    let credentials = Arc::new(crate::database::repositories::SqlxCredentialStore::new(
        f.pool.clone(),
        f.pool.clone(),
    ));
    credentials.bind_committed_streamers(f.store.clone());
    let source = CredentialSource {
        scope: CredentialScope::Streamer {
            streamer_id: "state-test".into(),
            streamer_name: "Original".into(),
        },
        cookies: "old".into(),
        refresh_token: None,
        access_token: None,
        platform_name: "huya".into(),
        reauth_extra: None,
    };
    let updated = RefreshedCredentials {
        cookies: "new".into(),
        refresh_token: Some("rotated".into()),
        access_token: None,
        expires_at: None,
    };
    let gate = Arc::new(CommitTestGate::default());
    f.writer
        .set_commit_gate(CommitPhase::AfterCommit, Some(gate.clone()));
    let refresh =
        tokio::spawn(async move { credentials.update_credentials(&source, &updated).await });
    reached(&gate.started).await;
    f.writer.set_commit_gate(CommitPhase::AfterCommit, None);
    let manager = f.manager.clone();
    let mut edit = patch();
    edit.name = Some("Renamed".into());
    let admin = tokio::spawn(async move { manager.partial_update_streamer(edit).await });
    gate.release.notify_one();
    refresh.await.unwrap().unwrap();
    admin.await.unwrap().unwrap();
    let row = f.repository.get_streamer("state-test").await.unwrap();
    assert_eq!(row.name, "Renamed");
    assert_eq!(row.consecutive_error_count, Some(1));
    assert_eq!(row.created_at, 1_788_784_496_123);
    let json: serde_json::Value =
        serde_json::from_str(row.streamer_specific_config.as_deref().unwrap()).unwrap();
    assert_eq!(json["cookies"], "new");
    assert_eq!(json["refresh_token"], "rotated");
    assert_eq!(json["extension"]["retained"], true);
    assert_eq!(
        f.manager
            .get_streamer("state-test")
            .unwrap()
            .streamer_specific_config,
        row.streamer_specific_config
    );
}

fn start() -> StartSessionInputs {
    StartSessionInputs {
        streamer_id: "state-test".into(),
        streamer_name: "Original".into(),
        streamer_url: "https://example.test/original".into(),
        current_avatar: None,
        new_avatar: Some("avatar".into()),
        title: "Title".into(),
        category: None,
        streams: Vec::new(),
        media_headers: None,
        media_extras: None,
        now: chrono::Utc::now(),
    }
}

#[tokio::test]
async fn lifecycle_fault_rolls_back_streamer_session_and_outbox_then_publishes_full_success() {
    let f = fixture().await;
    let lifecycle =
        SessionLifecycleRepository::new(f.pool.clone()).with_committed_state(f.store.clone());
    let original = f.manager.get_streamer_snapshot("state-test").unwrap();
    sqlx::query("CREATE TRIGGER reject_state_outbox BEFORE INSERT ON monitor_event_outbox BEGIN SELECT RAISE(ABORT, 'outbox fault'); END").execute(&f.pool).await.unwrap();
    assert!(lifecycle.start_or_resume(start()).await.is_err());
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT COUNT(*) FROM live_sessions),(SELECT COUNT(*) FROM monitor_event_outbox),(SELECT COUNT(*) FROM session_events)").fetch_one(&f.pool).await.unwrap();
    assert_eq!(counts, (0, 0, 0));
    sqlx::query("DROP TRIGGER reject_state_outbox")
        .execute(&f.pool)
        .await
        .unwrap();
    lifecycle.start_or_resume(start()).await.unwrap();
    let current = f.manager.get_streamer_snapshot("state-test").unwrap();
    assert_eq!(current.state, StreamerState::Live);
    assert_eq!(current.avatar_url.as_deref(), Some("avatar"));
    assert!(current.last_live_time.is_some());
}

#[tokio::test]
async fn retired_and_disabled_rows_reject_stale_lifecycle_and_monitor_mutations() {
    for retired in [false, true] {
        let f = fixture().await;
        let lifecycle =
            SessionLifecycleRepository::new(f.pool.clone()).with_committed_state(f.store.clone());
        if retired {
            f.manager.mark_deleting("state-test").await.unwrap();
        } else {
            let mut update = patch();
            update.state = Some(StreamerState::Disabled);
            f.manager.partial_update_streamer(update).await.unwrap();
        }
        assert!(matches!(
            lifecycle.start_or_resume(start()).await.unwrap(),
            crate::database::repositories::StartSessionOutcome::SuppressedInactive { .. }
        ));
        let skipped = f
            .store
            .transaction("stale monitor write", StatePublication::StateOnly, |tx| {
                Box::pin(async move {
                    if !StreamerTxOps::monitor_may_write(&mut *tx, "state-test").await? {
                        return Ok(StateChange::row(true, None));
                    }
                    let row = StreamerTxOps::increment_error_row(tx, "state-test", "stale").await?;
                    Ok(StateChange::row(false, Some(row)))
                })
            })
            .await
            .unwrap();
        assert!(skipped);
        let row = f.repository.get_streamer("state-test").await.unwrap();
        assert_eq!(row.consecutive_error_count, Some(0));
        assert_eq!(row.deleted_at.is_some(), retired);
        assert!(
            !f.manager
                .get_streamer_snapshot("state-test")
                .unwrap()
                .is_active()
        );
    }
}

#[tokio::test]
async fn committed_hot_write_does_not_need_the_repository_read_pool() {
    let f = fixture().await;
    let read = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    read.close().await;
    let repository = Arc::new(
        SqlxStreamerRepository::new(read, f.pool.clone()).with_committed_state(f.store.clone()),
    );
    let manager = StreamerManager::new(repository, f.store.cache.broadcaster.clone());
    manager
        .update_state("state-test", StreamerState::Live)
        .await
        .unwrap();
    assert_eq!(
        manager.get_streamer_snapshot("state-test").unwrap().state,
        StreamerState::Live
    );
}

#[tokio::test]
async fn commit_failure_rolls_back_without_publishing_the_returned_row() {
    let f = fixture().await;
    let original = f.manager.get_streamer_snapshot("state-test").unwrap();
    let mut events = f.store.cache.broadcaster.subscribe();
    let failed = f.store.transaction("deferred constraint fault", StatePublication::StateOnly, |tx| Box::pin(async move {
        sqlx::query("PRAGMA defer_foreign_keys = ON").execute(&mut *tx).await?;
        let row = sqlx::query_as::<_, StreamerDbModel>("UPDATE streamers SET platform_config_id = 'missing-platform' WHERE id = 'state-test' RETURNING *").fetch_optional(&mut *tx).await?;
        Ok(StateChange::row((), row))
    })).await;
    assert!(failed.is_err(), "deferred foreign key fails at COMMIT");
    assert_eq!(
        f.repository
            .get_streamer("state-test")
            .await
            .unwrap()
            .platform_config_id,
        "platform-huya"
    );
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn silent_removal_invalidates_without_events_and_deletion_notifies_after_invalidation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let f = fixture().await;
    let events = Arc::new(parking_lot::Mutex::new(
        f.store.cache.broadcaster.subscribe(),
    ));
    let observed_events = events.clone();
    let invalidated = Arc::new(AtomicUsize::new(0));
    let count = invalidated.clone();
    f.store.on_removed(Arc::new(move |_| {
        assert!(
            observed_events.lock().try_recv().is_err(),
            "no removal event may precede invalidation"
        );
        count.fetch_add(1, Ordering::SeqCst);
    }));
    let removal = StateChange {
        value: (),
        rows: Vec::new(),
        removed: vec!["state-test".into()],
    };
    f.store.cache.apply(&removal, StatePublication::Silent);
    assert_eq!(invalidated.load(Ordering::SeqCst), 1);
    assert!(events.lock().try_recv().is_err());
    let row = f.repository.get_streamer("state-test").await.unwrap();
    f.store
        .cache
        .apply(&StateChange::row((), Some(row)), StatePublication::Silent);
    f.store.cache.apply(&removal, StatePublication::Deleted);
    assert_eq!(invalidated.load(Ordering::SeqCst), 2);
    assert!(
        matches!(events.lock().try_recv().unwrap(), ConfigUpdateEvent::StreamerDeleted { streamer_id } if streamer_id == "state-test")
    );
}

#[tokio::test]
async fn full_row_publication_rejects_a_multi_connection_writer() {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 2)
        .await
        .unwrap();
    let supervisor = Arc::new(TaskSupervisor::new());
    assert!(CommittedWriter::new(pool.clone(), supervisor.clone()).is_err());
    let invalidation_writer = Arc::new(CommittedWriter::for_invalidation(pool, supervisor));
    let store = CommittedStreamerState::new(invalidation_writer, ConfigEventBroadcaster::new());
    let result: Result<()> = store
        .transaction("invalid row publisher", StatePublication::Silent, |_| {
            Box::pin(async { panic!("invalid writer must be rejected before SQL") })
        })
        .await;
    assert!(matches!(result, Err(crate::Error::Configuration(_))));
}

#[tokio::test]
async fn manager_mutation_notifications_survive_cancellation_inside_commit() {
    for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
        for operation in ["create", "update", "patch"] {
            let f = fixture().await;
            let mut events = f.store.cache.broadcaster.subscribe();
            let gate = Arc::new(CommitTestGate::default());
            f.writer.set_commit_gate(phase, Some(gate.clone()));
            let manager = f.manager.clone();
            let caller = tokio::spawn(async move {
                match operation {
                    "create" => {
                        let mut row = StreamerDbModel::new(
                            "Created",
                            "https://example.test/created",
                            "platform-huya",
                        );
                        row.id = "created".into();
                        manager
                            .create_streamer(StreamerMetadata::from_db_model(&row))
                            .await
                    }
                    "update" => {
                        let mut metadata = manager.get_streamer("state-test").unwrap();
                        metadata.name = "Updated".into();
                        manager.update_streamer(metadata).await
                    }
                    _ => {
                        let mut update = patch();
                        update.state = Some(StreamerState::Disabled);
                        manager.partial_update_streamer(update).await.map(|_| ())
                    }
                }
            });
            reached(&gate.started).await;
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            gate.release.notify_one();
            drop(
                tokio::time::timeout(Duration::from_secs(2), f.pool.acquire())
                    .await
                    .unwrap()
                    .unwrap(),
            );
            let id = if operation == "create" {
                "created"
            } else {
                "state-test"
            };
            assert!(
                matches!(events.try_recv().unwrap(), ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } if streamer_id == id)
            );
            assert!(
                events.try_recv().is_err(),
                "manager write emits only its metadata notification"
            );
            let row = f.repository.get_streamer(id).await.unwrap();
            let cached = f.manager.get_streamer(id).unwrap();
            assert_eq!(cached.name, row.name);
            assert_eq!(cached.state.to_string(), row.state);
        }
    }
}

#[tokio::test]
async fn manager_cancelled_before_writer_admission_does_not_mutate_or_notify() {
    let f = fixture().await;
    let lease = f.pool.acquire().await.unwrap();
    let mut events = f.store.cache.broadcaster.subscribe();
    let original = f.manager.get_streamer_snapshot("state-test").unwrap();
    let manager = f.manager.clone();
    let caller = tokio::spawn(async move {
        let mut update = patch();
        update.state = Some(StreamerState::Disabled);
        manager.partial_update_streamer(update).await
    });
    tokio::task::yield_now().await;
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    drop(lease);
    assert!(Arc::ptr_eq(
        &original,
        &f.manager.get_streamer_snapshot("state-test").unwrap()
    ));
    assert_eq!(
        f.repository.get_streamer("state-test").await.unwrap().state,
        "NOT_LIVE"
    );
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn mismatched_repository_pool_cannot_write_through_another_owner() {
    let f = fixture().await;
    let other = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    let repository =
        SqlxStreamerRepository::new(other.clone(), other).with_committed_state(f.store.clone());
    assert!(matches!(
        repository.update_streamer_state("state-test", "LIVE").await,
        Err(crate::Error::Configuration(_))
    ));
    assert_eq!(
        f.repository.get_streamer("state-test").await.unwrap().state,
        "NOT_LIVE"
    );
}
