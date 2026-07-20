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
use crate::session::{SessionLifecycle, SessionTransition, TerminalCause};
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

use super::session_cancels::SessionCancelTokens;

mod download_pipeline;

use download_pipeline::{StreamerLivePayload, run_live_download_pipeline};

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;
type RuntimeStreamMonitor = StreamMonitor<
    SqlxStreamerRepository,
    SqlxFilterRepository,
    SqlxSessionRepository,
    SqlxConfigRepository,
>;

/// Coordinates required side effects for configuration, monitor, and session events.
pub(crate) struct RuntimeCoordinator {
    download_manager: Arc<DownloadManager>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<RuntimeConfigService>,
    danmu_service: Arc<DanmuService>,
    stream_monitor: Arc<RuntimeStreamMonitor>,
    session_repository: Arc<SqlxSessionRepository>,
    session_cancels: Arc<SessionCancelTokens>,
    pending_pipelines: Arc<DashMap<String, ()>>,
    pipeline_manager: Arc<PipelineManager>,
    session_lifecycle: Arc<SessionLifecycle>,
    task_supervisor: Arc<TaskSupervisor>,
}

impl RuntimeCoordinator {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        download_manager: Arc<DownloadManager>,
        streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
        config_service: Arc<RuntimeConfigService>,
        danmu_service: Arc<DanmuService>,
        stream_monitor: Arc<RuntimeStreamMonitor>,
        session_repository: Arc<SqlxSessionRepository>,
        session_cancels: Arc<SessionCancelTokens>,
        pending_pipelines: Arc<DashMap<String, ()>>,
        pipeline_manager: Arc<PipelineManager>,
        session_lifecycle: Arc<SessionLifecycle>,
        task_supervisor: Arc<TaskSupervisor>,
    ) -> Self {
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
        let downloads: Vec<_> = self
            .download_manager
            .get_active_downloads()
            .into_iter()
            .filter(|download| download.streamer_id == streamer_id)
            .collect();

        for download in downloads {
            match self
                .download_manager
                .stop_download_with_reason(
                    &download.id,
                    crate::downloader::DownloadStopCause::StreamerDisabled,
                )
                .await
            {
                Ok(()) => info!(
                    download_id = %download.id,
                    streamer_id,
                    "Cancelled download for disabled streamer"
                ),
                Err(error) => warn!(
                    download_id = %download.id,
                    streamer_id,
                    error = %error,
                    "Failed to cancel download for disabled streamer"
                ),
            }
        }

        if let Some(session_id) = self.danmu_service.get_session_by_streamer(streamer_id) {
            match self.danmu_service.stop_collection(&session_id).await {
                Ok(stats) => info!(
                    streamer_id,
                    session_id,
                    messages = stats.total_count,
                    "Stopped danmu collection for disabled streamer"
                ),
                Err(error) => warn!(
                    streamer_id,
                    session_id,
                    error = %error,
                    "Failed to stop danmu collection for disabled streamer"
                ),
            }
        }

        let streamer_name = self
            .streamer_manager
            .get_streamer(streamer_id)
            .map(|metadata| metadata.name.clone())
            .unwrap_or_default();
        if let Err(error) = self
            .session_lifecycle
            .end_for_disable(streamer_id, &streamer_name)
            .await
        {
            warn!(
                streamer_id,
                error = %error,
                "Failed to end disabled streamer's session"
            );
        }
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
                            coordinator.download_manager.clone(),
                            coordinator.streamer_manager.clone(),
                            coordinator.config_service.clone(),
                            coordinator.danmu_service.clone(),
                            coordinator.stream_monitor.clone(),
                            coordinator.session_repository.clone(),
                            coordinator.session_cancels.clone(),
                            coordinator.pending_pipelines.clone(),
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
                    && let Err(error) = self
                        .download_manager
                        .stop_download_with_reason(
                            &download.id,
                            crate::downloader::DownloadStopCause::StreamerOffline,
                        )
                        .await
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
                    && let Err(error) = self
                        .download_manager
                        .stop_download_with_reason(
                            &download.id,
                            crate::downloader::DownloadStopCause::OutOfSchedule,
                        )
                        .await
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
