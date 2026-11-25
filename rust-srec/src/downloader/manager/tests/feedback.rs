use super::*;

use std::sync::atomic::{AtomicBool, AtomicUsize};

use dashmap::DashMap;
use tokio::sync::Semaphore;

use crate::downloader::manager::{RecorderDownloadFeedback, RecorderFeedbackPermit};

/// Bounded test sink mirroring `RecorderFeedbackSink` capacity and permit
/// semantics so `DownloadManager`'s admission, release-ordering, and resizing
/// invariants are verified directly against `RecorderDownloadFeedback`.
struct TestRecorderFeedback {
    global: Arc<Semaphore>,
    global_capacity: AtomicUsize,
    per_streamer: DashMap<String, Arc<Semaphore>>,
    per_streamer_capacity: usize,
    stopped: AtomicBool,
    published: tokio::sync::mpsc::UnboundedSender<(DownloadManagerEvent, RecorderFeedbackPermit)>,
    rechecks: Mutex<Vec<String>>,
}

impl TestRecorderFeedback {
    fn new(
        per_streamer_capacity: usize,
    ) -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<(DownloadManagerEvent, RecorderFeedbackPermit)>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let initial_global = 32;
        let sink = Arc::new(Self {
            global: Arc::new(Semaphore::new(initial_global)),
            global_capacity: AtomicUsize::new(initial_global),
            per_streamer: DashMap::new(),
            per_streamer_capacity,
            stopped: AtomicBool::new(false),
            published: tx,
            rechecks: Mutex::new(Vec::new()),
        });
        (sink, rx)
    }

    fn streamer_semaphore(&self, streamer_id: &str) -> Arc<Semaphore> {
        self.per_streamer
            .entry(streamer_id.to_owned())
            .or_insert_with(|| Arc::new(Semaphore::new(self.per_streamer_capacity)))
            .clone()
    }
}

impl RecorderDownloadFeedback for TestRecorderFeedback {
    fn ensure_attempt_capacity(&self, attempts: usize) {
        let target = attempts.saturating_mul(2).max(32);
        let current = self.global_capacity.load(Ordering::Acquire);
        if target > current
            && self
                .global_capacity
                .compare_exchange(current, target, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.global.add_permits(target - current);
        }
    }

    fn monitoring_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    fn reserve(&self, streamer_id: &str) -> std::result::Result<RecorderFeedbackPermit, String> {
        let global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| "global feedback capacity full".to_owned())?;
        let streamer = self
            .streamer_semaphore(streamer_id)
            .try_acquire_owned()
            .map_err(|_| "streamer feedback capacity full".to_owned())?;
        Ok(RecorderFeedbackPermit::new(
            streamer_id.to_owned(),
            global,
            streamer,
        ))
    }

    fn reserve_attempt(
        &self,
        streamer_id: &str,
    ) -> std::result::Result<(RecorderFeedbackPermit, RecorderFeedbackPermit), String> {
        let first = self.reserve(streamer_id)?;
        let second = self.reserve(streamer_id)?;
        Ok((first, second))
    }

    fn publish_reserved(&self, event: &DownloadManagerEvent, permit: RecorderFeedbackPermit) {
        let _ = self.published.send((event.clone(), permit));
    }

    fn request_recheck(&self, streamer_id: &str) {
        self.rechecks.lock().push(streamer_id.to_owned());
    }

    fn stop_monitoring(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[tokio::test]
async fn feedback_overload_rejects_before_start_and_retry_succeeds_after_capacity_release() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = DownloadManager::new();
        let (feedback, _rx) = TestRecorderFeedback::new(4);
        manager.set_recorder_feedback(feedback.clone()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = test_download_config(directory.path().to_owned(), "capacity-session");
        let mut held = Vec::new();
        while let Ok(permit) = feedback.reserve(&config.streamer_id) {
            held.push(permit);
        }
        assert!(!held.is_empty());
        let mut observer = manager.subscribe();
        let result = start_scripted_download_with_engine(
            &manager,
            config.clone(),
            ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
        )
        .await;
        assert!(matches!(
            result,
            Err(crate::Error::SchedulerFeedbackBusy { .. })
        ));
        assert_eq!(manager.active_count(), 0);
        assert!(matches!(
            manager
                .emit_rejected(
                    config.streamer_id.clone(),
                    config.streamer_name.clone(),
                    config.session_id.clone(),
                    "fixture rejection".to_owned(),
                    Some(60),
                    DownloadRejectedKind::CircuitBreaker
                )
                .await,
            Err(crate::Error::SchedulerFeedbackBusy { .. })
        ));
        assert!(
            observer.try_recv().is_err(),
            "overload must not fabricate Started or Rejected"
        );
        assert_eq!(feedback.rechecks.lock().len(), 2);
        drop(held);
        let id = start_scripted_download_with_engine(
            &manager,
            config,
            ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
        )
        .await
        .unwrap();
        assert_eq!(manager.active_count(), 1);
        // No receiver drains `_rx`, yet producer startup and terminal
        // containment finish using their reserved permits.
        manager.stop_download(&id).await.unwrap();
        assert_eq!(manager.active_count(), 0);
        let events: Vec<_> = std::iter::from_fn(|| observer.try_recv().ok()).collect();
        assert!(matches!(
            events.first(),
            Some(DownloadManagerEvent::Progress(
                DownloadProgressEvent::DownloadStarted { .. }
            ))
        ));
        assert!(matches!(
            events.last(),
            Some(DownloadManagerEvent::Terminal(_))
        ));
    })
    .await
    .expect("producer admission must never wait for actor application");
}

