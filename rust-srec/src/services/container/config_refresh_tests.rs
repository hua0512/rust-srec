use std::sync::{Arc, Weak};
use std::time::Duration;

use super::ServiceContainer;
use crate::config::ConfigUpdateEvent;
use crate::database::models::{StreamerDbModel, TemplateConfigDbModel};
use crate::database::repositories::{SqlxStreamerRepository, StreamerRepository};

async fn fixture() -> ServiceContainer {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let mut container = ServiceContainer::new(pool.clone(), pool).await.unwrap();
    container.output_root_gate = crate::downloader::OutputRootGate::new(
        Weak::new(),
        Arc::new(|_| {}),
        vec![],
        Duration::from_secs(30),
    );
    container
}

async fn assert_current(container: &ServiceContainer, id: &str) {
    let expected = container
        .config_service
        .get_config_for_streamer(id)
        .await
        .unwrap();
    let actual = container.streamer_manager.get_streamer(id).unwrap();
    assert_eq!(
        (actual.offline_check_count, actual.offline_check_delay_ms),
        (
            expected.offline_check_count,
            expected.offline_check_delay_ms
        ),
        "{id}"
    );
}

fn mark_stale(container: &ServiceContainer) {
    for mut entry in container.streamer_manager.metadata_store().iter_mut() {
        let entry = Arc::make_mut(entry.value_mut());
        entry.offline_check_count = 222;
        entry.offline_check_delay_ms = 333;
    }
}

#[tokio::test]
async fn startup_and_scoped_config_refreshes_preserve_merged_values_and_best_effort_failures() {
    let container = fixture().await;
    let mut template = TemplateConfigDbModel::new("refresh-template");
    template.offline_check_count = Some(6);
    template.offline_check_delay_ms = Some(6000);
    container
        .config_service
        .create_template_config(&template)
        .await
        .unwrap();
    for index in 0..40 {
        let platform = if index % 2 == 0 {
            "platform-twitch"
        } else {
            "platform-douyin"
        };
        let mut streamer = StreamerDbModel::new(
            format!("refresh-{index}"),
            format!("https://example.com/{index}"),
            platform,
        );
        streamer.id = format!("refresh-{index}");
        streamer.state = "DISABLED".to_owned();
        if index % 4 == 0 {
            streamer.template_config_id = Some(template.id.clone());
        }
        if index == 3 {
            streamer.streamer_specific_config =
                Some(r#"{"offline_check_count":7,"offline_check_delay_ms":7000}"#.to_owned());
        }
        SqlxStreamerRepository::new(container.pool.clone(), container.write_pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
    }
    container.streamer_manager.hydrate().await.unwrap();
    // The startup caller uses this exact batch path after hydration.
    container
        .runtime_coordinator
        .refresh_metadata_offline_checks(
            container
                .streamer_manager
                .get_all()
                .into_iter()
                .map(|streamer| streamer.id),
        )
        .await;
    for index in 0..40 {
        assert_current(&container, &format!("refresh-{index}")).await;
    }

    mark_stale(&container);
    template.offline_check_count = Some(8);
    container
        .config_service
        .update_template_config(&template)
        .await
        .unwrap();
    container
        .apply_config_event_for_test(ConfigUpdateEvent::TemplateUpdated {
            template_id: template.id.clone(),
        })
        .await;
    for index in 0..40 {
        let id = format!("refresh-{index}");
        if index % 4 == 0 {
            assert_current(&container, &id).await;
        } else {
            assert_eq!(
                container
                    .streamer_manager
                    .get_streamer(&id)
                    .unwrap()
                    .offline_check_count,
                222
            );
        }
    }

    let updated_template = container
        .streamer_manager
        .get_streamer("refresh-0")
        .unwrap();
    assert_eq!(
        (
            updated_template.offline_check_count,
            updated_template.offline_check_delay_ms
        ),
        (8, 6000)
    );

    mark_stale(&container);
    let mut platform = container
        .config_service
        .get_platform_config("platform-twitch")
        .await
        .unwrap();
    platform.offline_check_count = Some(9);
    platform.offline_check_delay_ms = Some(9000);
    container
        .config_service
        .update_platform_config(&platform)
        .await
        .unwrap();
    container
        .apply_config_event_for_test(ConfigUpdateEvent::PlatformUpdated {
            platform_id: platform.id.clone(),
        })
        .await;
    for index in 0..40 {
        let id = format!("refresh-{index}");
        if index % 2 == 0 {
            assert_current(&container, &id).await;
        } else {
            assert_eq!(
                container
                    .streamer_manager
                    .get_streamer(&id)
                    .unwrap()
                    .offline_check_count,
                222
            );
        }
    }

    let updated_platform = container
        .streamer_manager
        .get_streamer("refresh-2")
        .unwrap();
    assert_eq!(
        (
            updated_platform.offline_check_count,
            updated_platform.offline_check_delay_ms
        ),
        (9, 9000)
    );
    assert_eq!(
        container
            .streamer_manager
            .get_streamer("refresh-0")
            .unwrap()
            .offline_check_count,
        8
    );

    mark_stale(&container);
    sqlx::query("DELETE FROM streamers WHERE id = 'refresh-5'")
        .execute(&container.pool)
        .await
        .unwrap();
    container
        .streamer_manager
        .mark_deleting("refresh-7")
        .await
        .unwrap();
    let mut global = container.config_service.get_global_config().await.unwrap();
    global.offline_check_count = 10;
    global.offline_check_delay_ms = 10000;
    container
        .config_service
        .update_global_config(&global)
        .await
        .unwrap();
    container
        .apply_config_event_for_test(ConfigUpdateEvent::GlobalUpdated)
        .await;
    for index in 0..40 {
        let id = format!("refresh-{index}");
        if index == 5 || index == 7 {
            assert_eq!(
                container
                    .streamer_manager
                    .get_streamer(&id)
                    .unwrap()
                    .offline_check_count,
                222
            );
        } else {
            assert_current(&container, &id).await;
        }
    }
    let inherited_global = container
        .streamer_manager
        .get_streamer("refresh-1")
        .unwrap();
    assert_eq!(
        (
            inherited_global.offline_check_count,
            inherited_global.offline_check_delay_ms
        ),
        (10, 10000)
    );
    let overridden = container
        .streamer_manager
        .get_streamer("refresh-3")
        .unwrap();
    assert_eq!(
        (
            overridden.offline_check_count,
            overridden.offline_check_delay_ms
        ),
        (7, 7000)
    );

    assert!(
        container
            .streamer_manager
            .get_streamer("refresh-7")
            .unwrap()
            .is_deleted()
    );
    assert!(
        !container.startup_recovery_complete(),
        "cache fanout must not fabricate a persistent recovery acknowledgement"
    );
    container.cancellation_token.cancel();
}
