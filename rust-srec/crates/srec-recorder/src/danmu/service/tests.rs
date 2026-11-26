use super::*;

mod handoff;

#[tokio::test]
async fn cancelled_startup_reports_missing_cleanup_outcome_after_task_abort() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let provider = Arc::new(PendingConnectProvider::new());
        let mut providers = ProviderRegistry::new();
        providers.register(provider.clone());
        let service = DanmuService::with_providers(providers);
        let cancel = CancellationToken::new();
        let startup = service.start_collection_cancellable(
            collection_spec("aborted-start", "streamer", PendingConnectProvider::URL),
            &cancel,
        );
        tokio::pin!(startup);
        tokio::select! {
            _ = provider.entered.notified() => {}
            _ = &mut startup => panic!("provider must still be connecting"),
        }
        service.collection_tasks.lock().abort_all();
        while service.is_collecting("aborted-start") {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
        let error = match startup.await {
            Err(error) => error,
            Ok(_) => panic!("missing cleanup outcome must not acknowledge cancellation"),
        };
        assert!(error.to_string().contains("aborted-start"));
        assert!(error.to_string().contains("without a cleanup outcome"));
        let report = service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
            .await;
        assert_eq!(report.shutdown_failures.len(), 1);
    })
    .await
    .expect("aborted startup must report lost cleanup ownership promptly");
}

struct PendingConnectProvider {
    entered: tokio::sync::Notify,
}

impl PendingConnectProvider {
    const URL: &'static str = "https://pending.test/room-1";

    fn new() -> Self {
        Self {
            entered: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait::async_trait]
impl platforms_parser::danmaku::DanmuProvider for PendingConnectProvider {
    fn platform(&self) -> &str {
        "pending"
    }

    async fn connect(
        &self,
        _room_id: &str,
        _config: ConnectionConfig,
    ) -> platforms_parser::danmaku::error::Result<platforms_parser::danmaku::DanmuStream> {
        self.entered.notify_one();
        std::future::pending().await
    }

    async fn disconnect(
        &self,
        _connection: &mut platforms_parser::danmaku::DanmuConnection,
    ) -> platforms_parser::danmaku::error::Result<()> {
        Ok(())
    }

    fn supports_url(&self, url: &str) -> bool {
        url.contains("pending.test")
    }

    fn extract_room_id(&self, _url: &str) -> Option<String> {
        Some("room-1".to_string())
    }
}

/// Seed the service's maps as if a prior collector had spawned for this
/// `(streamer_id, session_id)`, without actually running a connection
/// task. Returns a oneshot tx the test can use to drive the runner's
/// "done" signal — `stop_collection` awaits the rx with a 10s timeout,
/// so resolving the tx makes the abort path return promptly.
fn seed_active_collection(
    service: &DanmuService,
    streamer_id: &str,
    session_id: &str,
) -> (oneshot::Sender<CollectionOutcome>, CancellationToken) {
    let (command_tx, _command_rx) = mpsc::channel(8);
    let (done_tx, done_rx) = oneshot::channel();
    let cancel_token = service.cancel_token.child_token();
    let state = CollectionState {
        streamer_id: streamer_id.to_string(),
        cancel_token: cancel_token.clone(),
        command_tx,
        done: shared_completion(done_rx),
        stop_requested: false,
    };
    service.collections.insert(session_id.to_string(), state);
    service
        .sessions_by_streamer
        .insert(streamer_id.to_string(), session_id.to_string());
    (done_tx, cancel_token)
}

fn collection_spec(session_id: &str, streamer_id: &str, streamer_url: &str) -> CollectionSpec {
    CollectionSpec {
        session_id: session_id.to_string(),
        streamer_id: streamer_id.to_string(),
        streamer_url: streamer_url.to_string(),
        cookies: None,
        extras: None,
        proxy: None,
        statistics: srec_core::domain::DanmuStatisticsConfig::default(),
    }
}

#[tokio::test]
async fn shutdown_finalizes_xml_and_joins_collection_before_returning() {
    use crate::danmu::test_support::{FakeProvider, temp_xml_path};

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(registry);
    let mut events = service.subscribe();
    let xml_path = temp_xml_path("service-shutdown");

    let handle = service
        .start_collection(collection_spec(
            "session-shutdown",
            "streamer-shutdown",
            FakeProvider::URL,
        ))
        .await
        .expect("collection should start");
    handle
        .start_segment("0", xml_path.clone(), jiff::Timestamp::now())
        .await
        .expect("segment start should be accepted");
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("segment start event should arrive")
            .expect("event channel should remain open");
        if matches!(event, DanmuEvent::SegmentStarted { .. }) {
            break;
        }
    }

    service
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(2))
        .await;

