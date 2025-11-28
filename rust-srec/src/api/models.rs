//! API request and response models (DTOs).
//!
//! Defines the data transfer objects for API endpoints.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::streamer::StreamerState;
use crate::domain::value_objects::Priority;

// ============================================================================
// Pagination
// ============================================================================

/// Pagination parameters for list endpoints.
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

/// Paginated response wrapper.
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
    pub max_concurrent_downloads: u32,
    pub max_concurrent_uploads: u32,
    pub max_concurrent_cpu_jobs: u32,
    pub max_concurrent_io_jobs: u32,
    pub streamer_check_delay_ms: u64,
    pub offline_check_delay_ms: u64,
    pub offline_check_count: u32,
    pub default_download_engine: String,
    pub record_danmu: bool,
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
}

/// Request to update platform configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePlatformConfigRequest {
    pub fetch_delay_ms: Option<u64>,
    pub download_delay_ms: Option<u64>,
    pub record_danmu: Option<bool>,
    pub cookies: Option<String>,
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
    pub usage_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ============================================================================
// Pipeline DTOs
// ============================================================================

/// Pipeline job status.
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
#[derive(Debug, Clone, Serialize)]
pub struct PipelineStatsResponse {
    pub pending_count: u64,
    pub processing_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub avg_processing_time_secs: Option<f64>,
}

/// Media output response.
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

/// Live session response.
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

/// Title change entry.
#[derive(Debug, Clone, Serialize)]
pub struct TitleChange {
    pub title: String,
    pub timestamp: DateTime<Utc>,
}

/// Filter parameters for listing sessions.
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
