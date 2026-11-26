//! Asynchronous side-effect execution for `StreamerActor`.
//!
//! All I/O, segment persistence, danmu segment commands, and pipeline
//! notifications run off the actor's `select!` loop so a slow database commit,
//! engine stop, or danmu command can never stall actor responsiveness.

use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{debug, warn};

use crate::danmu::{CollectionStopReason, DanmuService};
use crate::downloader::{DownloadManager, DownloadStopCause};
use crate::recorder::actor::{ActorMessage, ActorRuntimeDeps};
use crate::recorder::fsm::{FsmEffect, FsmInput};
use crate::recorder::ports::{
    ActorStopReason, DanmuCloseSegmentSpec, DanmuSegmentRecord, DanmuStopKind, DanmuStopResult,
    DbCommitOp, PipelineNotification, ProbeRequest, RecorderPipelinePort, StreamerRuntimeView,
    VideoSegmentRecord,
};

use super::startup_task::{StartupTaskDeps, spawn_startup_task};

/// Effects that stay owned directly by the actor's `select!` loop.
#[derive(Default)]
pub(super) struct LoopEffectSignals {
    pub spawn_probe: Option<ProbeRequest>,
    pub cancel_probe: Option<u64>,
    pub commit_db: Option<(u64, DbCommitOp)>,
    pub stopped: Option<ActorStopReason>,
}

/// Dispatch effects produced by a regular (non-acked) [`FsmInput`].
pub(super) fn dispatch_effects(
    streamer_id: &str,
    effects: Vec<FsmEffect>,
    deps: &ActorRuntimeDeps,
    actor_tx: &mpsc::UnboundedSender<ActorMessage>,
    effects_tasks: &mut JoinSet<()>,
) -> LoopEffectSignals {
    let mut signals = LoopEffectSignals::default();
    let mut deferred_pipeline: Vec<PipelineNotification> = Vec::new();
    let mut deferred_deletes: Vec<(Option<String>, Option<String>)> = Vec::new();
    let mut deferred_remove_danmu: Vec<(String, String)> = Vec::new();
    let mut deferred_video: Vec<VideoSegmentRecord> = Vec::new();
    let mut deferred_danmu: Vec<DanmuSegmentRecord> = Vec::new();
    let mut deferred_close_danmu: Vec<DanmuCloseSegmentSpec> = Vec::new();
    let mut deferred_start_danmu: Vec<(String, u32, String, jiff::Timestamp)> = Vec::new();

    for effect in effects {
        match effect {
            FsmEffect::SpawnProbe(req) => {
                signals.spawn_probe = Some(req);
            }
            FsmEffect::CancelProbe { probe_id } => {
                signals.cancel_probe = Some(probe_id);
            }
            FsmEffect::CommitDb { op_id, op } => {
                signals.commit_db = Some((op_id, op));
            }
            FsmEffect::StartAttempt(spec) => {
                spawn_startup_task(deps.startup_deps(), *spec, actor_tx.clone());
            }
            FsmEffect::StopAttempt {
                attempt_id,
                download_id,
                cause,
            } => {
                stop_attempt_nonblocking(streamer_id, attempt_id, download_id, cause, deps);
            }
            FsmEffect::StopDanmu { session_id, reason } => {
                spawn_stop_danmu(
                    session_id,
                    reason,
                    deps.danmu_service.clone(),
                    actor_tx.clone(),
                    effects_tasks,
                );
            }
            FsmEffect::StartDanmuSegment {
                session_id,
                segment_index,
                video_segment_path,
                started_at,
            } => {
                deferred_start_danmu.push((
                    session_id,
                    segment_index,
                    video_segment_path,
                    started_at,
                ));
            }
            FsmEffect::CloseDanmuSegment(spec) => {
                deferred_close_danmu.push(spec);
            }
            FsmEffect::DeleteDiscardedFiles {
                video_path,
                danmu_path,
            } => {
                deferred_deletes.push((video_path, danmu_path));
            }
            FsmEffect::RemoveDanmuSegmentOutput {
                session_id,
                danmu_path,
            } => {
                deferred_remove_danmu.push((session_id, danmu_path));
            }
            FsmEffect::PersistVideoSegment(record) => {
                deferred_video.push(record);
            }
            FsmEffect::PersistDanmuSegment(record) => {
                deferred_danmu.push(record);
            }
            FsmEffect::NotifyPipeline(notification) => {
                deferred_pipeline.push(notification);
            }
            FsmEffect::PublishRuntimeView(view) => {
                publish_runtime_view(&deps.runtime_views, view);
            }
            FsmEffect::ActorStopped { reason } => {
                signals.stopped = Some(reason);
            }
        }
    }

    let has_async_io = !deferred_start_danmu.is_empty()
        || !deferred_close_danmu.is_empty()
        || !deferred_deletes.is_empty()
        || !deferred_remove_danmu.is_empty()
        || !deferred_video.is_empty()
        || !deferred_danmu.is_empty()
        || !deferred_pipeline.is_empty();

    if has_async_io {
        let store = deps.store.clone();
        let danmu_service = deps.danmu_service.clone();
        let pipeline = deps.pipeline.clone();
        let download_manager = deps.download_manager.clone();
        effects_tasks.spawn(async move {
            for (session_id, segment_index, video_segment_path, started_at) in deferred_start_danmu
            {
                start_danmu_segment(
                    &danmu_service,
                    &session_id,
                    segment_index,
                    &video_segment_path,
                    started_at,
                )
                .await;
            }
            for spec in deferred_close_danmu {
                close_danmu_segment(&danmu_service, &spec).await;
            }
            for (video_path, danmu_path) in deferred_deletes {
                delete_discarded_files(video_path.as_deref(), danmu_path.as_deref()).await;
            }
            for (session_id, danmu_path) in deferred_remove_danmu {
                if let Err(error) = store
                    .remove_danmu_segment_output(&session_id, &danmu_path)
                    .await
                {
                    warn!(%session_id, %danmu_path, %error, "Failed to remove discarded danmu output");
                }
            }
            for record in deferred_video {
                if let Err(error) = store.persist_video_segment(record).await {
                    warn!(%error, "Failed to persist video segment");
                }
            }
            for record in deferred_danmu {
                if let Err(error) = store.persist_danmu_segment(record).await {
                    warn!(%error, "Failed to persist danmu segment");
                }
            }
            for notification in deferred_pipeline {
                deliver_pipeline_notification(pipeline.as_ref(), &download_manager, notification)
                    .await;
            }
        });
    }

    signals
}