#[tokio::test]
async fn cancelled_attempt_admission_releases_both_feedback_reservations() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = DownloadManager::new();
        let (feedback, _rx) = TestRecorderFeedback::new(4);
        manager.set_recorder_feedback(feedback.clone()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = test_download_config(directory.path().to_owned(), "cancelled-start");
        let mut held = Vec::new();
        while let Ok(permit) = feedback.reserve(&config.streamer_id) {
            held.push(permit);
        }
        let capacity = held.len();
        drop(held);
        manager.attempts.close_admission();
        assert!(
            start_scripted_download_with_engine(
                &manager,
                config.clone(),
                ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![])
            )
            .await
            .is_err()
        );
        assert_eq!(manager.active_count(), 0);
        let mut remaining = Vec::new();
        while let Ok(permit) = feedback.reserve(&config.streamer_id) {
            remaining.push(permit);
        }
        assert_eq!(remaining.len(), capacity);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn feedback_terminal_application_observes_the_slot_already_released() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = Arc::new(DownloadManager::new());
        let (feedback, mut rx) = TestRecorderFeedback::new(4);
        manager.set_recorder_feedback(feedback.clone()).unwrap();
        let observed_manager = manager.clone();
        let receiver = tokio::spawn(async move {
            let mut seen = 0;
            while let Some((event, permit)) = rx.recv().await {
                assert_eq!(permit.streamer_id(), "test-streamer-id");
                if matches!(event, DownloadManagerEvent::Terminal(_)) {
                    assert_eq!(observed_manager.active_count(), 0);
                }
                drop(permit);
                seen += 1;
                if seen == 2 {
                    break;
                }
            }
            seen
        });
        let directory = tempfile::tempdir().unwrap();
        let config = test_download_config(directory.path().to_owned(), "ordering");
        let id = start_scripted_download_with_engine(
            &manager,
            config,
            ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
        )
        .await
        .unwrap();
        manager.stop_download(&id).await.unwrap();
        assert_eq!(receiver.await.unwrap(), 2);
    })
    .await
    .expect("terminal application ordering must settle");
}

#[tokio::test]
async fn feedback_capacity_tracks_live_concurrency_and_extra_slots_without_revoking_reservations() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = DownloadManager::with_config(DownloadManagerConfig {
            max_concurrent_downloads: 1,
            high_priority_extra_slots: 5,
            ..Default::default()
        });
        let (feedback, _rx) = TestRecorderFeedback::new(8);
        manager.set_recorder_feedback(feedback.clone()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = test_download_config(directory.path().to_owned(), "old-reservations");
        let id = start_scripted_download_with_engine(
            &manager,
            config.clone(),
            ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
        )
        .await
        .unwrap();
        manager.stop_download(&id).await.unwrap();
        // Old terminal/application capacity is still held in `_rx`.
        assert_eq!(manager.set_max_concurrent_downloads(700), 700);
        let reservations: Vec<_> = (0..703)
            .map(|n| {
                feedback
                    .reserve_attempt(&format!("concurrent-{n}"))
                    .unwrap()
            })
            .collect();
        let id = start_scripted_download_with_engine(
            &manager,
            config,
            ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
        )
        .await
        .unwrap();
        manager.set_max_concurrent_downloads(1);
        assert_eq!(
            manager.active_count(),
            1,
            "shrinking does not revoke admitted work"
        );
        drop(reservations);
        manager.stop_download(&id).await.unwrap();
    })
    .await
    .expect("concurrency resizing must preserve admission ownership");
}

#[tokio::test]
async fn slot_acquisition_cancels_before_queue_admission_after_preflight() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let directory = tempfile::tempdir().unwrap();
        let manager = DownloadManager::new();
        manager.register_engine(Arc::new(ScriptedSegmentEngine::with_shutdown_tail(
            vec![],
            vec![],
        )));
        manager
            .preflight(PreflightRequest {
                streamer_id: "streamer".into(),
                streamer_name: "Streamer".into(),
                session_id: "session".into(),
                output_dir: directory.path().to_owned(),
                engine_id: Some("ffmpeg".into()),
                engines_override: None,
            })
            .await
            .unwrap();
        let maintenance = manager.try_admit_maintenance(0).unwrap();
        let cancel = CancellationToken::new();
        let mut events = manager.subscribe();
        let mut acquire = Box::pin(manager.acquire_slot(
            AcquireRequest {
                streamer_id: "streamer".into(),
                streamer_name: "Streamer".into(),
                session_id: "session".into(),
                engine_type: EngineType::Ffmpeg,
                priority: Priority::Normal,
            },
            cancel.clone(),
        ));
        assert!(futures::poll!(acquire.as_mut()).is_pending());
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
    })
    .await
    .expect("maintenance must not strand an acquire cancelled before queue insertion");
}
