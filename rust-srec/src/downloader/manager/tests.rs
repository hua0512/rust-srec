use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;

use crate::downloader::SegmentInfo;
use crate::downloader::engine::{EngineStartError, IoErrorKindSer, SegmentEvent};

use super::coordination::DownloadCoordinationReceipt;
use super::*;

#[derive(Clone)]
struct ScriptedSegmentEngine {
    prelude: Vec<SegmentEvent>,
    /// Optional barrier: when set, the engine emits `prelude`, then awaits
    /// this notify before emitting `tail` + `DownloadCompleted`. Lets a
    /// test interleave `stop_download` between the prelude and the tail
    /// to reproduce the trailing-flush race.
    release_tail: Option<Arc<tokio::sync::Notify>>,
    release_tail_on_cancel: bool,
    tail: Vec<SegmentEvent>,
}

impl ScriptedSegmentEngine {
    fn new(events: Vec<SegmentEvent>) -> Self {
        Self {
            prelude: events,
            release_tail: None,
            release_tail_on_cancel: false,
            tail: Vec::new(),
        }
    }

    fn with_gated_tail(
        prelude: Vec<SegmentEvent>,
        release: Arc<tokio::sync::Notify>,
        tail: Vec<SegmentEvent>,
    ) -> Self {
        Self {
            prelude,
            release_tail: Some(release),
            release_tail_on_cancel: false,
            tail,
        }
    }

    fn with_shutdown_tail(prelude: Vec<SegmentEvent>, tail: Vec<SegmentEvent>) -> Self {
        Self {
            prelude,
            release_tail: None,
            release_tail_on_cancel: true,
            tail,
        }
    }
}

#[async_trait]
impl DownloadEngine for ScriptedSegmentEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Ffmpeg
    }

    async fn run(&self, handle: Arc<DownloadHandle>) -> std::result::Result<(), EngineStartError> {
        for event in self.prelude.clone() {
            handle
                .event_tx
                .send(event)
                .await
                .map_err(|e| EngineStartError {
                    kind: DownloadFailureKind::Other,
                    message: format!("failed to emit scripted segment event: {}", e),
                })?;
        }
        if let Some(release) = self.release_tail.as_ref() {
            release.notified().await;
        } else if self.release_tail_on_cancel {
            handle.cancellation_token.cancelled().await;
        }
        for event in self.tail.clone() {
            handle
                .event_tx
                .send(event)
                .await
                .map_err(|e| EngineStartError {
                    kind: DownloadFailureKind::Other,
                    message: format!("failed to emit scripted tail event: {}", e),
                })?;
        }
        handle
            .event_tx
            .send(SegmentEvent::DownloadCompleted {
                total_bytes: 0,
                total_duration_secs: 0.0,
                total_segments: 0,
                engine_signal: EngineEndSignal::Unknown,
            })
            .await
            .map_err(|e| EngineStartError {
                kind: DownloadFailureKind::Other,
                message: format!("failed to emit scripted completion event: {}", e),
            })?;
        Ok(())
    }

    fn is_available(&self) -> bool {
        true
    }

    fn version(&self) -> Option<String> {
        Some("scripted".to_string())
    }
}

fn completed_segment(path: std::path::PathBuf, index: u32) -> SegmentEvent {
    SegmentEvent::SegmentCompleted(SegmentInfo {
        path,
        duration_secs: 1.0,
        size_bytes: 128,
        index,
        started_at: None,
        completed_at: Utc::now(),
        split_reason_code: None,
        split_reason_details_json: None,
    })
}

fn test_download_config(output_dir: std::path::PathBuf, session_id: &str) -> DownloadConfig {
    DownloadConfig::new(
        "https://example.com/test.flv",
        output_dir,
        "test-streamer-id",
        "TestStreamer",
        session_id,
    )
}

async fn start_scripted_download(
    manager: &DownloadManager,
    config: DownloadConfig,
    events: Vec<SegmentEvent>,
) -> Result<String> {
    start_scripted_download_with_engine(manager, config, ScriptedSegmentEngine::new(events)).await
}

async fn start_scripted_download_with_engine(
    manager: &DownloadManager,
    config: DownloadConfig,
    scripted: ScriptedSegmentEngine,
) -> Result<String> {
    let engine = EngineHandle {
        engine: Arc::new(scripted),
        engine_type: EngineType::Ffmpeg,
        engine_key: EngineKey::global(EngineType::Ffmpeg),
    };
    let slot = manager
        .acquire_slot(
            AcquireRequest {
                session_id: config.session_id.clone(),
                streamer_id: config.streamer_id.clone(),
                streamer_name: config.streamer_name.clone(),
                engine_type: EngineType::Ffmpeg,
                priority: Priority::Normal,
            },
            CancellationToken::new(),
        )
        .await?;

    manager.start_with_slot(slot, config, engine).await
}

async fn collect_segment_completed(
    events: &mut broadcast::Receiver<DownloadManagerEvent>,
) -> DownloadProgressEvent {
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for download event")
            .expect("download event channel closed");
        if let DownloadManagerEvent::Progress(
            progress @ DownloadProgressEvent::SegmentCompleted { .. },
        ) = event
        {
            return progress;
        }
    }
}

async fn wait_for_download_terminal(
    events: &mut broadcast::Receiver<DownloadManagerEvent>,
) -> DownloadTerminalEvent {
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for terminal download event")
            .expect("download event channel closed");
        if let DownloadManagerEvent::Terminal(terminal) = event {
            return terminal;
        }
    }
}

async fn collect_segment_progress_events(
    events: &mut broadcast::Receiver<DownloadManagerEvent>,
    count: usize,
) -> Vec<DownloadProgressEvent> {
    let mut collected = Vec::new();
    while collected.len() < count {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for download event")
            .expect("download event channel closed");
        if let DownloadManagerEvent::Progress(
            progress @ (DownloadProgressEvent::SegmentStarted { .. }
            | DownloadProgressEvent::SegmentCompleted { .. }),
        ) = event
        {
            collected.push(progress);
        }
    }
    collected
}

#[test]
fn test_download_manager_creation() {
    let manager = DownloadManager::new();
    assert_eq!(manager.active_count(), 0);
    assert!(!manager.available_engines().is_empty());
}

