use std::sync::atomic::AtomicU32;

use super::*;
use crate::web_push::{WebPushConfig, generate_vapid_keypair};

const EVENT_LOG_ID: &str = "00000000-0000-8000-8000-000000000001";

fn event() -> NotificationEvent {
    NotificationEvent::SystemStartup {
        version: "test".into(),
        timestamp: jiff::Timestamp::now(),
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
        created_at: Timestamp::new(0, 0).unwrap(),
        channel_state: HashMap::new(),
        targets: Vec::new(),
        delivery_lock: Default::default(),
        retry_generation: 0,
        retry_cancel: CancellationToken::new(),
        next_retry_at: None,
        durable_settled: None,
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
                // DashMap counts shards separately; freeze admission so concurrent
                // evictions and insertions cannot inflate the observed length.
                let _admission = service.queue_admission.lock();
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
    let before = jiff::Timestamp::now();
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
            >= before + jiff::SignedDuration::from_secs(60)
    );
    service.process_notification(id).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.pending_queue.get(&id).unwrap().channel_state["one"].attempts,
        1
    );
    let runtime = service.registry.read().by_key["one"].clone();
    runtime.breaker.lock().opened_at =
        Some(jiff::Timestamp::now() + jiff::SignedDuration::from_secs(120));
    service.process_notification(id).await;
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "clock rollback cannot bypass cooldown"
    );
    runtime.breaker.lock().opened_at =
        Some(jiff::Timestamp::now() - jiff::SignedDuration::from_secs(61));
    let recovery_attempt = jiff::Timestamp::now();
    service.process_notification(id).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let pending = service.pending_queue.get(&id).unwrap();
    assert_eq!(pending.channel_state["one"].attempts, 2);
    assert!(pending.channel_state["one"].last_error.is_some());
    assert!(
        pending.next_retry_at.unwrap() >= recovery_attempt + jiff::SignedDuration::from_secs(60)
    );
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
            .map_err(|_| crate::NotifyError::other("controlled send abandoned"))
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

#[tokio::test(start_paused = true)]
async fn dispatch_bounds_slow_persistence_backpressures_and_drains_admitted_events() {
    let repo = Arc::new(MockNotificationRepo {
        event_log_gate: Some(Semaphore::new(0)),
        ..MockNotificationRepo::new()
    });
    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig::default(),
        repo.clone(),
    ));
    for _ in 0..DISPATCH_QUEUE_CAPACITY + 5 {
        service.dispatch_notification(event());
    }
    assert_eq!(service.stats().dispatch_dropped, 5);
    tokio::time::timeout(Duration::from_secs(1), async {
        while service.dispatch_permits.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for _ in 0..MAX_NOTIFICATION_PASSES {
        service.dispatch_notification_wait(event()).await;
    }
    let mut waiting = Box::pin(service.dispatch_notification_wait(event()));
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut waiting)
            .await
            .is_err()
    );
    let admitted = DISPATCH_QUEUE_CAPACITY + MAX_NOTIFICATION_PASSES;
    let stopping = tokio::spawn({
        let service = service.clone();
        async move { service.stop().await }
    });
    tokio::time::timeout(Duration::from_secs(1), &mut waiting)
        .await
        .unwrap();
    assert_eq!(service.stats().dispatch_dropped, 6);
    repo.event_log_gate.as_ref().unwrap().add_permits(admitted);
    tokio::time::timeout(Duration::from_secs(1), stopping)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repo.event_logs.lock().await.len(), admitted);
    service.dispatch_notification(event());
    assert_eq!(service.stats().dispatch_dropped, 7);
}

