use super::*;

use crate::scheduler::feedback::SchedulerFeedback;

#[tokio::test]
async fn feedback_overload_rejects_before_start_and_retry_succeeds_after_capacity_release() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = DownloadManager::new();
        let feedback = SchedulerFeedback::new();
        manager.set_scheduler_feedback(feedback.clone()).unwrap();
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
        drop(held);
        let id = start_scripted_download_with_engine(
            &manager,
            config,
            ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
        )
        .await
        .unwrap();
        assert_eq!(manager.active_count(), 1);
        // No actor can acknowledge application. Producer startup and terminal
        // containment must nevertheless finish, using their reserved envelopes.
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
        feedback.shutdown().await;
    })
    .await
    .expect("producer admission must never wait for actor application");
}

#[tokio::test]
async fn cancelled_attempt_admission_releases_both_feedback_reservations() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = DownloadManager::new();
        let feedback = SchedulerFeedback::new();
        manager.set_scheduler_feedback(feedback.clone()).unwrap();
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
        drop(remaining);
        feedback.shutdown().await;
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn feedback_terminal_application_observes_the_slot_already_released() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = Arc::new(DownloadManager::new());
        let feedback = SchedulerFeedback::new();
        manager.set_scheduler_feedback(feedback.clone()).unwrap();
        let (sender, mut mailbox) = tokio::sync::mpsc::channel(1);
        let token = CancellationToken::new();
        let mut actor = crate::scheduler::actor::ActorHandle::new(
            sender,
            token.clone(),
            crate::scheduler::actor::ActorMetadata::streamer("test-streamer-id", false),
        );
        actor.set_generation(1);
        feedback.update_targets(&std::collections::HashMap::from([(
            "test-streamer-id".to_owned(),
            actor,
        )]));
        let observed_manager = manager.clone();
        let receiver = tokio::spawn(async move {
            let mut seen = 0;
            while let Some(message) = mailbox.recv().await {
                if let crate::scheduler::actor::StreamerMessage::LifecycleFeedback(envelope) =
                    message
                {
                    if matches!(envelope.event.as_ref(), DownloadManagerEvent::Terminal(_)) {
                        assert_eq!(observed_manager.active_count(), 0);
                    }
                    let _ = envelope
                        .applied
                        .send(Ok(crate::scheduler::feedback::FeedbackDisposition::Applied));
                    seen += 1;
                    if seen == 2 {
                        break;
                    }
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
        feedback.shutdown().await;
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
        let feedback = SchedulerFeedback::new();
        manager.set_scheduler_feedback(feedback.clone()).unwrap();
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
        // Old terminal/application capacity is still held: no actor has polled it.
        assert_eq!(manager.set_max_concurrent_downloads(700), 700);
        let reservations: Vec<_> = (0..705)
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
        feedback.shutdown().await;
    })
    .await
    .expect("concurrency resizing must preserve admission ownership");
}

struct StartupFromChecker {
    manager: Arc<DownloadManager>,
    config: std::sync::Mutex<Option<DownloadConfig>>,
    started: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
    release: tokio::sync::Notify,
}

#[async_trait]
impl crate::scheduler::actor::StatusChecker for StartupFromChecker {
    async fn check_status(
        &self,
        _streamer: &crate::streamer::StreamerMetadata,
    ) -> std::result::Result<
        (
            crate::scheduler::actor::CheckResult,
            crate::monitor::LiveStatus,
        ),
        crate::scheduler::actor::CheckError,
    > {
        let config = self.config.lock().unwrap().take();
        if let Some(config) = config {
            let id = start_scripted_download_with_engine(
                &self.manager,
                config,
                ScriptedSegmentEngine::with_shutdown_tail(vec![], vec![]),
            )
            .await
            .map_err(|error| crate::scheduler::actor::CheckError::transient(error.to_string()))?;
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(id);
            }
            self.release.notified().await;
        }
        Ok((
            crate::scheduler::actor::CheckResult::success(crate::domain::StreamerState::NotLive),
            crate::monitor::LiveStatus::Offline,
        ))
    }
    async fn process_status(
        &self,
        _streamer: &crate::streamer::StreamerMetadata,
        _status: crate::monitor::LiveStatus,
    ) -> std::result::Result<crate::monitor::ProcessStatusResult, crate::scheduler::actor::CheckError>
    {
        Ok(crate::monitor::ProcessStatusResult::Applied)
    }
    async fn handle_error(
        &self,
        _streamer: &crate::streamer::StreamerMetadata,
        _error: &str,
    ) -> std::result::Result<(), crate::scheduler::actor::CheckError> {
        Ok(())
    }
    async fn set_infra_blocked(
        &self,
        _streamer: &crate::streamer::StreamerMetadata,
        _reason: crate::monitor::InfraBlockReason,
    ) -> std::result::Result<(), crate::scheduler::actor::CheckError> {
        Ok(())
    }
}

#[tokio::test]
async fn actor_waiting_for_startup_cannot_deadlock_on_started_application() {
    tokio::time::timeout(Duration::from_secs(6), async {
        let manager = Arc::new(DownloadManager::new());
        let feedback = SchedulerFeedback::new();
        manager.set_scheduler_feedback(feedback.clone()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = test_download_config(directory.path().to_owned(), "checker-startup");
        let store = Arc::new(dashmap::DashMap::new());
        let mut row = crate::database::models::StreamerDbModel::new(
            "test",
            "https://example.com/live",
            "platform",
        );
        row.id = config.streamer_id.clone();
        store.insert(
            row.id.clone(),
            crate::streamer::StreamerMetadata::from_db_model(&row),
        );
        let (started, receive_started) = tokio::sync::oneshot::channel();
        let checker = Arc::new(StartupFromChecker {
            manager: manager.clone(),
            config: std::sync::Mutex::new(Some(config.clone())),
            started: std::sync::Mutex::new(Some(started)),
            release: tokio::sync::Notify::new(),
        });
        let cancel = CancellationToken::new();
        let (actor, mut handle) = crate::scheduler::actor::StreamerActor::new(
            config.streamer_id.clone(),
            store,
            crate::scheduler::actor::StreamerConfig::default(),
            cancel.clone(),
            checker.clone(),
        );
        handle.set_generation(1);
        handle
            .try_send(crate::scheduler::actor::StreamerMessage::CheckStatus)
            .unwrap();
        feedback.update_targets(&std::collections::HashMap::from([(
            config.streamer_id.clone(),
            handle.clone(),
        )]));
        let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(actor.run()));
        let id = receive_started
            .await
            .expect("checker must return from producer admission");
        for _ in 0..300 {
            manager.events.publish(DownloadManagerEvent::Progress(
                DownloadProgressEvent::Progress {
                    download_id: id.clone(),
                    streamer_id: config.streamer_id.clone(),
                    streamer_name: config.streamer_name.clone(),
                    session_id: config.session_id.clone(),
                    status: DownloadStatus::Downloading,
                    progress: DownloadProgress::default(),
                },
            ));
        }
        manager.stop_download(&id).await.unwrap();
        let (reply, state) = tokio::sync::oneshot::channel();
        handle
            .try_send(crate::scheduler::actor::StreamerMessage::GetState(reply))
            .unwrap();
        checker.release.notify_one();
        state.await.unwrap();
        feedback.shutdown().await;
        cancel.cancel();
        task.await.unwrap().unwrap();
    })
    .await
    .expect("actor/startup/application receipt cycle must not exist");
}