/// Dispatch effects produced by [`FsmInput::SegmentStarted`], acknowledging `ack`
/// once the segment start side effects finish.
pub(super) fn dispatch_segment_started_effects(
    streamer_id: &str,
    effects: Vec<FsmEffect>,
    ack: oneshot::Sender<std::result::Result<(), String>>,
    deps: &ActorRuntimeDeps,
    actor_tx: &mpsc::UnboundedSender<ActorMessage>,
    effects_tasks: &mut JoinSet<()>,
) -> LoopEffectSignals {
    let mut start_specs = Vec::new();
    let mut remaining = Vec::new();
    for effect in effects {
        match effect {
            FsmEffect::StartDanmuSegment {
                session_id,
                segment_index,
                video_segment_path,
                started_at,
            } => start_specs.push((session_id, segment_index, video_segment_path, started_at)),
            other => remaining.push(other),
        }
    }

    let signals = dispatch_effects(streamer_id, remaining, deps, actor_tx, effects_tasks);
    if start_specs.is_empty() {
        let _ = ack.send(Ok(()));
        return signals;
    }

    let danmu_service = deps.danmu_service.clone();
    effects_tasks.spawn(async move {
        for (session_id, segment_index, video_segment_path, started_at) in start_specs {
            start_danmu_segment(
                &danmu_service,
                &session_id,
                segment_index,
                &video_segment_path,
                started_at,
            )
            .await;
        }
        let _ = ack.send(Ok(()));
    });
    signals
}

