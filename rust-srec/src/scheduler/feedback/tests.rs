use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::Notify;
use tokio_util::task::AbortOnDropHandle;

use super::*;
use crate::database::models::StreamerDbModel;
use crate::domain::StreamerState;
use crate::downloader::EngineEndSignal;
use crate::downloader::engine::{DownloadProgress, EngineType};
use crate::monitor::{LiveStatus, ProcessStatusResult};
use crate::scheduler::actor::{
    ActorRegistry, CheckError, CheckResult, NoOpStatusChecker, StatusChecker, StreamerActor,
};
use crate::streamer::StreamerMetadata;

fn metadata(id: &str) -> Arc<DashMap<String, Arc<StreamerMetadata>>> {
    let mut row = StreamerDbModel::new(id, "https://example.com/live", "platform");
    row.id = id.to_owned();
    let map = Arc::new(DashMap::new());
    map.insert(
        id.to_owned(),
        Arc::new(StreamerMetadata::from_db_model(&row)),
    );
    map
}

fn started(id: &str, download: &str, session: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
        download_id: download.to_owned(),
        streamer_id: id.to_owned(),
        streamer_name: id.to_owned(),
        session_id: session.to_owned(),
        engine_type: EngineType::Ffmpeg,
        cdn_host: "example.com".to_owned(),
        download_url: "https://example.com/media".to_owned(),
    })
}

fn completed(id: &str, download: &str, session: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Terminal(crate::downloader::DownloadTerminalEvent::Completed {
        download_id: download.to_owned(),
        streamer_id: id.to_owned(),
        streamer_name: id.to_owned(),
        session_id: session.to_owned(),
        total_bytes: 1,
        total_duration_secs: 1.0,
        total_segments: 1,
        file_path: None,
        engine_signal: EngineEndSignal::CleanDisconnect,
        stop_cause: None,
    })
}

async fn applied(receipt: ApplicationReceipt) -> FeedbackDisposition {
    tokio::time::timeout(Duration::from_secs(3), receipt)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

async fn state(handle: &ActorHandle<StreamerMessage>) -> super::super::actor::StreamerActorState {
    let (reply, receive) = oneshot::channel();
    handle
        .send_reliable(StreamerMessage::GetState(reply))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), receive)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn saturated_mailbox_retains_lifecycle_and_does_not_block_another_streamer() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let feedback = SchedulerFeedback::new();
        let (actor, mut slow) = StreamerActor::new(
            "slow".to_owned(),
            metadata("slow"),
            StreamerConfig::default(),
            CancellationToken::new(),
            Arc::new(NoOpStatusChecker),
        );
        slow.set_generation(1);
        // The actor has not started: fill every ordinary mailbox slot.
        for _ in 0..slow.mailbox_capacity().1 {
            slow.try_send(StreamerMessage::CheckStatus).unwrap();
        }
        let (fast_actor, mut fast) = StreamerActor::new(
            "fast".to_owned(),
            metadata("fast"),
            StreamerConfig::default(),
            CancellationToken::new(),
            Arc::new(NoOpStatusChecker),
        );
        fast.set_generation(2);
        feedback.update_targets(&HashMap::from([
            ("slow".to_owned(), slow.clone()),
            ("fast".to_owned(), fast.clone()),
        ]));
        let fast_task = AbortOnDropHandle::new(tokio::spawn(fast_actor.run()));
        let mut receipts = Vec::new();
        for n in 0..MAX_STREAMER_ENVELOPES / 2 {
            let (start, end) = feedback.reserve_attempt("slow").unwrap();
            receipts
                .push(feedback.publish_reserved(&started("slow", &format!("d{n}"), "s"), start));
            receipts
                .push(feedback.publish_reserved(&completed("slow", &format!("d{n}"), "s"), end));
        }
        assert!(feedback.reserve_attempt("slow").is_err());
        let (start, end) = feedback.reserve_attempt("fast").unwrap();
        assert_eq!(
            applied(feedback.publish_reserved(&started("fast", "fast-d", "fast-s"), start)).await,
            FeedbackDisposition::Applied
        );
        assert_eq!(
            applied(feedback.publish_reserved(&completed("fast", "fast-d", "fast-s"), end)).await,
            FeedbackDisposition::Applied
        );
        let slow_task = AbortOnDropHandle::new(tokio::spawn(actor.run()));
        for (index, receipt) in receipts.into_iter().enumerate() {
            let disposition = applied(receipt).await;
            if index >= MAX_STREAMER_ENVELOPES - 2 {
                assert_eq!(disposition, FeedbackDisposition::Applied);
            } else {
                assert_eq!(disposition, FeedbackDisposition::Superseded);
            }
        }
        assert!(feedback.reserve_attempt("slow").is_ok());
        assert_eq!(state(&slow).await.streamer_state, StreamerState::NotLive);
        feedback.shutdown().await;
        slow.cancel();
        fast.cancel();
        slow_task.await.unwrap().unwrap();
        fast_task.await.unwrap().unwrap();
    })
    .await
    .expect("full mailbox delivery must settle");
}

