use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use super::*;
use crate::config::ConfigEventBroadcaster;
use crate::danmu::test_support::FakeProvider;
use crate::danmu::{CollectionSpec, ProviderRegistry};
use crate::database::models::StreamerDbModel;
use crate::database::repositories::{
    SessionLifecycleRepository, SessionRepository, StreamerRepository,
};
use crate::downloader::engine::{EngineStartError, EngineType};
use crate::downloader::{
    AcquireRequest, DownloadConfig, DownloadEngine, DownloadHandle, DownloadManagerConfig,
    DownloadManagerEvent, DownloadProgressEvent, DownloadStopCause, EngineEndSignal, SegmentEvent,
};
use crate::session::{LiveDetectedArgs, OfflineClassifier};
use crate::streamer::manager::ReloadPublish;

const STREAMER: &str = "coordinator-streamer";

#[async_trait]
pub(super) trait FreshnessCheck: Send + Sync {
    async fn check(
        &self,
        metadata: &crate::streamer::StreamerMetadata,
    ) -> crate::Result<crate::monitor::LiveStatus>;
}

struct FreshnessGate {
    calls: AtomicUsize,
    entered: Notify,
    release: tokio::sync::Semaphore,
    result: parking_lot::Mutex<Option<crate::Result<crate::monitor::LiveStatus>>>,
}

impl FreshnessGate {
    fn new(result: crate::Result<crate::monitor::LiveStatus>) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            result: parking_lot::Mutex::new(Some(result)),
        })
    }
}

#[async_trait]
impl FreshnessCheck for FreshnessGate {
    async fn check(
        &self,
        _metadata: &crate::streamer::StreamerMetadata,
    ) -> crate::Result<crate::monitor::LiveStatus> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        self.result.lock().take().unwrap()
    }
}

#[derive(Default)]
struct ScriptedEngine {
    started: Notify,
    handles: parking_lot::Mutex<Vec<Arc<DownloadHandle>>>,
}

#[async_trait]
impl DownloadEngine for ScriptedEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Ffmpeg
    }

    async fn run(&self, handle: Arc<DownloadHandle>) -> Result<(), EngineStartError> {
        self.handles.lock().push(handle.clone());
        self.started.notify_one();
        handle.cancellation_token.cancelled().await;
        handle
            .event_tx
            .send(SegmentEvent::DownloadCompleted {
                total_bytes: 0,
                total_duration_secs: 0.0,
                total_segments: 0,
                engine_signal: EngineEndSignal::CleanDisconnect,
            })
            .await
            .map_err(|error| {
                EngineStartError::new(
                    crate::downloader::DownloadFailureKind::Other,
                    error.to_string(),
                )
            })?;
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }
    fn version(&self) -> Option<String> {
        Some("scripted".into())
    }
}