#[tokio::test]
async fn download_info_views_project_consistent_live_metadata() {
    let manager = DownloadManager::new();
    let directory = tempfile::tempdir().unwrap();
    let mut config = test_download_config(directory.path().to_path_buf(), "projection-session");
    config
        .headers
        .push(("Authorization".to_owned(), "private-header".repeat(1024)));
    config.engines_override = Some(serde_json::json!({"nested": ["private-config".repeat(1024)]}));
    let id = start_scripted_download_with_engine(
        &manager,
        config,
        ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
    )
    .await
    .unwrap();
    let initial = manager.get_active_downloads().pop().unwrap();
    assert_eq!(initial.id, id);
    assert_eq!(initial.session_id, "projection-session");
    let by_streamer = manager
        .get_download_by_streamer(&initial.streamer_id)
        .unwrap();
    assert_eq!(by_streamer.url, initial.url);
    manager
        .active_downloads
        .get(&id)
        .unwrap()
        .handle
        .update_config(|config| {
            config.url = "https://invalid.test/refreshed".to_owned();
        });
    assert_eq!(
        manager
            .get_download_by_streamer(&initial.streamer_id)
            .unwrap()
            .url,
        "https://invalid.test/refreshed"
    );
    assert_ne!(
        initial.url, "https://invalid.test/refreshed",
        "earlier snapshots own their projected fields"
    );
    tokio::time::timeout(Duration::from_secs(2), manager.stop_download(&id))
        .await
        .unwrap()
        .unwrap();
    assert!(manager.get_active_downloads().is_empty());
    assert!(
        manager
            .get_download_by_streamer(&initial.streamer_id)
            .is_none()
    );
}

#[tokio::test]
async fn maintenance_admission_fences_preacquired_recording_starts() {
    let manager = DownloadManager::new();
    let temp = tempfile::tempdir().unwrap();
    let config = test_download_config(temp.path().to_path_buf(), "maintenance-fence");
    let slot = manager
        .acquire_slot(
            AcquireRequest {
                session_id: config.session_id.clone(),
                streamer_id: config.streamer_id.clone(),
                streamer_name: config.streamer_name.clone(),
                engine_type: EngineType::Ffmpeg,
                priority: Priority::Normal,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let admission = manager
        .try_admit_maintenance(0)
        .expect("idle maintenance admitted");
    let engine = EngineHandle {
        engine: Arc::new(ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![])),
        engine_type: EngineType::Ffmpeg,
        engine_key: EngineKey::global(EngineType::Ffmpeg),
    };
    let mut start = Box::pin(manager.start_with_slot(slot, config, engine));
    assert!(
        futures::poll!(start.as_mut()).is_pending(),
        "even a preacquired slot cannot bypass maintenance"
    );
    assert_eq!(manager.active_count(), 0);
    assert!(manager.try_admit_maintenance(0).is_none());
    drop(admission);
    let id = tokio::time::timeout(Duration::from_secs(2), start)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(manager.active_count(), 1);
    assert!(manager.try_admit_maintenance(0).is_none());
    let admission = manager
        .try_admit_maintenance(1)
        .expect("configured active threshold is honored");
    // Completion does not acquire the admission lock: it must remain able
    // to settle an already running recording while maintenance is admitted.
    tokio::time::timeout(Duration::from_secs(2), manager.stop_download(&id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(manager.active_count(), 0);
    drop(admission);
    assert!(manager.try_admit_maintenance(0).is_some());
    let operation = manager.begin_operation().await.unwrap();
    assert!(
        manager.try_admit_maintenance(0).is_none(),
        "in-flight admission work conservatively defers maintenance"
    );
    drop(operation);
}

#[tokio::test]
async fn maintenance_admission_respects_shutdown_and_cancelled_starts() {
    let manager = DownloadManager::new();
    let admission = manager.try_admit_maintenance(0).unwrap();
    let mut operation = Box::pin(manager.begin_operation());
    assert!(futures::poll!(operation.as_mut()).is_pending());
    drop(operation);
    drop(admission);
    assert!(
        manager.try_admit_maintenance(0).is_some(),
        "cancelled waiter does not retain the fence"
    );
    let report = tokio::time::timeout(
        Duration::from_secs(2),
        manager.shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1)),
    )
    .await
    .unwrap();
    assert!(report.failures.is_empty());
    assert!(
        manager.try_admit_maintenance(0).is_none(),
        "maintenance cannot race shutdown admission"
    );
}

#[tokio::test]
async fn cancelled_queued_start_clears_event_and_capacity_while_maintenance_is_admitted() {
    enum CancelMode {
        DropFuture,
        BeforeStart,
        DuringWait,
    }
    for mode in [
        CancelMode::DropFuture,
        CancelMode::BeforeStart,
        CancelMode::DuringWait,
    ] {
        let manager = DownloadManager::with_config(DownloadManagerConfig {
            max_concurrent_downloads: 1,
            high_priority_extra_slots: 0,
            ..Default::default()
        });
        let mut events = manager.subscribe();
        let request = |session: &str| AcquireRequest {
            session_id: session.to_owned(),
            streamer_id: "streamer".to_owned(),
            streamer_name: "Streamer".to_owned(),
            engine_type: EngineType::Ffmpeg,
            priority: Priority::Normal,
        };
        let holder = manager
            .acquire_slot(request("holder"), CancellationToken::new())
            .await
            .unwrap();
        let mut queued =
            Box::pin(manager.acquire_slot(request("queued"), CancellationToken::new()));
        assert!(futures::poll!(queued.as_mut()).is_pending());
        drop(holder);
        let slot = tokio::time::timeout(Duration::from_secs(2), queued)
            .await
            .unwrap()
            .unwrap();
        assert!(slot.queued_event_emitted());
        let admission = manager.try_admit_maintenance(0).unwrap();
        let config = DownloadConfig::new(
            "https://invalid.test/live",
            "recordings",
            "streamer",
            "Streamer",
            "queued",
        );
        let engine = EngineHandle {
            engine: Arc::new(ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![])),
            engine_type: EngineType::Ffmpeg,
            engine_key: EngineKey::global(EngineType::Ffmpeg),
        };
        let cancel = CancellationToken::new();
        if matches!(mode, CancelMode::BeforeStart) {
            cancel.cancel();
        }
        let mut start =
            Box::pin(manager.start_with_slot_cancellable(slot, config, engine, &cancel));
        if !matches!(mode, CancelMode::BeforeStart) {
            assert!(futures::poll!(start.as_mut()).is_pending());
        }
        match mode {
            CancelMode::DropFuture => drop(start),
            CancelMode::BeforeStart | CancelMode::DuringWait => {
                cancel.cancel();
                assert!(
                    tokio::time::timeout(Duration::from_secs(2), start)
                        .await
                        .unwrap()
                        .unwrap()
                        .is_none()
                );
            }
        }
        let mut queued_events = 0;
        let mut dequeued_events = 0;
        while let Ok(event) = events.try_recv() {
            match event {
                DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadQueued {
                    session_id,
                    ..
                }) if session_id == "queued" => queued_events += 1,
                DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadDequeued {
                    session_id,
                    ..
                }) if session_id == "queued" => dequeued_events += 1,
                DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                    ..
                }) => panic!("cancelled start must not launch an engine"),
                _ => {}
            }
        }
        assert_eq!(queued_events, 1);
        assert_eq!(dequeued_events, 1);
        drop(admission);
        let replacement = tokio::time::timeout(
            Duration::from_secs(2),
            manager.acquire_slot(request("queued"), CancellationToken::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            !replacement.queued_event_emitted(),
            "cancelled start releases capacity and its session reservation"
        );
        assert_eq!(
            manager.active_count(),
            0,
            "cancelled start stays stopped after maintenance releases admission"
        );
    }
}

