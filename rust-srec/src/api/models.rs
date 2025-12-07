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

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::streamer::StreamerState;
use crate::domain::value_objects::Priority;

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
/// ```
/// GET /api/pipeline/jobs?limit=50&offset=100
/// ```
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Serialize)]
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

// ============================================================================
// Streamer DTOs
// ============================================================================

/// Request to create a new streamer.
#[derive(Debug, Clone, Deserialize)]
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
}

fn default_true() -> bool {
    true
}

/// Request to update a streamer.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStreamerRequest {
    /// Streamer name
    pub name: Option<String>,
    /// Streamer URL
    pub url: Option<String>,
    /// Template ID
    pub template_id: Option<String>,
    /// Priority
    pub priority: Option<Priority>,
    /// Whether to enable recording
    pub enabled: Option<bool>,
}

/// Request to update streamer priority.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePriorityRequest {
    pub priority: Priority,
}

/// Streamer response.
#[derive(Debug, Clone, Serialize)]
pub struct StreamerResponse {
    pub id: String,
    pub name: String,
    pub url: String,
    pub platform_config_id: String,
    pub template_id: Option<String>,
    pub state: StreamerState,
    pub priority: Priority,
    pub enabled: bool,
    pub consecutive_error_count: i32,
    pub disabled_until: Option<DateTime<Utc>>,
    pub last_live_time: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Filter parameters for listing streamers.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct StreamerFilterParams {
    /// Filter by platform
    pub platform: Option<String>,
    /// Filter by state
    pub state: Option<StreamerState>,
    /// Filter by priority
    pub priority: Option<Priority>,
    /// Filter by enabled status
    pub enabled: Option<bool>,
    /// Sort field
    pub sort_by: Option<String>,
    /// Sort direction (asc/desc)
    pub sort_dir: Option<String>,
}

// ============================================================================
// Config DTOs
// ============================================================================

/// Global configuration response.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Request to update global configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateGlobalConfigRequest {
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub output_file_format: Option<String>,
    pub max_concurrent_downloads: Option<u32>,
    pub max_concurrent_uploads: Option<u32>,
    pub max_concurrent_cpu_jobs: Option<u32>,
    pub max_concurrent_io_jobs: Option<u32>,
    pub streamer_check_delay_ms: Option<u64>,
    pub offline_check_delay_ms: Option<u64>,
    pub offline_check_count: Option<u32>,
    pub default_download_engine: Option<String>,
    pub record_danmu: Option<bool>,
    pub proxy_config: Option<String>,
}

/// Platform configuration response.
#[derive(Debug, Clone, Serialize)]
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
    pub max_bitrate: Option<i32>,
    pub stream_selection_config: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<u64>,
    pub max_download_duration_secs: Option<u64>,
    pub max_part_size_bytes: Option<u64>,
    pub download_retry_policy: Option<String>,
    pub event_hooks: Option<String>,
}

/// Request to update platform configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePlatformConfigRequest {
    pub fetch_delay_ms: Option<u64>,
    pub download_delay_ms: Option<u64>,
    pub record_danmu: Option<bool>,
    pub cookies: Option<String>,
    pub platform_specific_config: Option<String>,
    pub proxy_config: Option<String>,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub download_engine: Option<String>,
    pub max_bitrate: Option<i32>,
    pub stream_selection_config: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<u64>,
    pub max_download_duration_secs: Option<u64>,
    pub max_part_size_bytes: Option<u64>,
    pub download_retry_policy: Option<String>,
    pub event_hooks: Option<String>,
}

// ============================================================================
// Template DTOs
// ============================================================================

/// Request to create a template.
#[derive(Debug, Clone, Deserialize)]
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
}

/// Request to update a template.
#[derive(Debug, Clone, Deserialize)]
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
}

/// Template response.
#[derive(Debug, Clone, Serialize)]
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
/// - `pending` - Job is queued and waiting to be processed
/// - `processing` - Job is currently being executed by a worker
/// - `completed` - Job finished successfully
/// - `failed` - Job encountered an error during processing
/// - `interrupted` - Job was cancelled by user or system
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
///     "status": "completed",
///     "processor_type": "remux",
///     "input_path": "/recordings/stream.flv",
///     "output_path": "/recordings/stream.mp4",
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
/// - `input_path` - Path to input file
/// - `output_path` - Path to output file (set after completion)
/// - `error_message` - Error details if job failed
/// - `progress` - Processing progress (0.0-1.0) if available
/// - `created_at` - When the job was created
/// - `started_at` - When processing started
/// - `completed_at` - When processing finished
#[derive(Debug, Clone, Serialize)]
pub struct JobResponse {
    pub id: String,
    pub session_id: String,
    pub streamer_id: String,
    pub status: JobStatus,
    pub processor_type: String,
    pub input_path: String,
    pub output_path: Option<String>,
    pub error_message: Option<String>,
    pub progress: Option<f32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
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
/// ```
/// GET /api/pipeline/jobs?status=failed&streamer_id=streamer-123&from_date=2025-01-01T00:00:00Z
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct JobFilterParams {
    /// Filter by status
    pub status: Option<JobStatus>,
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by date range start
    pub from_date: Option<DateTime<Utc>>,
    /// Filter by date range end
    pub to_date: Option<DateTime<Utc>>,
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
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub streamer_id: String,
    pub streamer_name: String,
    pub title: String,
    pub titles: Vec<TitleChange>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub duration_secs: Option<u64>,
    pub output_count: u32,
    pub total_size_bytes: u64,
    pub danmu_count: Option<u64>,
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
#[derive(Debug, Clone, Serialize)]
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
/// ```
/// GET /api/sessions?streamer_id=streamer-123&active_only=true
/// GET /api/sessions?from_date=2025-01-01T00:00:00Z&to_date=2025-12-31T23:59:59Z
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionFilterParams {
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Filter by date range start
    pub from_date: Option<DateTime<Utc>>,
    /// Filter by date range end
    pub to_date: Option<DateTime<Utc>>,
    /// Only include active sessions
    pub active_only: Option<bool>,
}

// ============================================================================
// Health DTOs
// ============================================================================

/// Health check response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub components: Vec<ComponentHealth>,
}

/// Component health status.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: String,
    pub message: Option<String>,
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
