use super::super::*;

use crate::database::filter_store::FilterStore;
use crate::database::models::{FilterDbModel, FilterType, StreamerDbModel};
use crate::database::repositories::{FilterRepository, StreamerRepository};
use crate::domain::filter::Filter;
use tokio_util::task::AbortOnDropHandle;

async fn fixture() -> (ServiceContainer, Arc<FilterStore>, FilterDbModel) {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = ServiceContainer::new(pool.clone(), pool).await.unwrap();
    let mut streamer = StreamerDbModel::new(
        "Filter owner",
        "https://example.test/filter-owner",
        "platform-twitch",
    );
    streamer.id = "filter-owner".into();
    container
        .streamer_repository
        .create_streamer(&streamer)
        .await
        .unwrap();
    let filter = FilterDbModel::new(
        "filter-owner",
        FilterType::Keyword,
        r#"{"include":["before"],"exclude":[]}"#,
    );
    container
        .filter_repository
        .create_filter(&filter)
        .await
        .unwrap();
    let store = container
        .config_service
        .filter_store_for(container.filter_repository.clone());
    assert_eq!(current(&store).await, "before");
    (container, store, filter)
}

async fn current(store: &FilterStore) -> String {
    let filters = store.get("filter-owner").await.unwrap();
    let Filter::Keyword(filter) = &filters[0] else {
        panic!("expected keyword")
    };
    filter.include[0].clone()
}

async fn raw_change(container: &ServiceContainer, filter: &FilterDbModel) {
    sqlx::query("UPDATE filters SET config = ? WHERE id = ?")
        .bind(r#"{"include":["after"],"exclude":[]}"#)
        .bind(&filter.id)
        .execute(&container.write_pool)
        .await
        .unwrap();
}

async fn stop_constructor_tasks(container: ServiceContainer) {
    container.stream_monitor.stop();
    container.notification_service.stop().await;
    container.cancellation_token.cancel();
    container
        .task_supervisor
        .shutdown(Duration::from_secs(2))
        .await;
}

#[tokio::test]
async fn runtime_filter_and_deletion_events_invalidate_the_container_store() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (container, store, filter) = fixture().await;
        raw_change(&container, &filter).await;
        assert_eq!(current(&store).await, "before");
        container
            .apply_config_event_for_test(ConfigUpdateEvent::StreamerFiltersUpdated {
                streamer_id: "filter-owner".into(),
            })
            .await;
        assert_eq!(current(&store).await, "after");
        sqlx::query("DELETE FROM streamers WHERE id = 'filter-owner'")
            .execute(&container.write_pool)
            .await
            .unwrap();
        container
            .apply_config_event_for_test(ConfigUpdateEvent::StreamerDeleted {
                streamer_id: "filter-owner".into(),
            })
            .await;
        assert!(store.get("filter-owner").await.unwrap().is_empty());
        stop_constructor_tasks(container).await;
    })
    .await
    .expect("runtime invalidation must reach the shared monitor store");
}

#[tokio::test]
async fn lagged_config_receiver_reconciles_snapshots_without_the_lost_filter_event() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let (container, store, filter) = fixture().await;
        raw_change(&container, &filter).await;
        let handler = ConfigEventHandler {
            streamer_manager: container.streamer_manager.clone(),
            config_service: container.config_service.clone(),
            download_manager: container.download_manager.clone(),
            pipeline_manager: container.pipeline_manager.clone(),
            runtime_coordinator: container.runtime_coordinator.clone(),
            gpu_health_monitor: None,
        };
        let (events, receiver) = broadcast::channel(1);
        events
            .send(ConfigUpdateEvent::StreamerFiltersUpdated {
                streamer_id: "filter-owner".into(),
            })
            .unwrap();
        events
            .send(ConfigUpdateEvent::EngineUpdated {
                engine_id: "unchanged".into(),
            })
            .unwrap();
        let cancel = CancellationToken::new();
        let task = AbortOnDropHandle::new(tokio::spawn(handler.run(receiver, cancel.clone())));
        while current(&store).await != "after" {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
        task.await.unwrap();
        stop_constructor_tasks(container).await;
    })
    .await
    .expect("lag recovery must invalidate even when the filter event was overwritten");
}