#[test]
fn relative_segment_paths_are_resolved_without_canonicalizing() {
    let relative = std::path::Path::new("recordings").join("segment.flv");
    let expected = std::env::current_dir()
        .expect("current directory")
        .join(&relative);

    assert_eq!(resolve_segment_path(&relative), expected.to_string_lossy());
}

/// `emit_rejected` must reach subscribers exactly like a
/// preflight-emitted rejection: the session lifecycle relies on the
/// `Rejected` terminal to close the session and the scheduler relies on
/// it to reschedule the streamer actor after `retry_after_secs`.
#[tokio::test]
async fn emit_rejected_broadcasts_terminal_event() {
    let manager = DownloadManager::new();
    let mut events = manager.subscribe();

    manager
        .emit_rejected(
            "streamer-1".to_string(),
            "Streamer One".to_string(),
            "session-1".to_string(),
            "streamer temporarily disabled (error backoff)".to_string(),
            Some(62),
            DownloadRejectedKind::StreamerBackoff,
        )
        .await
        .expect("rejection should publish");

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .expect("event should arrive")
        .expect("channel open");

    let DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
        streamer_id,
        session_id,
        retry_after_secs,
        kind,
        ..
    }) = event
    else {
        panic!("expected Rejected terminal event, got {event:?}");
    };
    assert_eq!(streamer_id, "streamer-1");
    assert_eq!(session_id, "session-1");
    assert_eq!(retry_after_secs, Some(62));
    assert!(matches!(kind, DownloadRejectedKind::StreamerBackoff));
}

#[tokio::test]
async fn repeated_engine_local_indices_use_session_scoped_indices() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manager = DownloadManager::new();
    let mut events = manager.subscribe();
    let session_id = "session-reused-local-index";

    let first_path = temp.path().join("segment-a.flv");
    start_scripted_download(
        &manager,
        test_download_config(temp.path().to_path_buf(), session_id),
        vec![completed_segment(first_path, 0)],
    )
    .await
    .expect("first download should start");
    let first = collect_segment_completed(&mut events).await;
    wait_for_download_terminal(&mut events).await;

    let second_path = temp.path().join("segment-b.flv");
    start_scripted_download(
        &manager,
        test_download_config(temp.path().to_path_buf(), session_id),
        vec![completed_segment(second_path, 0)],
    )
    .await
    .expect("second download should start");
    let second = collect_segment_completed(&mut events).await;
    wait_for_download_terminal(&mut events).await;

    let DownloadProgressEvent::SegmentCompleted {
        segment_index: first_index,
        ..
    } = first
    else {
        panic!("expected first completed segment event");
    };
    let DownloadProgressEvent::SegmentCompleted {
        segment_index: second_index,
        ..
    } = second
    else {
        panic!("expected second completed segment event");
    };

    assert_eq!(first_index, 0);
    assert_eq!(second_index, 1);
}

#[tokio::test]
async fn segment_started_and_completed_share_session_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manager = DownloadManager::new();
    let mut events = manager.subscribe();
    let segment_path = temp.path().join("segment.flv");

    start_scripted_download(
        &manager,
        test_download_config(temp.path().to_path_buf(), "session-start-complete"),
        vec![
            SegmentEvent::SegmentStarted {
                path: segment_path.clone(),
                sequence: 0,
                started_at: Utc::now(),
            },
            completed_segment(segment_path, 0),
        ],
    )
    .await
    .expect("download should start");

    let segment_events = collect_segment_progress_events(&mut events, 2).await;
    let DownloadProgressEvent::SegmentStarted {
        segment_index: started_index,
        ..
    } = &segment_events[0]
    else {
        panic!("expected segment started event first");
    };
    let DownloadProgressEvent::SegmentCompleted {
        segment_index: completed_index,
        ..
    } = &segment_events[1]
    else {
        panic!("expected segment completed event second");
    };

    assert_eq!(*started_index, 0);
    assert_eq!(completed_index, started_index);
    wait_for_download_terminal(&mut events).await;
}

#[tokio::test]
async fn segment_completion_preserves_engine_reported_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let nested = temp.path().join("nested");
    tokio::fs::create_dir_all(&nested)
        .await
        .expect("create nested directory");
    tokio::fs::write(temp.path().join("segment.flv"), b"segment")
        .await
        .expect("create segment");

    let manager = DownloadManager::new();
    let mut events = manager.subscribe();
    let segment_path = nested.join("..").join("segment.flv");
    let expected_path = segment_path.to_string_lossy().to_string();

    start_scripted_download(
        &manager,
        test_download_config(temp.path().to_path_buf(), "session-path-identity"),
        vec![
            SegmentEvent::SegmentStarted {
                path: segment_path.clone(),
                sequence: 0,
                started_at: Utc::now(),
            },
            completed_segment(segment_path, 0),
        ],
    )
    .await
    .expect("download should start");

    let segment_events = collect_segment_progress_events(&mut events, 2).await;
    let DownloadProgressEvent::SegmentStarted {
        segment_path: started_path,
        ..
    } = &segment_events[0]
    else {
        panic!("expected segment started event first");
    };
    let DownloadProgressEvent::SegmentCompleted {
        segment_path: completed_path,
        ..
    } = &segment_events[1]
    else {
        panic!("expected segment completed event second");
    };

    assert_eq!(started_path, &expected_path);
    assert_eq!(completed_path, &expected_path);

    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for terminal event")
            .expect("download event channel closed");
        if let DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
            file_path, ..
        }) = event
        {
            assert_eq!(file_path.as_deref(), Some(expected_path.as_str()));
            break;
        }
    }
}

