use super::*;
use crate::danmu::lifecycle::CollectionExitReason;
use crate::danmu::test_support::FakeProvider;

const OLD: &str = "old-session";
const STREAMER: &str = "streamer";

fn seed(
    service: &DanmuService,
) -> (
    oneshot::Sender<CollectionOutcome>,
    mpsc::Receiver<CollectionCommand>,
    CancellationToken,
) {
    let (commands, receiver) = mpsc::channel(1);
    let (done, completion) = oneshot::channel();
    let token = service.cancel_token.child_token();
    service.collections.insert(
        OLD.to_owned(),
        CollectionState {
            streamer_id: STREAMER.to_owned(),
            cancel_token: token.clone(),
            command_tx: commands,
            done: shared_completion(completion),
            stop_requested: false,
        },
    );
    service
        .sessions_by_streamer
        .insert(STREAMER.to_owned(), OLD.to_owned());
    (done, receiver, token)
}

fn finish(service: &DanmuService, done: oneshot::Sender<CollectionOutcome>) {
    remove_collection(&service.collections, &service.sessions_by_streamer, OLD);
    done.send(CollectionOutcome {
        statistics: DanmuStatistics {
            total_count: 42,
            ..Default::default()
        },
        reason: CollectionExitReason::SessionStopped,
        error: None,
        cleanup_errors: vec![],
    })
    .unwrap();
}

#[tokio::test]
async fn concurrent_stops_share_one_command_and_the_same_completion() {
    let service = DanmuService::new();
    let (done, mut commands, token) = seed(&service);
    let mut first = Box::pin(service.stop_collection(OLD));
    let mut second = Box::pin(service.stop_collection(OLD));
    assert!(futures::poll!(first.as_mut()).is_pending());
    assert!(futures::poll!(second.as_mut()).is_pending());
    assert!(matches!(
        commands.try_recv(),
        Ok(CollectionCommand::Stop(_))
    ));
    assert!(commands.try_recv().is_err());
    finish(&service, done);
    let (first, second) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(first, second)
    })
    .await
    .unwrap();
    assert_eq!(first.unwrap().total_count, 42);
    assert_eq!(second.unwrap().total_count, 42);
    assert!(!token.is_cancelled(), "normal completion remains graceful");
}

#[tokio::test]
async fn cancelling_a_stop_waiter_does_not_consume_the_followers_completion() {
    let service = DanmuService::new();
    let (done, _commands, token) = seed(&service);
    let mut first = Box::pin(service.stop_collection(OLD));
    assert!(futures::poll!(first.as_mut()).is_pending());
    drop(first);
    assert!(
        !token.is_cancelled(),
        "the queued Stop already belongs to the collector"
    );
    let mut follower = Box::pin(service.stop_collection(OLD));
    assert!(futures::poll!(follower.as_mut()).is_pending());
    finish(&service, done);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .unwrap()
            .unwrap()
            .total_count,
        42
    );
}

#[tokio::test]
async fn cancelling_the_elected_sender_while_queue_full_still_signals_stop() {
    let service = DanmuService::new();
    let (done, _commands, token) = seed(&service);
    let (ack_tx, _ack_rx) = oneshot::channel();
    service
        .collections
        .get(OLD)
        .unwrap()
        .command_tx
        .try_send(CollectionCommand::CloseSegment {
            segment_id: "queued".to_owned(),
            discard: false,
            ack: ack_tx,
        })
        .unwrap();
    let mut sender = Box::pin(service.stop_collection(OLD));
    assert!(futures::poll!(sender.as_mut()).is_pending());
    let mut follower = Box::pin(service.stop_collection(OLD));
    assert!(futures::poll!(follower.as_mut()).is_pending());
    drop(sender);
    assert!(token.is_cancelled());
    finish(&service, done);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .unwrap()
            .unwrap()
            .total_count,
        42
    );
}

