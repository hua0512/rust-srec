//! Operational policy for runtime events.

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::config::ConfigService;
use crate::danmu::DanmuService;
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::domain::StreamerState;
use crate::downloader::DownloadManager;
use crate::monitor::{MonitorEvent, StreamMonitor};
use crate::pipeline::PipelineManager;
use crate::scheduler::SchedulerHandle;
use crate::session::{SessionLifecycle, SessionTransition, TerminalCause};
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

use super::session_cancels::SessionCancelTokens;

mod download_pipeline;
mod retirement;

use download_pipeline::{StreamerLivePayload, run_live_download_pipeline};

pub(crate) use retirement::{INTERACTIVE_RETIREMENT, OBSERVE_RETIREMENT};

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;
type RuntimeStreamMonitor = StreamMonitor<
    SqlxStreamerRepository,
    SqlxFilterRepository,
    SqlxSessionRepository,
    SqlxConfigRepository,
>;

/// Whether [`RuntimeCoordinator::stop_streamer_work`] returns as soon as the
/// stops are requested or only once every download attempt has published its
/// terminal outcome.
#[derive(Clone, Copy)]
enum StopWait {
    /// For a disable, where no caller decides anything on the outcome and the
    /// stop must not block the loop it runs on.
    Requested,
    /// For a retirement, where the caller is about to delete the `streamers`
    /// row and needs the attempts finalized first. Blocks for as long as the
    /// engine's graceful stop takes, up to `bound`.
    Finalized { bound: std::time::Duration },
}

/// What [`RuntimeCoordinator::stop_streamer_work`] left behind.
#[derive(Default)]
struct StreamerWorkStopped {
    /// Owners that could not be stopped, phrased for a caller's log line. A
    /// download that finalized on its own between the snapshot and the stop is
    /// not one of them.
    failures: Vec<String>,
    /// The session `SessionLifecycle::end_for_disable` resolved, if there was
    /// one. It is the only session that can still gain pipeline work for the
    /// streamer, so it is what a retirement drains.
    session_id: Option<String>,
    /// Whether this call is the one that moved [`Self::session_id`] out of
    /// `Recording`/`Hysteresis`, as opposed to finding it already `Ended` and
    /// only retro-correcting its terminal cause.
    ///
    /// A caller that just ended a session cannot conclude anything from
    /// `PipelineManager::drain_for_session` yet: the
    /// `SessionTransition::Ended` this published still has to reach
    /// `PipelineCoordinator` before the session-complete DAG it owes is
    /// observable.
    session_ended_now: bool,
}

/// Coordinates required side effects for configuration, monitor, and session events.
pub(crate) struct RuntimeCoordinator {
    download_manager: Arc<DownloadManager>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<RuntimeConfigService>,
    danmu_service: Arc<DanmuService>,
    stream_monitor: Arc<RuntimeStreamMonitor>,
    session_repository: Arc<SqlxSessionRepository>,
    session_cancels: Arc<SessionCancelTokens>,
    /// Streamer ids with a `run_live_download_pipeline` task in flight.
    /// The pipeline inserts its streamer id before its first await (the
    /// per-streamer dedup that stops two concurrent `StreamerLive` events
    /// from both reaching `start_with_slot`) and removes it via
    /// `PipelineReservationGuard` on every exit path. Keyed by streamer
    /// id, unlike `session_cancels`, which is keyed by session id.
    pending_pipelines: Arc<DashMap<String, ()>>,
    pipeline_manager: Arc<PipelineManager>,
    session_lifecycle: Arc<SessionLifecycle>,
    task_supervisor: Arc<TaskSupervisor>,
    /// Reaches the scheduler's event loop for `RemoveStreamerAwaitable`, the
    /// only way to observe a `StreamerActor`'s task actually leaving the runtime
    /// from off that loop.
    scheduler_handle: SchedulerHandle,
}

pub(super) struct RuntimeCoordinatorDependencies {
    pub download_manager: Arc<DownloadManager>,
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    pub config_service: Arc<RuntimeConfigService>,
    pub danmu_service: Arc<DanmuService>,
    pub stream_monitor: Arc<RuntimeStreamMonitor>,
    pub session_repository: Arc<SqlxSessionRepository>,
    pub session_cancels: Arc<SessionCancelTokens>,
    pub pending_pipelines: Arc<DashMap<String, ()>>,
    pub pipeline_manager: Arc<PipelineManager>,
    pub session_lifecycle: Arc<SessionLifecycle>,
    pub task_supervisor: Arc<TaskSupervisor>,
    pub scheduler_handle: SchedulerHandle,
}