/// Trailing `SegmentCompleted` flushed after `stop_download_with_reason`
/// must reuse the session_segment_index that the matching
/// `SegmentStarted` allocated. The mapping is the spawn loop's local
/// `engine_to_session` `HashMap`, which is alive as long as the loop is
/// draining the engine's channel and is unaffected by
/// `active_downloads.remove(download_id)`.
#[tokio::test]
async fn segment_completed_after_stop_keeps_session_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manager = DownloadManager::new();
    let mut events = manager.subscribe();
    let segment_path = temp.path().join("segment.flv");
    let release_tail = Arc::new(tokio::sync::Notify::new());

    let scripted = ScriptedSegmentEngine::with_gated_tail(
        vec![SegmentEvent::SegmentStarted {
            path: segment_path.clone(),
            sequence: 0,
            started_at: Utc::now(),
        }],
        release_tail.clone(),
        vec![SegmentEvent::SegmentCompleted(SegmentInfo {
            path: segment_path,
            duration_secs: 1.0,
            size_bytes: 128,
            index: 0,
            started_at: None,
            completed_at: Utc::now(),
            split_reason_code: None,
            split_reason_details_json: None,
        })],
    );

    let download_id = start_scripted_download_with_engine(
        &manager,
        test_download_config(temp.path().to_path_buf(), "session-trailing-flush"),
        scripted,
    )
    .await
    .expect("download should start");

    // Drain the SegmentStarted so we know its session_index.
    let started_index = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for segment started")
            .expect("download event channel closed");
        if let DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
            segment_index,
            ..
        }) = event
        {
            break segment_index;
        }
    };

    // Stop must stay pending while the engine is still flushing its tail.
    let mut stop = Box::pin(manager.stop_download(&download_id));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), stop.as_mut())
            .await
            .is_err(),
        "stop returned before the engine flushed its trailing segment"
    );

    // Release the trailing SegmentCompleted.
    release_tail.notify_one();
    stop.await
        .expect("stop should finish after the tail drains");

    // The trailing SegmentCompleted must report the same session_index
    // as the SegmentStarted.
    let completed = collect_segment_completed(&mut events).await;
    let DownloadProgressEvent::SegmentCompleted {
        segment_index: completed_index,
        ..
    } = completed
    else {
        panic!("expected segment completed event");
    };
    assert_eq!(completed_index, started_index);

    let terminal = wait_for_download_terminal(&mut events).await;
    assert!(matches!(
        terminal,
        DownloadTerminalEvent::Completed {
            stop_cause: Some(DownloadStopCause::User),
            ..
        }
    ));
}

#[tokio::test]
async fn shutdown_waits_for_trailing_segment_and_emits_one_terminal_outcome() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (coordination_tx, mut coordination_rx) = download_coordination_channel();
    let manager = DownloadManager::new().with_coordination_sender(coordination_tx);
    let mut events = manager.subscribe();
    let segment_path = temp.path().join("shutdown-tail.flv");
    let completion_received = Arc::new(tokio::sync::Notify::new());
    let release_completion = Arc::new(tokio::sync::Notify::new());
    let coordination_completion_received = completion_received.clone();
    let coordination_release_completion = release_completion.clone();
    let coordination_task = tokio::spawn(async move {
        while let Some(delivery) = coordination_rx
            .recv()
            .await
            .expect("coordination channel should close with a marker")
        {
            let (event, acknowledgement) = delivery.into_parts();
            if matches!(
                event,
                DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted { .. })
            ) {
                coordination_completion_received.notify_one();
                coordination_release_completion.notified().await;
            }
            acknowledgement
                .send(Ok(()))
                .expect("attempt should await its acknowledgement");
        }
    });

    let scripted = ScriptedSegmentEngine::with_shutdown_tail(
        vec![SegmentEvent::SegmentStarted {
            path: segment_path.clone(),
            sequence: 0,
            started_at: Utc::now(),
        }],
        vec![completed_segment(segment_path, 0)],
    );

    start_scripted_download_with_engine(
        &manager,
        test_download_config(temp.path().to_path_buf(), "session-shutdown-tail"),
        scripted,
    )
    .await
    .expect("download should start");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for segment start")
            .expect("download event channel closed");
        if matches!(
            event,
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted { .. })
        ) {
            break;
        }
    }

    let shutdown = manager.shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1));
    tokio::pin!(shutdown);
    tokio::select! {
        () = completion_received.notified() => {}
        result = &mut shutdown => panic!(
            "shutdown returned before SegmentCompleted acknowledgement: {result:?}"
        ),
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), shutdown.as_mut())
            .await
            .is_err(),
        "shutdown must remain pending until SegmentCompleted is durably applied"
    );
    release_completion.notify_one();
    shutdown.await;
    coordination_task
        .await
        .expect("coordination task should finish after the drain marker");

    let mut saw_segment_completed = false;
    let terminal = loop {
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for shutdown events")
            .expect("download event channel closed");
        match event {
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted { .. }) => {
                saw_segment_completed = true
            }
            DownloadManagerEvent::Terminal(terminal) => break terminal,
            _ => {}
        }
    };

    assert!(
        saw_segment_completed,
        "the final segment must be published before the terminal outcome"
    );
    assert!(matches!(
        terminal,
        DownloadTerminalEvent::Cancelled {
            cause: DownloadStopCause::Shutdown,
            ..
        }
    ));
    let mut additional_terminals = 0;
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(20), events.recv()).await {
        if matches!(event, DownloadManagerEvent::Terminal(_)) {
            additional_terminals += 1;
        }
    }
    assert_eq!(additional_terminals, 0, "terminal outcome must be unique");
    assert_eq!(manager.active_count(), 0);
}

