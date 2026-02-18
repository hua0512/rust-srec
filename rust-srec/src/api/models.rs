//! API request and response models (DTOs).
//!
//! This module defines the data transfer objects for all API endpoints.
//! These models handle serialization/deserialization between the API layer
//! and internal domain models.
//!
//! # Model Categories
//!
//! - **Pagination**: Generic pagination parameters and response wrappers
//! - **Streamer**: Streamer CRUD operations
//! - **Config**: Global and platform configuration
//! - **Template**: Recording templates
//! - **Pipeline**: Job queue and processing pipeline
//! - **Session**: Recording sessions and outputs
//! - **Health**: System health checks
//! - **Utilities**: URL metadata extraction

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::streamer::StreamerState;
use crate::domain::value_objects::Priority;
use crate::utils::json::deserialize_field_present_nullable;

// ============================================================================
// Pagination
// ============================================================================

/// Pagination parameters for list endpoints.
///
/// # Query Parameters
///
/// - `limit` - Maximum number of items to return (default: 20, max: 100)
/// - `offset` - Number of items to skip for pagination (default: 0)
///
/// # Example
///
/// ```text
/// GET /api/pipeline/jobs?limit=50&offset=100
/// ```
#[derive(Debug, Clone, Deserialize, utoipa::IntoParams)]
pub struct PaginationParams {
    /// Number of items to return (default: 20, max: 100)
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Number of items to skip
    #[serde(default)]
    pub offset: u32,
}

fn default_limit() -> u32 {
    20
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            offset: 0,
        }
    }
}

/// Paginated response wrapper for list endpoints.
///
/// # Response Format
///
/// ```json
/// {
///     "items": [...],
///     "total": 100,
///     "limit": 20,
///     "offset": 0
/// }
/// ```
///
/// # Fields
///
/// - `items` - Array of items for the current page
/// - `total` - Total number of items matching the query (for calculating pages)
/// - `limit` - Number of items requested per page
/// - `offset` - Number of items skipped (for calculating current page)
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PaginatedResponse<T> {
    /// Items in this page
    pub items: Vec<T>,
    /// Total number of items
    pub total: u64,
    /// Number of items returned
    pub limit: u32,
    /// Number of items skipped
    pub offset: u32,
}

impl<T> PaginatedResponse<T> {
    /// Create a new paginated response.
    pub fn new(items: Vec<T>, total: u64, limit: u32, offset: u32) -> Self {
        Self {
            items,
            total,
            limit,
            offset,
        }
    }
}

/// Page response wrapper for list endpoints where computing a total count is expensive.
///
/// This omits `total` and returns only the current page.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PageResponse<T> {
    /// Items in this page
    pub items: Vec<T>,
    /// Number of items requested per page
    pub limit: u32,
    /// Number of items skipped
    pub offset: u32,
}

impl<T> PageResponse<T> {
    pub fn new(items: Vec<T>, limit: u32, offset: u32) -> Self {
        Self {
            items,
            limit,
            offset,
        }
    }
}

// ============================================================================
// Streamer DTOs
// ============================================================================

/// Request to create a new streamer.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateStreamerRequest {
    /// Streamer name
    pub name: String,
    /// Streamer URL
    pub url: String,
    /// Platform configuration ID
    pub platform_config_id: String,
    /// Template ID (optional)
    pub template_id: Option<String>,
    /// Priority (default: Normal)
    #[serde(default)]
    pub priority: Priority,
    /// Whether to enable recording
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Streamer specific configuration override (JSON object)
    pub streamer_specific_config: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// Request to update a streamer.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateStreamerRequest {
    /// Streamer name
    pub name: Option<String>,
    /// Streamer URL
    pub url: Option<String>,
    /// Template ID
    #[serde(default, deserialize_with = "deserialize_field_present_nullable")]
    pub template_id: Option<Option<String>>,
    /// Priority
    pub priority: Option<Priority>,
    /// Whether to enable recording
    pub enabled: Option<bool>,
    /// Streamer specific configuration override (JSON object)
    pub streamer_specific_config: Option<serde_json::Value>,
}

/// Request to update streamer priority.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdatePriorityRequest {
    pub priority: Priority,
}