impl RuntimeCoordinator {
    pub(super) fn new(dependencies: RuntimeCoordinatorDependencies) -> Self {
        let RuntimeCoordinatorDependencies {
            download_manager,
            streamer_manager,
            config_service,
            danmu_service,
            stream_monitor,
            session_repository,
            session_cancels,
            pending_pipelines,
            pipeline_manager,
            session_lifecycle,
            task_supervisor,
            scheduler_handle,
        } = dependencies;
        Self {
            download_manager,
            streamer_manager,
            config_service,
            danmu_service,
            stream_monitor,
            session_repository,
            session_cancels,
            pending_pipelines,
            pipeline_manager,
            session_lifecycle,
            task_supervisor,
            scheduler_handle,
        }
    }

    pub(crate) async fn refresh_metadata_offline_check(&self, streamer_id: &str) {
        match self
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
        {
            Ok(merged) => self
                .streamer_manager
                .apply_resolved_config(streamer_id, &merged),
            Err(error) => debug!(
                streamer_id,
                error = %error,
                "Skipping resolved scheduler configuration refresh"
            ),
        }
    }

    pub(crate) async fn handle_streamer_disabled(&self, streamer_id: &str) {
        // `StreamerManager` is the only source of the name here, and there is
        // no useful session-row fallback behind it. It drops a cache entry
        // only once the `streamers` row is gone (`reap_deleted`, or
        // `reload_from_repo` seeing `NotFound`), and by then the
        // `ON DELETE SET NULL` foreign key has cleared
        // `live_sessions.streamer_id` and `trg_live_session_orphan_ends` has
        // stamped `end_time`. `SessionLifecycle::end_for_disable` reads this
        // name only on the branch where the session is still active, which is
        // exactly the branch on which this lookup hits.
        let streamer_name = self
            .streamer_manager
            .get_streamer(streamer_id)
            .map(|metadata| metadata.name)
            .unwrap_or_default();

        // Every failure is already logged; a disable has no caller waiting to
        // decide anything on the outcome.
        let _ = self
            .stop_streamer_work(streamer_id, &streamer_name, StopWait::Requested)
            .await;
    }