#[tokio::test]
async fn shutdown_rejects_queued_start_before_active_slot_is_released() {
    let temp = tempfile::tempdir().expect("temp directory should be created");
    let manager = Arc::new(DownloadManager::with_config(DownloadManagerConfig {
        max_concurrent_downloads: 1,
        high_priority_extra_slots: 0,
        ..Default::default()
    }));
    manager.register_engine(Arc::new(ScriptedSegmentEngine::with_shutdown_tail(
        Vec::new(),
        Vec::new(),
    )));
    let mut events = manager.subscribe();

    manager
        .start_download(
            DownloadConfig::new(
                "https://example.com/active.flv",
                temp.path().to_path_buf(),
                "active-streamer",
                "Active Streamer",
                "active-session",
            ),
            Some("ffmpeg".to_string()),
            false,
        )
        .await
        .expect("first download should occupy the only slot");

    let queued_manager = manager.clone();
    let queued_output = temp.path().to_path_buf();
    let queued = tokio::spawn(async move {
        queued_manager
            .start_download(
                DownloadConfig::new(
                    "https://example.com/queued.flv",
                    queued_output,
                    "queued-streamer",
                    "Queued Streamer",
                    "queued-session",
                ),
                Some("ffmpeg".to_string()),
                false,
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(1), async {
        while manager.queue.pending_count() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second download should park in the queue");

    let report = manager
        .shutdown_until(tokio::time::Instant::now() + Duration::from_secs(1))
        .await;
    let queued_error = queued
        .await
        .expect("queued start task should not panic")
        .expect_err("queued start must be rejected during shutdown");

    assert!(queued_error.to_string().contains("shutting down"));
    assert_eq!(report.stopped_download_ids.len(), 1);
    assert!(report.deadline_exceeded_download_ids.is_empty());
    assert_eq!(manager.active_count(), 0);
    assert_eq!(manager.queue.pending_count(), 0);

    let started_count = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| {
            matches!(
                event,
                DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted { .. })
            )
        })
        .count();
    assert_eq!(started_count, 1, "the queued engine must never start");
}

/// `clear_session_segment_index` can fire mid-drain (the dedicated
/// `SessionTransition::Ended` subscriber in `services/container.rs` runs
/// as soon as `SessionLifecycle` writes `end_time`, independent of the
/// download's spawn loop). The trailing `SegmentCompleted` that follows
/// must still report the same session_index as its matching
/// `SegmentStarted`. The spawn loop's local `engine_to_session` map is
/// what makes this hold — it survives the session-counter eviction.
///
/// The prelude includes a complete first segment (Start+Complete for
/// engine seq=0) so the session counter advances past 0 before the
/// second `SegmentStarted` (seq=1) lands. A naive design that wipes the
/// mapping along with the counter would re-allocate the trailing
/// `SegmentCompleted(1)` as session_index=0 from the recreated counter,
/// not 1 — that's what this test rules out.
#[tokio::test]
async fn segment_completed_after_session_cleanup_keeps_index() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manager = DownloadManager::new();
    let mut events = manager.subscribe();
    let first_path = temp.path().join("segment-0.flv");
    let second_path = temp.path().join("segment-1.flv");
    let release_tail = Arc::new(tokio::sync::Notify::new());
    let session_id = "session-cleanup-mid-drain";

    let scripted = ScriptedSegmentEngine::with_gated_tail(
        vec![
            SegmentEvent::SegmentStarted {
                path: first_path.clone(),
                sequence: 0,
                started_at: Utc::now(),
            },
            completed_segment(first_path, 0),
            SegmentEvent::SegmentStarted {
                path: second_path.clone(),
                sequence: 1,
                started_at: Utc::now(),
            },
        ],
        release_tail.clone(),
        vec![completed_segment(second_path.clone(), 1)],
    );

    start_scripted_download_with_engine(
        &manager,
        test_download_config(temp.path().to_path_buf(), session_id),
        scripted,
    )
    .await
    .expect("download should start");

    // Drain events until we see the SegmentStarted for engine seq=1 —
    // that's the one whose matching SegmentCompleted is gated.
    let started_index = loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for segment started")
            .expect("download event channel closed");
        if let DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
            segment_index,
            segment_path,
            ..
        }) = event
            && segment_path == second_path.to_string_lossy()
        {
            break segment_index;
        }
    };
    // With the counter advanced past 0, the second SegmentStarted's
    // session_index must be > 0; otherwise the trailing assertion below
    // can't distinguish fixed from broken.
    assert!(
        started_index > 0,
        "test setup expects session counter to advance past 0 before the gated tail; got {started_index}"
    );

    // Simulate the `SessionTransition::Ended` subscriber firing while the
    // engine is still parked.
    manager.clear_session_segment_index(session_id);

    // Release the trailing SegmentCompleted.
    release_tail.notify_one();

    let completed = collect_segment_completed(&mut events).await;
    let DownloadProgressEvent::SegmentCompleted {
        segment_index: completed_index,
        ..
    } = completed
    else {
        panic!("expected segment completed event");
    };
    assert_eq!(completed_index, started_index);
}

/// `shutdown_until` contains attempts past its deadline, so an engine that
/// ignores `DownloadHandle::cancel` keeps it pending forever. A caller that
/// stops awaiting it must still be able to reclaim those attempts:
/// `abort_attempts` drops each attempt future, which is what kills the
/// engine child owned through `kill_on_drop`.
#[tokio::test]
async fn abort_attempts_reclaims_attempts_left_by_a_dropped_shutdown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let manager = DownloadManager::new();
    let mut events = manager.subscribe();
    let segment_path = temp.path().join("segment-0.flv");
    // Never notified, so the engine parks past the shutdown request.
    let wedged = Arc::new(tokio::sync::Notify::new());

    let scripted = ScriptedSegmentEngine::with_gated_tail(
        vec![SegmentEvent::SegmentStarted {
            path: segment_path.clone(),
            sequence: 0,
            started_at: Utc::now(),
        }],
        wedged,
        Vec::new(),
    );
    let download_id = start_scripted_download_with_engine(
        &manager,
        test_download_config(temp.path().to_path_buf(), "session-abort-attempts"),
        scripted,
    )
    .await
    .expect("download should start");

    loop {
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("timed out waiting for segment started")
            .expect("download event channel closed");
        if matches!(
            event,
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted { .. })
        ) {
            break;
        }
    }

    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            manager.shutdown_until(tokio::time::Instant::now()),
        )
        .await
        .is_err(),
        "shutdown_until must keep containing an attempt whose engine ignores cancellation"
    );

    let aborted = manager
        .abort_attempts(tokio::time::Instant::now() + Duration::from_secs(5))
        .await;
    assert_eq!(aborted, vec![download_id]);
    assert_eq!(manager.active_count(), 0);
}