#[tokio::test(start_paused = true)]
async fn channel_send_limit_is_shared_across_events_and_eviction_releases_it() {
    let service = Arc::new(NotificationService::with_config(
        NotificationServiceConfig {
            max_queue_size: MAX_CHANNEL_SENDS / 2 + 1,
            ..Default::default()
        },
    ));
    let mut starts = controlled_targets(&service, &["a", "b"]);
    let mut notifications = Vec::new();
    let mut releases = Vec::new();
    for _ in 0..MAX_CHANNEL_SENDS / 2 {
        let service = service.clone();
        notifications.push(tokio::spawn(async move {
            service.notify(event()).await.unwrap()
        }));
        for _ in 0..2 {
            releases.push(
                tokio::time::timeout(Duration::from_secs(1), starts.recv())
                    .await
                    .unwrap()
                    .unwrap()
                    .1,
            );
        }
    }
    let extra = tokio::spawn({
        let service = service.clone();
        async move { service.notify(event()).await.unwrap() }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(10), starts.recv())
            .await
            .is_err()
    );
    // Evict one live notification without admitting another delivery. Its two
    // sends must be dropped, letting the event waiting on the global limit run.
    assert!(service.enqueue_pending(u64::MAX, pending(u64::MAX)));
    for _ in 0..2 {
        let (_, release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
    }
    assert_eq!(
        releases
            .drain(..)
            .map(|release| usize::from(release.send(()).is_err()))
            .sum::<usize>(),
        2
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        for notification in notifications {
            notification.await.unwrap();
        }
        extra.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(
        service.delivery_permits.available_permits(),
        MAX_CHANNEL_SENDS
    );
    service.stop().await;
}

#[tokio::test(start_paused = true)]
async fn eviction_during_dead_letter_persistence_keeps_the_stored_row() {
    let repo = Arc::new(MockNotificationRepo {
        dead_letter_gate: Some(Semaphore::new(0)),
        ..MockNotificationRepo::new()
    });
    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig {
            max_queue_size: 1,
            max_retries: 1,
            ..Default::default()
        },
        repo.clone(),
    ));
    service.registry.write().insert(Arc::new(RuntimeChannel {
        breaker: service.new_breaker(),
        key: "channel-1".into(),
        db_channel_id: Some("channel-1".into()),
        display_name: "channel-1".into(),
        channel_type: "test".into(),
        channel: Arc::new(TestChannel {
            channel_type: "test",
            fail_for_attempts: u32::MAX,
            attempts: Arc::new(AtomicU32::new(0)),
        }),
    }));
    service
        .registry
        .write()
        .subscriptions_by_event
        .insert("system_startup".into(), vec!["channel-1".into()]);
    let notify = tokio::spawn({
        let service = service.clone();
        async move { service.notify(event()).await.unwrap() }
    });
    // The in-memory dead letter is recorded immediately before persistence,
    // which then waits on the gate.
    tokio::time::timeout(Duration::from_secs(1), async {
        while service.dead_letters.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(service.enqueue_pending(u64::MAX, pending(u64::MAX)));
    repo.dead_letter_gate.as_ref().unwrap().add_permits(1);
    tokio::time::timeout(Duration::from_secs(1), notify)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repo.dead_letters.lock().await.len(), 1);
    assert_eq!(service.dead_letters.len(), 1);
    service.stop().await;
}

#[tokio::test]
async fn dispatch_contains_event_panics_and_restarts_after_idle() {
    let repo = Arc::new(MockNotificationRepo {
        panic_next_event_log: AtomicBool::new(true),
        ..MockNotificationRepo::new()
    });
    let service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig::default(),
        repo.clone(),
    ));
    for _ in 0..3 {
        service.dispatch_notification(event());
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if repo.event_logs.lock().await.len() == 2 && service.dispatch_rx.lock().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service.dispatch_notification(event());
    tokio::time::timeout(Duration::from_secs(2), service.stop())
        .await
        .unwrap();
    assert_eq!(repo.event_logs.lock().await.len(), 3);
    assert_eq!(service.stats().dispatch_dropped, 0);
}

/// Work refused because of shutdown is counted as dropped and never sent.
/// Direct notifications after `stop()` are refused. A supervisor that already
/// shut down refuses the dispatch worker; the event queued for it is counted
/// rather than lost silently, and later events see the closed queue and are
/// counted too.
#[tokio::test]
async fn work_refused_at_shutdown_is_counted_as_dropped_and_never_sent() {
    let service = NotificationService::new();
    let attempts = channel(&service, "one", 0);
    service.stop().await;
    service.notify(event()).await.unwrap();
    service
        .notify_channel_instance("one", event())
        .await
        .unwrap();
    assert_eq!(service.stats().dispatch_dropped, 2);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);

    let supervisor = Arc::new(TaskSupervisor::new());
    let service = Arc::new(NotificationService::new().with_task_supervisor(supervisor.clone()));
    assert!(supervisor.shutdown(Duration::ZERO).await);
    service.dispatch_notification(event());
    assert_eq!(service.stats().dispatch_dropped, 1);
    service.dispatch_notification(event());
    assert_eq!(service.stats().dispatch_dropped, 2);
}

