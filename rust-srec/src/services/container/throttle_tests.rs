use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY, ServiceContainer, ServiceContainerConfig};
use crate::config::ConfigUpdateEvent;
use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
use crate::database::repositories::{
    SessionRepository, SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository,
};
use crate::downloader::engine::EngineType;
use crate::downloader::queue::{AcquireRequest, Priority};
use crate::pipeline::{Job, ThrottleConfig, ThrottleEvent};

fn request(id: &str, priority: Priority) -> AcquireRequest {
    AcquireRequest {
        session_id: id.into(),
        streamer_id: id.into(),
        streamer_name: id.into(),
        engine_type: EngineType::Ffmpeg,
        priority,
    }
}

#[tokio::test]
async fn runtime_throttle_preserves_live_limits_and_queue_ownership() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let streamer =
            StreamerDbModel::new("Backlog", "https://example.test/backlog", "platform-huya");
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
        let session = LiveSessionDbModel::new(&streamer.id);
        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .create_session(&session)
            .await
            .unwrap();
        let mut options =
            ServiceContainerConfig::standard(DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY);
        options.download_config.high_priority_extra_slots = 1;
        options.pipeline_config.throttle = ThrottleConfig {
            enabled: true,
            critical_threshold: 1,
            warning_threshold: 1,
            reduction_factor: 0.5,
            check_interval_ms: 10,
        };
        let container = ServiceContainer::with_full_config(pool.clone(), pool, options)
            .await
            .unwrap();
        let mut global = container.config_service.get_global_config().await.unwrap();
        global.max_concurrent_downloads = 6;
        global.max_concurrent_cpu_jobs = 1;
        global.max_concurrent_io_jobs = 1;
        container
            .config_service
            .update_global_config(&global)
            .await
            .unwrap();
        container
            .apply_config_event_for_test(ConfigUpdateEvent::GlobalUpdated)
            .await;
        let pipeline = container.pipeline_manager.clone();
        let downloads = &container.download_manager;
        let mut events = pipeline.subscribe_throttle_events().unwrap();
        let mut jobs = Vec::new();
        for _ in 0..2 {
            // No processor claims this synthetic backlog; no external commands run.
            jobs.push(
                pipeline
                    .enqueue(Job::new(
                        "held-backlog",
                        vec![],
                        vec![],
                        &streamer.id,
                        &session.id,
                    ))
                    .await
                    .unwrap(),
            );
        }
        pipeline.clone().start();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("enabled production throttle must start")
            .unwrap();
        assert!(matches!(
            event,
            ThrottleEvent::ThrottleActivated {
                original_limit: 6,
                new_limit: 3,
                ..
            }
        ));
        assert_eq!(downloads.max_concurrent_downloads(), 6);
        let mut slots = Vec::new();
        for id in ["normal-1", "normal-2", "normal-3"] {
            slots.push(
                downloads
                    .acquire_slot(request(id, Priority::Normal), CancellationToken::new())
                    .await
                    .unwrap(),
            );
        }
        let high = downloads
            .acquire_slot(
                request("priority", Priority::High),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let fourth = downloads.acquire_slot(
            request("normal-4", Priority::Normal),
            CancellationToken::new(),
        );
        tokio::pin!(fourth);
        assert!(futures::poll!(fourth.as_mut()).is_pending());

        global.max_concurrent_downloads = 8;
        container
            .config_service
            .update_global_config(&global)
            .await
            .unwrap();
        container
            .apply_config_event_for_test(ConfigUpdateEvent::GlobalUpdated)
            .await;
        assert_eq!(downloads.max_concurrent_downloads(), 8);
        slots.push(
            tokio::time::timeout(Duration::from_secs(1), fourth)
                .await
                .unwrap()
                .unwrap(),
        );

        let next = downloads.acquire_slot(
            request("normal-5", Priority::Normal),
            CancellationToken::new(),
        );
        tokio::pin!(next);
        assert!(futures::poll!(next.as_mut()).is_pending());

        global.max_concurrent_downloads = 2;
        container
            .config_service
            .update_global_config(&global)
            .await
            .unwrap();
        container
            .apply_config_event_for_test(ConfigUpdateEvent::GlobalUpdated)
            .await;
        assert!(futures::poll!(next.as_mut()).is_pending());
        for id in jobs {
            pipeline.cancel_job(&id).await.unwrap();
        }
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            ThrottleEvent::ThrottleDeactivated {
                restored_limit: 2,
                ..
            }
        ));
        assert_eq!(downloads.max_concurrent_downloads(), 2);
        assert!(futures::poll!(next.as_mut()).is_pending());
        drop(slots.pop());
        drop(slots.pop());
        assert!(futures::poll!(next.as_mut()).is_pending());
        drop(slots.pop());
        let next = tokio::time::timeout(Duration::from_secs(1), next)
            .await
            .unwrap()
            .unwrap();
        drop((slots, high, next));
        pipeline.stop().await;
        assert!(!pipeline.is_throttled());
    })
    .await
    .expect("throttle lifecycle must settle");
}