#[tokio::test]
async fn test_runtime_reconfigure_max_concurrent_downloads() {
    // Validates the public contract: increasing capacity is
    // immediately observable, decreasing capacity is observable
    // even though existing in-flight downloads aren't preempted.
    let config = DownloadManagerConfig {
        max_concurrent_downloads: 2,
        high_priority_extra_slots: 0,
        ..Default::default()
    };
    let manager = DownloadManager::with_config(config);

    // Increase beyond initial capacity.
    assert_eq!(manager.set_max_concurrent_downloads(4), 4);
    assert_eq!(manager.max_concurrent_downloads(), 4);

    // Decrease — getter reflects the new value immediately.
    assert_eq!(manager.set_max_concurrent_downloads(1), 1);
    assert_eq!(manager.max_concurrent_downloads(), 1);

    // After both slots are occupied, a third acquire waits in the queue.
    let q = manager.queue.clone();
    let req = AcquireRequest {
        session_id: "s1".to_string(),
        streamer_id: "x".to_string(),
        streamer_name: "x".to_string(),
        engine_type: EngineType::Ffmpeg,
        priority: Priority::Normal,
    };
    let _slot1 = q
        .acquire(req, CancellationToken::new(), |_| {})
        .await
        .unwrap();
    assert_eq!(q.in_flight(), 1);

    // Second acquire must queue (capacity = 1).
    let q2 = q.clone();
    let h = tokio::spawn(async move {
        q2.acquire(
            AcquireRequest {
                session_id: "s2".to_string(),
                streamer_id: "x".to_string(),
                streamer_name: "x".to_string(),
                engine_type: EngineType::Ffmpeg,
                priority: Priority::Normal,
            },
            CancellationToken::new(),
            |_| {},
        )
        .await
    });
    // Wait for it to register as pending.
    for _ in 0..50 {
        if q.pending_count() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(q.pending_count(), 1);

    // Bump the limit; the waiter should fire.
    manager.set_max_concurrent_downloads(2);
    let _slot2 = h.await.unwrap().unwrap();
    assert_eq!(q.in_flight(), 2);
}

#[tokio::test]
async fn queued_slot_abandoned_after_acquire_emits_dequeued() {
    let config = DownloadManagerConfig {
        max_concurrent_downloads: 1,
        high_priority_extra_slots: 0,
        ..Default::default()
    };
    let manager = Arc::new(DownloadManager::with_config(config));
    let mut events = manager.subscribe();

    let first = manager
        .acquire_slot(
            AcquireRequest {
                session_id: "active-session".to_string(),
                streamer_id: "streamer-active".to_string(),
                streamer_name: "Active".to_string(),
                engine_type: EngineType::Ffmpeg,
                priority: Priority::Normal,
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();

    let waiter_manager = manager.clone();
    let waiter = tokio::spawn(async move {
        waiter_manager
            .acquire_slot(
                AcquireRequest {
                    session_id: "queued-session".to_string(),
                    streamer_id: "streamer-queued".to_string(),
                    streamer_name: "Queued".to_string(),
                    engine_type: EngineType::Ffmpeg,
                    priority: Priority::Normal,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap()
    });

    let queued_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        queued_event,
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadQueued {
            ref session_id,
            ..
        }) if session_id == "queued-session"
    ));

    drop(first);
    let slot = waiter.await.unwrap();
    assert!(slot.queued_event_emitted());

    manager.emit_dequeued_for_slot(&slot, "streamer-queued", "Queued");

    let dequeued_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        dequeued_event,
        DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadDequeued {
            ref session_id,
            ..
        }) if session_id == "queued-session"
    ));
}

#[test]
fn test_engine_registration() {
    let manager = DownloadManager::new();

    // FFmpeg should be registered by default
    assert!(manager.get_engine(EngineType::Ffmpeg).is_some());
    assert!(manager.get_engine(EngineType::Streamlink).is_some());
    assert!(manager.get_engine(EngineType::Mesio).is_some());
}

// ========== Output-root write gate integration ==========

/// Build a `DownloadConfig` pointed at `output_dir`, with the other
/// fields set to minimal plausible values. The URL/streamer fields are
/// only used for logging — we never actually spawn an engine in these
/// tests.
fn test_config_with_output_dir(output_dir: std::path::PathBuf) -> DownloadConfig {
    DownloadConfig::new(
        "https://example.com/test.flv",
        output_dir,
        "test-streamer-id",
        "TestStreamer",
        "test-session-id",
    )
}

/// Wrap a `DownloadManager` with a freshly constructed gate and return
/// both the manager and a counter the recovery hook bumps each time it
/// fires. Used by the three `prepare_output_dir_*` tests below.
fn manager_with_gate() -> (
    DownloadManager,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<super::super::output_root_gate::OutputRootGate>,
) {
    use super::super::output_root_gate::{OutputRootGate, RecoveryHook};
    use std::sync::Weak;
    use std::sync::atomic::AtomicUsize;

    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let hook: RecoveryHook = Arc::new(move |_root: &std::path::Path| {
        c2.fetch_add(1, Ordering::SeqCst);
    });
    let gate = OutputRootGate::new(
        Weak::new(),
        hook,
        vec![],
        Duration::from_secs(1), // short cooldown for the test runner
    );
    let manager = DownloadManager::new();
    manager.set_output_root_gate(gate.clone());
    (manager, counter, gate)
}

#[tokio::test]
async fn prepare_output_dir_happy_path_returns_ok() {
    // Baseline: a real, writable temp dir. The gate starts Healthy and
    // stays Healthy; `ensure_output_dir` creates the nested subdir that
    // doesn't yet exist inside the temp root.
    let temp = tempfile::tempdir().expect("tempdir");
    let nested = temp.path().join("huya").join("X").join("20260415");
    let (manager, counter, gate) = manager_with_gate();

    let config = test_config_with_output_dir(nested.clone());
    let result = manager.prepare_output_dir(&config).await;

    assert!(result.is_ok(), "happy path should succeed: {:?}", result);
    assert!(
        nested.is_dir(),
        "nested output dir should have been created"
    );
    // Recovery hook only fires on a Degraded → Healthy transition.
    // A first-ever success against an untracked root is a no-op for the
    // gate, so the counter stays at zero.
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    // And the gate snapshot should be empty (no roots ever tracked).
    assert!(gate.snapshot().is_empty());
}

