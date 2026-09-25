use std::sync::atomic::AtomicU32;

use super::*;

fn event() -> NotificationEvent {
    NotificationEvent::SystemStartup {
        version: "test".into(),
        timestamp: Utc::now(),
    }
}

fn channel(service: &NotificationService, key: &str, failures: u32) -> Arc<AtomicU32> {
    let attempts = Arc::new(AtomicU32::new(0));
    service.registry.write().insert(Arc::new(RuntimeChannel {
        breaker: service.new_breaker(),
        key: key.into(),
        db_channel_id: None,
        display_name: key.into(),
        channel_type: "test".into(),
        channel: Arc::new(TestChannel {
            channel_type: "test",
            fail_for_attempts: failures,
            attempts: attempts.clone(),
        }),
    }));
    attempts
}

fn pending(id: u64) -> PendingNotification {
    PendingNotification {
        _id: id,
        event: event(),
        created_at: DateTime::from_timestamp(0, 0).unwrap(),
        channel_state: HashMap::new(),
        targets: Vec::new(),
        delivery_lock: Default::default(),
        retry_generation: 0,
        retry_cancel: CancellationToken::new(),
        next_retry_at: None,
    }
}

#[test]
fn concurrent_admission_never_exceeds_capacity_and_evicts_oldest_ties_by_id() {
    let service = Arc::new(NotificationService::with_config(
        NotificationServiceConfig {
            max_queue_size: 4,
            ..Default::default()
        },
    ));
    let barrier = Arc::new(std::sync::Barrier::new(16));
    std::thread::scope(|threads| {
        for id in 0..16 {
            let service = service.clone();
            let barrier = barrier.clone();
            threads.spawn(move || {
                barrier.wait();
                assert!(service.enqueue_pending(id, pending(id)));
                assert!(service.pending_queue.len() <= 4);
            });
        }
    });
    assert_eq!(service.pending_queue.len(), 4);
    // A deterministic second scenario pins oldest selection, including tied timestamps.
    service.pending_queue.clear();
    for id in [4, 2, 3, 1] {
        service.enqueue_pending(id, pending(id));
    }
    let cancellation = service.pending_queue.get(&1).unwrap().retry_cancel.clone();
    service.enqueue_pending(5, pending(5));
    assert!(!service.pending_queue.contains_key(&1));
    assert!(cancellation.is_cancelled());
    assert_eq!(service.pending_queue.len(), 4);
}

#[tokio::test]
async fn zero_capacity_and_unknown_targets_do_not_deliver_or_evict() {
    let service = NotificationService::with_config(NotificationServiceConfig {
        max_queue_size: 0,
        ..Default::default()
    });
    let attempts = channel(&service, "one", 0);
    service.notify(event()).await.unwrap();
    service
        .notify_channel_instance("one", event())
        .await
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert!(service.pending_queue.is_empty());
    assert!(
        service
            .notify_channel_instance("missing", event())
            .await
            .is_err()
    );
    assert!(service.pending_queue.is_empty());
}

#[tokio::test]
async fn stale_retry_generation_finishes_without_sending() {
    let service = NotificationService::with_config(NotificationServiceConfig {
        initial_retry_delay_ms: 10,
        max_retry_delay_ms: 10,
        ..Default::default()
    });
    let attempts = channel(&service, "one", u32::MAX);
    service
        .notify_channel_instance("one", event())
        .await
        .unwrap();
    let id = *service.pending_queue.iter().next().unwrap().key();
    service.pending_queue.get_mut(&id).unwrap().retry_generation += 1;
    tokio::time::timeout(Duration::from_secs(2), async {
        while Arc::strong_count(&service.pending_queue) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("obsolete retry must finish and release its delivery context");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.pending_queue.get(&id).unwrap().channel_state["one"].attempts,
        1
    );
}

#[tokio::test]
async fn breaker_cooldown_preserves_attempts_and_retry_floor() {
    let service = NotificationService::with_config(NotificationServiceConfig {
        circuit_breaker_threshold: 1,
        circuit_breaker_cooldown_secs: 60,
        initial_retry_delay_ms: 1,
        max_retry_delay_ms: 1,
        ..Default::default()
    });
    let attempts = channel(&service, "one", u32::MAX);
    let before = Utc::now();
    service
        .notify_channel_instance("one", event())
        .await
        .unwrap();
    let id = *service.pending_queue.iter().next().unwrap().key();
    assert!(
        service
            .pending_queue
            .get(&id)
            .unwrap()
            .next_retry_at
            .unwrap()
            >= before + chrono::Duration::seconds(60)
    );
    service.process_notification(id).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.pending_queue.get(&id).unwrap().channel_state["one"].attempts,
        1
    );
    let runtime = service.registry.read().by_key["one"].clone();
    runtime.breaker.lock().opened_at = Some(Utc::now() + chrono::Duration::seconds(120));
    service.process_notification(id).await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "clock rollback cannot bypass cooldown"
    );
    runtime.breaker.lock().opened_at = Some(Utc::now() - chrono::Duration::seconds(61));
    let recovery_attempt = Utc::now();
    service.process_notification(id).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let pending = service.pending_queue.get(&id).unwrap();
    assert_eq!(pending.channel_state["one"].attempts, 2);
    assert!(pending.channel_state["one"].last_error.is_some());
    assert!(pending.next_retry_at.unwrap() >= recovery_attempt + chrono::Duration::seconds(60));
    drop(pending);
    service.cancellation_token.cancel();
    assert!(
        service
            .task_supervisor
            .shutdown(Duration::from_secs(1))
            .await
    );
}