struct Fixture {
    coordinator: Arc<RuntimeCoordinator>,
    pool: sqlx::SqlitePool,
    directory: tempfile::TempDir,
    engine: Arc<ScriptedEngine>,
    provider: Arc<FakeProvider>,
    danmu_configs: Arc<parking_lot::Mutex<Vec<platforms_parser::danmaku::ConnectionConfig>>>,
    connect_gate: Arc<parking_lot::Mutex<Option<Arc<tokio::sync::Semaphore>>>>,
    _items: mpsc::Sender<platforms_parser::danmaku::DanmuItem>,
    persistence: AbortOnDropHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        sqlx::query("UPDATE global_config SET output_folder = ?, default_download_engine = 'ffmpeg', record_danmu = TRUE")
            .bind(directory.path().to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        sqlx::query(
            "UPDATE platform_config SET download_engine = NULL WHERE id = 'platform-twitch'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let mut row = StreamerDbModel::new("Coordinator", FakeProvider::URL, "platform-twitch");
        row.id = STREAMER.into();
        streamer_repo.create_streamer(&row).await.unwrap();
        let broadcaster = ConfigEventBroadcaster::new();
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            broadcaster.clone(),
        ));
        streamer_manager
            .reload_from_repo(STREAMER, ReloadPublish::StateOnly)
            .await
            .unwrap();
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let config_service = Arc::new(ConfigService::new(config_repo, streamer_repo));
        let session_repository = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        let resolver_manager = streamer_manager.clone();
        let session_lifecycle = Arc::new(
            SessionLifecycle::new(
                Arc::new(SessionLifecycleRepository::new(pool.clone())),
                Arc::new(OfflineClassifier::new()),
                64,
            )
            .with_hysteresis_resolver(Arc::new(move |id| {
                resolver_manager.get_streamer(id).map(|metadata| {
                    crate::session::HysteresisConfig::from_scheduler(
                        metadata.offline_check_count,
                        metadata.offline_check_delay_ms,
                    )
                })
            })),
        );
        let task_supervisor = Arc::new(TaskSupervisor::new());
        let stream_monitor = Arc::new(StreamMonitor::with_runtime(
            streamer_manager.clone(),
            Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone())),
            session_repository.clone(),
            config_service.clone(),
            pool.clone(),
            session_lifecycle.clone(),
            crate::monitor::StreamMonitorRuntimeConfig {
                monitor: Default::default(),
                required_event_sender: None,
                task_supervisor: task_supervisor.clone(),
            },
        ));
        let (sender, mut receiver) = crate::downloader::download_coordination_channel();
        let download_manager = Arc::new(
            DownloadManager::with_config(DownloadManagerConfig {
                max_concurrent_downloads: 1,
                high_priority_extra_slots: 0,
                ..Default::default()
            })
            .with_coordination_sender(sender),
        );
        let persistence_lifecycle = session_lifecycle.clone();
        let persistence = AbortOnDropHandle::new(tokio::spawn(async move {
            while let Some(delivery) = receiver.recv().await.unwrap() {
                let (event, acknowledgement) = delivery.into_parts();
                let result = match event {
                    DownloadManagerEvent::Terminal(event) => persistence_lifecycle
                        .on_download_terminal(&event)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string()),
                    _ => Ok(()),
                };
                acknowledgement.send(result).unwrap();
            }
        }));
        let engine = Arc::new(ScriptedEngine::default());
        download_manager.register_engine(engine.clone());
        let (items, received) = mpsc::channel(8);
        let provider = Arc::new(FakeProvider::new(vec![received]));
        let danmu_configs = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let connect_gate = Arc::new(parking_lot::Mutex::new(None));
        let mut providers = ProviderRegistry::new();
        providers.register(Arc::new(CapturedProvider {
            inner: provider.clone(),
            configs: danmu_configs.clone(),
            connect_gate: connect_gate.clone(),
        }));
        let danmu_service = Arc::new(
            DanmuService::with_providers(providers)
                .with_session_repository(session_repository.clone()),
        );
        let scheduler = crate::scheduler::Scheduler::new(streamer_manager.clone(), broadcaster);
        let coordinator = Arc::new(RuntimeCoordinator::new(RuntimeCoordinatorDependencies {
            download_manager,
            streamer_manager,
            config_service,
            danmu_service,
            stream_monitor,
            session_repository,
            session_cancels: Arc::new(SessionCancelTokens::new()),
            pending_pipelines: Arc::new(DashMap::new()),
            pipeline_manager: Arc::new(PipelineManager::new()),
            session_lifecycle,
            task_supervisor,
            scheduler_handle: scheduler.handle(),
        }));
        Self {
            coordinator,
            pool,
            directory,
            engine,
            provider,
            danmu_configs,
            connect_gate,
            _items: items,
            persistence,
        }
    }

    async fn live(&self) -> String {
        let streams = streams("cached");
        let result = self
            .coordinator
            .session_lifecycle
            .on_live_detected(LiveDetectedArgs {
                streamer_id: STREAMER,
                streamer_name: "Coordinator",
                streamer_url: FakeProvider::URL,
                current_avatar: None,
                new_avatar: None,
                title: "Contract",
                category: None,
                streams: &streams,
                media_headers: None,
                media_extras: None,
                now: Utc::now(),
            })
            .await
            .unwrap();
        self.coordinator
            .streamer_manager
            .reload_from_repo(STREAMER, ReloadPublish::StateOnly)
            .await
            .unwrap();
        result.session_id().to_owned()
    }

    async fn start_direct(&self, session: &str) -> String {
        let id = self
            .coordinator
            .download_manager
            .start_download(
                DownloadConfig::new(
                    "https://example.invalid/cached.flv",
                    self.directory.path(),
                    STREAMER,
                    "Coordinator",
                    session,
                ),
                Some("ffmpeg".into()),
                false,
            )
            .await
            .unwrap();
        self.engine.started.notified().await;
        id
    }

    fn freshness(&mut self, checker: Arc<FreshnessGate>) {
        Arc::get_mut(&mut self.coordinator).unwrap().freshness_check = Some(checker);
        self.coordinator
            .download_manager
            .set_queue_freshness_threshold_ms(0);
    }

    async fn blocker(&self) -> crate::downloader::SlotGuard {
        self.coordinator
            .download_manager
            .acquire_slot(
                AcquireRequest {
                    streamer_id: "other".into(),
                    streamer_name: "Other".into(),
                    session_id: "other-session".into(),
                    engine_type: EngineType::Ffmpeg,
                    priority: crate::downloader::Priority::Normal,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap()
    }

    async fn queued(&self) -> tokio::sync::broadcast::Receiver<DownloadManagerEvent> {
        let mut events = self.coordinator.download_manager.subscribe();
        while self.coordinator.download_manager.pending_count() == 0 {
            tokio::task::yield_now().await;
        }
        // A queue timestamp uses wall time. Wait across a wall-clock millisecond
        // so the zero threshold deterministically takes the freshness branch.
        let since = self.coordinator.download_manager.snapshot_pending()[0].queued_at_ms;
        while crate::database::time::now_ms() <= since {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // The initial event is asserted separately where queue ordering matters.
        while events.try_recv().is_ok() {}
        events
    }

    async fn stand_down(&self, schedule: bool) {
        if schedule {
            self.coordinator
                .session_lifecycle
                .end_for_out_of_schedule(STREAMER, "Coordinator", StreamerState::Live)
                .await
                .unwrap();
            self.coordinator
                .streamer_manager
                .reload_from_repo(STREAMER, ReloadPublish::StateOnly)
                .await
                .unwrap();
            self.coordinator
                .handle_monitor_event(outside_schedule(), false)
                .await;
        } else {
            sqlx::query("UPDATE streamers SET state = 'DISABLED' WHERE id = ?")
                .bind(STREAMER)
                .execute(&self.pool)
                .await
                .unwrap();
            self.coordinator
                .streamer_manager
                .reload_from_repo(STREAMER, ReloadPublish::StateOnly)
                .await
                .unwrap();
            self.coordinator.handle_streamer_disabled(STREAMER).await;
        }
    }

    fn assert_no_start(&self) {
        assert!(self.engine.handles.lock().is_empty());
        assert_eq!(self.provider.connects(), 0);
        assert_eq!(self.coordinator.download_manager.active_count(), 0);
        assert_eq!(self.coordinator.download_manager.pending_count(), 0);
        assert!(self.coordinator.pending_pipelines.is_empty());
    }

    async fn close(self) {
        self.coordinator.stream_monitor.stop();
        self.coordinator
            .task_supervisor
            .shutdown(Duration::from_secs(2))
            .await;
        self.coordinator
            .download_manager
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
        self.persistence.await.unwrap();
        self.coordinator.danmu_service.shutdown().await.unwrap();
        self.coordinator
            .session_lifecycle
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
    }
}

struct CapturedProvider {
    inner: Arc<FakeProvider>,
    configs: Arc<parking_lot::Mutex<Vec<platforms_parser::danmaku::ConnectionConfig>>>,
    connect_gate: Arc<parking_lot::Mutex<Option<Arc<tokio::sync::Semaphore>>>>,
}

#[async_trait]
impl platforms_parser::danmaku::DanmuProvider for CapturedProvider {
    fn platform(&self) -> &str {
        "fake"
    }
    async fn connect(
        &self,
        room: &str,
        config: platforms_parser::danmaku::ConnectionConfig,
    ) -> platforms_parser::danmaku::error::Result<platforms_parser::danmaku::DanmuStream> {
        self.configs.lock().push(config.clone());
        let gate = self.connect_gate.lock().clone();
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        platforms_parser::danmaku::DanmuProvider::connect(self.inner.as_ref(), room, config).await
    }
    async fn disconnect(
        &self,
        connection: &mut platforms_parser::danmaku::DanmuConnection,
    ) -> platforms_parser::danmaku::error::Result<()> {
        platforms_parser::danmaku::DanmuProvider::disconnect(self.inner.as_ref(), connection).await
    }
    fn supports_url(&self, url: &str) -> bool {
        url == FakeProvider::URL
    }
    fn extract_room_id(&self, _url: &str) -> Option<String> {
        Some("room-1".into())
    }
}

fn fresh_live() -> crate::monitor::LiveStatus {
    let mut streams = streams("fresh");
    streams[0].extras =
        Some(serde_json::json!({"headers": {"X-Media": "stream"}, "host_header": "fresh-host"}));
    crate::monitor::LiveStatus::Live {
        title: "Refreshed".into(),
        category: None,
        started_at: None,
        viewer_count: None,
        avatar: None,
        streams,
        media_headers: Some(HashMap::from([
            ("Referer".into(), "fresh".into()),
            ("X-Media".into(), "media".into()),
        ])),
        media_extras: Some(HashMap::from([(
            "signed-room".into(),
            "fresh-extra".into(),
        )])),
        next_check_hint: None,
        candidates: vec![],
    }
}

fn spawned_pipeline(fixture: &Fixture, session: &str, resumed: bool) -> AbortOnDropHandle<()> {
    let coordinator = fixture.coordinator.clone();
    let payload = payload(session);
    AbortOnDropHandle::new(tokio::spawn(run_live_download_pipeline(
        coordinator,
        payload,
        resumed,
    )))
}

fn dequeued_count(
    events: &mut tokio::sync::broadcast::Receiver<DownloadManagerEvent>,
    session: &str,
) -> usize {
    std::iter::from_fn(|| events.try_recv().ok()).filter(|event| matches!(event,
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadDequeued { session_id, .. }) if session_id == session
    )).count()
}

#[tokio::test]
async fn disabled_and_schedule_stops_cancel_preflight_and_queued_startup() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for schedule in [false, true] {
            for preflight in [false, true] {
                let fixture = Fixture::new().await;
                let session = fixture.live().await;
                fixture.coordinator.config_service.get_config_for_streamer(STREAMER).await.unwrap();
                let manager = fixture.coordinator.download_manager.clone();
                let maintenance = if preflight { Some(manager.try_admit_maintenance(0).unwrap()) } else { None };
                let blocker = if preflight { None } else { Some(fixture.blocker().await) };
                let mut events = manager.subscribe();
                let pipeline = run_live_download_pipeline(fixture.coordinator.clone(), payload(&session), false);
                tokio::pin!(pipeline);
                poll_pending(pipeline.as_mut()).await;
                if !preflight {
                    // Drive filesystem preflight to queue admission using this
                    // future itself, without relying on scheduling delays.
                    tokio::select! {
                        _ = &mut pipeline => panic!("held slot must prevent startup"),
                        _ = async { while manager.pending_count() == 0 { tokio::task::yield_now().await; } } => {}
                    }
                }
                fixture.stand_down(schedule).await;
                tokio::time::timeout(Duration::from_secs(2), pipeline).await.unwrap();
                fixture.assert_no_start();
                assert_eq!(dequeued_count(&mut events, &session), usize::from(!preflight));
                assert!(fixture.coordinator.session_repository.get_session(&session).await.unwrap().end_time.is_some());
                assert!(!fixture.coordinator.session_lifecycle.is_session_active(&session));
                drop(blocker);
                drop(maintenance);
                // All slot ownership from the cancelled path was released.
                drop(fixture.blocker().await);
                fixture.close().await;
            }
        }
    }).await.expect("preflight and queue cancellation must settle");
}