#[tokio::test]
async fn mid_stream_output_io_degrades_the_gate_before_the_terminal() {
    for io_kind in [
        IoErrorKindSer::StorageFull,
        IoErrorKindSer::PermissionDenied,
        IoErrorKindSer::ReadOnlyFilesystem,
        IoErrorKindSer::NotFound,
        IoErrorKindSer::TimedOut,
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let output_dir = temp.path().join("recording");
        let (manager, _, gate) = manager_with_gate();
        let mut events = manager.subscribe();
        start_scripted_download(
            &manager,
            test_download_config(output_dir.clone(), "output-io-session"),
            vec![
                SegmentEvent::OutputIoError {
                    output_dir: output_dir.clone(),
                    io_kind,
                    detail: "output fault".to_owned(),
                },
                SegmentEvent::DownloadFailed {
                    kind: DownloadFailureKind::OutputRootUnavailable { io_kind },
                    message: "writer failed".to_owned(),
                },
            ],
        )
        .await
        .unwrap();
        let terminal = wait_for_download_terminal(&mut events).await;
        assert!(
            matches!(terminal, DownloadTerminalEvent::Failed { kind: DownloadFailureKind::OutputRootUnavailable { io_kind: actual }, .. } if actual == io_kind)
        );
        let blocked = gate
            .check(&output_dir)
            .expect_err("gate must reject before the terminal is observed");
        assert_eq!(blocked.kind, io_kind);
        assert_eq!(blocked.message, "output fault");
    }
}

#[tokio::test]
async fn output_failures_without_a_gate_record_keep_the_circuit_breaker() {
    for attach_gate in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let output_dir = temp.path().join("recording");
        let (manager, gate) = if attach_gate {
            let (manager, _, gate) = manager_with_gate();
            (manager, Some(gate))
        } else {
            (DownloadManager::new(), None)
        };
        let mut events = manager.subscribe();
        let output_error = SegmentEvent::OutputIoError {
            output_dir: output_dir.clone(),
            io_kind: IoErrorKindSer::PermissionDenied,
            detail: "denied".to_owned(),
        };
        let terminal = SegmentEvent::DownloadFailed {
            kind: DownloadFailureKind::OutputRootUnavailable {
                io_kind: IoErrorKindSer::PermissionDenied,
            },
            message: "writer failed".to_owned(),
        };
        // Either the event is correctly ordered but there is no gate, or
        // the gate is attached but the engine sent its terminal too early.
        let scripted = if attach_gate {
            vec![terminal, output_error]
        } else {
            vec![output_error, terminal]
        };
        start_scripted_download(
            &manager,
            test_download_config(output_dir.clone(), "unpaired-output-failure"),
            scripted,
        )
        .await
        .unwrap();
        let terminal = wait_for_download_terminal(&mut events).await;
        assert!(matches!(
            terminal,
            DownloadTerminalEvent::Failed {
                kind: DownloadFailureKind::Io,
                ..
            }
        ));
        if let Some(gate) = gate {
            assert!(gate.check(&output_dir).is_ok());
        }
    }
}

#[tokio::test]
async fn early_cancellation_does_not_charge_breaker_but_racing_failure_does() {
    for kind in [DownloadFailureKind::Cancelled, DownloadFailureKind::Other] {
        let manager = DownloadManager::new();
        let temp = tempfile::tempdir().unwrap();
        let mut events = manager.subscribe();
        let key = EngineKey::global(EngineType::Ffmpeg);
        for attempt in 0..5 {
            let scripted = ScriptedSegmentEngine::with_shutdown_tail(
                vec![],
                vec![SegmentEvent::DownloadFailed {
                    kind,
                    message: "early engine outcome".to_owned(),
                }],
            );
            let id = start_scripted_download_with_engine(
                &manager,
                test_download_config(temp.path().to_path_buf(), &format!("early-stop-{attempt}")),
                scripted,
            )
            .await
            .unwrap();
            manager
                .request_stop(&id, DownloadStopCause::Shutdown)
                .unwrap();
            assert!(matches!(
                wait_for_download_terminal(&mut events).await,
                DownloadTerminalEvent::Cancelled { .. }
            ));
        }
        assert_eq!(
            manager.circuit_breakers.is_allowed(&key),
            kind == DownloadFailureKind::Cancelled
        );
    }
}