/// Streamer response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct StreamerResponse {
    pub id: String,
    pub name: String,
    pub url: String,
    pub avatar_url: Option<String>,
    pub platform_config_id: String,
    pub template_id: Option<String>,
    pub state: StreamerState,
    pub priority: Priority,
    pub enabled: bool,
    pub consecutive_error_count: i32,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_live_time: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub streamer_specific_config: Option<serde_json::Value>,
}

/// Filter parameters for listing streamers.
#[derive(Debug, Clone, Deserialize, Default, utoipa::IntoParams)]
pub struct StreamerFilterParams {
    /// Filter by platform
    pub platform: Option<String>,
    /// Filter by state (comma-separated for multiple)
    pub state: Option<String>,
    /// Filter by priority
    pub priority: Option<Priority>,
    /// Filter by enabled status
    pub enabled: Option<bool>,
    /// Sort field
    pub sort_by: Option<String>,
    /// Sort direction (asc/desc)
    pub sort_dir: Option<String>,
    /// Search query (name or URL)
    pub search: Option<String>,
}

// ============================================================================
// Config DTOs
// ============================================================================

/// Global configuration response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GlobalConfigResponse {
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,
    pub min_segment_size_bytes: u64,
    pub max_download_duration_secs: u64,
    pub max_part_size_bytes: u64,
    pub record_danmu: bool,
    pub max_concurrent_downloads: u32,
    pub max_concurrent_uploads: u32,
    pub streamer_check_delay_ms: u64,
    pub proxy_config: Option<String>,
    pub offline_check_delay_ms: u64,
    pub offline_check_count: u32,
    pub default_download_engine: String,
    pub max_concurrent_cpu_jobs: u32,
    pub max_concurrent_io_jobs: u32,
    pub job_history_retention_days: u32,
    pub notification_event_log_retention_days: u32,
    pub session_gap_time_secs: u64,
    pub pipeline: Option<String>,
    pub session_complete_pipeline: Option<String>,
    pub paired_segment_pipeline: Option<String>,
    /// Log filter directive for dynamic logging (e.g., "rust_srec=debug,sqlx=warn")
    pub log_filter_directive: String,
    /// Whether to automatically generate thumbnails for new sessions
    pub auto_thumbnail: bool,

    /// Maximum execution time (seconds) for a single CPU-bound pipeline job.
    pub pipeline_cpu_job_timeout_secs: u64,
    /// Maximum execution time (seconds) for a single IO-bound pipeline job.
    pub pipeline_io_job_timeout_secs: u64,
    /// Maximum execution time (seconds) for the `execute` processor command.
    pub pipeline_execute_timeout_secs: u64,
}

/// Request to update global configuration.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateGlobalConfigRequest {
    pub output_folder: Option<serde_json::Value>,
    pub output_filename_template: Option<serde_json::Value>,
    pub output_file_format: Option<serde_json::Value>,
    pub min_segment_size_bytes: Option<serde_json::Value>,
    pub max_download_duration_secs: Option<serde_json::Value>,
    pub max_part_size_bytes: Option<serde_json::Value>,
    pub max_concurrent_downloads: Option<serde_json::Value>,
    pub max_concurrent_uploads: Option<serde_json::Value>,
    pub max_concurrent_cpu_jobs: Option<serde_json::Value>,
    pub max_concurrent_io_jobs: Option<serde_json::Value>,
    pub streamer_check_delay_ms: Option<serde_json::Value>,
    pub offline_check_delay_ms: Option<serde_json::Value>,
    pub offline_check_count: Option<serde_json::Value>,
    pub job_history_retention_days: Option<serde_json::Value>,
    pub notification_event_log_retention_days: Option<serde_json::Value>,
    pub default_download_engine: Option<serde_json::Value>,
    pub record_danmu: Option<serde_json::Value>,
    pub proxy_config: Option<serde_json::Value>,
    /// Session gap time in seconds
    pub session_gap_time_secs: Option<serde_json::Value>,
    /// Global pipeline configuration (JSON serialized Vec<PipelineStep>)
    pub pipeline: Option<serde_json::Value>,
    /// Session-complete pipeline configuration (JSON serialized DagPipelineDefinition)
    pub session_complete_pipeline: Option<serde_json::Value>,
    /// Paired-segment pipeline configuration (JSON serialized DagPipelineDefinition)
    pub paired_segment_pipeline: Option<serde_json::Value>,
    /// Log filter directive for dynamic logging
    pub log_filter_directive: Option<serde_json::Value>,
    /// Whether to automatically generate thumbnails for new sessions
    pub auto_thumbnail: Option<serde_json::Value>,

    pub pipeline_cpu_job_timeout_secs: Option<serde_json::Value>,
    pub pipeline_io_job_timeout_secs: Option<serde_json::Value>,
    pub pipeline_execute_timeout_secs: Option<serde_json::Value>,
}

