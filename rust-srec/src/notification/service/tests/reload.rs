use super::*;

fn db_channel(id: &str, name: &str) -> NotificationChannelDbModel {
    NotificationChannelDbModel {
        id: id.to_owned(),
        name: name.to_owned(),
        channel_type: "Webhook".to_owned(),
        settings: r#"{"enabled":true,"url":"http://example.invalid","method":"POST"}"#.to_owned(),
    }
}

fn event() -> NotificationEvent {
    NotificationEvent::SystemStartup {
        version: "test".to_owned(),
        timestamp: Utc::now(),
    }
}

async fn fixture() -> (Arc<MockNotificationRepo>, NotificationService) {
    let repo = Arc::new(MockNotificationRepo::new());
    repo.channels
        .lock()
        .await
        .push(db_channel("stable", "Original"));
    repo.subscriptions
        .lock()
        .await
        .insert("stable".to_owned(), vec!["system_startup".to_owned()]);
    let service = NotificationService::with_repository(
        NotificationServiceConfig {
            circuit_breaker_threshold: 1,
            max_retries: 1,
            ..Default::default()
        },
        repo.clone(),
    );
    service.reload_from_db().await.unwrap();
    (repo, service)
}

fn install_transport(
    service: &NotificationService,
    channel: Arc<dyn NotificationChannel>,
) -> Arc<RuntimeChannel> {
    let mut registry = service.registry.write();
    let mut runtime = registry.by_key["stable"].as_ref().clone();
    runtime.channel = channel;
    let runtime = Arc::new(runtime);
    for entry in &mut registry.channels {
        if entry.key == "stable" {
            *entry = runtime.clone();
        }
    }
    registry.by_key.insert("stable".to_owned(), runtime.clone());
    runtime
}

#[tokio::test]
async fn same_db_identity_preserves_breaker_across_edits_and_repeated_reload() {
    let (repo, service) = fixture().await;
    let old = service.registry.read().by_key["stable"].clone();
    old.breaker.lock().record_failure(1);
    let opened = old.breaker.lock().opened_at;
    repo.channels.lock().await[0].name = "Edited".to_owned();
    repo.channels.lock().await[0].settings =
        r#"{"enabled":true,"url":"http://edited.invalid","method":"POST"}"#.to_owned();
    service.reload_from_db().await.unwrap();
    service.reload_from_db().await.unwrap();
    let current = service.registry.read().by_key["stable"].clone();
    assert_eq!(current.display_name, "Edited");
    assert!(Arc::ptr_eq(&old.breaker, &current.breaker));
    assert_eq!(current.breaker.lock().opened_at, opened);
    assert_eq!(current.breaker.lock().failures, 1);
    assert!(service.stats().circuit_breakers["stable"]);
}

#[tokio::test]
async fn failed_reload_preserves_registry_breakers_and_defers_subscription_migration() {
    let (repo, service) = fixture().await;
    let old = service.registry.read().by_key["stable"].clone();
    old.breaker.lock().record_failure(1);
    *repo.channels.lock().await = vec![
        db_channel("stable", "Not published"),
        db_channel("broken", "Broken"),
    ];
    repo.subscriptions
        .lock()
        .await
        .insert("stable".to_owned(), vec!["SystemStartup".to_owned()]);
    *repo.fail_subscriptions_for.lock() = Some("broken".to_owned());
    assert!(service.reload_from_db().await.is_err());
    {
        let registry = service.registry.read();
        assert_eq!(registry.channels.len(), 1);
        assert!(Arc::ptr_eq(&registry.by_key["stable"], &old));
        assert_eq!(
            registry.subscriptions_by_event["system_startup"],
            ["stable"]
        );
    }
    assert!(old.breaker.lock().is_open);
    assert!(repo.subscribe_calls.lock().await.is_empty());
    assert!(repo.unsubscribe_calls.lock().await.is_empty());
}