#[tokio::test]
async fn evicted_retry_is_cancelled_and_channel_failures_are_isolated() {
    let service = NotificationService::with_config(NotificationServiceConfig {
        max_queue_size: 1,
        initial_retry_delay_ms: 60_000,
        max_retry_delay_ms: 60_000,
        max_retries: 2,
        ..Default::default()
    });
    let bad = channel(&service, "bad", u32::MAX);
    let good = channel(&service, "good", 0);
    service.notify(event()).await.unwrap();
    assert_eq!(bad.load(Ordering::SeqCst), 1);
    assert_eq!(good.load(Ordering::SeqCst), 1);
    let old = *service.pending_queue.iter().next().unwrap().key();
    let cancellation = service
        .pending_queue
        .get(&old)
        .unwrap()
        .retry_cancel
        .clone();
    assert!(
        service
            .notify_channel_instance("missing", event())
            .await
            .is_err()
    );
    assert!(service.pending_queue.contains_key(&old));
    service
        .notify_channel_instance("bad", event())
        .await
        .unwrap();
    assert!(cancellation.is_cancelled());
    assert!(!service.pending_queue.contains_key(&old));
    let next = *service.pending_queue.iter().next().unwrap().key();
    service.process_notification(old).await;
    assert_eq!(bad.load(Ordering::SeqCst), 2);
    service.process_notification(next).await;
    assert_eq!(bad.load(Ordering::SeqCst), 3);
    assert_eq!(good.load(Ordering::SeqCst), 1);
    assert!(service.pending_queue.is_empty());
    let dead = service.dead_letters.iter().next().unwrap();
    assert_eq!(dead.attempts, 2);
    assert!(!dead.error.is_empty());
    drop(dead);
    service.cancellation_token.cancel();
    assert!(
        service
            .task_supervisor
            .shutdown(Duration::from_secs(1))
            .await
    );
}