    let xml = tokio::fs::read_to_string(&xml_path)
        .await
        .expect("finalized XML should remain readable");
    assert!(xml.trim_end().ends_with("</i>"));
    assert!(!service.is_collecting("session-shutdown"));
    assert!(service.collection_tasks.lock().is_empty());

    let mut completed_positions = Vec::new();
    let mut stopped_positions = Vec::new();
    let mut position = 0;
    while let Ok(event) = events.try_recv() {
        match event {
            DanmuEvent::SegmentCompleted {
                ref output_path, ..
            } => {
                assert_eq!(output_path, &xml_path);
                completed_positions.push(position);
            }
            DanmuEvent::CollectionStopped { .. } => stopped_positions.push(position),
            _ => {}
        }
        position += 1;
    }
    assert_eq!(completed_positions.len(), 1);
    assert_eq!(stopped_positions.len(), 1);
    assert!(completed_positions[0] < stopped_positions[0]);

    tokio::fs::remove_file(xml_path)
        .await
        .expect("test XML should be removable after shutdown");
}

#[tokio::test]
async fn shutdown_rejects_late_collection_starts() {
    let service = DanmuService::new();
    service
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;

    let result = service
        .start_collection(collection_spec(
            "late-session",
            "late-streamer",
            "https://example.invalid/live",
        ))
        .await;
    let Err(error) = result else {
        panic!("late collection must be rejected");
    };
    assert!(error.to_string().contains("shutting down"));
}

#[tokio::test]
async fn shutdown_report_retains_genuine_collection_start_failure() {
    use crate::danmu::test_support::FakeProvider;

    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(Vec::new())));
    let service = DanmuService::with_providers(registry);

    let start_result = service
        .start_collection(collection_spec(
            "failed-session",
            "failed-streamer",
            FakeProvider::URL,
        ))
        .await;
    let Err(start_error) = start_result else {
        panic!("provider without a stream must reject collection startup");
    };
    assert!(
        start_error
            .to_string()
            .contains("fake provider has no streams left")
    );

    let report = service
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    assert!(
        report.runtime_failures.iter().any(|failure| {
            failure.contains("failed-session")
                && failure.contains("fake provider has no streams left")
        }),
        "a collection that never connected must still be reported"
    );
    assert!(
        report.shutdown_failures.is_empty(),
        "a collection that failed to start is not a shutdown-phase failure: {:?}",
        report.shutdown_failures
    );
}

#[tokio::test]
async fn shutdown_report_retains_collection_runtime_failure() {
    use crate::danmu::test_support::{FakeProvider, temp_xml_path};

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(registry);
    for index in 0..256 {
        service.seed_runtime_failure_for_test(format!("historical failure {index}: old error"));
    }
    let mut events = service.subscribe();
    let handle = service
        .start_collection(collection_spec(
            "runtime-failure-session",
            "runtime-failure-streamer",
            FakeProvider::URL,
        ))
        .await
        .expect("collection should become ready");

    // A regular file cannot be used as the parent directory of the XML.
    let blocker = temp_xml_path("service-runtime-failure-blocker");
    tokio::fs::write(&blocker, b"not a directory")
        .await
        .expect("blocker file should be created");
    handle
        .start_segment(
            "0",
            blocker.join("child").join("segment.xml"),
            jiff::Timestamp::now(),
        )
        .await
        .expect("failing segment command should be admitted");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let event = events
                .recv()
                .await
                .expect("event channel should remain available");
            if matches!(event, DanmuEvent::CollectionStopped { .. }) {
                break;
            }
        }
    })
    .await
    .expect("failed collection should publish its terminal event");

    let report = service
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    tokio::fs::remove_file(blocker)
        .await
        .expect("blocker file should be removable");

    assert!(
        report
            .runtime_failures
            .iter()
            .any(|failure| failure.contains("danmu collection runtime-failure-session failed")),
        "the runner's error must still be reported"
    );
    assert!(report.runtime_failures[0].contains("earlier runtime failures omitted"));
    assert!(
        report
            .runtime_failures
            .iter()
            .all(|failure| !failure.starts_with("historical failure 0:")),
        "older diagnostics must not remain resident indefinitely"
    );
    assert!(
        report.shutdown_failures.is_empty(),
        "a collection that ended before shutdown must not fail the shutdown: {:?}",
        report.shutdown_failures
    );
}