#[tokio::test]
async fn successor_identity_rejects_old_terminal_and_late_heartbeat() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let feedback = SchedulerFeedback::new();
        let (actor, mut handle) = StreamerActor::new(
            "one".to_owned(),
            metadata("one"),
            StreamerConfig::default(),
            CancellationToken::new(),
            Arc::new(NoOpStatusChecker),
        );
        handle.set_generation(1);
        feedback.update_targets(&HashMap::from([("one".to_owned(), handle.clone())]));
        let task = AbortOnDropHandle::new(tokio::spawn(actor.run()));
        let (old_start, old_end) = feedback.reserve_attempt("one").unwrap();
        let (new_start, new_end) = feedback.reserve_attempt("one").unwrap();
        applied(feedback.publish_reserved(&started("one", "old", "old-session"), old_start)).await;
        applied(feedback.publish_reserved(&started("one", "new", "new-session"), new_start)).await;
        assert_eq!(
            applied(feedback.publish_reserved(&completed("one", "old", "old-session"), old_end))
                .await,
            FeedbackDisposition::Superseded
        );
        assert_eq!(state(&handle).await.streamer_state, StreamerState::Live);
        applied(feedback.publish_reserved(&completed("one", "new", "new-session"), new_end)).await;
        handle
            .send_reliable(StreamerMessage::DownloadHeartbeat {
                download_id: "old".to_owned(),
                session_id: "old-session".to_owned(),
                progress: Some(DownloadProgress::default()),
            })
            .await
            .unwrap();
        let snapshot = state(&handle).await;
        assert_eq!(snapshot.streamer_state, StreamerState::NotLive);
        assert!(snapshot.last_download_activity_at.is_none());
        feedback.shutdown().await;
        handle.cancel();
        task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

