use std::sync::atomic::{AtomicU32, AtomicUsize};

use super::*;
use crate::database::models::StreamerDbModel;
use crate::database::repositories::SqlxStreamerRepository;
use crate::downloader::EngineEndSignal;
use crate::downloader::engine::{DownloadProgress, DownloadStatus, EngineType};
use crate::scheduler::feedback::FeedbackDisposition;
use tokio::sync::Notify;
use tokio_util::task::AbortOnDropHandle;

async fn scheduler(ids: &[&str]) -> Scheduler<SqlxStreamerRepository> {
    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool));
    let broadcaster = ConfigEventBroadcaster::new();
    let manager = Arc::new(StreamerManager::new(repo, broadcaster.clone()));
    for id in ids {
        let mut row = StreamerDbModel::new(*id, "https://example.com/live", "platform");
        row.id = (*id).to_owned();
        manager
            .metadata_store()
            .insert((*id).to_owned(), StreamerMetadata::from_db_model(&row));
    }
    Scheduler::with_full_config(
        manager,
        broadcaster,
        SchedulerConfig::default(),
        CancellationToken::new(),
    )
}

struct GateResolver {
    entered: Notify,
    release: Notify,
    blocked: AtomicBool,
    version: AtomicU32,
    active: AtomicUsize,
    peak: AtomicUsize,
    completed_version: AtomicU32,
}

impl GateResolver {
    fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
            blocked: AtomicBool::new(true),
            version: AtomicU32::new(4),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            completed_version: AtomicU32::new(0),
        }
    }
    fn unblock(&self) {
        self.blocked.store(false, Ordering::Release);
        self.release.notify_waiters();
    }
}

#[async_trait::async_trait]
impl ActorConfigResolver for GateResolver {
    async fn resolve(&self, _id: &str, mut base: StreamerConfig) -> Result<StreamerConfig> {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak.fetch_max(active, Ordering::AcqRel);
        let version = self.version.load(Ordering::Acquire);
        self.entered.notify_one();
        while self.blocked.load(Ordering::Acquire) {
            let ready = self.release.notified();
            if !self.blocked.load(Ordering::Acquire) {
                break;
            }
            ready.await;
        }
        self.active.fetch_sub(1, Ordering::AcqRel);
        self.completed_version.fetch_max(version, Ordering::AcqRel);
        base.offline_check_count = version;
        Ok(base)
    }
}

fn started(id: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
        download_id: "d".to_owned(),
        streamer_id: id.to_owned(),
        streamer_name: id.to_owned(),
        session_id: "s".to_owned(),
        engine_type: EngineType::Ffmpeg,
        cdn_host: "example.com".to_owned(),
        download_url: "https://example.com/media".to_owned(),
    })
}

fn terminal(id: &str) -> DownloadManagerEvent {
    DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
        download_id: "d".to_owned(),
        streamer_id: id.to_owned(),
        streamer_name: id.to_owned(),
        session_id: "s".to_owned(),
        total_bytes: 1,
        total_duration_secs: 1.0,
        total_segments: 1,
        file_path: None,
        engine_signal: EngineEndSignal::CleanDisconnect,
        stop_cause: None,
    })
}

#[tokio::test]
async fn stopped_and_cancelled_generations_settle_retained_feedback_without_restarting() {
    tokio::time::timeout(Duration::from_secs(5), async {
        for cancelled in [false, true] {
            let mut scheduler = scheduler(&["one"]).await;
            let feedback = scheduler.reliable_feedback();
            let metadata = scheduler.streamer_manager.get_streamer("one").unwrap();
            scheduler.add_streamer(metadata).await.unwrap();
            let actor = scheduler
                .supervisor
                .registry()
                .get_streamer("one")
                .unwrap()
                .clone();
            let (start, end) = feedback.reserve_attempt("one").unwrap();
            assert_eq!(
                feedback
                    .publish_reserved(&started("one"), start)
                    .await
                    .unwrap()
                    .unwrap(),
                FeedbackDisposition::Applied
            );
            if cancelled {
                actor.cancel();
            } else {
                actor.send_reliable(StreamerMessage::Stop).await.unwrap();
            }
            let completion = scheduler
                .supervisor
                .registry_mut()
                .join_next()
                .await
                .unwrap()
                .unwrap();
            // The actor has exited, but its generation has not been reaped yet.
            let receipt = feedback.publish_reserved(&terminal("one"), end);
            let action = scheduler.supervisor.handle_task_completion(completion);
            scheduler.handle_task_completion_action(action, true);
            feedback.update_targets(scheduler.supervisor.registry().streamer_handles_map());
            assert_eq!(
                receipt.await.unwrap().unwrap(),
                FeedbackDisposition::Retired
            );
            assert!(!scheduler.supervisor.has_pending_streamer_restart("one"));
            feedback.shutdown().await;
        }
    })
    .await
    .expect("terminal actor exits must settle retained feedback");
}

