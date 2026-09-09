use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, response::IntoResponse};
use serde_json::json;

use crate::config::ConfigUpdateEvent;
use crate::config::backup::{ConfigExport, ImportMode, JobPresetExport};
use crate::database::models::StreamerDbModel;

use super::ServiceContainer;

#[tokio::test]
async fn public_import_preserves_warm_caches_until_commit_and_publishes_only_after_success() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let container = ServiceContainer::new(pool.clone(), pool.clone()).await.unwrap();
        let (logging, _layer) = crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
        assert!(container.logging_config.set(Arc::new(logging)).is_ok());
        let state = container.build_api_state(None).unwrap();
        let streamer = StreamerDbModel::new("Warm cache", "https://example.com/warm-cache", "platform-huya");
        state.streamer_repository.create_streamer(&streamer).await.unwrap();
        let cached_global = state.config_service.get_cached_global_config().await.unwrap();
        let cached_streamer = state.config_service.get_config_for_streamer(&streamer.id).await.unwrap();
        let filter_store = state.config_service.filter_store_for(state.filter_repository.clone());
        let cached_filters = filter_store.get(&streamer.id).await.unwrap();
        let exported = crate::api::routes::export_import::export_config(State(state.clone())).await.unwrap().into_response();
        let bytes = axum::body::to_bytes(exported.into_body(), 4 * 1024 * 1024).await.unwrap();
        let mut config: ConfigExport = serde_json::from_slice(&bytes).unwrap();
        // Merge leaves the unmentioned streamer active; only global configuration changes.
        config.streamers.clear(); config.users.clear();
        let mut events = container.event_broadcaster.subscribe();
        let mut failing = config.clone();
        failing.global_config.output_folder = directory.path().join("rejected").to_string_lossy().into_owned();
        failing.job_presets.push(JobPresetExport { name: "late-failure".into(), description: None, category: None, processor: "remux".into(), config: json!({}) });
        sqlx::query("CREATE TRIGGER reject_public_import BEFORE INSERT ON job_presets WHEN NEW.name = 'late-failure' BEGIN SELECT RAISE(ABORT, 'public import failure'); END").execute(&pool).await.unwrap();
        assert!(state.configuration_import_service.import(failing, ImportMode::Merge).await.is_err());
        assert!(events.try_recv().is_err(), "failed import must publish nothing");
        assert!(Arc::ptr_eq(&cached_global, &state.config_service.get_cached_global_config().await.unwrap()));
        assert!(Arc::ptr_eq(&cached_streamer, &state.config_service.get_config_for_streamer(&streamer.id).await.unwrap()));
        assert!(Arc::ptr_eq(&cached_filters, &filter_store.get(&streamer.id).await.unwrap()), "rollback retains the filter snapshot");
        assert_eq!(state.config_service.get_global_config().await.unwrap().output_folder, cached_global.output_folder);

        config.global_config.output_folder = directory.path().join("committed").to_string_lossy().into_owned();
        state.configuration_import_service.import(config.clone(), ImportMode::Merge).await.unwrap();
        assert!(matches!(events.try_recv().unwrap(), ConfigUpdateEvent::GlobalUpdated));
        assert!(events.try_recv().is_err());
        let global = state.config_service.get_cached_global_config().await.unwrap();
        assert!(!Arc::ptr_eq(&cached_global, &global));
        assert_eq!(global.output_folder, config.global_config.output_folder);
        let merged = state.config_service.get_config_for_streamer(&streamer.id).await.unwrap();
        assert!(!Arc::ptr_eq(&cached_streamer, &merged));
        assert_eq!(merged.output_folder, config.global_config.output_folder);
        assert!(!Arc::ptr_eq(&cached_filters, &filter_store.get(&streamer.id).await.unwrap()), "committed import invalidates filters before notification");
        assert_eq!(state.streamer_repository.get_streamer(&streamer.id).await.unwrap().id, streamer.id);
        container.stream_monitor.stop();
        container.notification_service.stop().await;
        container.pipeline_manager.stop().await;
        container.cancellation_token.cancel();
        assert!(container.task_supervisor.shutdown(Duration::from_secs(1)).await);
    }).await.expect("public import/cache scenario must finish");
}