#[tokio::test]
async fn disabled_and_schedule_stops_finalize_active_download_and_danmu() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for schedule in [false, true] {
            let fixture = Fixture::new().await;
            let session = fixture.live().await;
            let mut events = fixture.coordinator.download_manager.subscribe();
            run_live_download_pipeline(fixture.coordinator.clone(), payload(&session), false).await;
            fixture.engine.started.notified().await;
            assert_eq!(fixture.provider.connects(), 1);
            assert!(fixture.coordinator.danmu_service.is_collecting(&session));
            fixture.stand_down(schedule).await;
            let terminal = loop {
                if let DownloadManagerEvent::Terminal(terminal) = events.recv().await.unwrap() {
                    break terminal;
                }
            };
            assert_eq!(terminal.session_id(), session);
            let cause = match terminal {
                crate::downloader::DownloadTerminalEvent::Cancelled { cause, .. } => cause,
                crate::downloader::DownloadTerminalEvent::Completed {
                    stop_cause: Some(cause),
                    ..
                } => cause,
                other => panic!("expected an explicit stop cause, got {other:?}"),
            };
            assert_eq!(
                cause,
                if schedule {
                    DownloadStopCause::OutOfSchedule
                } else {
                    DownloadStopCause::StreamerDisabled
                }
            );
            assert_eq!(fixture.coordinator.download_manager.active_count(), 0);
            assert!(!fixture.coordinator.danmu_service.is_collecting(&session));
            assert!(
                !fixture
                    .coordinator
                    .session_lifecycle
                    .is_session_active(&session)
            );
            assert!(
                fixture
                    .coordinator
                    .session_repository
                    .get_session(&session)
                    .await
                    .unwrap()
                    .end_time
                    .is_some()
            );
            fixture.close().await;
        }
    })
    .await
    .expect("active cancellation must finish before its terminal is observed");
}

