use super::*;
use crate::config::ConfigService;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::{
    SqlxConfigRepository, SqlxFilterRepository, SqlxSessionRepository, SqlxStreamerRepository,
};

fn assert_config(
    scheduler: &Scheduler<SqlxStreamerRepository>,
    count: u32,
    delay: u64,
    normal: u64,
) {
    let config = scheduler
        .supervisor
        .streamer_restart_config("resolved-streamer")
        .unwrap();
    assert_eq!(config.offline_check_count, count);
    assert_eq!(config.offline_check_interval_ms, delay);
    assert_eq!(config.check_interval_ms, normal);
    assert!(!config.batch_capable);
}

async fn schedule_restart(scheduler: &mut Scheduler<SqlxStreamerRepository>) {
    scheduler
        .supervisor
        .registry()
        .get_streamer("resolved-streamer")
        .unwrap()
        .cancel();
    let mut result = tokio::time::timeout(
        Duration::from_secs(2),
        scheduler.supervisor.registry_mut().join_next(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    result.outcome = Err(crate::scheduler::ActorError::recoverable("fixture restart"));
    assert!(matches!(
        scheduler.supervisor.handle_task_completion(result),
        TaskCompletionAction::RestartScheduled { .. }
    ));
}

struct FailOnceResolver {
    inner: Arc<dyn ActorConfigResolver>,
    fail: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl ActorConfigResolver for FailOnceResolver {
    async fn resolve(&self, id: &str, base: StreamerConfig) -> Result<StreamerConfig> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(crate::Error::config(
                "injected transient config lookup failure",
            ));
        }
        self.inner.resolve(id, base).await
    }
}

struct BlockingResolver(Arc<tokio::sync::Notify>);

#[async_trait::async_trait]
impl ActorConfigResolver for BlockingResolver {
    async fn resolve(&self, _id: &str, _base: StreamerConfig) -> Result<StreamerConfig> {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn actor_creation_and_every_config_scope_resolve_four_layers_without_metadata_fanout() {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let configs = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
    let streamers = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
    sqlx::query("UPDATE global_config SET offline_check_count = 4, offline_check_delay_ms = 40000")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE platform_config SET offline_check_count = 5, offline_check_delay_ms = 50000 WHERE id = 'platform-twitch'").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO template_config(id,name,offline_check_count,offline_check_delay_ms) VALUES('timing-template','Timing',6,60000)").execute(&pool).await.unwrap();
    let mut row = StreamerDbModel::new(
        "Resolved",
        "https://www.twitch.tv/resolved",
        "platform-twitch",
    );
    row.id = "resolved-streamer".to_owned();
    row.template_config_id = Some("timing-template".to_owned());
    row.streamer_specific_config =
        Some(r#"{"offline_check_count":1,"offline_check_delay_ms":10000}"#.to_owned());
    streamers.create_streamer(&row).await.unwrap();
    let broadcaster = ConfigEventBroadcaster::new();
    let manager = Arc::new(StreamerManager::new(streamers.clone(), broadcaster.clone()));
    manager.hydrate().await.unwrap();
    let service = Arc::new(ConfigService::new(configs.clone(), streamers));
    assert_eq!(
        service
            .get_config_for_streamer(&row.id)
            .await
            .unwrap()
            .offline_check_count,
        1
    );
    let lifecycle = Arc::new(crate::session::SessionLifecycle::new(
        Arc::new(
            crate::database::repositories::session_lifecycle::SessionLifecycleRepository::new(
                pool.clone(),
            ),
        ),
        Arc::new(crate::session::OfflineClassifier::new()),
        16,
    ));
    let monitor = Arc::new(StreamMonitor::new(
        manager.clone(),
        Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone())),
        Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone())),
        service.clone(),
        pool.clone(),
        lifecycle,
    ));
    let token = CancellationToken::new();
    let mut scheduler = Scheduler::with_monitor_and_config(
        manager.clone(),
        broadcaster,
        monitor,
        SchedulerConfig::default(),
        token.clone(),
    )
    .with_config_repo(configs);
    assert!(
        scheduler.config_resolver.is_some(),
        "real monitor constructors must wire the shared resolver"
    );
    // Keep the actual resolver wiring while preventing any real platform/network checks.
    scheduler.supervisor = Supervisor::with_config(
        token.clone(),
        SupervisorConfig::default(),
        manager.metadata_store(),
    );
    let metadata = manager.get_streamer(&row.id).unwrap();
    assert_eq!(
        (
            metadata.offline_check_count,
            metadata.offline_check_delay_ms
        ),
        (3, 20_000)
    );
    scheduler.add_streamer(metadata).await.unwrap();
    assert_config(&scheduler, 1, 10_000, 60_000);
    let actor = scheduler
        .supervisor
        .registry()
        .get_streamer(&row.id)
        .unwrap()
        .clone();
    actor
        .send(StreamerMessage::DownloadStarted {
            download_id: "fixture-download".to_owned(),
            session_id: "fixture-session".to_owned(),
        })
        .await
        .unwrap();
    actor
        .send(StreamerMessage::DownloadEnded(DownloadEndPolicy::Other(
            "fixture transport end".to_owned(),
        )))
        .await
        .unwrap();
    let (reply, received) = tokio::sync::oneshot::channel();
    actor.send(StreamerMessage::GetState(reply)).await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(2), received)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.recurring_interval_ms,
        Some(10_000),
        "the spawned actor must use the resolved post-live interval"
    );

    sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = ?")
        .bind(r#"{"offline_check_count":2,"offline_check_delay_ms":12000}"#)
        .bind(&row.id)
        .execute(&pool)
        .await
        .unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::StreamerMetadataUpdated {
            streamer_id: row.id.clone(),
        })
        .await;
    assert_config(&scheduler, 2, 12_000, 60_000);
    assert_eq!(
        manager.get_streamer(&row.id).unwrap().offline_check_count,
        3,
        "no independently scheduled metadata refresh participates in this fixture"
    );

    sqlx::query("UPDATE streamers SET streamer_specific_config = '{}' WHERE id = ?")
        .bind(&row.id)
        .execute(&pool)
        .await
        .unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::TemplateUpdated {
            template_id: "timing-template".to_owned(),
        })
        .await;
    assert_config(&scheduler, 6, 60_000, 60_000);
    sqlx::query("UPDATE template_config SET offline_check_count = 7, offline_check_delay_ms = 70000 WHERE id = 'timing-template'").execute(&pool).await.unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::TemplateUpdated {
            template_id: "timing-template".to_owned(),
        })
        .await;
    assert_config(&scheduler, 7, 70_000, 60_000);

    sqlx::query("UPDATE template_config SET offline_check_count = NULL, offline_check_delay_ms = NULL WHERE id = 'timing-template'").execute(&pool).await.unwrap();
    sqlx::query("UPDATE platform_config SET offline_check_count = 8, offline_check_delay_ms = 80000 WHERE id = 'platform-twitch'").execute(&pool).await.unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::PlatformUpdated {
            platform_id: "platform-twitch".to_owned(),
        })
        .await;
    assert_config(&scheduler, 8, 80_000, 60_000);

    sqlx::query("UPDATE platform_config SET offline_check_count = NULL, offline_check_delay_ms = NULL WHERE id = 'platform-twitch'").execute(&pool).await.unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::GlobalUpdated)
        .await;
    assert_config(&scheduler, 4, 40_000, 60_000);
    // Import-like changes can affect a streamer while all global timing columns stay identical.
    sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = ?")
        .bind(r#"{"offline_check_count":9,"offline_check_delay_ms":90000}"#)
        .bind(&row.id)
        .execute(&pool)
        .await
        .unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::GlobalUpdated)
        .await;
    assert_config(&scheduler, 9, 90_000, 60_000);

    sqlx::query("UPDATE streamers SET streamer_specific_config = '{}' WHERE id = ?")
        .bind(&row.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE global_config SET streamer_check_delay_ms = 100000, offline_check_count = 10, offline_check_delay_ms = 100000").execute(&pool).await.unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::GlobalUpdated)
        .await;
    assert_config(&scheduler, 10, 100_000, 100_000);
    let (reply, received) = tokio::sync::oneshot::channel();
    actor.send(StreamerMessage::GetState(reply)).await.unwrap();
    let state = tokio::time::timeout(Duration::from_secs(2), received)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.recurring_interval_ms,
        Some(100_000),
        "the delivered config matches the restart snapshot"
    );

    schedule_restart(&mut scheduler).await;
    scheduler
        .supervisor
        .defer_streamer_restart(&row.id, Duration::from_secs(60));
    sqlx::query("UPDATE template_config SET offline_check_count = 11, offline_check_delay_ms = 110000 WHERE id = 'timing-template'").execute(&pool).await.unwrap();
    scheduler
        .handle_config_event(ConfigUpdateEvent::TemplateUpdated {
            template_id: "timing-template".to_owned(),
        })
        .await;
    scheduler
        .supervisor
        .defer_streamer_restart(&row.id, Duration::ZERO);
    assert_eq!(scheduler.process_pending_restarts().await, 1);
    assert_config(&scheduler, 11, 110_000, 100_000);
    let restarted = scheduler
        .supervisor
        .registry()
        .get_streamer(&row.id)
        .unwrap()
        .clone();
    restarted
        .send(StreamerMessage::DownloadStarted {
            download_id: "restarted".to_owned(),
            session_id: "fixture-session".to_owned(),
        })
        .await
        .unwrap();
    restarted
        .send(StreamerMessage::DownloadEnded(DownloadEndPolicy::Other(
            "fixture transport end".to_owned(),
        )))
        .await
        .unwrap();
    let (reply, received) = tokio::sync::oneshot::channel();
    restarted
        .send(StreamerMessage::GetState(reply))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), received)
            .await
            .unwrap()
            .unwrap()
            .recurring_interval_ms,
        Some(110_000)
    );

    schedule_restart(&mut scheduler).await;
    scheduler.config_resolver = Some(Arc::new(FailOnceResolver {
        inner: scheduler.config_resolver.clone().unwrap(),
        fail: std::sync::atomic::AtomicBool::new(true),
    }));
    assert_eq!(scheduler.process_pending_restarts().await, 0);
    assert!(!scheduler.supervisor.registry().has_streamer(&row.id));
    assert_eq!(scheduler.supervisor.pending_restart_count(), 1);
    sqlx::query("UPDATE template_config SET offline_check_count = 12, offline_check_delay_ms = 120000 WHERE id = 'timing-template'").execute(&pool).await.unwrap();
    scheduler
        .supervisor
        .defer_streamer_restart(&row.id, Duration::ZERO);
    assert_eq!(
        scheduler.process_pending_restarts().await,
        1,
        "a later successful resolution resumes the deferred restart"
    );
    assert_config(&scheduler, 12, 120_000, 100_000);

    pool.close().await;
    scheduler
        .handle_config_event(ConfigUpdateEvent::TemplateUpdated {
            template_id: "timing-template".to_owned(),
        })
        .await;
    assert_config(&scheduler, 12, 120_000, 100_000);
    assert!(
        scheduler
            .create_streamer_config(&manager.get_streamer(&row.id).unwrap())
            .await
            .is_err(),
        "failed resolution must never substitute metadata defaults"
    );
    schedule_restart(&mut scheduler).await;
    scheduler
        .supervisor
        .defer_streamer_restart(&row.id, Duration::ZERO);
    assert_eq!(
        scheduler.process_pending_restarts().await,
        0,
        "failed resolution defers the restart instead of using captured stale timing"
    );
    assert!(!scheduler.supervisor.registry().has_streamer(&row.id));
    assert_eq!(scheduler.supervisor.pending_restart_count(), 1);
    scheduler
        .supervisor
        .defer_streamer_restart(&row.id, Duration::ZERO);
    let entered = Arc::new(tokio::sync::Notify::new());
    scheduler.config_resolver = Some(Arc::new(BlockingResolver(entered.clone())));
    let resolving = tokio::spawn(async move {
        let restarted = scheduler.process_pending_restarts().await;
        (scheduler, restarted)
    });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    token.cancel();
    let (mut scheduler, restarted) = tokio::time::timeout(Duration::from_secs(2), resolving)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        restarted, 0,
        "cancellation interrupts an in-flight resolver without starting the actor"
    );
    assert!(!scheduler.supervisor.registry().has_streamer(&row.id));
    tokio::time::timeout(Duration::from_secs(2), scheduler.shutdown())
        .await
        .unwrap();
}