/// Platform configuration response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlatformConfigResponse {
    pub id: String,
    pub name: String,
    pub fetch_delay_ms: Option<u64>,
    pub download_delay_ms: Option<u64>,
    pub record_danmu: Option<bool>,
    pub cookies: Option<String>,
    pub platform_specific_config: Option<String>,
    pub proxy_config: Option<String>,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub download_engine: Option<String>,
    pub stream_selection_config: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<u64>,
    pub max_download_duration_secs: Option<u64>,
    pub max_part_size_bytes: Option<u64>,
    pub download_retry_policy: Option<String>,
    pub event_hooks: Option<String>,
    /// Platform-specific pipeline configuration (JSON serialized Vec<PipelineStep>)
    pub pipeline: Option<String>,
    pub session_complete_pipeline: Option<String>,
    pub paired_segment_pipeline: Option<String>,
}

// ============================================================================
// Template DTOs
// ============================================================================

/// Request to create a template.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub output_file_format: Option<String>,
    pub download_engine: Option<String>,
    pub record_danmu: Option<bool>,
    pub platform_overrides: Option<serde_json::Value>,
    pub engines_override: Option<serde_json::Value>,
    pub stream_selection_config: Option<String>,
    pub cookies: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub download_retry_policy: Option<String>,
    pub danmu_sampling_config: Option<String>,
    pub proxy_config: Option<String>,
    pub event_hooks: Option<String>,
    pub pipeline: Option<String>,
    pub session_complete_pipeline: Option<String>,
    pub paired_segment_pipeline: Option<String>,
}

/// Request to update a template.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateTemplateRequest {
    pub name: Option<String>,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub output_file_format: Option<String>,
    pub download_engine: Option<String>,
    pub record_danmu: Option<bool>,
    pub platform_overrides: Option<serde_json::Value>,
    pub engines_override: Option<serde_json::Value>,
    pub stream_selection_config: Option<String>,
    pub cookies: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub download_retry_policy: Option<String>,
    pub danmu_sampling_config: Option<String>,
    pub proxy_config: Option<String>,
    pub event_hooks: Option<String>,
    pub pipeline: Option<String>,
    pub session_complete_pipeline: Option<String>,
    pub paired_segment_pipeline: Option<String>,
}

/// Template response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub output_file_format: Option<String>,
    pub download_engine: Option<String>,
    pub record_danmu: Option<bool>,
    pub platform_overrides: Option<serde_json::Value>,
    pub engines_override: Option<serde_json::Value>,
    pub stream_selection_config: Option<String>,
    pub cookies: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub download_retry_policy: Option<String>,
    pub danmu_sampling_config: Option<String>,
    pub proxy_config: Option<String>,
    pub event_hooks: Option<String>,
    pub pipeline: Option<String>,
    pub session_complete_pipeline: Option<String>,
    pub paired_segment_pipeline: Option<String>,
    pub usage_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ============================================================================
// Pipeline DTOs
// ============================================================================

/// Pipeline job status enumeration.
///
/// # Status Values
///
/// - `PENDING` - Job is queued and waiting to be processed
/// - `PROCESSING` - Job is currently being executed by a worker
/// - `COMPLETED` - Job finished successfully
/// - `FAILED` - Job encountered an error during processing
/// - `INTERRUPTED` - Job was cancelled by user or system
///
/// # State Transitions
///
/// ```text
/// pending -> processing -> completed
///                      \-> failed
/// pending -> interrupted (via cancel)
/// processing -> interrupted (via cancel)
/// failed -> pending (via retry)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Interrupted,
}