#[tokio::test]
async fn freshness_equality_keeps_cached_media_and_long_wait_replaces_urls_headers_and_extras() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for queued in [false, true] {
            let mut fixture = Fixture::new().await;
            let checker = FreshnessGate::new(Ok(fresh_live()));
            fixture.freshness(checker.clone());
            let session = fixture.live().await;
            let blocker = if queued {
                Some(fixture.blocker().await)
            } else {
                None
            };
            let pipeline = spawned_pipeline(&fixture, &session, false);
            if queued {
                fixture.queued().await;
                drop(blocker);
                checker.entered.notified().await;
                checker.release.add_permits(1);
            }
            pipeline.await.unwrap();
            fixture.engine.started.notified().await;
            assert_eq!(checker.calls.load(Ordering::SeqCst), usize::from(queued));
            let config = fixture.engine.handles.lock()[0].config.read().clone();
            let headers: HashMap<_, _> = config.headers.into_iter().collect();
            if queued {
                assert!(config.url.ends_with("fresh.flv"));
                assert_eq!(headers["Referer"], "fresh");
                assert_eq!(headers["X-Media"], "stream");
                assert_eq!(headers["Host"], "fresh-host");
                assert_eq!(
                    fixture.danmu_configs.lock()[0].extras.as_ref().unwrap()["signed-room"],
                    "fresh-extra"
                );
            } else {
                // Immediate queue grants stamp queued_at == acquired_at, so
                // waited_ms equals the configured zero threshold exactly.
                assert!(config.url.ends_with("cached.flv"));
                assert_eq!(headers["Referer"], "cached");
                assert!(fixture.danmu_configs.lock()[0].extras.is_none());
            }
            fixture.close().await;
        }
    })
    .await
    .expect("freshness contracts must use no real network");
}

#[tokio::test]
async fn stale_queue_missing_offline_and_empty_live_dequeue_but_checker_errors_use_cache() {
    tokio::time::timeout(Duration::from_secs(40), async {
        for case in ["missing", "offline", "empty", "error"] {
            let mut fixture = Fixture::new().await;
            let result = match case {
                "error" => Err(crate::Error::Other("injected checker failure".into())),
                "empty" => {
                    let mut status = fresh_live();
                    if let crate::monitor::LiveStatus::Live { streams, .. } = &mut status {
                        streams.clear();
                    }
                    Ok(status)
                }
                _ => Ok(crate::monitor::LiveStatus::Offline),
            };
            let checker = FreshnessGate::new(result);
            fixture.freshness(checker.clone());
            let session = fixture.live().await;
            let blocker = fixture.blocker().await;
            let pipeline = spawned_pipeline(&fixture, &session, false);
            let mut events = fixture.queued().await;
            if case == "missing" {
                fixture
                    .coordinator
                    .streamer_manager
                    .metadata_store()
                    .remove(STREAMER);
            }
            drop(blocker);
            if case != "missing" {
                checker.entered.notified().await;
                checker.release.add_permits(1);
            }
            pipeline.await.unwrap();
            if case == "error" {
                fixture.engine.started.notified().await;
                assert!(
                    fixture.engine.handles.lock()[0]
                        .config
                        .read()
                        .url
                        .ends_with("cached.flv")
                );
                assert_eq!(fixture.provider.connects(), 1);
                assert_eq!(dequeued_count(&mut events, &session), 0);
            } else {
                fixture.assert_no_start();
                assert_eq!(dequeued_count(&mut events, &session), 1);
                drop(fixture.blocker().await);
            }
            fixture.close().await;
        }
    })
    .await
    .expect("stale queue outcomes must release or transfer the granted slot");
}