struct PanicChecker {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

struct CountingChecker {
    checks: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl StatusChecker for CountingChecker {
    async fn check_status(
        &self,
        metadata: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        self.checks.fetch_add(1, Ordering::AcqRel);
        NoOpStatusChecker.check_status(metadata).await
    }

    async fn process_status(
        &self,
        _metadata: &StreamerMetadata,
        _status: LiveStatus,
    ) -> Result<ProcessStatusResult, CheckError> {
        Ok(ProcessStatusResult::Applied)
    }

    async fn handle_error(
        &self,
        _metadata: &StreamerMetadata,
        _error: &str,
    ) -> Result<(), CheckError> {
        Ok(())
    }

    async fn set_infra_blocked(
        &self,
        _metadata: &StreamerMetadata,
        _reason: crate::monitor::InfraBlockReason,
    ) -> Result<(), CheckError> {
        Ok(())
    }
}

#[tokio::test]
async fn admission_rechecks_wait_for_capacity_and_coalesce_until_recovery() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let feedback = SchedulerFeedback::new();
        let checks = Arc::new(AtomicU64::new(0));
        let (actor, mut handle) = StreamerActor::new(
            "one".to_owned(),
            metadata("one"),
            StreamerConfig::default(),
            CancellationToken::new(),
            Arc::new(CountingChecker {
                checks: checks.clone(),
            }),
        );
        handle.set_generation(1);
        feedback.update_targets(&HashMap::from([("one".to_owned(), handle.clone())]));
        let task = AbortOnDropHandle::new(tokio::spawn(actor.run()));
        let held = feedback
            .capacity
            .clone()
            .acquire_many_owned(MAX_FEEDBACK_ENVELOPES as u32)
            .await
            .unwrap();
        for _ in 0..30 {
            feedback.request_recheck("one");
            state(&handle).await;
            assert_eq!(
                checks.load(Ordering::Acquire),
                0,
                "capacity pressure must not repeat extraction"
            );
        }
        drop(held);
        while checks.load(Ordering::Acquire) == 0 {
            state(&handle).await;
            tokio::task::yield_now().await;
        }
        for _ in 0..30 {
            state(&handle).await;
            assert_eq!(
                checks.load(Ordering::Acquire),
                1,
                "recovery must coalesce the retained overload requests"
            );
        }
        // Retirement cancels a recovery wait and cannot restart an actor after
        // the held capacity becomes available again.
        let held = feedback
            .capacity
            .clone()
            .acquire_many_owned(MAX_FEEDBACK_ENVELOPES as u32)
            .await
            .unwrap();
        feedback.request_recheck("one");
        state(&handle).await;
        feedback.retire("one");
        drop(held);
        feedback.shutdown().await;
        assert_eq!(checks.load(Ordering::Acquire), 1);
        handle.cancel();
        task.await.unwrap().unwrap();
        assert_eq!(
            feedback.capacity.available_permits(),
            MAX_FEEDBACK_ENVELOPES
        );
    })
    .await
    .expect("capacity recovery must remain owned and bounded");
}
#[async_trait::async_trait]
impl StatusChecker for PanicChecker {
    async fn check_status(
        &self,
        _metadata: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        self.entered.notify_one();
        self.release.notified().await;
        std::panic::resume_unwind(Box::new("injected feedback actor panic"));
    }
    async fn process_status(
        &self,
        _metadata: &StreamerMetadata,
        _status: LiveStatus,
    ) -> Result<ProcessStatusResult, CheckError> {
        Ok(ProcessStatusResult::Applied)
    }
    async fn handle_error(
        &self,
        _metadata: &StreamerMetadata,
        _error: &str,
    ) -> Result<(), CheckError> {
        Ok(())
    }
    async fn set_infra_blocked(
        &self,
        _metadata: &StreamerMetadata,
        _reason: crate::monitor::InfraBlockReason,
    ) -> Result<(), CheckError> {
        Ok(())
    }
}

#[tokio::test]
async fn panic_before_application_retains_feedback_for_the_replacement_generation() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let feedback = SchedulerFeedback::new();
        let cancel = CancellationToken::new();
        let mut registry = ActorRegistry::new(cancel.clone());
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (actor, handle) = StreamerActor::new(
            "one".to_owned(),
            metadata("one"),
            StreamerConfig::default(),
            cancel.child_token(),
            Arc::new(PanicChecker {
                entered: entered.clone(),
                release: release.clone(),
            }),
        );
        handle.try_send(StreamerMessage::CheckStatus).unwrap();
        let old = registry.spawn_streamer(actor, handle).unwrap();
        feedback.update_targets(registry.streamer_handles_map());
        entered.notified().await;
        let (start, end) = feedback.reserve_attempt("one").unwrap();
        let receipt = feedback.publish_reserved(&started("one", "d", "s"), start);
        release.notify_one();
        let finished = registry.join_next().await.unwrap().unwrap();
        assert!(finished.is_crash());
        registry.handle_task_completion(finished);
        feedback.update_targets(registry.streamer_handles_map());
        let (actor, handle) = StreamerActor::new(
            "one".to_owned(),
            metadata("one"),
            StreamerConfig::default(),
            cancel.child_token(),
            Arc::new(NoOpStatusChecker),
        );
        let replacement = registry.spawn_streamer(actor, handle).unwrap();
        assert_ne!(old.generation(), replacement.generation());
        feedback.update_targets(registry.streamer_handles_map());
        assert_eq!(applied(receipt).await, FeedbackDisposition::Applied);
        assert_eq!(
            state(&replacement).await.streamer_state,
            StreamerState::Live
        );
        assert_eq!(
            applied(feedback.publish_reserved(&completed("one", "d", "s"), end)).await,
            FeedbackDisposition::Applied
        );
        feedback.shutdown().await;
        cancel.cancel();
        registry.join_next().await.unwrap().unwrap();
    })
    .await
    .expect("replacement must receive retained lifecycle work");
}

