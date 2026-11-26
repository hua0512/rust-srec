//! Adapters bridging `ConfigService` and database repositories to the
//! pipeline module's narrow ports.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::debug;

use crate::config::ConfigService;
use crate::database::models::{
    LiveSessionDbModel, MediaFileType, MediaOutputDbModel, Pagination, TitleEntry,
};
use crate::database::repositories::{ConfigRepository, SessionRepository, StreamerRepository};
use crate::pipeline::{
    PipelineError, PipelineManager, PipelineNotification, PipelineResult, PipelineSessionMeta,
    PipelineSessionPort, PipelineSessionSegment, PipelineSessionSummary, PipelineStreamerMeta,
    SessionPipelineConfig, SessionPipelineConfigPort,
};
use crate::recorder::{PipelineDrainStatus, RecorderPipelinePort};
use srec_core::CoreError;
use srec_recorder::{RecorderError, RecorderResult};

fn to_pipeline_error(error: RecorderError) -> PipelineError {
    match error {
        RecorderError::Core(error) => PipelineError::Core(error),
        RecorderError::Db(error) => PipelineError::from(error),
        other => PipelineError::other(other.to_string()),
    }
}

fn pipeline_to_recorder_error(error: PipelineError) -> RecorderError {
    match error {
        PipelineError::Core(error) => RecorderError::Core(error),
        PipelineError::Db(error) => RecorderError::from(error),
        other => RecorderError::other(other.to_string()),
    }
}

/// Adapter implementing [`SessionPipelineConfigPort`] via [`ConfigService`].
pub(crate) struct ConfigServicePipelineConfigPort<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    config_service: Arc<ConfigService<CR, SR>>,
}

impl<CR, SR> ConfigServicePipelineConfigPort<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    pub(crate) fn new(config_service: Arc<ConfigService<CR, SR>>) -> Self {
        Self { config_service }
    }
}

#[async_trait]
impl<CR, SR> SessionPipelineConfigPort for ConfigServicePipelineConfigPort<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    async fn get_session_pipeline_config(
        &self,
        streamer_id: &str,
    ) -> PipelineResult<SessionPipelineConfig> {
        let merged = self
            .config_service
            .get_config_for_streamer(streamer_id)
            .await
            .map_err(to_pipeline_error)?;
        Ok(SessionPipelineConfig {
            segment_pipeline: merged.pipeline.clone(),
            paired_segment_pipeline: merged.paired_segment_pipeline.clone(),
            session_complete_pipeline: merged.session_complete_pipeline.clone(),
            record_danmu: merged.record_danmu,
            auto_thumbnail: merged.auto_thumbnail,
        })
    }

    async fn get_platform_name(&self, platform_config_id: &str) -> PipelineResult<Option<String>> {
        match self
            .config_service
            .get_platform_config(platform_config_id)
            .await
        {
            Ok(platform) => Ok(Some(platform.platform_name)),
            Err(RecorderError::Core(CoreError::NotFound { .. })) => Ok(None),
            Err(error) => Err(to_pipeline_error(error)),
        }
    }
}

/// Adapter implementing [`PipelineSessionPort`] over database repositories.
pub(crate) struct DatabasePipelineSessionPort {
    session_repo: Arc<dyn SessionRepository>,
    streamer_repo: Option<Arc<dyn StreamerRepository>>,
    config_port: Option<Arc<dyn SessionPipelineConfigPort>>,
}

impl DatabasePipelineSessionPort {
    pub(crate) fn new(session_repo: Arc<dyn SessionRepository>) -> Self {
        Self {
            session_repo,
            streamer_repo: None,
            config_port: None,
        }
    }

    pub(crate) fn with_streamer_repository(
        mut self,
        streamer_repo: Arc<dyn StreamerRepository>,
    ) -> Self {
        self.streamer_repo = Some(streamer_repo);
        self
    }

    pub(crate) fn with_config_port(
        mut self,
        config_port: Arc<dyn SessionPipelineConfigPort>,
    ) -> Self {
        self.config_port = Some(config_port);
        self
    }
}

fn to_session_summary(session: LiveSessionDbModel) -> PipelineSessionSummary {
    PipelineSessionSummary {
        id: session.id,
        streamer_id: session.streamer_id,
        ended: session.end_time.is_some(),
    }
}

#[async_trait]
impl PipelineSessionPort for DatabasePipelineSessionPort {
    async fn session_has_thumbnail(&self, session_id: &str) -> PipelineResult<bool> {
        Ok(self
            .session_repo
            .first_thumbnail_id(session_id)
            .await?
            .is_some())
    }

    async fn record_thumbnail_output(
        &self,
        session_id: &str,
        output_path: &str,
        size_bytes: i64,
    ) -> PipelineResult<bool> {
        // Best-effort dedupe: repeated retries/completions should not create duplicate rows.
        // (Schema does not enforce uniqueness on file_path.)
        if let Ok(existing) = self
            .session_repo
            .get_media_outputs_for_session(session_id)
            .await
        {
            let output_key = if cfg!(windows) {
                output_path.to_ascii_lowercase()
            } else {
                output_path.to_string()
            };

            if existing.iter().any(|row| {
                row.file_type == MediaFileType::Thumbnail.as_str() && {
                    let row_key = if cfg!(windows) {
                        row.file_path.to_ascii_lowercase()
                    } else {
                        row.file_path.clone()
                    };
                    row_key == output_key
                }
            }) {
                return Ok(false);
            }
        }

        let output = MediaOutputDbModel::new(
            session_id,
            output_path,
            MediaFileType::Thumbnail,
            size_bytes,
        );
        self.session_repo.create_media_output(&output).await?;
        Ok(true)
    }