/// Dispatch effects produced by [`FsmInput::SegmentCompleted`] in strict receipt
/// order:
/// - For discarded segments: `CloseDanmuSegment(discard=true)` -> delete files ->
///   reverse-clean any pre-persisted danmu row -> `ack(Ok(()))`.
/// - For kept segments: `CloseDanmuSegment(discard=false)` -> `PersistVideoSegment`
///   -> on `Err`, `ack(Err(...))` without notifying pipeline -> on `Ok`,
///   `NotifyPipeline(VideoSegmentCompleted)` -> `ack(Ok(()))`.
pub(super) fn dispatch_segment_completed_effects(
    streamer_id: &str,
    effects: Vec<FsmEffect>,
    ack: oneshot::Sender<std::result::Result<(), String>>,
    deps: &ActorRuntimeDeps,
    actor_tx: &mpsc::UnboundedSender<ActorMessage>,
    effects_tasks: &mut JoinSet<()>,
) -> LoopEffectSignals {
    let mut close_danmu = Vec::new();
    let mut deletes = Vec::new();
    let mut remove_danmu = Vec::new();
    let mut persist_video = Vec::new();
    let mut pipeline_notifs = Vec::new();
    let mut remaining = Vec::new();

    for effect in effects {
        match effect {
            FsmEffect::CloseDanmuSegment(spec) => close_danmu.push(spec),
            FsmEffect::DeleteDiscardedFiles {
                video_path,
                danmu_path,
            } => deletes.push((video_path, danmu_path)),
            FsmEffect::RemoveDanmuSegmentOutput {
                session_id,
                danmu_path,
            } => remove_danmu.push((session_id, danmu_path)),
            FsmEffect::PersistVideoSegment(record) => persist_video.push(record),
            FsmEffect::NotifyPipeline(notification) => pipeline_notifs.push(notification),
            other => remaining.push(other),
        }
    }

    let signals = dispatch_effects(streamer_id, remaining, deps, actor_tx, effects_tasks);
    let store = deps.store.clone();
    let danmu_service = deps.danmu_service.clone();
    let pipeline = deps.pipeline.clone();
    let download_manager = deps.download_manager.clone();

    effects_tasks.spawn(async move {
        for spec in close_danmu {
            close_danmu_segment(&danmu_service, &spec).await;
        }
        for (video_path, danmu_path) in deletes {
            delete_discarded_files(video_path.as_deref(), danmu_path.as_deref()).await;
        }
        for (session_id, danmu_path) in remove_danmu {
            if let Err(error) = store
                .remove_danmu_segment_output(&session_id, &danmu_path)
                .await
            {
                warn!(%session_id, %danmu_path, %error, "Failed to remove discarded danmu output");
            }
        }
        for record in persist_video {
            if let Err(error) = store.persist_video_segment(record).await {
                let _ = ack.send(Err(format!("failed to persist video segment: {error}")));
                return;
            }
        }
        for notification in pipeline_notifs {
            if let Err(error) = deliver_pipeline_notification_result(
                pipeline.as_ref(),
                &download_manager,
                notification,
            )
            .await
            {
                let _ = ack.send(Err(format!(
                    "failed to apply segment notification to pipeline: {error}"
                )));
                return;
            }
        }
        let _ = ack.send(Ok(()));
    });
    signals
}

/// Dispatch effects produced by a FIFO danmu coordination event and resolve
/// `done` strictly after persistence and pipeline notification settle.
pub(super) fn dispatch_danmu_fifo_effects(
    streamer_id: &str,
    effects: Vec<FsmEffect>,
    done: oneshot::Sender<std::result::Result<(), String>>,
    deps: &ActorRuntimeDeps,
    actor_tx: &mpsc::UnboundedSender<ActorMessage>,
    effects_tasks: &mut JoinSet<()>,
) -> LoopEffectSignals {
    let mut deletes = Vec::new();
    let mut persist_danmu = Vec::new();
    let mut pipeline_notifs = Vec::new();
    let mut remaining = Vec::new();

    for effect in effects {
        match effect {
            FsmEffect::DeleteDiscardedFiles {
                video_path,
                danmu_path,
            } => deletes.push((video_path, danmu_path)),
            FsmEffect::PersistDanmuSegment(record) => persist_danmu.push(record),
            FsmEffect::NotifyPipeline(notification) => pipeline_notifs.push(notification),
            other => remaining.push(other),
        }
    }

    let signals = dispatch_effects(streamer_id, remaining, deps, actor_tx, effects_tasks);
    if deletes.is_empty() && persist_danmu.is_empty() && pipeline_notifs.is_empty() {
        let _ = done.send(Ok(()));
        return signals;
    }

    let store = deps.store.clone();
    let pipeline = deps.pipeline.clone();
    let download_manager = deps.download_manager.clone();

    effects_tasks.spawn(async move {
        for (video_path, danmu_path) in deletes {
            delete_discarded_files(video_path.as_deref(), danmu_path.as_deref()).await;
        }
        for record in persist_danmu {
            if let Err(error) = store.persist_danmu_segment(record).await {
                let _ = done.send(Err(format!("failed to persist danmu segment: {error}")));
                return;
            }
        }
        for notification in pipeline_notifs {
            if let Err(error) = deliver_pipeline_notification_result(
                pipeline.as_ref(),
                &download_manager,
                notification,
            )
            .await
            {
                let _ = done.send(Err(format!(
                    "failed to apply danmu notification to pipeline: {error}"
                )));
                return;
            }
        }
        let _ = done.send(Ok(()));
    });
    signals
}

pub(super) fn publish_runtime_view(
    views: &DashMap<String, StreamerRuntimeView>,
    view: StreamerRuntimeView,
) {
    views.insert(view.streamer_id.clone(), view);
}

