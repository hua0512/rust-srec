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
        assert_eq!(state.streamer_repository.get_streamer(&streamer.id).await.unwrap().id, streamer.id);
        container.stream_monitor.stop();
        container.notification_service.stop().await;
        container.pipeline_manager.stop().await;
        container.cancellation_token.cancel();
        assert!(container.task_supervisor.shutdown(Duration::from_secs(1)).await);
    }).await.expect("public import/cache scenario must finish");
}