#[tokio::test]
async fn cancellation_wins_when_a_retry_timer_is_already_ready() {
    tokio::time::timeout(Duration::from_secs(2), async {
        let service = NotificationService::with_config(NotificationServiceConfig {
            initial_retry_delay_ms: 0,
            max_retry_delay_ms: 0,
            max_retries: 2,
            ..Default::default()
        });
        let attempts = channel(&service, "one", u32::MAX);
        service
            .notify_channel_instance("one", event())
            .await
            .unwrap();
        // The current-thread runtime has not polled the spawned retry yet.
        // Its zero-delay timer and cancellation are both ready on first poll.
        service.cancellation_token.cancel();
        assert!(
            service
                .task_supervisor
                .shutdown(Duration::from_secs(1))
                .await
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(service.pending_queue.len(), 1);
    })
    .await
    .expect("ready retry cancellation must settle promptly");
}

struct ControlledChannel {
    key: &'static str,
    started: tokio::sync::mpsc::UnboundedSender<(&'static str, tokio::sync::oneshot::Sender<()>)>,
}

#[async_trait::async_trait]
impl NotificationChannel for ControlledChannel {
    fn channel_type(&self) -> &'static str {
        "controlled"
    }
    fn is_enabled(&self) -> bool {
        true
    }
    async fn send(&self, _: &NotificationEvent) -> Result<()> {
        let (release, released) = tokio::sync::oneshot::channel();
        self.started.send((self.key, release)).unwrap();
        released
            .await
            .map_err(|_| crate::Error::Other("controlled send abandoned".into()))
    }
}

fn controlled_targets(
    service: &NotificationService,
    keys: &[&'static str],
) -> tokio::sync::mpsc::UnboundedReceiver<(&'static str, tokio::sync::oneshot::Sender<()>)> {
    let (started, receiver) = tokio::sync::mpsc::unbounded_channel();
    for key in keys {
        service.registry.write().insert(Arc::new(RuntimeChannel {
            breaker: service.new_breaker(),
            key: (*key).into(),
            db_channel_id: None,
            display_name: (*key).into(),
            channel_type: "controlled".into(),
            channel: Arc::new(ControlledChannel {
                key,
                started: started.clone(),
            }),
        }));
    }
    receiver
}

#[tokio::test(start_paused = true)]
async fn healthy_targets_finish_before_stalled_targets_without_overlapping_passes() {
    let service = Arc::new(NotificationService::new());
    let mut starts = controlled_targets(&service, &["stalled", "healthy"]);
    let notify = tokio::spawn({
        let service = service.clone();
        async move {
            service.notify(event()).await.unwrap();
        }
    });
    let mut releases = HashMap::new();
    for _ in 0..2 {
        let (key, release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .expect("independent targets must both start")
            .unwrap();
        releases.insert(key, release);
    }
    releases.remove("healthy").unwrap().send(()).unwrap();
    let id = *service.pending_queue.iter().next().unwrap().key();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if service.pending_queue.get(&id).unwrap().channel_state["healthy"].status
                == DeliveryStatus::Delivered
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(!notify.is_finished(), "the stalled target is still pending");
    let another_pass = tokio::spawn({
        let service = service.clone();
        async move {
            service.process_notification(id).await;
        }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(10), starts.recv())
            .await
            .is_err(),
        "an overlapping pass must not duplicate a target already in flight"
    );
    releases.remove("stalled").unwrap().send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        notify.await.unwrap();
        another_pass.await.unwrap();
    })
    .await
    .unwrap();
    assert!(starts.try_recv().is_err());
    assert_eq!(service.stats().pending_count, 0);
    service.stop().await;
}

#[tokio::test(start_paused = true)]
async fn channel_concurrency_is_bounded_per_notification() {
    let service = Arc::new(NotificationService::new());
    let mut starts = controlled_targets(&service, &["a", "b", "c", "d", "e", "f", "g", "h"]);
    let notify = tokio::spawn({
        let service = service.clone();
        async move {
            service.notify(event()).await.unwrap();
        }
    });
    let mut releases = Vec::new();
    for _ in 0..4 {
        let (_, release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap();
        releases.push(release);
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(10), starts.recv())
            .await
            .is_err()
    );
    for _ in 0..4 {
        releases.pop().unwrap().send(()).unwrap();
        let (_, release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap();
        releases.push(release);
    }
    for release in releases {
        release.send(()).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(1), notify)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(service.stats().pending_count, 0);
    service.stop().await;
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_delivery_pass_drops_sends_and_releases_the_pass_lock() {
    let service = Arc::new(NotificationService::new());
    let mut starts = controlled_targets(&service, &["a", "b", "c", "d", "e", "f", "g", "h"]);
    let notify = tokio::spawn({
        let service = service.clone();
        async move {
            service.notify(event()).await.unwrap();
        }
    });
    let mut releases = Vec::new();
    for _ in 0..4 {
        let (_, release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap();
        releases.push(release);
    }
    let id = *service.pending_queue.iter().next().unwrap().key();
    notify.abort();
    assert!(notify.await.unwrap_err().is_cancelled());
    for release in releases {
        assert!(
            release.send(()).is_err(),
            "cancelled channel futures must not remain detached"
        );
    }
    assert!(starts.try_recv().is_err());
    let resume = tokio::spawn({
        let service = service.clone();
        async move {
            service.process_notification(id).await;
        }
    });
    for _ in 0..8 {
        let (_, release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(1), resume)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(service.stats().pending_count, 0);
    service.stop().await;
}