#[tokio::test]
async fn shutdown_cancellation_during_startup_is_not_a_runtime_failure() {
    let provider = Arc::new(PendingConnectProvider::new());
    let mut registry = ProviderRegistry::new();
    registry.register(provider.clone());
    let service = Arc::new(DanmuService::with_providers(registry));

    let start_service = service.clone();
    let start = tokio::spawn(async move {
        start_service
            .start_collection(collection_spec(
                "cancelled-session",
                "cancelled-streamer",
                PendingConnectProvider::URL,
            ))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), provider.entered.notified())
        .await
        .expect("provider connect should begin");

    let report = service
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    let start_result = start.await.expect("start task should join");
    let Err(start_error) = start_result else {
        panic!("shutdown must cancel startup");
    };

    assert!(start_error.to_string().contains("cancelled during startup"));
    assert!(report.runtime_failures.is_empty());
    assert!(report.shutdown_failures.is_empty());
    assert!(report.forced_session_ids.is_empty());
}

#[tokio::test]
async fn shutdown_drains_collection_cleanup_after_graceful_deadline() {
    let service = Arc::new(DanmuService::new());
    let cancelled = Arc::new(tokio::sync::Notify::new());
    let release_cleanup = Arc::new(tokio::sync::Notify::new());
    let task_token = service.cancel_token.child_token();
    let task_cancelled = cancelled.clone();
    let task_release = release_cleanup.clone();
    service.collection_tasks.lock().spawn(async move {
        task_token.cancelled().await;
        task_cancelled.notify_one();
        task_release.notified().await;
        CollectionTaskReport {
            session_id: "slow-cleanup-session".to_string(),
            runtime_error: None,
            cleanup_errors: Vec::new(),
        }
    });

    let shutdown_service = service.clone();
    let mut shutdown = tokio::spawn(async move {
        shutdown_service
            .shutdown_until(tokio::time::Instant::now() + Duration::from_millis(10))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), cancelled.notified())
        .await
        .expect("owned task should observe service cancellation");
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut shutdown)
            .await
            .is_err(),
        "deadline expiry must not abort an owned cleanup future"
    );

    release_cleanup.notify_one();
    let report = tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("shutdown should finish after cleanup is released")
        .expect("shutdown task should join");
    assert!(
        report
            .overruns
            .iter()
            .any(|overrun| overrun.contains("graceful collection deadline exceeded")),
        "a missed graceful deadline is an overrun, not a containment failure"
    );
    assert!(
        report.shutdown_failures.is_empty(),
        "cleanup that stayed contained must not be reported as a failure: {:?}",
        report.shutdown_failures
    );
}

/// A caller that stops awaiting `shutdown_until` — which is what
/// `ServiceContainer::shutdown_with_schedule` does at its force deadline —
/// must leave the collection tasks reachable. Taking the `JoinSet` into a
/// plain local instead of `DrainedTasks` would abort them on drop, cutting
/// a collection off mid segment-XML finalize with nothing left to report it.
#[tokio::test]
async fn dropping_the_drain_returns_collections_instead_of_aborting_them() {
    let service = Arc::new(DanmuService::new());
    let started = Arc::new(tokio::sync::Notify::new());
    let task_started = started.clone();
    service.collection_tasks.lock().spawn(async move {
        task_started.notify_one();
        std::future::pending::<()>().await;
        unreachable!("the wedged collection never completes on its own")
    });

    let shutdown_service = service.clone();
    // Boxed rather than `tokio::pin!` so the drop below drops the future
    // itself, matching `ServiceContainer::shutdown_with_schedule`.
    let mut shutdown = Box::pin(shutdown_service.shutdown_until(tokio::time::Instant::now()));
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("collection task should start");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
            .await
            .is_err(),
        "a collection that ignores cancellation must keep the drain pending"
    );
    drop(shutdown);

    assert_eq!(
        service.collection_tasks.lock().len(),
        1,
        "the dropped drain must hand its JoinSet back to the service"
    );

    let aborted = service
        .abort_collections(tokio::time::Instant::now() + Duration::from_secs(5))
        .await;
    assert!(aborted.is_empty(), "no session was registered: {aborted:?}");
    assert!(
        service.collection_tasks.lock().is_empty(),
        "abort_collections must reap the tasks it aborted"
    );
}

