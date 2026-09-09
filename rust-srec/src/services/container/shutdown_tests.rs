use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, response::IntoResponse};
use tokio::sync::{Notify, oneshot};

use super::ServiceContainer;
use crate::api::server::{ApiServices, AppState};
use crate::config::ConfigUpdateEvent;
use crate::config::backup::{ConfigExport, ImportMode};
use crate::database::committed_writer::{CommitPhase, CommitTestGate};
use crate::database::models::StreamerDbModel;
use crate::utils::task_supervisor::TaskSupervisor;

async fn fixture() -> (
    tempfile::TempDir,
    Arc<ServiceContainer>,
    AppState,
    StreamerDbModel,
) {
    let directory = tempfile::tempdir().unwrap();
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = Arc::new(ServiceContainer::new(pool.clone(), pool).await.unwrap());
    let (logging, _layer) =
        crate::logging::LoggingConfig::for_route_tests(directory.path().to_owned());
    let state = AppState::new(ApiServices {
        config_service: container.config_service.clone(),
        streamer_manager: container.streamer_manager.clone(),
        pipeline_manager: container.pipeline_manager.clone(),
        download_manager: container.download_manager.clone(),
        session_repository: container.session_repository.clone(),
        session_event_repository: container.session_event_repository.clone(),
        streamer_check_history_repository: container.streamer_check_history_repository.clone(),
        check_history_broadcaster: container.check_history_broadcaster.clone(),
        upload_status_broadcaster: container.upload_status_broadcaster.clone(),
        upload_record_repository: container.upload_record_repository.clone(),
        filter_repository: container.filter_repository.clone(),
        health_checker: container.health_checker.clone(),
        streamer_repository: container.streamer_repository.clone(),
        pipeline_preset_repository: container.pipeline_preset_repository.clone(),
        job_preset_repository: container.job_preset_repository.clone(),
        notification_repository: container.notification_repository.clone(),
        notification_service: container.notification_service.clone(),
        logging_config: Arc::new(logging),
        logging_download_tokens: Arc::new(dashmap::DashMap::new()),
        logging_archives: Arc::new(crate::api::routes::logging::LogArchiveService::new()),
        credential_service: container.credential_service.clone(),
        configuration_import_service: container.configuration_import_service.clone(),
        runtime_coordinator: container.runtime_coordinator.clone(),
    });
    let mut row = StreamerDbModel::new(
        "Before shutdown",
        "https://example.test/retained-commit",
        "platform-huya",
    );
    row.streamer_specific_config = Some(r#"{"cookies":"session=before"}"#.into());
    state
        .streamer_repository
        .create_streamer(&row)
        .await
        .unwrap();
    (directory, container, state, row)
}

async fn exported(state: &AppState) -> ConfigExport {
    let response = crate::api::routes::export_import::export_config(State(state.clone()))
        .await
        .unwrap()
        .into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    let mut config: ConfigExport = serde_json::from_slice(&bytes).unwrap();
    config.users.clear();
    config.streamers[0].name = "After shutdown".into();
    config
}

async fn close_fixture_after_hard_cap(container: &ServiceContainer) {
    // A force-deadline shutdown is terminal and may have already sent a one-shot
    // coordination marker; re-entering it cannot replay that drain. These
    // fixtures start no recordings or sessions, so drain their remaining task
    // owners after the retained-write assertions and close the pools explicitly.
    tokio::time::timeout(Duration::from_secs(3), async {
        assert!(
            container
                .committed_write_supervisor
                .drain_retained(Duration::from_secs(1))
                .await
        );
        assert!(
            container
                .task_supervisor
                .shutdown(Duration::from_secs(1))
                .await
        );
        tokio::join!(container.write_pool.close(), container.pool.close());
    })
    .await
    .expect("hard-cap fixture task drains and pool closure must settle");
    assert!(container.pool.is_closed());
    assert!(container.write_pool.is_closed());
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Manager,
    Credential,
    Import,
}

#[tokio::test]
async fn hard_cap_retains_manager_credential_and_import_commit_publication() {
    for phase in [CommitPhase::BeforeCommit, CommitPhase::AfterCommit] {
        for mutation in [Mutation::Manager, Mutation::Credential, Mutation::Import] {
            let (_directory, container, state, row) = fixture().await;
            let previous = state
                .config_service
                .get_context_for_streamer(&row.id)
                .await
                .unwrap();
            let source = previous.credential_source.clone().unwrap();
            let config = exported(&state).await;
            let owner = container.streamer_manager.committed_state().unwrap();
            let gate = Arc::new(CommitTestGate::default());
            owner.writer.set_commit_gate(phase, Some(gate.clone()));
            let mut events = container.event_broadcaster.subscribe();
            let caller_state = state.clone();
            let id = row.id.clone();
            let (reply, received) = oneshot::channel();
            // The real force path aborts this request owner. The transaction
            // and its required publication belong to the retained supervisor.
            assert!(
                container
                    .task_supervisor
                    .spawn("mutation request", async move {
                        let result: crate::Result<()> = match mutation {
                            Mutation::Manager => {
                                let mut metadata =
                                    caller_state.streamer_manager.get_streamer(&id).unwrap();
                                metadata.name = "After shutdown".into();
                                caller_state
                                    .streamer_manager
                                    .update_streamer(metadata)
                                    .await
                            }
                            Mutation::Credential => caller_state
                                .credential_service
                                .persist_session_cookies(&source, "session=after".into())
                                .await
                                .map_err(|error| crate::Error::Other(error.to_string())),
                            Mutation::Import => caller_state
                                .configuration_import_service
                                .import(config, ImportMode::Merge)
                                .await
                                .map(|_| ())
                                .map_err(|error| crate::Error::Other(error.to_string())),
                        };
                        let _ = reply.send(result);
                    })
            );
            tokio::time::timeout(Duration::from_secs(2), gate.started.notified())
                .await
                .unwrap();

            let hard_cap = Duration::from_millis(100);
            let started = std::time::Instant::now();
            let error = tokio::time::timeout(
                Duration::from_secs(2),
                container.shutdown_with_hard_cap(Duration::from_millis(10), hard_cap),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert!(started.elapsed() < hard_cap + Duration::from_millis(500));
            assert!(
                error
                    .to_string()
                    .contains("unfinished committed writes remain supervised")
            );
            assert!(
                received.await.is_err(),
                "the ordinary request owner must be aborted"
            );
            assert!(!container.pool.is_closed());
            assert!(!container.write_pool.is_closed());
            assert!(
                !container
                    .committed_write_supervisor
                    .spawn("late write", async {})
            );
            assert!(
                events.try_recv().is_err(),
                "publication must still wait for COMMIT completion"
            );

            owner.writer.set_commit_gate(phase, None);
            gate.release.notify_one();
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(2),
                    container
                        .committed_write_supervisor
                        .drain_retained(Duration::from_secs(1)),
                )
                .await
                .unwrap()
            );
            let persisted = state
                .streamer_repository
                .get_streamer(&row.id)
                .await
                .unwrap();
            let snapshot = state
                .streamer_manager
                .get_streamer_snapshot(&row.id)
                .unwrap();
            let current = state
                .config_service
                .get_context_for_streamer(&row.id)
                .await
                .unwrap();
            assert!(!Arc::ptr_eq(&previous, &current));
            match mutation {
                Mutation::Manager | Mutation::Import => {
                    assert_eq!(persisted.name, "After shutdown");
                    assert_eq!(snapshot.name, persisted.name);
                    assert!(matches!(events.try_recv().unwrap(),
                        ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } if streamer_id == row.id));
                    if matches!(mutation, Mutation::Import) {
                        assert!(matches!(
                            events.try_recv().unwrap(),
                            ConfigUpdateEvent::GlobalUpdated
                        ));
                    }
                }
                Mutation::Credential => {
                    let config: serde_json::Value = serde_json::from_str(
                        persisted.streamer_specific_config.as_deref().unwrap(),
                    )
                    .unwrap();
                    assert_eq!(config["cookies"], "session=after");
                    assert_eq!(
                        snapshot.streamer_specific_config,
                        persisted.streamer_specific_config
                    );
                    assert_eq!(current.config.cookies.as_deref(), Some("session=after"));
                }
            }
            close_fixture_after_hard_cap(&container).await;
        }
    }
}