#[tokio::test]
async fn blocked_resolution_and_lagged_progress_do_not_lose_terminal_or_block_removal() {
    tokio::time::timeout(Duration::from_secs(6), async {
        let mut scheduler = scheduler(&["one", "removed"]).await;
        for metadata in scheduler.streamer_manager.get_all() {
            scheduler.add_streamer(metadata).await.unwrap();
        }
        let feedback = scheduler.reliable_feedback();
        let resolver = Arc::new(GateResolver::new());
        scheduler.config_resolver = Some(resolver.clone());
        let (progress, receiver) = broadcast::channel(4);
        scheduler.set_download_receiver(receiver);
        let configuration_events = scheduler.event_broadcaster.clone();
        let handle = scheduler.handle();
        let cancel = scheduler.cancellation_token.clone();
        let runner = AbortOnDropHandle::new(tokio::spawn(async move { scheduler.run().await }));
        resolver.entered.notified().await;
        let (start, end) = feedback.reserve_attempt("one").unwrap();
        assert_eq!(
            feedback
                .publish_reserved(&started("one"), start)
                .await
                .unwrap()
                .unwrap(),
            FeedbackDisposition::Applied
        );
        resolver.version.store(9, Ordering::Release);
        for _ in 0..300 {
            configuration_events.publish(ConfigUpdateEvent::GlobalUpdated);
            progress
                .send(DownloadManagerEvent::Progress(
                    DownloadProgressEvent::Progress {
                        download_id: "d".to_owned(),
                        streamer_id: "one".to_owned(),
                        streamer_name: "one".to_owned(),
                        session_id: "s".to_owned(),
                        status: DownloadStatus::Downloading,
                        progress: DownloadProgress::default(),
                    },
                ))
                .unwrap();
        }
        assert_eq!(
            feedback
                .publish_reserved(&terminal("one"), end)
                .await
                .unwrap()
                .unwrap(),
            FeedbackDisposition::Applied
        );
        let removal = tokio::time::timeout(
            Duration::from_secs(1),
            handle.remove_streamer_awaitable("removed"),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(removal.generation().is_some());
        resolver.unblock();
        while resolver.completed_version.load(Ordering::Acquire) < 9 {
            tokio::task::yield_now().await;
        }
        cancel.cancel();
        runner.await.unwrap().unwrap();
    })
    .await
    .expect("slow configuration must not stall lifecycle/control delivery");
}

#[tokio::test]
async fn resolution_is_bounded_and_stale_revisions_cannot_replace_the_latest_config() {
    tokio::time::timeout(Duration::from_secs(6), async {
        let ids: Vec<_> = (0..20).map(|n| format!("streamer-{n}")).collect();
        let refs: Vec<_> = ids.iter().map(String::as_str).collect();
        let mut scheduler = scheduler(&refs).await;
        let resolver = Arc::new(GateResolver::new());
        scheduler.config_resolver = Some(resolver.clone());
        scheduler.queue_reconciliation();
        scheduler.pump_configuration();
        while resolver.active.load(Ordering::Acquire) < 8 {
            tokio::task::yield_now().await;
        }
        assert_eq!(resolver.peak.load(Ordering::Acquire), 8);
        resolver.version.store(9, Ordering::Release);
        scheduler.queue_reconciliation();
        resolver.unblock();
        scheduler.drain_configuration().await;
        assert_eq!(scheduler.supervisor.registry().streamer_count(), 20);
        for id in &ids {
            assert_eq!(
                scheduler
                    .supervisor
                    .streamer_restart_config(id)
                    .unwrap()
                    .offline_check_count,
                9
            );
        }
        assert!(resolver.peak.load(Ordering::Acquire) <= 8);
        scheduler.feedback.shutdown().await;
        scheduler.shutdown().await;
    })
    .await
    .expect("bounded config work must converge to the newest revision");
}

#[tokio::test]
async fn replacement_generation_and_state_sync_invalidate_inflight_configuration() {
    tokio::time::timeout(Duration::from_secs(6), async {
        let mut scheduler = scheduler(&["one", "disabled"]).await;
        for metadata in scheduler.streamer_manager.get_all() {
            scheduler.add_streamer(metadata).await.unwrap();
        }
        let old = scheduler
            .supervisor
            .registry()
            .get_streamer("one")
            .unwrap()
            .generation();
        let resolver = Arc::new(GateResolver::new());
        scheduler.config_resolver = Some(resolver.clone());
        scheduler.queue_reconciliation();
        scheduler.pump_configuration();
        resolver.entered.notified().await;
        scheduler.supervisor.remove_streamer("one");
        scheduler
            .supervisor
            .spawn_streamer("one", StreamerConfig::default(), None)
            .unwrap();
        assert_ne!(
            scheduler
                .supervisor
                .registry()
                .get_streamer("one")
                .unwrap()
                .generation(),
            old
        );
        scheduler
            .streamer_manager
            .metadata_store()
            .get_mut("disabled")
            .unwrap()
            .state = crate::domain::StreamerState::Disabled;
        scheduler.queue_configuration(ConfigUpdateEvent::StreamerStateSyncedFromDb {
            streamer_id: "disabled".to_owned(),
            is_active: true,
        });
        resolver.version.store(7, Ordering::Release);
        resolver.unblock();
        scheduler.drain_configuration().await;
        assert!(!scheduler.supervisor.registry().has_streamer("disabled"));
        assert_eq!(
            scheduler
                .supervisor
                .streamer_restart_config("one")
                .unwrap()
                .offline_check_count,
            7
        );
        scheduler.feedback.shutdown().await;
        scheduler.shutdown().await;
    })
    .await
    .expect("replacement and current metadata must fence old resolver results");
}

#[tokio::test]
async fn replacement_seeds_the_recording_identity_even_when_metadata_has_not_caught_up() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut scheduler = scheduler(&["one"]).await;
        let feedback = scheduler.reliable_feedback();
        scheduler
            .add_streamer(scheduler.streamer_manager.get_streamer("one").unwrap())
            .await
            .unwrap();
        let (start, end) = feedback.reserve_attempt("one").unwrap();
        assert_eq!(
            feedback
                .publish_reserved(&started("one"), start)
                .await
                .unwrap()
                .unwrap(),
            FeedbackDisposition::Applied
        );
        let old_generation = scheduler
            .supervisor
            .registry()
            .get_streamer("one")
            .unwrap()
            .generation();
        scheduler.supervisor.remove_streamer("one");
        let replacement = scheduler
            .supervisor
            .spawn_streamer("one", StreamerConfig::default(), None)
            .unwrap();
        assert_ne!(replacement.generation(), old_generation);
        feedback.update_targets(scheduler.supervisor.registry().streamer_handles_map());
        let (reply, state) = tokio::sync::oneshot::channel();
        replacement
            .send_reliable(StreamerMessage::GetState(reply))
            .await
            .unwrap();
        assert_eq!(
            state.await.unwrap().streamer_state,
            crate::domain::StreamerState::Live
        );
        assert_eq!(
            feedback
                .publish_reserved(&terminal("one"), end)
                .await
                .unwrap()
                .unwrap(),
            FeedbackDisposition::Applied
        );
        feedback.shutdown().await;
        scheduler.shutdown().await;
    })
    .await
    .expect("replacement must retain actual recording identity");
}

#[tokio::test]
async fn reconciliation_retires_feedback_for_a_deleted_streamer_that_never_spawned() {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut scheduler = scheduler(&["one"]).await;
        let feedback = scheduler.reliable_feedback();
        let (start, end) = feedback.reserve_attempt("one").unwrap();
        let receipt = feedback.publish_reserved(&started("one"), start);
        scheduler.streamer_manager.metadata_store().remove("one");
        scheduler.queue_reconciliation();
        assert_eq!(
            receipt.await.unwrap().unwrap(),
            FeedbackDisposition::Retired
        );
        assert_eq!(
            feedback
                .publish_reserved(&terminal("one"), end)
                .await
                .unwrap()
                .unwrap(),
            FeedbackDisposition::Retired
        );
        assert!(!scheduler.supervisor.registry().has_streamer("one"));
        feedback.shutdown().await;
    })
    .await
    .expect("lag reconciliation must include retained feedback before actor creation");
}
