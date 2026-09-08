use std::sync::Arc;
use std::time::Duration;

use crate::database::models::StreamerDbModel;

use super::ServiceContainer;

#[tokio::test]
async fn api_states_retain_shared_services_and_keep_archive_caches_local() {
    let stage = std::cell::Cell::new("open SQLite pool");
    tokio::time::timeout(Duration::from_secs(30), async {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        stage.set("run migrations");
        crate::database::run_migrations(&pool).await.unwrap();
        stage.set("build container");
        let container = ServiceContainer::new(pool.clone(), pool).await.unwrap();
        stage.set("assemble API states");
        let (logging, _layer) =
            crate::logging::LoggingConfig::for_route_tests(directory.path().to_path_buf());
        assert!(container.logging_config.set(Arc::new(logging)).is_ok());
        let first = container.build_api_state(None).unwrap();
        let second = container.build_api_state(None).unwrap();

        assert!(Arc::ptr_eq(
            &first.configuration_import_service,
            &container.configuration_import_service
        ));
        assert!(Arc::ptr_eq(
            &first.configuration_import_service,
            &second.configuration_import_service
        ));
        assert!(Arc::ptr_eq(
            &first.runtime_coordinator,
            &container.runtime_coordinator
        ));
        assert!(Arc::ptr_eq(
            &first.session_event_repository,
            &container.session_event_repository
        ));
        assert!(Arc::ptr_eq(
            &first.streamer_check_history_repository,
            &container.streamer_check_history_repository
        ));
        assert!(Arc::ptr_eq(
            &first.upload_record_repository,
            &container.upload_record_repository
        ));
        assert!(Arc::ptr_eq(
            &first.filter_repository,
            &second.filter_repository
        ));
        assert!(Arc::ptr_eq(
            &first.streamer_repository,
            &second.streamer_repository
        ));
        assert!(Arc::ptr_eq(
            &first.job_preset_repository,
            &second.job_preset_repository
        ));
        assert!(Arc::ptr_eq(
            &first.pipeline_preset_repository,
            &second.pipeline_preset_repository
        ));
        assert!(!Arc::ptr_eq(
            &first.logging_download_tokens,
            &second.logging_download_tokens
        ));
        assert!(!Arc::ptr_eq(
            &first.logging_archives,
            &second.logging_archives
        ));

        let streamer =
            StreamerDbModel::new("Shared owner", "https://example.com/owner", "platform-huya");
        stage.set("write streamer through first API state");
        first
            .streamer_repository
            .create_streamer(&streamer)
            .await
            .unwrap();
        stage.set("stop constructor-owned services");
        // These services use their own cancellation tokens for tasks registered
        // with the shared supervisor. Stop them before joining, without the
        // full container shutdown that would close the retained API's SQL pools.
        container.stream_monitor.stop();
        container.notification_service.stop().await;
        stage.set("join container background tasks");
        container.cancellation_token.cancel();
        assert!(
            container
                .task_supervisor
                .shutdown(Duration::from_secs(1))
                .await
        );
        stage.set("drop container and first API state");
        drop(container);
        drop(first);
        stage.set("read streamer through retained API state");
        assert_eq!(
            second
                .streamer_repository
                .get_streamer(&streamer.id)
                .await
                .unwrap()
                .name,
            "Shared owner"
        );
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "real-container API ownership timed out during {}",
            stage.get()
        )
    });
}