#[tokio::test]
async fn hard_cap_still_cancels_a_prepared_write_before_commit() {
    let (_directory, container, state, row) = fixture().await;
    let writer = container
        .streamer_manager
        .committed_state()
        .unwrap()
        .writer
        .clone();
    let prepared = Arc::new(Notify::new());
    let prepared_task = prepared.clone();
    let id = row.id.clone();
    let mut events = container.event_broadcaster.subscribe();
    assert!(
        container
            .task_supervisor
            .spawn("prepared mutation request", async move {
                let result = writer
                    .transaction(
                        "prepared mutation",
                        move |connection| {
                            Box::pin(async move {
                                sqlx::query(
                                    "UPDATE streamers SET name = 'must roll back' WHERE id = ?",
                                )
                                .bind(id)
                                .execute(connection)
                                .await?;
                                prepared_task.notify_one();
                                std::future::pending::<()>().await;
                                Ok(())
                            })
                        },
                        |_| panic!("a cancelled prepared mutation must not publish"),
                    )
                    .await;
                assert!(result.is_err());
            })
    );
    tokio::time::timeout(Duration::from_secs(2), prepared.notified())
        .await
        .unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        container.shutdown_with_hard_cap(Duration::from_millis(10), Duration::from_millis(200)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains("committed writes drained"));
    assert!(!container.pool.is_closed());
    assert_eq!(
        state
            .streamer_repository
            .get_streamer(&row.id)
            .await
            .unwrap()
            .name,
        row.name
    );
    assert_eq!(
        state
            .streamer_manager
            .get_streamer_snapshot(&row.id)
            .unwrap()
            .name,
        row.name
    );
    assert!(events.try_recv().is_err());
    close_fixture_after_hard_cap(&container).await;
}