    /// Stop everything bound to `streamer_id`: its queued and running download
    /// attempts, its danmu collection and its open session.
    ///
    /// Ordering matters. The session cancel tokens go first, because a
    /// `run_live_download_pipeline` task past `SessionCancelTokens::register`
    /// but not yet queued appears in neither `DownloadManager::snapshot_pending`
    /// nor `get_active_downloads`, and its `cancel.is_cancelled()` checks are
    /// the only thing that stops it before `acquire_slot`. The session end runs
    /// last, so `SessionLifecycle::end_for_disable` writes `end_time` after the
    /// attempts that would otherwise reopen it have finalized.
    ///
    /// Ending through `SessionLifecycle` rather than letting the reap's
    /// `ON DELETE SET NULL` and `trg_live_session_orphan_ends` stamp `end_time`
    /// is what keeps the `session_ended` row, the `SessionTransition::Ended`
    /// broadcast, the in-memory `Recording` -> `Ended` transition and
    /// `schedule_ended_eviction`.
    async fn stop_streamer_work(
        &self,
        streamer_id: &str,
        streamer_name: &str,
        wait: StopWait,
    ) -> StreamerWorkStopped {
        let mut stopped = StreamerWorkStopped::default();

        if let Some(session_id) = self
            .session_lifecycle
            .current_session_id_for_streamer(streamer_id)
        {
            self.session_cancels.cancel(&session_id);
            debug!(
                streamer_id,
                session_id, "Cancelled current session token for stopped streamer"
            );
        }

        // A queued attempt waiting on a download slot is not in
        // `get_active_downloads`, and would otherwise start after the caller has
        // stood the streamer down.
        for pending in self.download_manager.snapshot_pending() {
            if pending.streamer_id == streamer_id {
                self.session_cancels.cancel(&pending.session_id);
                info!(
                    streamer_id,
                    session_id = %pending.session_id,
                    "Cancelled queued download for stopped streamer"
                );
            }
        }

        let downloads: Vec<_> = self
            .download_manager
            .get_active_downloads()
            .into_iter()
            .filter(|download| download.streamer_id == streamer_id)
            .collect();

        for download in downloads {
            let (result, outcome) = match wait {
                StopWait::Requested => (
                    self.download_manager.request_stop_download(
                        &download.id,
                        crate::downloader::DownloadStopCause::StreamerDisabled,
                    ),
                    "Requested download stop for stopped streamer",
                ),
                StopWait::Finalized { bound } => (
                    match tokio::time::timeout(
                        bound,
                        self.download_manager.stop_download_with_reason(
                            &download.id,
                            crate::downloader::DownloadStopCause::StreamerDisabled,
                        ),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(crate::Error::Other(format!(
                            "download did not finalize within {} seconds",
                            bound.as_secs()
                        ))),
                    },
                    "Finalized download for retired streamer",
                ),
            };
            match result {
                Ok(()) => info!(
                    download_id = %download.id,
                    streamer_id,
                    "{outcome}"
                ),
                // The attempt finalized between `get_active_downloads` and the
                // stop, so it holds nothing.
                Err(crate::Error::NotFound { .. }) => debug!(
                    download_id = %download.id,
                    streamer_id,
                    "Download already finalized before the stop request"
                ),
                Err(error) => {
                    warn!(
                        download_id = %download.id,
                        streamer_id,
                        error = %error,
                        "Failed to stop download for stopped streamer"
                    );
                    stopped
                        .failures
                        .push(format!("download {}: {error}", download.id));
                }
            }
        }

        if let Some(session_id) = self.danmu_service.get_session_by_streamer(streamer_id) {
            match self.danmu_service.stop_collection(&session_id).await {
                Ok(stats) => info!(
                    streamer_id,
                    session_id,
                    messages = stats.total_count,
                    "Stopped danmu collection for stopped streamer"
                ),
                // A collector that released its `collections` entry between
                // `get_session_by_streamer` and the stop reports an error but
                // holds nothing, so it cannot keep writing danmu rows.
                Err(_) if !self.danmu_service.is_collecting(&session_id) => debug!(
                    streamer_id,
                    session_id, "Danmu collection already released its slot before the stop"
                ),
                Err(error) => {
                    warn!(
                        streamer_id,
                        session_id,
                        error = %error,
                        "Failed to stop danmu collection for stopped streamer"
                    );
                    stopped
                        .failures
                        .push(format!("danmu collection {session_id}: {error}"));
                }
            }
        }

        // Read before the end so the result can be attributed: an in-memory
        // session that is already `Ended` sends `end_for_disable` down its
        // retro-correction path, which returns the same session id without
        // moving anything. No in-memory entry at all means the repository is
        // about to close a row that was left `end_time IS NULL`, which is a real
        // end.
        let was_recording = self
            .session_lifecycle
            .current_session_id_for_streamer(streamer_id)
            .is_none()
            || self
                .session_lifecycle
                .has_active_session_for_streamer(streamer_id);

        match self
            .session_lifecycle
            .end_for_disable(streamer_id, streamer_name)
            .await
        {
            Ok(session_id) => {
                stopped.session_ended_now = session_id.is_some() && was_recording;
                stopped.session_id = session_id;
            }
            Err(error) => {
                warn!(
                    streamer_id,
                    error = %error,
                    "Failed to end stopped streamer's session"
                );
                stopped.failures.push(format!("session: {error}"));
            }
        }

        stopped
    }