/// Pipeline job response.
///
/// # Example Response
///
/// ```json
/// {
///     "id": "job-uuid-123",
///     "session_id": "session-123",
///     "streamer_id": "streamer-456",
///     "status": "COMPLETED",
///     "processor_type": "remux",
///     "input_path": ["/recordings/stream.flv"],
///     "output_path": ["/recordings/stream.mp4"],
///     "error_message": null,
///     "progress": null,
///     "created_at": "2025-12-03T10:00:00Z",
///     "started_at": "2025-12-03T10:00:01Z",
///     "completed_at": "2025-12-03T10:05:00Z"
/// }
/// ```
///
/// # Fields
///
/// - `id` - Unique job identifier (UUID)
/// - `session_id` - Associated recording session ID
/// - `streamer_id` - Associated streamer ID
/// - `status` - Current job status (pending, processing, completed, failed, interrupted)
/// - `processor_type` - Type of processing (remux, upload, thumbnail)
/// - `input_path` - List of input file paths
/// - `output_path` - List of output file paths (set after completion)
/// - `error_message` - Error details if job failed
/// - `progress` - Processing progress (0.0-1.0) if available
/// - `created_at` - When the job was created
/// - `started_at` - When processing started
/// - `completed_at` - When processing finished
/// - `duration_secs` - Processing duration in seconds
/// - `queue_wait_secs` - Time spent waiting in queue before processing started
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobResponse {
    pub id: String,
    pub session_id: String,
    pub streamer_id: String,
    /// Streamer display name
    pub streamer_name: Option<String>,
    pub pipeline_id: Option<String>,
    pub status: JobStatus,
    pub processor_type: String,
    pub input_path: Vec<String>,
    pub output_path: Option<Vec<String>>,
    pub error_message: Option<String>,
    pub progress: Option<f32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub execution_info: Option<JobExecutionInfo>,
    /// Processing duration in seconds (from processor).
    pub duration_secs: Option<f64>,
    /// Time spent waiting in queue before processing started (seconds).
    pub queue_wait_secs: Option<f64>,
}

/// Execution details for a job.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobExecutionInfo {
    /// Current processor name.
    pub current_processor: Option<String>,
    /// Current step number (1-indexed).
    pub current_step: Option<u32>,
    /// Total steps in pipeline.
    pub total_steps: Option<u32>,
    /// Intermediate artifacts produced.
    pub items_produced: Vec<String>,
    /// Input file size in bytes.
    pub input_size_bytes: Option<u64>,
    /// Output file size in bytes.
    pub output_size_bytes: Option<u64>,
    /// Detailed execution logs.
    pub logs: Vec<JobLogEntry>,
    /// Total number of log lines captured for this job (across all steps).
    pub log_lines_total: u64,
    /// Number of WARN lines captured.
    pub log_warn_count: u64,
    /// Number of ERROR lines captured.
    pub log_error_count: u64,
    /// Per-step duration tracking for pipeline jobs.
    pub step_durations: Vec<StepDurationInfo>,
}

/// Per-step duration information for pipeline jobs.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct StepDurationInfo {
    /// Step number (1-indexed).
    pub step: u32,
    /// Processor/job type name.
    pub processor: String,
    /// Duration in seconds.
    pub duration_secs: f64,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// End timestamp.
    pub completed_at: DateTime<Utc>,
}

/// Log entry for job execution.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobLogEntry {
    /// Log timestamp.
    pub timestamp: DateTime<Utc>,
    /// Log level.
    pub level: String,
    /// Log message.
    pub message: String,
}

/// Filter parameters for listing jobs.
///
/// # Query Parameters
///
/// - `status` - Filter by job status (pending, processing, completed, failed, interrupted)
/// - `streamer_id` - Filter by streamer ID
/// - `session_id` - Filter by session ID
/// - `from_date` - Filter jobs created after this date (ISO 8601 format)
/// - `to_date` - Filter jobs created before this date (ISO 8601 format)
///
/// # Example
///
/// ```text
/// GET /api/pipeline/jobs?status=failed&streamer_id=streamer-123&from_date=2025-01-01T00:00:00Z
/// ```
#[derive(Debug, Clone, Deserialize, Default, utoipa::IntoParams)]
pub struct JobFilterParams {
    /// Filter by status
    pub status: Option<JobStatus>,
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by pipeline ID
    pub pipeline_id: Option<String>,
    /// Filter by date range start
    pub from_date: Option<DateTime<Utc>>,
    /// Filter by date range end
    pub to_date: Option<DateTime<Utc>>,
    /// Search query (matches ID, streamer ID, or session ID)
    pub search: Option<String>,
}