#[tokio::test(start_paused = true)]
async fn stop_timeouts_leave_late_completion_available_to_followers() {
    for full_queue in [false, true] {
        let service = DanmuService::new();
        let (done, _commands, token) = seed(&service);
        if full_queue {
            let (ack_tx, _ack_rx) = oneshot::channel();
            service
                .collections
                .get(OLD)
                .unwrap()
                .command_tx
                .try_send(CollectionCommand::CloseSegment {
                    segment_id: "queued".to_owned(),
                    discard: false,
                    ack: ack_tx,
                })
                .unwrap();
        }
        let started = tokio::time::Instant::now();
        assert!(
            service
                .stop_collection(OLD)
                .await
                .unwrap_err()
                .to_string()
                .contains("timed out")
        );
        assert_eq!(started.elapsed(), STOP_TIMEOUT);
        assert!(token.is_cancelled());
        let mut follower = Box::pin(service.stop_collection(OLD));
        assert!(futures::poll!(follower.as_mut()).is_pending());
        finish(&service, done);
        assert_eq!(follower.await.unwrap().total_count, 42);
    }
}

#[tokio::test]
async fn replacement_waits_for_an_existing_stop_before_connecting() {
    let (_items, items_rx) = mpsc::channel(1);
    let mut providers = ProviderRegistry::new();
    providers.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(providers);
    let (done, _commands, _token) = seed(&service);
    let mut stop = Box::pin(service.stop_collection(OLD));
    assert!(futures::poll!(stop.as_mut()).is_pending());
    let mut replacement = Box::pin(service.start_collection(collection_spec(
        "new-session",
        STREAMER,
        FakeProvider::URL,
    )));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    assert!(!service.is_collecting("new-session"));
    finish(&service, done);
    let (stopped, started) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(stop, replacement)
    })
    .await
    .unwrap();
    assert_eq!(stopped.unwrap().total_count, 42);
    started.unwrap();
    assert_eq!(
        service.get_session_by_streamer(STREAMER).as_deref(),
        Some("new-session")
    );
    service.stop_collection("new-session").await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelled_replacement_does_not_strand_the_next_replacement() {
    let (_items, items_rx) = mpsc::channel(1);
    let mut providers = ProviderRegistry::new();
    providers.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(providers);
    let (done, _commands, token) = seed(&service);
    let mut abandoned = Box::pin(service.start_collection(collection_spec(
        "abandoned",
        STREAMER,
        FakeProvider::URL,
    )));
    assert!(futures::poll!(abandoned.as_mut()).is_pending());
    drop(abandoned);
    assert!(!token.is_cancelled());
    let mut replacement = Box::pin(service.start_collection(collection_spec(
        "replacement",
        STREAMER,
        FakeProvider::URL,
    )));
    assert!(futures::poll!(replacement.as_mut()).is_pending());
    finish(&service, done);
    tokio::time::timeout(Duration::from_secs(1), replacement)
        .await
        .unwrap()
        .unwrap();
    assert!(!service.is_collecting("abandoned"));
    assert!(service.is_collecting("replacement"));
    service.stop_collection("replacement").await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn competing_replacements_share_one_bounded_handoff_budget() {
    let service = DanmuService::new();
    let (_done, _commands, token) = seed(&service);
    let mut first =
        Box::pin(service.start_collection(collection_spec("new-a", STREAMER, FakeProvider::URL)));
    let mut second =
        Box::pin(service.start_collection(collection_spec("new-b", STREAMER, FakeProvider::URL)));
    assert!(futures::poll!(first.as_mut()).is_pending());
    assert!(futures::poll!(second.as_mut()).is_pending());
    tokio::time::advance(STOP_TIMEOUT).await;
    assert!(first.await.is_err());
    assert!(second.await.is_err());
    assert!(token.is_cancelled());
    assert!(!service.is_collecting("new-a"));
    assert!(!service.is_collecting("new-b"));
}

struct GatedProvider {
    inner: FakeProvider,
    connect_gate: parking_lot::Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    connect_attempts: std::sync::atomic::AtomicUsize,
}

impl GatedProvider {
    fn new(streams: Vec<mpsc::Receiver<platforms_parser::danmaku::DanmuItem>>) -> Self {
        Self {
            inner: FakeProvider::new(streams),
            connect_gate: parking_lot::Mutex::new(None),
            connect_attempts: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn connects(&self) -> usize {
        self.inner.connects()
    }

    fn disconnects(&self) -> usize {
        self.inner.disconnects()
    }
}

#[async_trait::async_trait]
impl platforms_parser::danmaku::DanmuProvider for GatedProvider {
    fn platform(&self) -> &str {
        self.inner.platform()
    }

    async fn connect(
        &self,
        room_id: &str,
        config: ConnectionConfig,
    ) -> platforms_parser::danmaku::error::Result<platforms_parser::danmaku::DanmuStream> {
        self.connect_attempts.fetch_add(1, Ordering::SeqCst);
        let gate = self.connect_gate.lock().clone();
        if let Some(gate) = gate {
            let _ = gate.acquire().await;
        }
        self.inner.connect(room_id, config).await
    }

    async fn disconnect(
        &self,
        connection: &mut platforms_parser::danmaku::DanmuConnection,
    ) -> platforms_parser::danmaku::error::Result<()> {
        self.inner.disconnect(connection).await
    }

    fn supports_url(&self, url: &str) -> bool {
        self.inner.supports_url(url)
    }

    fn extract_room_id(&self, url: &str) -> Option<String> {
        self.inner.extract_room_id(url)
    }
}

struct CancellableStartFixture {
    pool: sqlx::SqlitePool,
    service: Arc<DanmuService>,
    provider: Arc<GatedProvider>,
    _senders: Vec<mpsc::Sender<platforms_parser::danmaku::DanmuItem>>,
}

impl CancellableStartFixture {
    async fn new() -> Self {
        use srec_sqlite::models::{LiveSessionDbModel, StreamerDbModel};
        use srec_sqlite::repositories::session::{SessionRepository, SqlxSessionRepository};
        use srec_sqlite::repositories::streamer::{SqlxStreamerRepository, StreamerRepository};

        let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        srec_sqlite::run_migrations(&pool).await.unwrap();
        let mut streamer = StreamerDbModel::new("Danmu", FakeProvider::URL, "platform-huya");
        streamer.id = STREAMER.to_owned();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
        let mut session = LiveSessionDbModel::new(STREAMER);
        session.id = OLD.to_owned();
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        session_repo.create_session(&session).await.unwrap();

        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        for _ in 0..3 {
            let (tx, rx) = mpsc::channel(4);
            senders.push(tx);
            receivers.push(rx);
        }
        let provider = Arc::new(GatedProvider::new(receivers));
        let mut providers = ProviderRegistry::new();
        providers.register(provider.clone());
        let service =
            Arc::new(DanmuService::with_providers(providers).with_session_repository(session_repo));
        Self {
            pool,
            service,
            provider,
            _senders: senders,
        }
    }

    fn spec(session_id: &str, statistics: bool) -> CollectionSpec {
        CollectionSpec {
            session_id: session_id.to_owned(),
            streamer_id: STREAMER.to_owned(),
            streamer_url: FakeProvider::URL.to_owned(),
            cookies: None,
            extras: None,
            proxy: None,
            statistics: srec_core::domain::DanmuStatisticsConfig {
                enabled: statistics,
                ..Default::default()
            },
        }
    }

    async fn close(self) {
        self.service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(2))
            .await;
        self.pool.close().await;
    }
}

#[tokio::test]
async fn startup_cancellation_before_statistics_load_publishes_no_collector() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let fixture = CancellableStartFixture::new().await;
        let connection = fixture.pool.acquire().await.unwrap();
        let cancel = CancellationToken::new();
        let mut startup = Box::pin(
            fixture
                .service
                .start_collection_cancellable(CancellableStartFixture::spec(OLD, true), &cancel),
        );
        assert!(futures::poll!(startup.as_mut()).is_pending());
        assert!(!fixture.service.is_collecting(OLD));
        assert_eq!(fixture.provider.connects(), 0);
        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), startup)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert!(!fixture.service.is_collecting(OLD));
        assert!(fixture.service.get_session_by_streamer(STREAMER).is_none());
        drop(connection);
        fixture.close().await;
    })
    .await
    .expect("cancelled statistics loading must not publish a collector later");
}