#[tokio::test]
async fn cancelled_import_commit_finishes_filter_invalidation_and_runtime_publication() {
    use crate::database::committed_writer::{CommitPhase, CommitTestGate};
    use crate::database::models::{FilterDbModel, FilterType};
    for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let container = ServiceContainer::new(pool.clone(), pool.clone())
            .await
            .unwrap();
        let (logging, _layer) =
            crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
        assert!(container.logging_config.set(Arc::new(logging)).is_ok());
        let state = container.build_api_state(None).unwrap();
        let streamer = StreamerDbModel::new(
            "Before import",
            "https://example.test/cancelled-import",
            "platform-huya",
        );
        state
            .streamer_repository
            .create_streamer(&streamer)
            .await
            .unwrap();
        state
            .filter_repository
            .create_filter(&FilterDbModel::new(
                &streamer.id,
                FilterType::Keyword,
                r#"{"include":["original"],"exclude":[]}"#,
            ))
            .await
            .unwrap();
        let filters = state
            .config_service
            .filter_store_for(state.filter_repository.clone());
        let original = filters.get(&streamer.id).await.unwrap();
        assert_eq!(original.len(), 1);
        let exported = crate::api::routes::export_import::export_config(State(state.clone()))
            .await
            .unwrap()
            .into_response();
        let bytes = axum::body::to_bytes(exported.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let mut config: ConfigExport = serde_json::from_slice(&bytes).unwrap();
        config.users.clear();
        config.streamers[0].name = "Committed import".into();
        config.streamers[0].filters.clear();
        let writer = state
            .streamer_manager
            .committed_state()
            .unwrap()
            .writer
            .clone();
        let gate = Arc::new(CommitTestGate::default());
        writer.set_commit_gate(phase, Some(gate.clone()));
        let mut events = container.event_broadcaster.subscribe();
        let importer = state.configuration_import_service.clone();
        let caller = tokio::spawn(async move { importer.import(config, ImportMode::Merge).await });
        tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &original,
            &filters.get(&streamer.id).await.unwrap()
        ));
        caller.abort();
        assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
        writer.set_commit_gate(phase, None);
        gate.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if matches!(
                    events.recv().await.unwrap(),
                    ConfigUpdateEvent::GlobalUpdated
                ) {
                    break;
                }
            }
        })
        .await
        .expect("owned import runtime effects survive cancellation");
        assert!(filters.get(&streamer.id).await.unwrap().is_empty());
        assert_eq!(
            state
                .streamer_manager
                .get_streamer(&streamer.id)
                .unwrap()
                .name,
            "Committed import"
        );
        let filter_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM filters WHERE streamer_id = ?")
                .bind(&streamer.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(filter_count, 0);
        container.stream_monitor.stop();
        container.notification_service.stop().await;
        container.pipeline_manager.stop().await;
        container.cancellation_token.cancel();
        assert!(
            container
                .task_supervisor
                .shutdown(Duration::from_secs(1))
                .await
        );
    }
}