/// Pipeline statistics response.
///
/// # Example Response
///
/// ```json
/// {
///     "pending_count": 5,
///     "processing_count": 2,
///     "completed_count": 100,
///     "failed_count": 3,
///     "avg_processing_time_secs": 45.5
/// }
/// ```
///
/// # Fields
///
/// - `pending_count` - Number of jobs waiting to be processed
/// - `processing_count` - Number of jobs currently being processed
/// - `completed_count` - Number of successfully completed jobs
/// - `failed_count` - Number of failed jobs
/// - `avg_processing_time_secs` - Average processing time in seconds (null if no completed jobs)
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PipelineStatsResponse {
    pub pending_count: u64,
    pub processing_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub avg_processing_time_secs: Option<f64>,
}

/// Media output response.
///
/// # Example Response
///
/// ```json
/// {
///     "id": "output-uuid-123",
///     "session_id": "session-123",
///     "streamer_id": "streamer-456",
///     "file_path": "/recordings/stream.mp4",
///     "file_size_bytes": 1073741824,
///     "duration_secs": 3600.5,
///     "format": "mp4",
///     "created_at": "2025-12-03T10:05:00Z"
/// }
/// ```
///
/// # Fields
///
/// - `id` - Unique output identifier
/// - `session_id` - Associated recording session ID
/// - `streamer_id` - Associated streamer ID
/// - `file_path` - Path to the output file
/// - `file_size_bytes` - File size in bytes
/// - `duration_secs` - Duration in seconds (if applicable)
/// - `format` - File format (mp4, flv, ts, etc.)
/// - `created_at` - When the output was created
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MediaOutputResponse {
    pub id: String,
    pub session_id: String,
    pub streamer_id: String,
    pub file_path: String,
    pub file_size_bytes: u64,
    pub duration_secs: Option<f64>,
    pub format: String,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Session DTOs
// ============================================================================

/// Recording session response.
///
/// # Example Response
///
/// ```json
/// {
///     "id": "session-123",
///     "streamer_id": "streamer-456",
///     "streamer_name": "StreamerName",
///     "title": "Current Stream Title",
///     "titles": [
///         {"title": "Initial Title", "timestamp": "2025-12-03T10:00:00Z"},
///         {"title": "Current Stream Title", "timestamp": "2025-12-03T12:00:00Z"}
///     ],
///     "start_time": "2025-12-03T10:00:00Z",
///     "end_time": "2025-12-03T14:00:00Z",
///     "duration_secs": 14400,
///     "output_count": 3,
///     "total_size_bytes": 5368709120,
///     "danmu_count": 15000
/// }
/// ```
///
/// # Fields
///
/// - `id` - Unique session identifier
/// - `streamer_id` - Associated streamer ID
/// - `streamer_name` - Streamer display name
/// - `title` - Current/last stream title
/// - `titles` - History of title changes during the session
/// - `start_time` - When the recording started
/// - `end_time` - When the recording ended (null if still active)
/// - `duration_secs` - Total duration in seconds (null if still active)
/// - `output_count` - Number of output files produced
/// - `total_size_bytes` - Total size of all output files
/// - `danmu_count` - Number of danmu (chat) messages recorded
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SessionResponse {
    pub id: String,
    pub streamer_id: String,
    pub streamer_name: String,
    pub streamer_avatar: Option<String>,
    pub title: String,
    pub titles: Vec<TitleChange>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_secs: Option<u64>,
    pub output_count: u32,
    pub total_size_bytes: u64,
    pub danmu_count: Option<u64>,
    pub thumbnail_url: Option<String>,
}

/// Full danmu statistics for a session.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SessionDanmuStatisticsResponse {
    pub session_id: String,
    pub total_danmus: u64,
    pub danmu_rate_timeseries: Vec<DanmuRatePoint>,
    pub top_talkers: Vec<DanmuTopTalker>,
    pub word_frequency: Vec<DanmuWordFrequency>,
}

/// Danmu rate datapoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DanmuRatePoint {
    /// Unix epoch milliseconds (UTC).
    pub ts: i64,
    pub count: i64,
}

/// Top talker entry.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DanmuTopTalker {
    pub user_id: String,
    pub username: String,
    pub message_count: i64,
}

/// Word frequency entry.
#[derive(Debug, Clone, Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct DanmuWordFrequency {
    pub word: String,
    pub count: i64,
}

/// Title change entry representing a stream title update.
///
/// # Example
///
/// ```json
/// {
///     "title": "Playing Game XYZ",
///     "timestamp": "2025-12-03T12:00:00Z"
/// }
/// ```
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TitleChange {
    pub title: String,
    pub timestamp: DateTime<Utc>,
}

