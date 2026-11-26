//! Narrow ports decoupling the pipeline engine from recorder, configuration,
//! and session/streamer persistence internals.

use async_trait::async_trait;
use jiff::Timestamp;

use crate::PipelineResult as Result;
use srec_sqlite::models::job::DagPipelineDefinition;

pub use srec_core::domain::pipeline_notification::PipelineNotification;

/// Effective pipeline rules resolved for a streamer/session.
#[derive(Debug, Clone, Default)]
pub struct SessionPipelineConfig {
    pub segment_pipeline: Option<DagPipelineDefinition>,
    pub paired_segment_pipeline: Option<DagPipelineDefinition>,
    pub session_complete_pipeline: Option<DagPipelineDefinition>,
    pub record_danmu: bool,
    pub auto_thumbnail: bool,
}

/// Narrow port for resolving effective pipeline configuration for a streamer.
///
/// Supports cold-start recovery when no live streamer actor is resident.
#[async_trait]
pub trait SessionPipelineConfigPort: Send + Sync {
    /// Resolve the effective pipeline configuration for `streamer_id`.
    async fn get_session_pipeline_config(&self, streamer_id: &str)
    -> Result<SessionPipelineConfig>;

    /// Look up the display name for a platform configuration ID.
    async fn get_platform_name(&self, platform_config_id: &str) -> Result<Option<String>> {
        let _ = platform_config_id;
        Ok(None)
    }
}

/// Minimal session snapshot required by startup pipeline coordination recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineSessionSummary {
    pub id: String,
    pub streamer_id: Option<String>,
    pub ended: bool,
}

/// Minimal persisted video segment reference for startup coordination recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineSessionSegment {
    pub segment_index: i64,
    pub file_path: String,
}

/// Session metadata used for pipeline placeholder expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineSessionMeta {
    pub title: Option<String>,
    pub started_at: Timestamp,
}

/// Streamer metadata used for pipeline placeholder expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineStreamerMeta {
    pub name: String,
    pub platform: Option<String>,
}

/// Narrow port for session and streamer metadata/persistence consumed by the
/// pipeline manager and job queue.
///
/// Supports cold-start recovery without live streamer actors.
#[async_trait]
pub trait PipelineSessionPort: Send + Sync {
    /// Whether `session_id` already has a persisted thumbnail output.
    async fn session_has_thumbnail(&self, session_id: &str) -> Result<bool>;

    /// Persist a thumbnail output for `session_id` if not already recorded.
    /// Returns `Ok(true)` when a new output row was inserted.
    async fn record_thumbnail_output(
        &self,
        session_id: &str,
        output_path: &str,
        size_bytes: i64,
    ) -> Result<bool>;

    /// Record that `session_id` needs no further session-complete dispatch.
    async fn mark_session_complete_dispatched(&self, session_id: &str) -> Result<()>;

    /// Whether `session_id` already has a durable session-complete dispatch or skip receipt.
    async fn session_pipeline_settled(&self, session_id: &str) -> Result<bool>;

    /// List session IDs that must settle before `streamer_id` can be physically retired.
    async fn list_session_ids_for_retirement(&self, streamer_id: &str) -> Result<Vec<String>>;

    /// Page through currently active (open) sessions for startup coordination recovery.
    async fn list_active_sessions(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<PipelineSessionSummary>>;

    /// Page through ended sessions still owed a session-complete pipeline dispatch.
    async fn list_ended_sessions_pending_pipeline_recovery(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<PipelineSessionSummary>>;

    /// Look up one session summary by ID (e.g. for an ended session pulled in by an in-flight DAG).
    async fn get_session_summary(&self, session_id: &str)
    -> Result<Option<PipelineSessionSummary>>;

    /// Page through persisted video segments for `session_id`.
    async fn list_session_segments(
        &self,
        session_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<PipelineSessionSegment>>;

    /// List persisted danmu XML output paths for `session_id`.
    async fn list_session_danmu_outputs(&self, session_id: &str) -> Result<Vec<String>>;

    /// Look up the latest session title and start timestamp for placeholder expansion.
    async fn get_session_meta(&self, session_id: &str) -> Result<Option<PipelineSessionMeta>>;

    /// Look up streamer display name and optional platform name for placeholder expansion.
    async fn get_streamer_meta(&self, streamer_id: &str) -> Result<Option<PipelineStreamerMeta>>;
}