#[tokio::test]
async fn freshness_and_admission_cancellation_release_slots_without_danmu() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for admission in [false, true] {
            let mut fixture = Fixture::new().await;
            let checker = FreshnessGate::new(Ok(fresh_live()));
            fixture.freshness(checker.clone());
            let session = fixture.live().await;
            let blocker = fixture.blocker().await;
            let pipeline = spawned_pipeline(&fixture, &session, false);
            let mut events = fixture.queued().await;
            drop(blocker);
            checker.entered.notified().await;
            let manager = fixture.coordinator.download_manager.clone();
            let maintenance = if admission {
                Some(manager.try_admit_maintenance(0).unwrap())
            } else {
                None
            };
            if admission {
                let database_gate = fixture.pool.acquire().await.unwrap();
                checker.release.add_permits(1);
                while checker.result.lock().is_some() {
                    tokio::task::yield_now().await;
                }
                // Freshness has returned and the pipeline is queued for the
                // sole SQL connection to load its segment index. Queue a FIFO
                // follower behind it: obtaining this connection proves that
                // read finished and the pipeline reached manager admission,
                // which remains blocked by the maintenance guard.
                let after_index = fixture.pool.acquire();
                tokio::pin!(after_index);
                poll_pending(after_index.as_mut()).await;
                drop(database_gate);
                drop(after_index.await.unwrap());
            }
            fixture.coordinator.session_cancels.cancel(&session);
            tokio::time::timeout(Duration::from_secs(2), pipeline)
                .await
                .unwrap()
                .unwrap();
            fixture.assert_no_start();
            assert_eq!(dequeued_count(&mut events, &session), 1);
            drop(maintenance);
            drop(fixture.blocker().await);
            fixture.close().await;
        }
    })
    .await
    .expect("cancelled freshness/admission must not await the blocked owner");
}

#[tokio::test]
async fn resumed_started_requires_active_session_payload_and_resume_flag() {
    tokio::time::timeout(Duration::from_secs(45), async {
        for (active, payload_present, resumed) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
            (true, true, true),
        ] {
            let fixture = Fixture::new().await;
            let session = fixture.live().await;
            if !active {
                fixture
                    .coordinator
                    .session_lifecycle
                    .end_for_disable(STREAMER, "Coordinator")
                    .await
                    .unwrap();
            }
            Arc::make_mut(
                &mut fixture
                    .coordinator
                    .streamer_manager
                    .metadata_store()
                    .get_mut(STREAMER)
                    .unwrap(),
            )
            .state = StreamerState::NotLive;
            fixture
                .coordinator
                .handle_session_transition(SessionTransition::Started {
                    session_id: session.clone(),
                    streamer_id: STREAMER.into(),
                    streamer_name: "Coordinator".into(),
                    title: "Resumed".into(),
                    category: None,
                    started_at: Utc::now(),
                    from_hysteresis: resumed,
                    download_start: payload_present.then(|| {
                        Box::new(crate::session::DownloadStartPayload {
                            streamer_url: FakeProvider::URL.into(),
                            streams: streams("resumed"),
                            media_headers: None,
                            media_extras: None,
                        })
                    }),
                })
                .await;
            if active && payload_present && resumed {
                fixture.engine.started.notified().await;
                while !fixture.coordinator.pending_pipelines.is_empty() {
                    tokio::task::yield_now().await;
                }
                assert_eq!(fixture.coordinator.download_manager.active_count(), 1);
                assert_eq!(fixture.provider.connects(), 1);
                assert!(
                    fixture.engine.handles.lock()[0]
                        .config
                        .read()
                        .url
                        .ends_with("resumed.flv")
                );
            } else {
                fixture.assert_no_start();
            }
            fixture.close().await;
        }
    })
    .await
    .expect("resume handling must preserve lifecycle authority");
}

#[tokio::test]
async fn short_queue_rechecks_missing_disabled_and_out_of_schedule_without_a_stop_event() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for state in [
            None,
            Some(StreamerState::Disabled),
            Some(StreamerState::OutOfSchedule),
        ] {
            let fixture = Fixture::new().await;
            fixture
                .coordinator
                .download_manager
                .set_queue_freshness_threshold_ms(i64::MAX);
            let session = fixture.live().await;
            let blocker = fixture.blocker().await;
            let pipeline = spawned_pipeline(&fixture, &session, false);
            let mut events = fixture.queued().await;
            if let Some(state) = state {
                Arc::make_mut(
                    &mut fixture
                        .coordinator
                        .streamer_manager
                        .metadata_store()
                        .get_mut(STREAMER)
                        .unwrap(),
                )
                .state = state;
            } else {
                fixture
                    .coordinator
                    .streamer_manager
                    .metadata_store()
                    .remove(STREAMER);
            }
            drop(blocker);
            pipeline.await.unwrap();
            fixture.assert_no_start();
            assert_eq!(dequeued_count(&mut events, &session), 1);
            drop(fixture.blocker().await);
            fixture.close().await;
        }
    })
    .await
    .expect("short waits must not bypass changed metadata");
}