#[tokio::test(start_paused = true)]
async fn durable_entries_survive_full_queue_eviction_and_settle_on_completion() {
    let service = Arc::new(NotificationService::with_config(
        NotificationServiceConfig {
            max_queue_size: 1,
            max_retries: 2,
            initial_retry_delay_ms: 60_000,
            max_retry_delay_ms: 60_000,
            ..Default::default()
        },
    ));
    let mut starts = controlled_targets(&service, &["target"]);

    let durable_task = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
                .await
        }
    });
    let (_, release_durable) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
        .await
        .expect("durable send must start")
        .unwrap();
    let durable_id = *service.pending_queue.iter().next().unwrap().key();
    let durable_cancel = service
        .pending_queue
        .get(&durable_id)
        .unwrap()
        .retry_cancel
        .clone();

    // Queue is at capacity (1) with a durable entry: ordinary traffic must be
    // rejected instead of evicting the durable notification.
    service.notify(event()).await.unwrap();
    assert!(service.pending_queue.contains_key(&durable_id));
    assert!(!durable_cancel.is_cancelled());
    assert_eq!(service.pending_queue.len(), 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), starts.recv())
            .await
            .is_err(),
        "dropped ordinary notification must not start delivery"
    );

    release_durable.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), durable_task)
        .await
        .unwrap()
        .unwrap()
        .expect("durable delivery must settle once its target succeeds");
    assert!(service.pending_queue.is_empty());

    // With capacity 2 holding [durable (older), ordinary (newer)], an incoming
    // ordinary notification evicts the newer ordinary entry and spares the older durable one.
    let service2 = Arc::new(NotificationService::with_config(
        NotificationServiceConfig {
            max_queue_size: 2,
            ..Default::default()
        },
    ));
    let (tx, _rx) = oneshot::channel();
    let mut older_durable = pending(1);
    older_durable.created_at = Timestamp::new(10, 0).unwrap();
    older_durable.durable_settled = Some(DurableSettlement::new(tx));
    let mut newer_ordinary = pending(2);
    newer_ordinary.created_at = Timestamp::new(20, 0).unwrap();
    let ordinary_cancel = newer_ordinary.retry_cancel.clone();
    assert!(service2.enqueue_pending(1, older_durable));
    assert!(service2.enqueue_pending(2, newer_ordinary));
    assert!(service2.enqueue_pending(3, pending(3)));
    assert!(service2.pending_queue.contains_key(&1));
    assert!(!service2.pending_queue.contains_key(&2));
    assert!(ordinary_cancel.is_cancelled());
    assert!(service2.pending_queue.contains_key(&3));

    service.stop().await;
    service2.stop().await;
}