#[tokio::test]
async fn shutdown_and_retirement_release_every_reserved_envelope_without_polling() {
    let feedback = SchedulerFeedback::new();
    let (start, end) = feedback.reserve_attempt("one").unwrap();
    let first = feedback.publish_reserved(&started("one", "d", "s"), start);
    feedback.retire("one");
    assert_eq!(applied(first).await, FeedbackDisposition::Retired);
    assert_eq!(
        applied(feedback.publish_reserved(&completed("one", "d", "s"), end)).await,
        FeedbackDisposition::Retired
    );
    feedback.activate("one");
    let (start, end) = feedback.reserve_attempt("one").unwrap();
    let first = feedback.publish_reserved(&started("one", "next", "next-s"), start);
    feedback.shutdown().await;
    assert_eq!(applied(first).await, FeedbackDisposition::MonitoringStopped);
    assert_eq!(
        applied(feedback.publish_reserved(&completed("one", "next", "next-s"), end)).await,
        FeedbackDisposition::MonitoringStopped
    );
    assert_eq!(
        feedback.capacity.available_permits(),
        MAX_FEEDBACK_ENVELOPES
    );
}

#[tokio::test]
async fn capacity_scaling_saturates_without_arithmetic_or_semaphore_overflow() {
    let feedback = SchedulerFeedback::new();
    feedback.ensure_attempt_capacity(usize::MAX);
    assert_eq!(
        feedback.capacity.available_permits(),
        Semaphore::MAX_PERMITS
    );
    let held = feedback.reserve_attempt("one").unwrap();
    feedback.ensure_attempt_capacity(1);
    assert_eq!(
        feedback.capacity.available_permits(),
        Semaphore::MAX_PERMITS - 2
    );
    drop(held);
    feedback.shutdown().await;
}

#[tokio::test]
async fn full_mailbox_retains_the_latest_configuration_until_application() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let feedback = SchedulerFeedback::new();
        let (actor, mut handle) = StreamerActor::new(
            "one".to_owned(),
            metadata("one"),
            StreamerConfig::default(),
            CancellationToken::new(),
            Arc::new(NoOpStatusChecker),
        );
        handle.set_generation(1);
        for _ in 0..handle.mailbox_capacity().1 {
            handle.try_send(StreamerMessage::CheckStatus).unwrap();
        }
        feedback.update_targets(&HashMap::from([("one".to_owned(), handle.clone())]));
        feedback.configure(
            "one",
            DesiredConfig {
                generation: 1,
                revision: 1,
                config: StreamerConfig {
                    check_interval_ms: 80_000,
                    ..Default::default()
                },
            },
        );
        tokio::task::yield_now().await;
        feedback.configure(
            "one",
            DesiredConfig {
                generation: 1,
                revision: 2,
                config: StreamerConfig {
                    check_interval_ms: 90_000,
                    ..Default::default()
                },
            },
        );
        let task = AbortOnDropHandle::new(tokio::spawn(actor.run()));
        loop {
            let pending = feedback.state.lock().queues["one"]
                .config
                .borrow()
                .is_some();
            if !pending {
                break;
            }
            tokio::task::yield_now().await;
        }
        loop {
            if state(&handle).await.recurring_interval_ms == Some(90_000) {
                break;
            }
            tokio::task::yield_now().await;
        }
        feedback.shutdown().await;
        handle.cancel();
        task.await.unwrap().unwrap();
    })
    .await
    .expect("latest desired configuration must survive mailbox pressure");
}

#[tokio::test]
async fn exhausted_restart_policy_settles_retained_work_as_unavailable() {
    let feedback = SchedulerFeedback::new();
    let (start, end) = feedback.reserve_attempt("one").unwrap();
    let receipt = feedback.publish_reserved(&started("one", "d", "s"), start);
    feedback.unavailable("one");
    assert_eq!(
        applied(receipt).await,
        FeedbackDisposition::ActorUnavailable
    );
    assert_eq!(
        applied(feedback.publish_reserved(&completed("one", "d", "s"), end)).await,
        FeedbackDisposition::ActorUnavailable
    );
    feedback.shutdown().await;
    assert_eq!(
        feedback.capacity.available_permits(),
        MAX_FEEDBACK_ENVELOPES
    );
}