    pub(crate) async fn handle_monitor_event(
        self: &Arc<Self>,
        event: MonitorEvent,
        from_hysteresis_resume: bool,
    ) {
        match event {
            MonitorEvent::StreamerLive {
                streamer_id,
                session_id,
                streamer_name,
                title,
                streams,
                streamer_url,
                media_headers,
                media_extras,
                ..
            } => {
                info!(
                    streamer_id,
                    streamer_name,
                    title,
                    stream_count = streams.len(),
                    media_header_count = media_headers.as_ref().map_or(0, |value| value.len()),
                    media_extra_count = media_extras.as_ref().map_or(0, |value| value.len()),
                    "Streamer went live"
                );

                let coordinator = self.clone();
                self.task_supervisor
                    .spawn("live download pipeline", async move {
                        run_live_download_pipeline(
                            coordinator,
                            StreamerLivePayload {
                                streamer_id,
                                session_id,
                                streamer_name,
                                title,
                                streams,
                                streamer_url,
                                media_headers,
                                media_extras,
                            },
                            from_hysteresis_resume,
                        )
                        .await;
                    });
            }
            MonitorEvent::StreamerOffline {
                streamer_id,
                streamer_name,
                session_id,
                ..
            } => {
                info!(streamer_id, streamer_name, "Streamer went offline");

                if let Some(session_id) = session_id.as_deref() {
                    self.session_cancels.cancel(session_id);
                }

                let danmu_session_id = session_id
                    .filter(|session_id| self.danmu_service.is_collecting(session_id))
                    .or_else(|| self.danmu_service.get_session_by_streamer(&streamer_id));
                if let Some(session_id) = danmu_session_id
                    && let Err(error) = self.danmu_service.stop_collection(&session_id).await
                {
                    warn!(
                        session_id,
                        error = %error,
                        "Failed to stop danmu collection for offline streamer"
                    );
                }

                if let Some(download) = self.download_manager.get_download_by_streamer(&streamer_id)
                    && let Err(error) = self.download_manager.request_stop_download(
                        &download.id,
                        crate::downloader::DownloadStopCause::StreamerOffline,
                    )
                {
                    warn!(
                        streamer_id,
                        download_id = %download.id,
                        error = %error,
                        "Failed to stop download for offline streamer"
                    );
                }
            }
            MonitorEvent::StateChanged {
                streamer_id,
                streamer_name,
                new_state: StreamerState::OutOfSchedule,
                reason,
                ..
            } if reason.as_deref() == Some("out_of_schedule") => {
                info!(
                    streamer_id,
                    streamer_name, "Streamer left its schedule window; stopping active work"
                );

                for pending in self.download_manager.snapshot_pending() {
                    if pending.streamer_id == streamer_id {
                        self.session_cancels.cancel(&pending.session_id);
                    }
                }

                if let Some(session_id) = self.danmu_service.get_session_by_streamer(&streamer_id)
                    && let Err(error) = self.danmu_service.stop_collection(&session_id).await
                {
                    warn!(
                        session_id,
                        error = %error,
                        "Failed to stop out-of-schedule danmu collection"
                    );
                }

                if let Some(download) = self.download_manager.get_download_by_streamer(&streamer_id)
                    && let Err(error) = self.download_manager.request_stop_download(
                        &download.id,
                        crate::downloader::DownloadStopCause::OutOfSchedule,
                    )
                {
                    warn!(
                        streamer_id,
                        download_id = %download.id,
                        error = %error,
                        "Failed to stop out-of-schedule download"
                    );
                }
            }
            _ => {}
        }
    }

    pub(crate) async fn handle_session_transition(self: &Arc<Self>, transition: SessionTransition) {
        if let SessionTransition::Ended { session_id, .. } = &transition {
            self.download_manager
                .clear_session_segment_index(session_id);
        }

        if let SessionTransition::Ended {
            session_id,
            cause: TerminalCause::Failed { .. },
            ..
        } = &transition
            && self.danmu_service.is_collecting(session_id)
            && let Err(error) = self.danmu_service.stop_collection(session_id).await
        {
            warn!(
                session_id,
                error = %error,
                "Failed to stop danmu collection after download failure"
            );
        }

        if let SessionTransition::Started {
            from_hysteresis: true,
            download_start: Some(payload),
            session_id,
            streamer_id,
            streamer_name,
            title,
            category,
            started_at,
            ..
        } = &transition
        {
            if self.session_lifecycle.is_session_active(session_id) {
                info!(
                    streamer_id,
                    session_id,
                    streamer_name,
                    "Session resumed from hysteresis; restarting download"
                );
                self.handle_monitor_event(
                    MonitorEvent::StreamerLive {
                        streamer_id: streamer_id.clone(),
                        session_id: session_id.clone(),
                        streamer_name: streamer_name.clone(),
                        streamer_url: payload.streamer_url.clone(),
                        title: title.clone(),
                        category: category.clone(),
                        streams: payload.streams.clone(),
                        media_headers: payload.media_headers.clone(),
                        media_extras: payload.media_extras.clone(),
                        timestamp: started_at.to_owned(),
                    },
                    true,
                )
                .await;
            } else {
                debug!(
                    session_id,
                    streamer_id, "Session no longer active; skipping resumed download"
                );
            }
        }

        self.pipeline_manager
            .handle_session_transition(transition)
            .await;
    }
}