/// Filter parameters for listing sessions.
///
/// # Query Parameters
///
/// - `streamer_id` - Filter by streamer ID
/// - `from_date` - Filter sessions started after this date (ISO 8601 format)
/// - `to_date` - Filter sessions started before this date (ISO 8601 format)
/// - `active_only` - If true, return only sessions without an end_time
///
/// # Example
///
/// ```text
/// GET /api/sessions?streamer_id=streamer-123&active_only=true
/// GET /api/sessions?from_date=2025-01-01T00:00:00Z&to_date=2025-12-31T23:59:59Z
/// ```
#[derive(Debug, Clone, Deserialize, Default, utoipa::IntoParams)]
pub struct SessionFilterParams {
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Filter by date range start
    pub from_date: Option<DateTime<Utc>>,
    /// Filter by date range end
    pub to_date: Option<DateTime<Utc>>,
    /// Only include active sessions
    pub active_only: Option<bool>,
    /// Search query (matches title, streamer name, etc.)
    pub search: Option<String>,
}

// ============================================================================
// Health DTOs
// ============================================================================

/// Health check response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub components: Vec<ComponentHealth>,
}

/// Component health status.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ComponentHealth {
    pub name: String,
    pub status: String,
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_check: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_duration_ms: Option<u64>,
}

// ============================================================================
// Utilities DTOs
// ============================================================================

/// Request to extract metadata from a URL.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ExtractMetadataRequest {
    pub url: String,
}

/// Response from metadata extraction.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ExtractMetadataResponse {
    /// Detected platform name (e.g., "Twitch", "YouTube")
    pub platform: Option<String>,
    /// List of platform configurations that match the detected platform
    pub valid_platform_configs: Vec<PlatformConfigResponse>,
    /// Detected channel ID (if available)
    pub channel_id: Option<String>,
}

/// Request to parse a URL and extract media info.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ParseUrlRequest {
    /// URL to parse
    pub url: String,
    /// Optional cookies for authentication
    pub cookies: Option<String>,
}

/// Response from URL parsing with full media info.
///
/// This returns the complete MediaInfo from platforms_parser crate as JSON.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ParseUrlResponse {
    /// Whether extraction was successful
    pub success: bool,
    /// Whether the stream is currently live
    pub is_live: bool,
    /// The full media info from platforms_parser (serialized)
    pub media_info: Option<serde_json::Value>,
    /// Error message if extraction failed
    pub error: Option<String>,
}

/// Request to resolve the true URL for a stream.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ResolveUrlRequest {
    /// The page URL (needed to create the extractor)
    pub url: String,
    /// The stream info object (as JSON) containing the stream to resolve
    pub stream_info: serde_json::Value,
    /// Optional cookies
    pub cookies: Option<String>,
}

/// Response with the resolved stream info.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ResolveUrlResponse {
    /// Whether resolution was successful
    pub success: bool,
    /// The updated stream info object (as JSON)
    pub stream_info: Option<serde_json::Value>,
    /// Error message if resolution failed
    pub error: Option<String>,
}

// ============================================================================
// Filter DTOs
// ============================================================================

/// Filter response.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct FilterResponse {
    pub id: String,
    pub streamer_id: String,
    pub filter_type: String,
    pub config: serde_json::Value,
}

/// Request to create a new filter.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateFilterRequest {
    pub streamer_id: String,
    pub filter_type: String,
    pub config: serde_json::Value,
}

/// Request to update a filter.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateFilterRequest {
    pub filter_type: Option<String>,
    pub config: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagination_defaults() {
        let params = PaginationParams::default();
        assert_eq!(params.limit, 20);
        assert_eq!(params.offset, 0);
    }

    #[test]
    fn test_paginated_response() {
        let items = vec![1, 2, 3];
        let response = PaginatedResponse::new(items, 100, 20, 0);

        assert_eq!(response.items.len(), 3);
        assert_eq!(response.total, 100);
        assert_eq!(response.limit, 20);
        assert_eq!(response.offset, 0);
    }

    #[test]
    fn test_create_streamer_request_deserialize() {
        let json = r#"{
            "name": "Test Streamer",
            "url": "https://example.com/stream",
            "platform_config_id": "platform1"
        }"#;

        let request: CreateStreamerRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "Test Streamer");
        assert_eq!(request.priority, Priority::Normal);
        assert!(request.enabled);
    }
}