#[tokio::test]
async fn startup_cancellation_or_dropped_waiter_cleans_registered_connect() {
    tokio::time::timeout(Duration::from_secs(15), async {
        for dropped in [false, true] {
            let fixture = CancellableStartFixture::new().await;
            *fixture.provider.connect_gate.lock() = Some(Arc::new(tokio::sync::Semaphore::new(0)));
            let cancel = CancellationToken::new();
            let mut startup =
                Box::pin(fixture.service.start_collection_cancellable(
                    CancellableStartFixture::spec(OLD, false),
                    &cancel,
                ));
            assert!(futures::poll!(startup.as_mut()).is_pending());
            while fixture.provider.connect_attempts.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
            assert!(
                fixture.service.is_collecting(OLD),
                "the owned task is installed before connecting"
            );
            if dropped {
                drop(startup);
                while fixture.service.is_collecting(OLD) {
                    tokio::task::yield_now().await;
                }
            } else {
                cancel.cancel();
                assert!(startup.await.unwrap().is_none());
            }
            assert_eq!(fixture.provider.connects(), 0);
            assert!(fixture.service.get_session_by_streamer(STREAMER).is_none());
            fixture.close().await;
        }
    })
    .await
    .expect("readiness cancellation must retain task cleanup ownership");
}

#[tokio::test]
async fn cancelled_readiness_waits_for_registered_collectors_final_cleanup() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let fixture = CancellableStartFixture::new().await;
        let cancel = CancellationToken::new();
        let mut startup = Box::pin(
            fixture
                .service
                .start_collection_cancellable(CancellableStartFixture::spec(OLD, true), &cancel),
        );
        // Drive statistics loading and task publication, then stop polling the
        // readiness receipt while the independently owned task connects.
        while !fixture.service.is_collecting(OLD) {
            assert!(futures::poll!(startup.as_mut()).is_pending());
            if !fixture.service.is_collecting(OLD) {
                tokio::task::yield_now().await;
            }
        }
        while fixture.provider.connects() == 0 {
            tokio::task::yield_now().await;
        }
        let connection = fixture.pool.acquire().await.unwrap();
        cancel.cancel();
        assert!(futures::poll!(startup.as_mut()).is_pending());
        while fixture.provider.disconnects() == 0 {
            tokio::task::yield_now().await;
        }
        // Runner finalization still owns a statistics/checkpoint write blocked
        // on this connection. Cancellation cannot report success before it ends.
        assert!(futures::poll!(startup.as_mut()).is_pending());
        assert!(fixture.service.is_collecting(OLD));
        drop(connection);
        assert!(startup.await.unwrap().is_none());
        assert!(!fixture.service.is_collecting(OLD));
        assert!(fixture.service.get_session_by_streamer(STREAMER).is_none());
        assert_eq!(fixture.provider.disconnects(), 1);
        fixture.close().await;
    })
    .await
    .expect("cancelled readiness must join final collection persistence");
}