#[tokio::test]
async fn overlapping_reloads_serialize_and_preserve_dynamic_additions_during_io() {
    let (repo, service) = fixture().await;
    let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
    install_transport(
        &service,
        Arc::new(TestChannel {
            channel_type: "during-reload",
            fail_for_attempts: 0,
            attempts: attempts.clone(),
        }),
    );
    repo.pause_next_list.store(true, Ordering::SeqCst);
    let mut first = Box::pin(service.reload_from_db());
    assert!(futures::poll!(first.as_mut()).is_pending());
    let mut second = Box::pin(service.reload_from_db());
    assert!(futures::poll!(second.as_mut()).is_pending());
    assert_eq!(
        repo.list_calls.load(Ordering::SeqCst),
        2,
        "second reload must not start stale discovery"
    );
    service.notify(event()).await.unwrap();
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "lookups and subscriptions remain usable during reload IO"
    );
    service.add_channel(ChannelConfig::Discord(DiscordConfig {
        enabled: true,
        webhook_url: "http://dynamic.invalid".to_owned(),
        ..Default::default()
    }));
    let dynamic = service
        .list_channel_instances()
        .into_iter()
        .find(|channel| channel.key.starts_with("dynamic:"))
        .unwrap();
    *repo.channels.lock().await = vec![db_channel("replacement", "Replacement")];
    repo.subscriptions
        .lock()
        .await
        .insert("replacement".to_owned(), vec!["system_startup".to_owned()]);
    repo.list_release.notify_one();
    tokio::time::timeout(Duration::from_secs(1), async {
        first.await.unwrap();
        second.await.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(repo.list_calls.load(Ordering::SeqCst), 3);
    let registry = service.registry.read();
    assert_eq!(registry.channels.len(), 2);
    assert!(!registry.by_key.contains_key("stable"));
    assert!(registry.by_key.contains_key(&dynamic.key));
    assert_eq!(
        registry.subscriptions_by_event["system_startup"],
        ["replacement"]
    );
    assert!(
        registry
            .channels
            .iter()
            .all(|channel| registry.by_key.contains_key(&channel.key))
    );
}

#[tokio::test]
async fn admitted_targets_survive_removal_before_delivery_begins() {
    let (repo, service) = fixture().await;
    let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let captured = install_transport(
        &service,
        Arc::new(TestChannel {
            channel_type: "captured",
            fail_for_attempts: 0,
            attempts: attempts.clone(),
        }),
    );
    // The admitted notification owns the selected instance, before its first delivery poll.
    service.pending_queue.insert(
        99,
        PendingNotification {
            _id: 99,
            event: event(),
            created_at: Utc::now(),
            targets: vec![captured],
            channel_state: HashMap::from([(
                "stable".to_owned(),
                ChannelDeliveryState {
                    status: DeliveryStatus::Pending,
                    attempts: 0,
                    last_attempt: None,
                    last_error: None,
                },
            )]),
            retry_generation: 0,
            retry_cancel: CancellationToken::new(),
            next_retry_at: None,
        },
    );
    repo.channels.lock().await.clear();
    service.reload_from_db().await.unwrap();
    assert!(service.list_channel_instances().is_empty());
    assert!(service.pending_queue.contains_key(&99));
    service.process_notification(99).await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(!service.pending_queue.contains_key(&99));
}

struct BlockedChannel {
    release: tokio::sync::Notify,
    fail: bool,
}

#[async_trait::async_trait]
impl NotificationChannel for BlockedChannel {
    fn channel_type(&self) -> &'static str {
        "blocked"
    }
    fn is_enabled(&self) -> bool {
        true
    }
    async fn send(&self, _: &NotificationEvent) -> Result<()> {
        self.release.notified().await;
        if self.fail {
            Err(crate::Error::Other("injected delivery failure".to_owned()))
        } else {
            Ok(())
        }
    }
    async fn test(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn removed_and_readded_channel_has_a_breaker_isolated_from_old_delivery() {
    for old_fails in [false, true] {
        let (repo, service) = fixture().await;
        let transport = Arc::new(BlockedChannel {
            release: tokio::sync::Notify::new(),
            fail: old_fails,
        });
        let old = install_transport(&service, transport.clone());
        let mut delivery = Box::pin(service.notify_channel_instance("stable", event()));
        assert!(futures::poll!(delivery.as_mut()).is_pending());
        repo.channels.lock().await.clear();
        service.reload_from_db().await.unwrap();
        repo.channels
            .lock()
            .await
            .push(db_channel("stable", "Readded"));
        service.reload_from_db().await.unwrap();
        let current = service.registry.read().by_key["stable"].clone();
        assert!(!Arc::ptr_eq(&old.breaker, &current.breaker));
        if !old_fails {
            current.breaker.lock().record_failure(1);
        }
        transport.release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), delivery)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            current.breaker.lock().is_open,
            !old_fails,
            "old outcomes must not modify the new generation"
        );
        assert_eq!(old.breaker.lock().is_open, old_fails);
    }
}

#[tokio::test]
async fn failed_canonical_subscription_write_keeps_the_legacy_subscription() {
    let (repo, service) = fixture().await;
    repo.subscriptions
        .lock()
        .await
        .insert("stable".to_owned(), vec!["SystemStartup".to_owned()]);
    repo.fail_subscribe.store(true, Ordering::SeqCst);
    service.reload_from_db().await.unwrap();
    assert_eq!(repo.subscriptions.lock().await["stable"], ["SystemStartup"]);
    assert!(repo.unsubscribe_calls.lock().await.is_empty());
    assert_eq!(
        service.registry.read().subscriptions_by_event["system_startup"],
        ["stable"]
    );
}