/// `start_collection` for a different session of an already-tracked
/// streamer must abort the prior collector before claiming the
/// streamer-level slot; otherwise the old collector keeps running
/// orphaned alongside the new session's collector.
#[tokio::test]
async fn start_collection_aborts_previous_for_same_streamer() {
    use crate::danmu::test_support::FakeProvider;

    let (_first_items_tx, first_items_rx) = mpsc::channel(8);
    let (_second_items_tx, second_items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![
        first_items_rx,
        second_items_rx,
    ])));
    let service = DanmuService::with_providers(registry);
    let streamer_id = "streamer-1";
    let old_session = "session-old";
    let new_session = "session-new";

    service
        .start_collection(collection_spec(old_session, streamer_id, FakeProvider::URL))
        .await
        .expect("first collection starts");

    assert!(
        service.is_collecting(old_session),
        "fixture: prior collector seeded"
    );
    assert_eq!(
        service.get_session_by_streamer(streamer_id).as_deref(),
        Some(old_session)
    );

    service
        .start_collection(collection_spec(new_session, streamer_id, FakeProvider::URL))
        .await
        .expect("replacement collection starts");

    // The old collection must be gone before the replacement claims the
    // streamer-level slot.
    assert!(
        !service.is_collecting(old_session),
        "prior collector for {old_session} must have been aborted"
    );
    assert!(
        service.is_collecting(new_session),
        "the replacement collector must own the session slot"
    );
    service
        .stop_collection(new_session)
        .await
        .expect("replacement stops");
}

/// A prior collector that already delivered its `CollectionOutcome` has
/// released its `collections` entry, so there is nothing for the new
/// collector to overlap and the errors it reported must not deny the next
/// session a collector. Only the `CollectionStopOutcome::Unconfirmed`
/// timeout paths refuse.
///
/// The errors are still reported: they reach `collection_failures` and come
/// back as `DanmuShutdownReport::runtime_failures`, which never fails a
/// shutdown.
#[tokio::test]
async fn replacement_starts_after_a_prior_collector_terminated_with_errors() {
    use crate::danmu::lifecycle::CollectionExitReason;
    use crate::danmu::test_support::FakeProvider;

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(registry);
    let streamer_id = "streamer-1";
    let old_session = "session-old";
    let new_session = "session-new";
    let cleanup_error = "failed to finalize danmu XML for session-old";

    // Stands in for a collector that finished its cleanup and reported a
    // failed XML flush. Resolving `done_tx` up front is what the real task
    // does after `remove_collection`, so the stop's outcome wait returns
    // immediately with the errors attached. The owned task separately
    // returns the same failure through its JoinSet report; that report is
    // the single path that records the failure for shutdown diagnostics.
    let (done_tx, _cancel_token) = seed_active_collection(&service, streamer_id, old_session);
    done_tx
        .send(CollectionOutcome {
            statistics: DanmuStatistics::default(),
            reason: CollectionExitReason::Failed,
            error: None,
            cleanup_errors: vec![Error::other(cleanup_error.to_string())],
        })
        .expect("the seeded collector should deliver its outcome");
    service.collection_tasks.lock().spawn(async move {
        CollectionTaskReport {
            session_id: old_session.to_string(),
            runtime_error: None,
            cleanup_errors: vec![cleanup_error.to_string()],
        }
    });

    service
        .start_collection(collection_spec(new_session, streamer_id, FakeProvider::URL))
        .await
        .expect("a terminated prior collector must not block the replacement");

    assert!(
        service.is_collecting(new_session),
        "the replacement collector must own the session slot"
    );
    assert_eq!(
        service.get_session_by_streamer(streamer_id).as_deref(),
        Some(new_session),
        "the replacement must claim the streamer-level slot"
    );

    let report = service
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    let matching_failures = report
        .runtime_failures
        .iter()
        .filter(|failure| failure.contains(cleanup_error))
        .count();
    assert_eq!(
        matching_failures, 1,
        "the prior collector's error must be reported exactly once: {:?}",
        report.runtime_failures
    );
    assert!(
        report.shutdown_failures.is_empty(),
        "and must not fail the shutdown: {:?}",
        report.shutdown_failures
    );
}