#[tokio::test]
async fn retained_drain_allows_only_its_own_nested_completion_after_a_hard_timeout() {
    let (_directory, container, state, row) = fixture().await;
    let owner = container.committed_write_supervisor.clone();
    let writer = container
        .streamer_manager
        .committed_state()
        .unwrap()
        .writer
        .clone();
    let other = Arc::new(TaskSupervisor::for_committed_work());
    assert!(other.drain_retained(Duration::ZERO).await);
    let completing = Arc::new(Notify::new());
    let completing_task = completing.clone();
    let release = Arc::new(Notify::new());
    let release_task = release.clone();
    let nested_writer = writer.clone();
    let id = row.id.clone();
    let caller = tokio::spawn(async move {
        writer
            .transaction_with_completion(
                "first committed mutation",
                move |connection| {
                    Box::pin(async move {
                        sqlx::query("UPDATE streamers SET name = 'first commit' WHERE id = ?")
                            .bind(&id)
                            .execute(connection)
                            .await?;
                        Ok::<_, crate::Error>(id)
                    })
                },
                |_| {},
                move |id| {
                    Box::pin(async move {
                        completing_task.notify_one();
                        release_task.notified().await;
                        assert!(!other.spawn("wrong owner", async {}));
                        nested_writer
                            .transaction(
                                "nested committed mutation",
                                move |connection| {
                                    Box::pin(async move {
                                        sqlx::query(
                                            "UPDATE streamers SET name = 'nested commit' WHERE id = ?",
                                        )
                                        .bind(id)
                                        .execute(connection)
                                        .await?;
                                        Ok(())
                                    })
                                },
                                |_| {},
                            )
                            .await
                    })
                },
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), completing.notified())
        .await
        .unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        container.shutdown_with_hard_cap(Duration::from_millis(10), Duration::from_millis(100)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unfinished committed writes remain supervised")
    );
    assert_eq!(
        state
            .streamer_repository
            .get_streamer(&row.id)
            .await
            .unwrap()
            .name,
        "first commit"
    );
    assert!(!owner.spawn("late external caller", async {}));
    release.notify_one();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            owner.drain_retained(Duration::from_secs(1)),
        )
        .await
        .unwrap()
    );
    caller.await.unwrap().unwrap();
    assert_eq!(
        state
            .streamer_repository
            .get_streamer(&row.id)
            .await
            .unwrap()
            .name,
        "nested commit"
    );
    close_fixture_after_hard_cap(&container).await;
}
