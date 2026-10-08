use super::super::*;

use crate::credentials::UnavailableReason;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::StreamerRepository;

const STREAMER: &str = "blocked-owner";

async fn fixture() -> ServiceContainer {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let container = ServiceContainer::new(pool.clone(), pool).await.unwrap();
    let mut streamer = StreamerDbModel::new(
        "Blocked owner",
        "https://example.test/blocked-owner",
        "platform-twitch",
    );
    streamer.id = STREAMER.into();
    container
        .streamer_repository
        .create_streamer(&streamer)
        .await
        .unwrap();
    container.streamer_manager.hydrate().await.unwrap();
    container
}

fn block(container: &ServiceContainer) {
    container.stream_monitor.credential_blocks().block(
        STREAMER,
        "platform-twitch",
        UnavailableReason::LoginRequired,
    );
}

fn blocked(container: &ServiceContainer) -> bool {
    container
        .stream_monitor
        .credential_blocks()
        .get(STREAMER)
        .is_some()
}

#[tokio::test]
async fn streamers_that_stop_being_monitored_lose_their_credential_block() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let container = fixture().await;

        block(&container);
        container
            .apply_config_event_for_test(ConfigUpdateEvent::StreamerMetadataUpdated {
                streamer_id: STREAMER.into(),
            })
            .await;
        assert!(
            blocked(&container),
            "an edit to a monitored streamer keeps it"
        );

        container
            .apply_config_event_for_test(ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id: STREAMER.into(),
                is_active: false,
            })
            .await;
        assert!(!blocked(&container));

        block(&container);
        container
            .apply_config_event_for_test(ConfigUpdateEvent::StreamerDeleted {
                streamer_id: STREAMER.into(),
            })
            .await;
        assert!(!blocked(&container));

        container.stream_monitor.stop();
        container.notification_service.stop().await;
        container.cancellation_token.cancel();
        container
            .task_supervisor
            .shutdown(Duration::from_secs(2))
            .await;
    })
    .await
    .expect("config events must be applied");
}