#[tokio::test]
async fn prepare_output_dir_on_unwritable_parent_trips_gate() {
    // Force create_dir_all to fail portably. GHA's Windows runner
    // runs as admin, so `C:\nonexistent\...` is creatable there;
    // Linux CI sandboxes often mount `/` read-only → EROFS. Instead,
    // put a regular file in a tempdir and point the output path at
    // a child of it — `create_dir_all` then fails with NotADirectory
    // on Unix and ERROR_DIRECTORY on Windows, both of which the gate
    // classifies into DownloadFailureKind::OutputRootUnavailable.
    // We don't pin the exact io_kind because the classification
    // bucket (never `Other`) is what's actually under test.
    let temp = tempfile::tempdir().expect("tempdir");
    let blocker = temp.path().join("blocker");
    std::fs::write(&blocker, b"i am a file, not a dir").unwrap();
    let bad_output = blocker.join("huya").join("X").join("20260415");
    let (manager, counter, gate) = manager_with_gate();

    let config = test_config_with_output_dir(bad_output.clone());
    let result = manager.prepare_output_dir(&config).await;

    // Must fail with OutputRootUnavailable{...}, NOT the generic Io
    // kind — this proves `EngineStartError::from` correctly walked
    // the error chain and the manager's `prepare_output_dir` routed
    // the io::Error into the gate before returning.
    let err = result.expect_err("should fail on unwritable parent");
    let io_kind = match err.kind {
        DownloadFailureKind::OutputRootUnavailable { io_kind } => io_kind,
        other => panic!("expected OutputRootUnavailable{{_}}, got {:?}", other),
    };
    assert!(
        !matches!(io_kind, IoErrorKindSer::Other),
        "expected a classified io_kind (NotFound / PermissionDenied / \
         ReadOnlyFilesystem / StorageFull / TimedOut), got Other"
    );

    // The gate must now be tracking a Degraded root with a matching
    // cached error kind.
    let snapshot = gate.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(
        snapshot[0].state,
        crate::downloader::RootHealthState::Degraded
    );
    let last_error = snapshot[0]
        .last_error
        .as_ref()
        .expect("degraded root must have cached error");
    assert_eq!(last_error.0, io_kind);

    // A second `prepare_output_dir` call inside the cooldown window
    // must fast-reject via `gate.check()`, not re-try `create_dir_all`.
    // This bounds filesystem retries while the output root is unavailable.
    let result2 = manager.prepare_output_dir(&config).await;
    let err2 = result2.expect_err("second call should fast-reject");
    assert!(matches!(
        err2.kind,
        DownloadFailureKind::OutputRootUnavailable { .. }
    ));
    // Recovery hook must NOT have fired — we're still Degraded.
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prepare_output_dir_recovers_after_path_becomes_valid() {
    // Trip the gate by pointing at a child of a regular file (fails
    // portably on Unix ENOTDIR and Windows ERROR_DIRECTORY — tokio's
    // create_dir_all will not recreate a file-as-directory ancestor).
    // Then fix the filesystem, wait past the cooldown, retry. The
    // winning CAS caller should see ensure_output_dir succeed, flip
    // the gate to Healthy, and fire the recovery hook exactly once.
    let temp = tempfile::tempdir().expect("tempdir");

    use super::super::output_root_gate::{OutputRootGate, RecoveryHook};
    use std::sync::Weak;
    use std::sync::atomic::AtomicUsize;

    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let hook: RecoveryHook = Arc::new(move |_root: &std::path::Path| {
        c2.fetch_add(1, Ordering::SeqCst);
    });
    let configured_root = temp.path().to_path_buf();
    let gate = OutputRootGate::new(
        Weak::new(),
        hook,
        vec![configured_root.clone()],
        Duration::from_secs(1),
    );
    let manager = DownloadManager::new();
    manager.set_output_root_gate(gate.clone());

    // `doomed` is a regular file; any path under it will fail
    // create_dir_all because an ancestor component is not a directory.
    let doomed = temp.path().join("doomed");
    std::fs::write(&doomed, b"i am a file, not a dir").unwrap();
    let under_doomed = doomed.join("will-fail");

    // Now `create_dir_all(under_doomed)` will fail with NotADirectory
    // or similar because one of the ancestor components is a regular
    // file. On Linux this surfaces as ErrorKind::NotFound or
    // ErrorKind::NotADirectory depending on kernel version; either
    // way the gate records a failure.
    let bad_config = test_config_with_output_dir(under_doomed.clone());
    let first = manager.prepare_output_dir(&bad_config).await;
    assert!(first.is_err(), "should fail when ancestor is a file");
    let snap = gate.snapshot();
    assert_eq!(snap.len(), 1, "gate should be tracking the configured root");
    assert_eq!(snap[0].state, crate::downloader::RootHealthState::Degraded);
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Fix the filesystem: remove the blocking file and recreate the
    // directory structure. Wait past the 1s cooldown before retrying.
    std::fs::remove_file(&doomed).unwrap();
    std::fs::create_dir_all(&under_doomed).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Retry. The winning CAS caller's ensure_output_dir succeeds, the
    // gate flips to Healthy, the recovery hook fires.
    let second = manager.prepare_output_dir(&bad_config).await;
    assert!(
        second.is_ok(),
        "retry should succeed after cooldown and filesystem fix: {:?}",
        second
    );

    // Recovery hook is spawned on a tokio task; yield so it runs.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "recovery hook should fire exactly once per Degraded→Healthy transition"
    );

    // Gate snapshot should now show Healthy.
    let snap = gate.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].state, crate::downloader::RootHealthState::Healthy);
}

#[tokio::test]
async fn prepare_output_dir_without_gate_is_transparent() {
    // Safety guarantee: with no gate installed, prepare_output_dir is a
    // plain create-dir call — success creates the dir, failure returns
    // a classified EngineStartError, no panics, no hidden state.
    let temp = tempfile::tempdir().expect("tempdir");
    let nested = temp.path().join("a").join("b").join("c");
    let manager = DownloadManager::new();
    // Deliberately NO set_output_root_gate.

    let config = test_config_with_output_dir(nested.clone());
    assert!(manager.prepare_output_dir(&config).await.is_ok());
    assert!(nested.is_dir());
}

#[tokio::test]
async fn required_terminal_event_survives_lagged_observer() {
    let (coordination_tx, mut coordination_rx) = download_coordination_channel();
    let manager = DownloadManager::new().with_coordination_sender(coordination_tx);
    let mut observer = manager.subscribe();

    for index in 0..300 {
        manager.events.publish(DownloadManagerEvent::Progress(
            DownloadProgressEvent::DownloadDequeued {
                streamer_id: "streamer".to_string(),
                streamer_name: "Streamer".to_string(),
                session_id: format!("session-{index}"),
            },
        ));
    }

    let publish = manager.emit_rejected(
        "streamer".to_string(),
        "Streamer".to_string(),
        "required-session".to_string(),
        "test rejection".to_string(),
        Some(5),
        DownloadRejectedKind::CircuitBreaker,
    );
    tokio::pin!(publish);

    let delivery = tokio::select! {
        delivery = coordination_rx.recv() => delivery
            .expect("required terminal channel stays open")
            .expect("required terminal event should arrive"),
        result = &mut publish => panic!("publish returned before acknowledgement: {result:?}"),
    };
    let (event, acknowledgement) = delivery.into_parts();
    let DownloadManagerEvent::Terminal(terminal) = event else {
        panic!("expected terminal coordination event");
    };
    let _ = acknowledgement.send(Ok(()));
    publish
        .await
        .expect("rejection should complete after acknowledgement");
    assert_eq!(terminal.session_id(), "required-session");
    assert!(matches!(
        observer.recv().await,
        Err(broadcast::error::RecvError::Lagged(_))
    ));
}

#[tokio::test]
async fn coordination_shutdown_rejects_events_behind_marker() {
    let (coordination_tx, mut coordination_rx) = download_coordination_channel();
    let late_sender = coordination_tx.clone();
    let shutdown = tokio::spawn(async move { coordination_tx.shutdown().await });

    assert!(matches!(coordination_rx.recv().await, Ok(None)));
    shutdown
        .await
        .expect("shutdown task should not panic")
        .expect("shutdown marker should be acknowledged");

    let late_event = DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
        streamer_id: "streamer".to_string(),
        streamer_name: "Streamer".to_string(),
        session_id: "session".to_string(),
        reason: "late event".to_string(),
        retry_after_secs: None,
        kind: DownloadRejectedKind::CircuitBreaker,
    });
    assert!(matches!(
        late_sender.publish(late_event),
        DownloadCoordinationReceipt::Unavailable
    ));
}

mod feedback;