/// The guarantee `PipelineCoordinator` depends on: a collector that ends on a
/// failure rather than a stop request must still emit `CollectionStopped`.
///
/// `apply_event`'s `DanmuCollectionStopped` arm is the only writer of
/// `danmu_complete`, and `is_ready` will not emit `CreateSessionCompleteDag`
/// while a session has `danmu_expected && danmu_observed` but not
/// `danmu_complete` — so withholding the event after a failure strands the
/// session-complete pipeline for the rest of the process's life.
#[tokio::test(start_paused = true)]
async fn collection_stopped_is_emitted_when_the_runner_fails() {
    use crate::danmu::test_support::{FakeProvider, temp_xml_path};

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(registry);
    let mut events = service.subscribe();

    let handle = service
        .start_collection(collection_spec(
            "session-1",
            "streamer-1",
            FakeProvider::URL,
        ))
        .await
        .expect("collection starts against the fake provider");
    assert_eq!(handle.session_id(), "session-1");

    // A segment path whose parent is a regular file cannot be created, so the
    // runner's loop ends on an error instead of a stop request.
    let blocker = temp_xml_path("service-loop-error");
    tokio::fs::write(&blocker, b"not a directory")
        .await
        .expect("write blocker file");
    handle
        .start_segment(
            "0",
            blocker.join("child").join("seg.xml"),
            jiff::Timestamp::now(),
        )
        .await
        .expect("command is accepted; the failure happens in the runner");

    let stopped = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match events.recv().await {
                Ok(DanmuEvent::CollectionStopped { session_id, .. }) => return session_id,
                Ok(_) => continue,
                Err(error) => panic!("danmu event stream ended early: {error}"),
            }
        }
    })
    .await
    .expect("CollectionStopped must follow a runner failure");
    let _ = tokio::fs::remove_file(&blocker).await;

    assert_eq!(stopped, "session-1");
    assert!(
        !service.is_collecting("session-1"),
        "the collection slot must be released"
    );
    assert!(
        service.get_session_by_streamer("streamer-1").is_none(),
        "the streamer reverse index must be released"
    );
}

/// The last line of defence for the session-complete gate: if the collector
/// task unwinds, `CollectionCleanupGuard::drop` must still release the session
/// and emit `CollectionStopped`.
///
/// Without it a panic inside `CollectionRunner::run` leaves the `collections`
/// entry behind — so `is_collecting` keeps reporting a live collector, which
/// the health probe trusts — and withholds the only event that sets the
/// coordinator's `danmu_complete`.
#[tokio::test]
async fn cleanup_guard_releases_the_session_when_the_task_unwinds() {
    let service = DanmuService::new();
    let mut events = service.subscribe();
    let (_done_tx, _) = seed_active_collection(&service, "streamer-1", "session-1");
    assert!(service.is_collecting("session-1"));

    // Dropping without disarming is what an unwind through the task body does.
    {
        let _guard = CollectionCleanupGuard {
            collections: service.collections.clone(),
            sessions_by_streamer: service.sessions_by_streamer.clone(),
            events: service.events.clone(),
            session_id: "session-1".to_string(),
            started: true,
            armed: true,
        };
    }

    assert!(
        !service.is_collecting("session-1"),
        "the collection slot must be released"
    );
    assert!(
        service.get_session_by_streamer("streamer-1").is_none(),
        "the streamer reverse index must be released"
    );
    assert!(
        matches!(
            events.try_recv(),
            Ok(DanmuEvent::CollectionStopped { session_id, .. }) if session_id == "session-1"
        ),
        "the coordinator's danmu arm must still be released"
    );
}

#[tokio::test]
async fn concurrent_starts_reserve_a_session_exactly_once() {
    use crate::danmu::test_support::FakeProvider;

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = Arc::new(DanmuService::with_providers(registry));

    let first = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .start_collection(collection_spec(
                    "session-1",
                    "streamer-1",
                    FakeProvider::URL,
                ))
                .await
        })
    };
    let second = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .start_collection(collection_spec(
                    "session-1",
                    "streamer-1",
                    FakeProvider::URL,
                ))
                .await
        })
    };

    let first = first.await.expect("first start task should join");
    let second = second.await.expect("second start task should join");
    assert_ne!(first.is_ok(), second.is_ok());
    assert_eq!(service.active_sessions(), vec!["session-1".to_string()]);
    assert!(service.start_serialization.is_empty());

    service
        .shutdown()
        .await
        .expect("the winning collection should stop cleanly");
}