#[tokio::test]
async fn live_and_offline_calls_serialize_memory_database_and_transition_order() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for offline_first in [false, true] {
            let fixture = Fixture::new().await;
            let original = fixture.live().await;
            let lifecycle = fixture.coordinator.session_lifecycle.clone();
            let mut transitions = lifecycle.subscribe();
            // One DB connection holds the first operation after it enters the
            // lifecycle. The competing operation must wait behind its streamer
            // guard until persistence, memory and publication have all finished.
            let connection = fixture.pool.acquire().await.unwrap();
            let live_owner = lifecycle.clone();
            let live = async move {
                let streams = streams("race");
                live_owner.on_live_detected(LiveDetectedArgs {
                    streamer_id: STREAMER, streamer_name: "Coordinator", streamer_url: FakeProvider::URL,
                    current_avatar: None, new_avatar: None, title: "Race", category: None,
                    streams: &streams, media_headers: None, media_extras: None, now: Utc::now(),
                }).await.unwrap().session_id().to_owned()
            };
            let offline_owner = lifecycle.clone();
            let offline_session = original.clone();
            let offline = async move {
                offline_owner.on_offline_detected(crate::session::OfflineDetectedArgs {
                    streamer_id: STREAMER, streamer_name: "Coordinator", session_id: Some(&offline_session),
                    state_was_live: true, clear_errors: false, signal: None, now: Utc::now(),
                }).await.unwrap()
            };
            tokio::pin!(live, offline);
            if offline_first {
                poll_pending(offline.as_mut()).await;
                poll_pending(live.as_mut()).await;
            } else {
                poll_pending(live.as_mut()).await;
                poll_pending(offline.as_mut()).await;
            }
            assert!(transitions.try_recv().is_err());
            drop(connection);
            let (current, _) = tokio::join!(live, offline);
            let events: Vec<_> = std::iter::from_fn(|| transitions.try_recv().ok()).collect();
            assert_eq!(events.len(), 2);
            assert!(matches!(&events[usize::from(offline_first)], SessionTransition::Started { session_id, .. } if session_id == &current));
            assert!(matches!(&events[usize::from(!offline_first)], SessionTransition::Ended { session_id, .. } if session_id == &original));
            let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL")
                .bind(STREAMER).fetch_one(&fixture.pool).await.unwrap();
            assert_eq!(active, i64::from(offline_first));
            assert!(fixture.coordinator.session_repository.get_session(&original).await.unwrap().end_time.is_some());
            assert_eq!(lifecycle.is_session_active(&current), offline_first);
            assert_eq!(original == current, !offline_first);
            fixture.close().await;
        }
    }).await.expect("live/offline ownership must settle in both controlled orders");
}

#[tokio::test]
async fn hysteresis_uses_resolved_window_or_fallback_and_abort_preserves_open_session() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for resolved in [false, true] {
            let fixture = Fixture::new().await;
            let session = fixture.live().await;
            let expected = if resolved {
                let store = fixture.coordinator.streamer_manager.metadata_store();
                let mut metadata = store.get_mut(STREAMER).unwrap();
                let metadata = Arc::make_mut(metadata.value_mut());
                metadata.offline_check_count = 2;
                metadata.offline_check_delay_ms = 1_000;
                Duration::from_secs(3)
            } else {
                fixture
                    .coordinator
                    .streamer_manager
                    .metadata_store()
                    .remove(STREAMER);
                crate::session::HysteresisConfig::default().window()
            };
            let lifecycle = &fixture.coordinator.session_lifecycle;
            let mut transitions = lifecycle.subscribe();
            lifecycle
                .on_download_terminal(&crate::downloader::DownloadTerminalEvent::Completed {
                    download_id: "ended-attempt".into(),
                    streamer_id: STREAMER.into(),
                    streamer_name: "Coordinator".into(),
                    session_id: session.clone(),
                    total_bytes: 0,
                    total_duration_secs: 0.0,
                    total_segments: 0,
                    file_path: None,
                    engine_signal: EngineEndSignal::CleanDisconnect,
                    stop_cause: None,
                })
                .await
                .unwrap();
            let SessionTransition::Ending {
                observed_at,
                resume_deadline,
                ..
            } = transitions.recv().await.unwrap()
            else {
                panic!("ambiguous completion must enter hysteresis");
            };
            assert_eq!((resume_deadline - observed_at).to_std().unwrap(), expected);
            assert!(
                lifecycle
                    .session_snapshot(&session)
                    .unwrap()
                    .is_hysteresis()
            );
            assert_eq!(
                lifecycle
                    .abort_timers(tokio::time::Instant::now() + Duration::from_secs(2))
                    .await,
                1
            );
            assert!(
                fixture
                    .coordinator
                    .session_repository
                    .get_session(&session)
                    .await
                    .unwrap()
                    .end_time
                    .is_none()
            );
            assert!(
                transitions.try_recv().is_err(),
                "aborted timers cannot publish a terminal transition"
            );
            fixture.close().await;
        }
    })
    .await
    .expect("hysteresis resolver and timer containment must be bounded");
}