#[tokio::test]
async fn cancelled_handoff_and_setup_wait_do_not_replace_an_owned_predecessor() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let fixture = CancellableStartFixture::new().await;
        fixture
            .service
            .start_collection(CancellableStartFixture::spec(OLD, true))
            .await
            .unwrap();
        let connection = fixture.pool.acquire().await.unwrap();
        let first_cancel = CancellationToken::new();
        let mut replacement = Box::pin(fixture.service.start_collection_cancellable(
            CancellableStartFixture::spec("replacement", false),
            &first_cancel,
        ));
        assert!(futures::poll!(replacement.as_mut()).is_pending());
        while fixture.provider.disconnects() == 0 {
            tokio::task::yield_now().await;
        }
        assert!(
            fixture.service.is_collecting(OLD),
            "predecessor finalization is blocked at persistence"
        );
        let second_cancel = CancellationToken::new();
        let mut contender = Box::pin(fixture.service.start_collection_cancellable(
            CancellableStartFixture::spec("contender", false),
            &second_cancel,
        ));
        assert!(futures::poll!(contender.as_mut()).is_pending());
        second_cancel.cancel();
        assert!(contender.await.unwrap().is_none());
        first_cancel.cancel();
        assert!(replacement.await.unwrap().is_none());
        assert!(fixture.service.is_collecting(OLD));
        assert!(!fixture.service.is_collecting("replacement"));
        assert!(!fixture.service.is_collecting("contender"));
        assert_eq!(fixture.provider.connects(), 1);
        drop(connection);
        while fixture.service.is_collecting(OLD) {
            tokio::task::yield_now().await;
        }
        assert!(fixture.service.get_session_by_streamer(STREAMER).is_none());
        assert_eq!(fixture.provider.disconnects(), 1);
        fixture.close().await;
    })
    .await
    .expect("cancelled handoff waiters must leave predecessor finalization owned");
}