#[tokio::test]
async fn committed_reaping_invalidates_filters_only_after_delete_succeeds() {
    use crate::database::models::{FilterDbModel, FilterType};
    let directory = tempfile::tempdir().unwrap();
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = ServiceContainer::new(pool.clone(), pool.clone())
        .await
        .unwrap();
    let (logging, _layer) =
        crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
    assert!(container.logging_config.set(Arc::new(logging)).is_ok());
    let state = container.build_api_state(None).unwrap();
    let streamer = StreamerDbModel::new(
        "Retire",
        "https://example.test/reap-filter",
        "platform-huya",
    );
    state
        .streamer_repository
        .create_streamer(&streamer)
        .await
        .unwrap();
    state
        .filter_repository
        .create_filter(&FilterDbModel::new(
            &streamer.id,
            FilterType::Keyword,
            r#"{"include":["retire"],"exclude":[]}"#,
        ))
        .await
        .unwrap();
    let filters = state
        .config_service
        .filter_store_for(state.filter_repository.clone());
    let original = filters.get(&streamer.id).await.unwrap();
    assert_eq!(original.len(), 1);
    state
        .streamer_manager
        .mark_deleting(&streamer.id)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_reaping BEFORE DELETE ON streamers BEGIN SELECT RAISE(ABORT, 'reap fault'); END").execute(&pool).await.unwrap();
    let mut events = container.event_broadcaster.subscribe();
    assert!(
        state
            .streamer_manager
            .reap_deleted(&streamer.id)
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &original,
        &filters.get(&streamer.id).await.unwrap()
    ));
    assert!(events.try_recv().is_err());
    sqlx::query("DROP TRIGGER reject_reaping")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        state
            .streamer_manager
            .reap_deleted(&streamer.id)
            .await
            .unwrap()
    );
    assert!(
        matches!(events.try_recv().unwrap(), ConfigUpdateEvent::StreamerDeleted { streamer_id } if streamer_id == streamer.id)
    );
    assert!(filters.get(&streamer.id).await.unwrap().is_empty());
    assert!(
        state
            .streamer_manager
            .get_streamer_by_url(&streamer.url)
            .is_none()
    );
    container.stream_monitor.stop();
    container.notification_service.stop().await;
    container.pipeline_manager.stop().await;
    container.cancellation_token.cancel();
    assert!(
        container
            .task_supervisor
            .shutdown(Duration::from_secs(1))
            .await
    );
}

#[tokio::test]
async fn cancelled_credential_commit_invalidates_merged_configuration_without_its_caller() {
    use crate::database::committed_writer::{CommitPhase, CommitTestGate};
    let directory = tempfile::tempdir().unwrap();
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = ServiceContainer::new(pool.clone(), pool.clone())
        .await
        .unwrap();
    let (logging, _layer) =
        crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
    assert!(container.logging_config.set(Arc::new(logging)).is_ok());
    let state = container.build_api_state(None).unwrap();
    let mut streamer = StreamerDbModel::new(
        "Credentials",
        "https://example.test/credential-commit",
        "platform-huya",
    );
    streamer.streamer_specific_config = Some(r#"{"cookies":"session=before"}"#.into());
    state
        .streamer_repository
        .create_streamer(&streamer)
        .await
        .unwrap();
    let previous = state
        .config_service
        .get_context_for_streamer(&streamer.id)
        .await
        .unwrap();
    assert_eq!(previous.config.cookies.as_deref(), Some("session=before"));
    let source = previous.credential_source.clone().unwrap();
    let owner = state.streamer_manager.committed_state().unwrap();
    let gate = Arc::new(CommitTestGate::default());
    owner
        .writer
        .set_commit_gate(CommitPhase::AfterCommit, Some(gate.clone()));
    let credentials = state.credential_service.clone();
    let caller = tokio::spawn(async move {
        credentials
            .persist_session_cookies(&source, "session=after".into())
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    owner.writer.set_commit_gate(CommitPhase::AfterCommit, None);
    gate.release.notify_one();
    let connection = tokio::time::timeout(Duration::from_secs(2), pool.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(connection);
    let current = state
        .config_service
        .get_context_for_streamer(&streamer.id)
        .await
        .unwrap();
    assert!(!Arc::ptr_eq(&previous, &current));
    assert_eq!(current.config.cookies.as_deref(), Some("session=after"));
    container.stream_monitor.stop();
    container.notification_service.stop().await;
    container.pipeline_manager.stop().await;
    container.cancellation_token.cancel();
    assert!(
        container
            .task_supervisor
            .shutdown(Duration::from_secs(1))
            .await
    );
}
