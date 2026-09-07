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
    service
        .collections
        .get(OLD)
        .unwrap()
        .command_tx
        .try_send(CollectionCommand::EndSegment {
            segment_id: "queued".to_owned(),
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
            service
                .collections
                .get(OLD)
                .unwrap()
                .command_tx
                .try_send(CollectionCommand::EndSegment {
                    segment_id: "queued".to_owned(),
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

#[tokio::test]
async fn completed_shared_outcome_is_observable_before_or_after_waiter_poll() {
    let service = DanmuService::new();
    let (done, _commands, _token) = seed(&service);
    let completion = service.collections.get(OLD).unwrap().done.clone();
    let late = completion.clone();
    finish(&service, done);
    assert_eq!(completion.await.unwrap().statistics.total_count, 42);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), late)
            .await
            .unwrap()
            .unwrap()
            .statistics
            .total_count,
        42
    );
}