fn stop_attempt_nonblocking(
    streamer_id: &str,
    attempt_id: u64,
    download_id: Option<String>,
    cause: DownloadStopCause,
    deps: &ActorRuntimeDeps,
) {
    deps.leases.cancel_attempt(streamer_id, attempt_id);
    if let Some(download_id) = download_id {
        if let Err(error) = deps
            .download_manager
            .request_stop_download(&download_id, cause)
        {
            debug!(%streamer_id, %download_id, %error, "Requested stop for download already gone");
        }
    } else {
        // Attempt may have transitioned from startup to active concurrently;
        // request stop on any active download owned by this streamer.
        for (active_id, _) in deps
            .download_manager
            .active_download_ids_for_streamer(streamer_id)
        {
            if let Err(error) = deps
                .download_manager
                .request_stop_download(&active_id, cause.clone())
            {
                debug!(%streamer_id, download_id = %active_id, %error, "Requested stop for active download already gone");
            }
        }
    }
}

fn spawn_stop_danmu(
    session_id: String,
    reason: DanmuStopKind,
    danmu_service: Arc<DanmuService>,
    actor_tx: mpsc::UnboundedSender<ActorMessage>,
    effects_tasks: &mut JoinSet<()>,
) {
    let stop_reason = match reason {
        DanmuStopKind::SessionEnded => CollectionStopReason::SessionEnded,
        DanmuStopKind::ServiceShutdown => CollectionStopReason::ServiceShutdown,
    };
    effects_tasks.spawn(async move {
        let outcome = danmu_service
            .stop_collection_with_outcome(&session_id, stop_reason)
            .await;
        // When `Terminated`, the runner emits `DanmuEvent::CollectionStopped`
        // over the FIFO coordination channel after any final `SegmentCompleted`.
        // When `NotRegistered` or `Unconfirmed`, no FIFO completion is guaranteed,
        // so report directly to the actor so stopping can converge accurately.
        if !matches!(outcome, DanmuStopResult::Terminated) {
            let _ = actor_tx.send(ActorMessage::Input(FsmInput::DanmuCollectionStopped {
                session_id,
                total_count: 0,
                result: outcome,
            }));
        }
    });
}

pub(crate) async fn start_danmu_segment(
    danmu_service: &DanmuService,
    session_id: &str,
    segment_index: u32,
    video_segment_path: &str,
    started_at: jiff::Timestamp,
) {
    let Some(handle) = danmu_service.get_handle(session_id) else {
        return;
    };
    let segment_id = segment_index.to_string();
    let mut danmu_path = PathBuf::from(video_segment_path);
    danmu_path.set_extension("xml");
    if let Err(error) = handle
        .start_segment(&segment_id, danmu_path, started_at)
        .await
    {
        warn!(%session_id, segment_index, %error, "Failed to start danmu segment");
    }
}

pub(crate) async fn close_danmu_segment(
    danmu_service: &DanmuService,
    spec: &DanmuCloseSegmentSpec,
) {
    let Some(handle) = danmu_service.get_handle(&spec.session_id) else {
        return;
    };
    let segment_id = spec.segment_index.to_string();
    if let Err(error) = handle.close_segment(&segment_id, spec.discard).await {
        warn!(
            session_id = %spec.session_id,
            segment_index = spec.segment_index,
            discard = spec.discard,
            %error,
            "Failed to close danmu segment"
        );
    }
}

pub(crate) async fn delete_discarded_files(video_path: Option<&str>, danmu_path: Option<&str>) {
    if let Some(video_path) = video_path {
        match tokio::fs::remove_file(video_path).await {
            Ok(()) => debug!(video_path, "Deleted discarded video segment"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!(video_path, %error, "Failed to delete discarded video segment"),
        }
    }
    if let Some(danmu_path) = danmu_path {
        match tokio::fs::remove_file(danmu_path).await {
            Ok(()) => debug!(danmu_path, "Deleted discarded danmu segment"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!(danmu_path, %error, "Failed to delete discarded danmu segment"),
        }
    }
}

pub(crate) async fn deliver_pipeline_notification(
    pipeline: &dyn RecorderPipelinePort,
    download_manager: &DownloadManager,
    notification: PipelineNotification,
) {
    if let Err(error) =
        deliver_pipeline_notification_result(pipeline, download_manager, notification).await
    {
        warn!(%error, "Failed to deliver recorder pipeline notification");
    }
}

async fn deliver_pipeline_notification_result(
    pipeline: &dyn RecorderPipelinePort,
    download_manager: &DownloadManager,
    notification: PipelineNotification,
) -> crate::Result<()> {
    if let PipelineNotification::SessionEnded { ref session_id, .. } = notification {
        download_manager.clear_session_segment_index(session_id);
    }
    pipeline.notify(notification).await
}

impl ActorRuntimeDeps {
    pub(super) fn startup_deps(&self) -> StartupTaskDeps {
        StartupTaskDeps {
            download_manager: self.download_manager.clone(),
            danmu_service: self.danmu_service.clone(),
            streamer_manager: self.streamer_manager.clone(),
            config_service: self.config_service.clone(),
            store: self.store.clone(),
            prober: self.prober.clone(),
            leases: self.leases.clone(),
        }
    }
}