fn collection_spec(session: &str, statistics: bool) -> CollectionSpec {
    CollectionSpec {
        session_id: session.into(),
        streamer_id: STREAMER.into(),
        streamer_url: FakeProvider::URL.into(),
        cookies: None,
        extras: None,
        statistics: crate::domain::DanmuStatisticsConfig {
            enabled: statistics,
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn slot_acquisition_cancels_before_queue_admission_after_preflight() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        let session = fixture.live().await;
        let manager = fixture.coordinator.download_manager.clone();
        manager
            .preflight(crate::downloader::PreflightRequest {
                streamer_id: STREAMER.into(),
                streamer_name: "Coordinator".into(),
                session_id: session.clone(),
                output_dir: fixture.directory.path().to_owned(),
                engine_id: Some("ffmpeg".into()),
                engines_override: None,
            })
            .await
            .unwrap();
        let maintenance = manager.try_admit_maintenance(0).unwrap();
        let cancel = CancellationToken::new();
        let mut events = manager.subscribe();
        let acquire = manager.acquire_slot(
            AcquireRequest {
                streamer_id: STREAMER.into(),
                streamer_name: "Coordinator".into(),
                session_id: session,
                engine_type: EngineType::Ffmpeg,
                priority: crate::downloader::Priority::Normal,
            },
            cancel.clone(),
        );
        tokio::pin!(acquire);
        poll_pending(acquire.as_mut()).await;
        assert_eq!(manager.pending_count(), 0);
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), acquire)
                .await
                .unwrap()
                .is_err()
        );
        assert!(
            events.try_recv().is_err(),
            "a never-queued request has no queued/dequeued pair"
        );
        drop(maintenance);
        drop(fixture.blocker().await);
        fixture.assert_no_start();
        fixture.close().await;
    })
    .await
    .expect("maintenance must not strand an acquire cancelled before queue insertion");
}

#[tokio::test]
async fn danmu_startup_cancellation_before_statistics_load_publishes_no_collector() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        let session = fixture.live().await;
        let service = fixture.coordinator.danmu_service.clone();
        let connection = fixture.pool.acquire().await.unwrap();
        let cancel = CancellationToken::new();
        let startup =
            service.start_collection_cancellable(collection_spec(&session, true), &cancel);
        tokio::pin!(startup);
        poll_pending(startup.as_mut()).await;
        assert!(!service.is_collecting(&session));
        assert_eq!(fixture.provider.connects(), 0);
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), startup)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert!(!service.is_collecting(&session));
        assert!(service.get_session_by_streamer(STREAMER).is_none());
        drop(connection);
        fixture.assert_no_start();
        fixture.close().await;
    })
    .await
    .expect("cancelled statistics loading must not publish a collector later");
}

#[tokio::test]
async fn danmu_startup_cancellation_or_dropped_waiter_cleans_registered_connect() {
    tokio::time::timeout(Duration::from_secs(25), async {
        for dropped in [false, true] {
            let fixture = Fixture::new().await;
            let session = fixture.live().await;
            let service = fixture.coordinator.danmu_service.clone();
            *fixture.connect_gate.lock() = Some(Arc::new(tokio::sync::Semaphore::new(0)));
            let cancel = CancellationToken::new();
            let mut startup = Box::pin(
                service.start_collection_cancellable(collection_spec(&session, false), &cancel),
            );
            poll_pending(startup.as_mut()).await;
            while fixture.danmu_configs.lock().is_empty() {
                tokio::task::yield_now().await;
            }
            assert!(
                service.is_collecting(&session),
                "the owned task is installed before connecting"
            );
            if dropped {
                drop(startup);
                while service.is_collecting(&session) {
                    tokio::task::yield_now().await;
                }
            } else {
                cancel.cancel();
                assert!(startup.await.unwrap().is_none());
            }
            assert_eq!(fixture.provider.connects(), 0);
            assert!(service.get_session_by_streamer(STREAMER).is_none());
            fixture.close().await;
        }
    })
    .await
    .expect("readiness cancellation must retain task cleanup ownership");
}

#[tokio::test]
async fn danmu_cancelled_readiness_waits_for_registered_collectors_final_cleanup() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        let session = fixture.live().await;
        let service = fixture.coordinator.danmu_service.clone();
        let cancel = CancellationToken::new();
        let mut startup = Box::pin(
            service.start_collection_cancellable(collection_spec(&session, true), &cancel),
        );
        // Drive statistics loading and task publication, then stop polling the
        // readiness receipt while the independently owned task connects.
        while !service.is_collecting(&session) {
            poll_pending(startup.as_mut()).await;
            if !service.is_collecting(&session) {
                tokio::task::yield_now().await;
            }
        }
        while fixture.provider.connects() == 0 {
            tokio::task::yield_now().await;
        }
        let connection = fixture.pool.acquire().await.unwrap();
        cancel.cancel();
        poll_pending(startup.as_mut()).await;
        while fixture.provider.disconnects() == 0 {
            tokio::task::yield_now().await;
        }
        // Runner finalization still owns a statistics/checkpoint write blocked
        // on this connection. Cancellation cannot report success before it ends.
        poll_pending(startup.as_mut()).await;
        assert!(service.is_collecting(&session));
        drop(connection);
        assert!(startup.await.unwrap().is_none());
        assert!(!service.is_collecting(&session));
        assert!(service.get_session_by_streamer(STREAMER).is_none());
        assert_eq!(fixture.provider.disconnects(), 1);
        fixture.close().await;
    })
    .await
    .expect("cancelled readiness must join final collection persistence");
}