    async fn mark_session_complete_dispatched(&self, session_id: &str) -> PipelineResult<()> {
        Ok(self
            .session_repo
            .mark_session_complete_dispatched(session_id)
            .await?)
    }

    async fn session_pipeline_settled(&self, session_id: &str) -> PipelineResult<bool> {
        Ok(self
            .session_repo
            .session_pipeline_settled(session_id)
            .await?)
    }

    async fn list_session_ids_for_retirement(
        &self,
        streamer_id: &str,
    ) -> PipelineResult<Vec<String>> {
        let sessions = self
            .session_repo
            .list_sessions_for_retirement(streamer_id)
            .await?;
        Ok(sessions.into_iter().map(|session| session.id).collect())
    }

    async fn list_active_sessions(
        &self,
        limit: u32,
        offset: u32,
    ) -> PipelineResult<Vec<PipelineSessionSummary>> {
        let pagination = Pagination::new(limit, offset);
        let page = self.session_repo.list_active_sessions(&pagination).await?;
        Ok(page.into_iter().map(to_session_summary).collect())
    }

    async fn list_ended_sessions_pending_pipeline_recovery(
        &self,
        limit: u32,
        offset: u32,
    ) -> PipelineResult<Vec<PipelineSessionSummary>> {
        let pagination = Pagination::new(limit, offset);
        let page = self
            .session_repo
            .list_ended_sessions_pending_pipeline_recovery(&pagination)
            .await?;
        Ok(page.into_iter().map(to_session_summary).collect())
    }

    async fn get_session_summary(
        &self,
        session_id: &str,
    ) -> PipelineResult<Option<PipelineSessionSummary>> {
        match self.session_repo.get_session(session_id).await {
            Ok(session) => Ok(Some(to_session_summary(session))),
            Err(srec_sqlite::DbError::Core(CoreError::NotFound { .. })) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn list_session_segments(
        &self,
        session_id: &str,
        limit: u32,
        offset: u32,
    ) -> PipelineResult<Vec<PipelineSessionSegment>> {
        let pagination = Pagination::new(limit, offset);
        let segments = self
            .session_repo
            .list_session_segments_page(session_id, &pagination)
            .await?;
        Ok(segments
            .into_iter()
            .map(|segment| PipelineSessionSegment {
                segment_index: segment.segment_index,
                file_path: segment.file_path,
            })
            .collect())
    }

    async fn list_session_danmu_outputs(&self, session_id: &str) -> PipelineResult<Vec<String>> {
        let outputs = self
            .session_repo
            .get_media_outputs_for_session(session_id)
            .await?;
        Ok(outputs
            .into_iter()
            .filter(|output| output.file_type == MediaFileType::DanmuXml.as_str())
            .map(|output| output.file_path)
            .collect())
    }

    async fn get_session_meta(
        &self,
        session_id: &str,
    ) -> PipelineResult<Option<PipelineSessionMeta>> {
        match self.session_repo.get_session(session_id).await {
            Ok(session) => {
                let started_at = crate::database::time::ms_to_datetime(session.start_time);
                let title = session
                    .titles
                    .as_deref()
                    .and_then(|json| serde_json::from_str::<Vec<TitleEntry>>(json).ok())
                    .and_then(|entries| entries.into_iter().last().map(|entry| entry.title));
                Ok(Some(PipelineSessionMeta { title, started_at }))
            }
            Err(srec_sqlite::DbError::Core(CoreError::NotFound { .. })) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn get_streamer_meta(
        &self,
        streamer_id: &str,
    ) -> PipelineResult<Option<PipelineStreamerMeta>> {
        let Some(streamer_repo) = self.streamer_repo.as_ref() else {
            return Ok(None);
        };
        let streamer = match streamer_repo.get_streamer(streamer_id).await {
            Ok(streamer) => streamer,
            Err(srec_sqlite::DbError::Core(CoreError::NotFound { .. })) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let platform = if let Some(config_port) = self.config_port.as_ref() {
            match config_port
                .get_platform_name(&streamer.platform_config_id)
                .await
            {
                Ok(platform) => platform,
                Err(error) => {
                    debug!(
                        streamer_id,
                        platform_id = %streamer.platform_config_id,
                        %error,
                        "Failed to look up platform name"
                    );
                    None
                }
            }
        } else {
            None
        };
        Ok(Some(PipelineStreamerMeta {
            name: streamer.name,
            platform,
        }))
    }
}

/// Adapter implementing [`RecorderPipelinePort`] over [`PipelineManager`] so
/// the recorder layer stays decoupled from the pipeline crate.
pub(crate) struct PipelineManagerRecorderPort {
    pipeline_manager: Arc<PipelineManager>,
}

impl PipelineManagerRecorderPort {
    pub(crate) fn new(pipeline_manager: Arc<PipelineManager>) -> Self {
        Self { pipeline_manager }
    }
}

#[async_trait]
impl RecorderPipelinePort for PipelineManagerRecorderPort {
    async fn notify(&self, notification: PipelineNotification) -> RecorderResult<()> {
        self.pipeline_manager
            .handle_recorder_notification(notification)
            .await
            .map_err(pipeline_to_recorder_error)
    }

    async fn drain_for_streamer(
        &self,
        streamer_id: &str,
        timeout: Duration,
    ) -> RecorderResult<PipelineDrainStatus> {
        let drain = self
            .pipeline_manager
            .drain_for_streamer(streamer_id, timeout)
            .await
            .map_err(pipeline_to_recorder_error)?;
        Ok(PipelineDrainStatus {
            unsettled_sessions: drain.sessions.len(),
            outstanding_jobs: drain.jobs,
        })
    }
}