/// Replaying an outbox row reuses its event-log id: the event log, the
/// broadcast that drives desktop notifications and Web Push see the alert once,
/// while channels may receive it again.
#[tokio::test]
async fn replayed_outbox_row_is_logged_broadcast_and_pushed_once() {
    let repo = Arc::new(MockNotificationRepo::new());
    let keys = generate_vapid_keypair();
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .unwrap();
    let web_push = WebPushService::new(
        pool.clone(),
        pool,
        WebPushConfig::from_vapid(
            keys.public_key_b64,
            keys.private_key_b64,
            "mailto:test@localhost",
        )
        .unwrap(),
    )
    .unwrap();
    let service =
        NotificationService::with_repository(NotificationServiceConfig::default(), repo.clone())
            .with_web_push_service(Arc::new(web_push));
    let (push_tx, mut pushes) = mpsc::channel(8);
    *service.web_push_tx.write() = Some(push_tx);
    let mut broadcasts = service.subscribe();
    let sends = channel(&service, "target", 0);

    for _ in 0..2 {
        service
            .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
            .await
            .unwrap();
    }
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    assert_eq!(repo.event_logs.lock().await.len(), 1);
    assert!(broadcasts.try_recv().is_ok());
    assert!(broadcasts.try_recv().is_err());
    assert_eq!(
        pushes.try_recv().unwrap().event_log_id.as_deref(),
        Some(EVENT_LOG_ID)
    );
    assert!(pushes.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn deliver_durable_rejects_shutdown_but_not_dead_letter_persistence_failures() {
    // 1. Mid-delivery shutdown and post-stop calls must return TransientShuttingDown,
    //    drop in-flight sends, and leave pending_queue empty.
    let service = Arc::new(NotificationService::new());
    let mut starts = controlled_targets(&service, &["stalled"]);
    let in_flight = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
                .await
        }
    });
    let (_key, held_release) = tokio::time::timeout(Duration::from_secs(1), starts.recv())
        .await
        .expect("target must begin delivery")
        .unwrap();
    service.stop().await;
    let mid_result = tokio::time::timeout(Duration::from_secs(1), in_flight)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mid_result, Err(DurableDeliveryError::TransientShuttingDown));
    assert!(
        held_release.send(()).is_err(),
        "cancelled durable send must be dropped rather than left detached"
    );
    assert!(service.pending_queue.is_empty());
    assert_eq!(
        service
            .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
            .await,
        Err(DurableDeliveryError::TransientShuttingDown)
    );

    // 2. Dropping the caller future mid-delivery (e.g. drainer cancellation before stop)
    //    must clean up the non-evictable pending_queue entry and cancel retry_cancel.
    let drop_service = Arc::new(NotificationService::new());
    let mut drop_starts = controlled_targets(&drop_service, &["stalled"]);
    let caller_task = tokio::spawn({
        let service = drop_service.clone();
        async move {
            service
                .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
                .await
        }
    });
    let (_key, drop_release) = tokio::time::timeout(Duration::from_secs(1), drop_starts.recv())
        .await
        .expect("target must begin delivery")
        .unwrap();
    let entry_cancel = drop_service
        .pending_queue
        .iter()
        .next()
        .unwrap()
        .retry_cancel
        .clone();
    caller_task.abort();
    assert!(caller_task.await.unwrap_err().is_cancelled());
    assert!(drop_service.pending_queue.is_empty());
    assert!(entry_cancel.is_cancelled());
    assert!(drop_release.send(()).is_err());
    drop_service.stop().await;

    // 3. Failing to persist the terminal dead-letter row still settles Ok(()): a
    //    Transient error would resend the alert to channels that already got it.
    let repo = Arc::new(MockNotificationRepo {
        fail_dead_letter_insert: true,
        ..MockNotificationRepo::new()
    });
    let failing_dl_service = Arc::new(NotificationService::with_repository(
        NotificationServiceConfig {
            max_retries: 1,
            initial_retry_delay_ms: 1,
            max_retry_delay_ms: 1,
            ..Default::default()
        },
        repo,
    ));
    failing_dl_service
        .registry
        .write()
        .insert(Arc::new(RuntimeChannel {
            breaker: failing_dl_service.new_breaker(),
            key: "db:ch-1".into(),
            db_channel_id: Some("ch-1".into()),
            display_name: "ch-1".into(),
            channel_type: "test".into(),
            channel: Arc::new(TestChannel {
                channel_type: "test",
                fail_for_attempts: u32::MAX,
                attempts: Arc::new(AtomicU32::new(0)),
            }),
        }));
    failing_dl_service
        .registry
        .write()
        .subscriptions_by_event
        .insert("system_startup".into(), vec!["ch-1".into()]);
    failing_dl_service.db_loaded.store(true, Ordering::Release);
    let dl_result = tokio::time::timeout(
        Duration::from_secs(1),
        failing_dl_service.deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into()),
    )
    .await
    .unwrap();
    assert_eq!(dl_result, Ok(()));
    assert!(failing_dl_service.pending_queue.is_empty());
    failing_dl_service.stop().await;

    // 4. Disabled service and zero subscribed targets settle Ok(()) while running,
    //    and reject with TransientShuttingDown once stopped.
    let disabled = NotificationService::with_config(NotificationServiceConfig {
        enabled: false,
        ..Default::default()
    });
    assert_eq!(
        disabled
            .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
            .await,
        Ok(())
    );
    disabled.stop().await;
    assert_eq!(
        disabled
            .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
            .await,
        Err(DurableDeliveryError::TransientShuttingDown)
    );

    let unsubscribed = NotificationService::new();
    assert_eq!(
        unsubscribed
            .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
            .await,
        Ok(())
    );
    unsubscribed.stop().await;
    assert_eq!(
        unsubscribed
            .deliver_durable_for_outbox(event(), None, EVENT_LOG_ID.into())
            .await,
        Err(DurableDeliveryError::TransientShuttingDown)
    );
}