#[tokio::test]
async fn danmu_cancelled_handoff_and_setup_wait_do_not_replace_an_owned_predecessor() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        let session = fixture.live().await;
        let service = fixture.coordinator.danmu_service.clone();
        service
            .start_collection(collection_spec(&session, true))
            .await
            .unwrap();
        let connection = fixture.pool.acquire().await.unwrap();
        let first_cancel = CancellationToken::new();
        let mut replacement = Box::pin(
            service
                .start_collection_cancellable(collection_spec("replacement", false), &first_cancel),
        );
        poll_pending(replacement.as_mut()).await;
        while fixture.provider.disconnects() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(
            service.is_collecting(&session),
            "predecessor finalization is blocked at persistence"
        );
        let second_cancel = CancellationToken::new();
        let mut contender = Box::pin(
            service
                .start_collection_cancellable(collection_spec("contender", false), &second_cancel),
        );
        poll_pending(contender.as_mut()).await;
        second_cancel.cancel();
        assert!(contender.await.unwrap().is_none());
        first_cancel.cancel();
        assert!(replacement.await.unwrap().is_none());
        assert!(service.is_collecting(&session));
        assert!(!service.is_collecting("replacement"));
        assert!(!service.is_collecting("contender"));
        assert_eq!(fixture.provider.connects(), 1);
        drop(connection);
        while service.is_collecting(&session) {
            tokio::task::yield_now().await;
        }
        assert!(service.get_session_by_streamer(STREAMER).is_none());
        assert_eq!(fixture.provider.disconnects(), 1);
        fixture.close().await;
    })
    .await
    .expect("cancelled handoff waiters must leave predecessor finalization owned");
}

fn streams(label: &str) -> Vec<crate::monitor::StreamInfo> {
    vec![crate::monitor::StreamInfo {
        url: format!("https://example.invalid/{label}.flv"),
        stream_format: platforms_parser::media::StreamFormat::Flv,
        media_format: platforms_parser::media::formats::MediaFormat::Flv,
        quality: "best".into(),
        bitrate: 1,
        priority: 1,
        extras: None,
        codec: "h264".into(),
        fps: 30.0,
        is_headers_needed: false,
        is_audio_only: false,
    }]
}

fn payload(session: &str) -> StreamerLivePayload {
    StreamerLivePayload {
        streamer_id: STREAMER.into(),
        session_id: session.into(),
        streamer_name: "Coordinator".into(),
        title: "Contract".into(),
        streams: streams("cached"),
        streamer_url: FakeProvider::URL.into(),
        media_headers: Some(HashMap::from([("Referer".into(), "cached".into())])),
        media_extras: None,
    }
}

fn outside_schedule() -> MonitorEvent {
    MonitorEvent::StateChanged {
        streamer_id: STREAMER.into(),
        streamer_name: "Coordinator".into(),
        old_state: StreamerState::Live,
        new_state: StreamerState::OutOfSchedule,
        reason: Some("out_of_schedule".into()),
        timestamp: Utc::now(),
    }
}

async fn poll_pending<F: Future>(future: std::pin::Pin<&mut F>) {
    let mut future = future;
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn delayed_old_offline_does_not_stop_successor_download_or_danmu() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        let old = fixture.live().await;
        fixture
            .coordinator
            .session_lifecycle
            .end_for_disable(STREAMER, "Coordinator")
            .await
            .unwrap();
        let successor = fixture.live().await;
        assert_ne!(old, successor);
        let id = fixture.start_direct(&successor).await;
        fixture
            .coordinator
            .danmu_service
            .start_collection(CollectionSpec {
                session_id: successor.clone(),
                streamer_id: STREAMER.into(),
                streamer_url: FakeProvider::URL.into(),
                cookies: None,
                extras: None,
                statistics: Default::default(),
            })
            .await
            .unwrap();
        fixture
            .coordinator
            .handle_monitor_event(
                MonitorEvent::StreamerOffline {
                    streamer_id: STREAMER.into(),
                    streamer_name: "Coordinator".into(),
                    session_id: Some(old.clone()),
                    timestamp: Utc::now(),
                },
                false,
            )
            .await;
        assert!(
            !fixture.engine.handles.lock()[0]
                .cancellation_token
                .is_cancelled(),
            "an old Offline must not select the successor by streamer ID"
        );
        assert!(fixture.coordinator.danmu_service.is_collecting(&successor));
        assert!(
            fixture
                .coordinator
                .session_lifecycle
                .is_session_active(&successor)
        );
        assert!(
            fixture
                .coordinator
                .session_repository
                .get_session(&old)
                .await
                .unwrap()
                .end_time
                .is_some()
        );
        assert!(
            fixture
                .coordinator
                .session_repository
                .get_session(&successor)
                .await
                .unwrap()
                .end_time
                .is_none()
        );
        assert_eq!(
            fixture
                .coordinator
                .download_manager
                .get_download_by_streamer(STREAMER)
                .unwrap()
                .id,
            id
        );
        fixture.close().await;
    })
    .await
    .expect("old Offline contract must settle");
}

#[tokio::test]
async fn out_of_schedule_cancels_registered_pipeline_before_it_reaches_the_queue() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new().await;
        let session = fixture.live().await;
        let token = fixture.coordinator.session_cancels.register(&session);
        let connection = fixture.pool.acquire().await.unwrap();
        let pipeline =
            run_live_download_pipeline(fixture.coordinator.clone(), payload(&session), true);
        tokio::pin!(pipeline);
        poll_pending(pipeline.as_mut()).await;
        assert!(fixture.coordinator.pending_pipelines.contains_key(STREAMER));
        assert_eq!(fixture.coordinator.download_manager.pending_count(), 0);
        fixture
            .coordinator
            .handle_monitor_event(outside_schedule(), false)
            .await;
        assert!(
            token.token().is_cancelled(),
            "prequeue work is reachable only through the current lifecycle session"
        );
        drop(connection);
        pipeline.await;
        assert!(fixture.engine.handles.lock().is_empty());
        assert_eq!(fixture.provider.connects(), 0);
        assert!(fixture.coordinator.pending_pipelines.is_empty());
        fixture.close().await;
    })
    .await
    .expect("prequeue cancellation must settle");
}