#[tokio::test]
async fn cancelled_setup_waiter_cannot_split_the_next_start_lock() {
    let service = DanmuService::new();
    let first = service.start_lock("streamer-1");
    let first_guard = first.mutex.lock().await;
    let mut cancelled = Box::pin(async {
        let waiter = service.start_lock("streamer-1");
        let _guard = waiter.mutex.lock().await;
    });
    assert!(futures::poll!(cancelled.as_mut()).is_pending());
    drop(cancelled);

    let next = service.start_lock("streamer-1");
    assert!(Arc::ptr_eq(&first.mutex, &next.mutex));
    let mut acquire = Box::pin(next.mutex.lock());
    assert!(futures::poll!(acquire.as_mut()).is_pending());
    drop(first_guard);
    let next_guard = acquire.await;
    drop(first);

    let arriving = service.start_lock("streamer-1");
    assert!(Arc::ptr_eq(&next.mutex, &arriving.mutex));
    assert!(arriving.mutex.try_lock().is_err());
    drop(arriving);
    drop(next_guard);
    drop(next);
    assert!(service.start_serialization.is_empty());
}

#[tokio::test]
async fn concurrent_final_setup_owners_release_before_registry_cleanup() {
    let service = DanmuService::new();
    let first = service.start_lock("streamer-1");
    let second = service.start_lock("streamer-1");
    let entry = service.start_serialization.get("streamer-1").unwrap();
    let start = std::sync::Barrier::new(3);

    std::thread::scope(|scope| {
        let start = &start;
        scope.spawn(move || {
            start.wait();
            drop(first);
        });
        scope.spawn(move || {
            start.wait();
            drop(second);
        });
        start.wait();
        // Keep both registration cleanups blocked on this shard. Their
        // strong owners must already be gone, so neither can miss the
        // final cleanup by observing the other's not-yet-dropped Arc.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(entry.value()) != 1 && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        let owners = Arc::strong_count(entry.value());
        drop(entry);
        assert_eq!(owners, 1);
    });
    assert!(service.start_serialization.is_empty());
}

#[tokio::test]
async fn normal_stop_does_not_cancel_the_collection_token() {
    use crate::danmu::test_support::FakeProvider;

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(registry);
    let mut events = service.subscribe();
    service
        .start_collection(collection_spec(
            "session-1",
            "streamer-1",
            FakeProvider::URL,
        ))
        .await
        .expect("collection starts");
    let cancel_token = service
        .collections
        .get("session-1")
        .expect("active collection")
        .cancel_token
        .clone();

    service
        .stop_collection("session-1")
        .await
        .expect("collection stops");

    assert!(
        !cancel_token.is_cancelled(),
        "a normal session stop must not be treated as process shutdown"
    );
    let stopped_events = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| matches!(event, DanmuEvent::CollectionStopped { .. }))
        .count();
    assert_eq!(
        stopped_events, 1,
        "normal completion must have one terminal event owner"
    );
}

#[tokio::test]
async fn disabled_statistics_are_not_persisted_on_stop() {
    use srec_sqlite::models::{LiveSessionDbModel, StreamerDbModel};
    use srec_sqlite::repositories::{
        SessionRepository as _, SqlxSessionRepository, SqlxStreamerRepository,
        StreamerRepository as _,
    };
    use srec_sqlite::{init_pool_with_size, run_migrations};

    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();

    let mut streamer = StreamerDbModel::new(
        "Streamer One",
        "https://example.com/streamer-1",
        "platform-twitch",
    );
    streamer.id = "streamer-1".to_string();
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&streamer)
        .await
        .unwrap();

    let mut session = LiveSessionDbModel::new("streamer-1");
    session.id = "session-1".to_string();
    let repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool));
    repo.create_session(&session).await.unwrap();

    use crate::danmu::test_support::FakeProvider;

    let (_items_tx, items_rx) = mpsc::channel(8);
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(FakeProvider::new(vec![items_rx])));
    let service = DanmuService::with_providers(registry).with_session_repository(repo.clone());
    let mut spec = collection_spec("session-1", "streamer-1", FakeProvider::URL);
    spec.statistics.enabled = false;
    service
        .start_collection(spec)
        .await
        .expect("collection starts");

    service
        .stop_collection("session-1")
        .await
        .expect("collection stops");

    assert!(
        repo.get_danmu_statistics("session-1")
            .await
            .unwrap()
            .is_none(),
        "disabled collection must not create an empty statistics row"
    );
}
