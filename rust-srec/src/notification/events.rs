//! Notification events.
//!
//! Defines the events that can trigger notifications and their priority levels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::credentials::CredentialEvent;

/// Static metadata about a supported notification event type.
#[derive(Debug, Clone, Copy, serde::Serialize, utoipa::ToSchema)]
pub struct NotificationEventTypeInfo {
    /// Canonical subscription key (snake_case).
    pub event_type: &'static str,
    /// Human-friendly label.
    pub label: &'static str,
    /// Default priority level.
    pub priority: NotificationPriority,
    /// Additional accepted subscription keys (legacy / aliases).
    pub aliases: &'static [&'static str],
}

const NOTIFICATION_EVENT_TYPES: &[NotificationEventTypeInfo] = &[
    NotificationEventTypeInfo {
        event_type: "stream_online",
        label: "Stream Online",
        priority: NotificationPriority::Normal,
        aliases: &["stream_online", "streamer.online", "StreamOnline"],
    },
    NotificationEventTypeInfo {
        event_type: "stream_offline",
        label: "Stream Offline",
        priority: NotificationPriority::Low,
        aliases: &["stream_offline", "streamer.offline", "StreamOffline"],
    },
    NotificationEventTypeInfo {
        event_type: "download_started",
        label: "Download Started",
        priority: NotificationPriority::Low,
        aliases: &["download_started", "download.started", "DownloadStarted"],
    },
    NotificationEventTypeInfo {
        event_type: "download_completed",
        label: "Download Completed",
        priority: NotificationPriority::Normal,
        aliases: &[
            "download_completed",
            "download.complete",
            "download.completed",
            "DownloadCompleted",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "download_error",
        label: "Download Error",
        priority: NotificationPriority::High,
        aliases: &["download_error", "download.error", "DownloadError"],
    },
    NotificationEventTypeInfo {
        event_type: "segment_started",
        label: "Segment Started",
        priority: NotificationPriority::Low,
        aliases: &["segment_started", "segment.started", "SegmentStarted"],
    },
    NotificationEventTypeInfo {
        event_type: "segment_completed",
        label: "Segment Completed",
        priority: NotificationPriority::Low,
        aliases: &["segment_completed", "segment.completed", "SegmentCompleted"],
    },
    NotificationEventTypeInfo {
        event_type: "download_cancelled",
        label: "Download Cancelled",
        priority: NotificationPriority::Normal,
        aliases: &[
            "download_cancelled",
            "download.cancelled",
            "DownloadCancelled",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "download_rejected",
        label: "Download Rejected",
        priority: NotificationPriority::High,
        aliases: &["download_rejected", "download.rejected", "DownloadRejected"],
    },
    NotificationEventTypeInfo {
        event_type: "config_updated",
        label: "Config Updated",
        priority: NotificationPriority::Low,
        aliases: &["config_updated", "config.updated", "ConfigUpdated"],
    },
    NotificationEventTypeInfo {
        event_type: "pipeline_started",
        label: "Pipeline Started",
        priority: NotificationPriority::Low,
        aliases: &["pipeline_started", "pipeline.started", "PipelineStarted"],
    },
    NotificationEventTypeInfo {
        event_type: "pipeline_completed",
        label: "Pipeline Completed",
        priority: NotificationPriority::Low,
        aliases: &[
            "pipeline_completed",
            "pipeline.complete",
            "pipeline.completed",
            "PipelineCompleted",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "pipeline_failed",
        label: "Pipeline Failed",
        priority: NotificationPriority::High,
        aliases: &["pipeline_failed", "pipeline.failed", "PipelineFailed"],
    },
    NotificationEventTypeInfo {
        event_type: "pipeline_cancelled",
        label: "Pipeline Cancelled",
        priority: NotificationPriority::Normal,
        aliases: &[
            "pipeline_cancelled",
            "pipeline.cancelled",
            "PipelineCancelled",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "fatal_error",
        label: "Fatal Error",
        priority: NotificationPriority::Critical,
        aliases: &["fatal_error", "fatal.error", "FatalError"],
    },
    NotificationEventTypeInfo {
        event_type: "out_of_space",
        label: "Out Of Space",
        priority: NotificationPriority::Critical,
        aliases: &["out_of_space", "disk.out_of_space", "OutOfSpace"],
    },
    NotificationEventTypeInfo {
        event_type: "pipeline_queue_warning",
        label: "Pipeline Queue Warning",
        priority: NotificationPriority::High,
        aliases: &[
            "pipeline_queue_warning",
            "pipeline.queue.warning",
            "PipelineQueueWarning",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "pipeline_queue_critical",
        label: "Pipeline Queue Critical",
        priority: NotificationPriority::Critical,
        aliases: &[
            "pipeline_queue_critical",
            "pipeline.queue.critical",
            "PipelineQueueCritical",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "system_startup",
        label: "System Startup",
        priority: NotificationPriority::Normal,
        aliases: &["system_startup", "system.startup", "SystemStartup"],
    },
    NotificationEventTypeInfo {
        event_type: "system_shutdown",
        label: "System Shutdown",
        priority: NotificationPriority::Normal,
        aliases: &["system_shutdown", "system.shutdown", "SystemShutdown"],
    },
    // ========== Credential Events ==========
    NotificationEventTypeInfo {
        event_type: "credential_refreshed",
        label: "Credential Refreshed",
        priority: NotificationPriority::Normal,
        aliases: &[
            "credential_refreshed",
            "credential.refreshed",
            "CredentialRefreshed",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "credential_refresh_failed",
        label: "Credential Refresh Failed",
        priority: NotificationPriority::High,
        aliases: &[
            "credential_refresh_failed",
            "credential.refresh_failed",
            "credential.refresh.failed",
            "CredentialRefreshFailed",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "credential_invalid",
        label: "Credential Invalid",
        priority: NotificationPriority::Critical,
        aliases: &[
            "credential_invalid",
            "credential.invalid",
            "CredentialInvalid",
        ],
    },
    NotificationEventTypeInfo {
        event_type: "credential_expiring",
        label: "Credential Expiring Soon",
        priority: NotificationPriority::Normal,
        aliases: &[
            "credential_expiring",
            "credential.expiring",
            "CredentialExpiring",
        ],
    },
];

pub fn notification_event_types() -> &'static [NotificationEventTypeInfo] {
    NOTIFICATION_EVENT_TYPES
}

pub fn canonicalize_subscription_event_name(input: &str) -> Option<&'static str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized_input = normalize_subscription_key(trimmed);
    for info in NOTIFICATION_EVENT_TYPES {
        for alias in info.aliases {
            if normalize_subscription_key(alias) == normalized_input {
                return Some(info.event_type);
            }
        }

        if normalize_subscription_key(info.event_type) == normalized_input {
            return Some(info.event_type);
        }
    }

    None
}

fn normalize_subscription_key(input: &str) -> String {
    let lower = input.trim().to_ascii_lowercase();
    let snakeish = lower.replace(['.', '-', ' '], "_");
    let compact: String = snakeish.chars().filter(|c| *c != '_').collect();
    compact
}

/// Priority level for notifications.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Default,
    utoipa::ToSchema,
)]
pub enum NotificationPriority {
    /// Low priority - informational only.
    Low,
    /// Normal priority - standard notifications.
    #[default]
    Normal,
    /// High priority - important events.
    High,
    /// Critical priority - requires immediate attention.
    Critical,
}

impl std::fmt::Display for NotificationPriority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Normal => write!(f, "normal"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

/// Events that can trigger notifications.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationEvent {
    // ========== Stream Events ==========
    /// Streamer went online.
    StreamOnline {
        streamer_id: String,
        streamer_name: String,
        title: String,
        category: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// Streamer went offline.
    StreamOffline {
        streamer_id: String,
        streamer_name: String,
        duration_secs: Option<f64>,
        timestamp: DateTime<Utc>,
    },

    // ========== Download Events ==========
    /// Download started.
    DownloadStarted {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Download completed successfully.
    DownloadCompleted {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        file_size_bytes: u64,
        duration_secs: f64,
        timestamp: DateTime<Utc>,
    },
    /// Download failed with error.
    DownloadError {
        streamer_id: String,
        streamer_name: String,
        error_message: String,
        recoverable: bool,
        timestamp: DateTime<Utc>,
    },
    /// Segment started - a new segment file has begun recording.
    SegmentStarted {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        segment_path: String,
        segment_index: u32,
        timestamp: DateTime<Utc>,
    },
    /// Segment completed - a segment file has finished recording.
    SegmentCompleted {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        segment_path: String,
        segment_index: u32,
        size_bytes: u64,
        duration_secs: f64,
        timestamp: DateTime<Utc>,
    },
    /// Download was cancelled by user.
    DownloadCancelled {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Download was rejected before starting (e.g., circuit breaker open).
    DownloadRejected {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// Download configuration was updated dynamically.
    ConfigUpdated {
        streamer_id: String,
        streamer_name: String,
        update_type: String,
        timestamp: DateTime<Utc>,
    },

    // ========== Pipeline Events ==========
    /// Pipeline job started.
    PipelineStarted {
        job_id: String,
        job_type: String,
        streamer_id: String,
        timestamp: DateTime<Utc>,
    },
    /// Pipeline job completed.
    PipelineCompleted {
        job_id: String,
        job_type: String,
        output_path: Option<String>,
        duration_secs: f64,
        timestamp: DateTime<Utc>,
    },
    /// Pipeline job failed.
    PipelineFailed {
        job_id: String,
        job_type: String,
        error_message: String,
        timestamp: DateTime<Utc>,
    },
    /// Pipeline job cancelled.
    PipelineCancelled {
        job_id: String,
        job_type: String,
        pipeline_id: Option<String>,
        timestamp: DateTime<Utc>,
    },

    // ========== System Events ==========
    /// Fatal error occurred for a streamer.
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: String,
        message: String,
        timestamp: DateTime<Utc>,
    },
    /// Disk space running low.
    OutOfSpace {
        path: String,
        available_bytes: u64,
        threshold_bytes: u64,
        timestamp: DateTime<Utc>,
    },
    /// Pipeline queue depth warning.
    PipelineQueueWarning {
        queue_depth: usize,
        threshold: usize,
        timestamp: DateTime<Utc>,
    },
    /// Pipeline queue depth critical.
    PipelineQueueCritical {
        queue_depth: usize,
        threshold: usize,
        timestamp: DateTime<Utc>,
    },
    /// System startup.
    SystemStartup {
        version: String,
        timestamp: DateTime<Utc>,
    },
    /// System shutdown.
    SystemShutdown {
        reason: String,
        timestamp: DateTime<Utc>,
    },

    // ========== Credential Events ==========
    /// Credentials subsystem event (refresh, invalidation, etc.).
    Credential { event: CredentialEvent },
}

impl NotificationEvent {
    /// Get the priority of this event.
    pub fn priority(&self) -> NotificationPriority {
        match self {
            // Stream events
            Self::StreamOnline { .. } => NotificationPriority::Normal,
            Self::StreamOffline { .. } => NotificationPriority::Low,

            // Download events
            Self::DownloadStarted { .. } => NotificationPriority::Low,
            Self::DownloadCompleted { .. } => NotificationPriority::Normal,
            Self::DownloadError { recoverable, .. } => {
                if *recoverable {
                    NotificationPriority::Normal
                } else {
                    NotificationPriority::High
                }
            }
            Self::SegmentStarted { .. } => NotificationPriority::Low,
            Self::SegmentCompleted { .. } => NotificationPriority::Low,
            Self::DownloadCancelled { .. } => NotificationPriority::Normal,
            Self::DownloadRejected { .. } => NotificationPriority::High,
            Self::ConfigUpdated { .. } => NotificationPriority::Low,

            // Pipeline events
            Self::PipelineStarted { .. } => NotificationPriority::Low,
            Self::PipelineCompleted { .. } => NotificationPriority::Low,
            Self::PipelineFailed { .. } => NotificationPriority::High,
            Self::PipelineCancelled { .. } => NotificationPriority::Normal,

            // System events
            Self::FatalError { .. } => NotificationPriority::Critical,
            Self::OutOfSpace { .. } => NotificationPriority::Critical,
            Self::PipelineQueueWarning { .. } => NotificationPriority::High,
            Self::PipelineQueueCritical { .. } => NotificationPriority::Critical,
            Self::SystemStartup { .. } => NotificationPriority::Normal,
            Self::SystemShutdown { .. } => NotificationPriority::Normal,

            // Credential events
            Self::Credential { event } => event.severity(),
        }
    }

    /// Get the event type as a string.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::StreamOnline { .. } => "stream_online",
            Self::StreamOffline { .. } => "stream_offline",
            Self::DownloadStarted { .. } => "download_started",
            Self::DownloadCompleted { .. } => "download_completed",
            Self::DownloadError { .. } => "download_error",
            Self::SegmentStarted { .. } => "segment_started",
            Self::SegmentCompleted { .. } => "segment_completed",
            Self::DownloadCancelled { .. } => "download_cancelled",
            Self::DownloadRejected { .. } => "download_rejected",
            Self::ConfigUpdated { .. } => "config_updated",
            Self::PipelineStarted { .. } => "pipeline_started",
            Self::PipelineCompleted { .. } => "pipeline_completed",
            Self::PipelineFailed { .. } => "pipeline_failed",
            Self::PipelineCancelled { .. } => "pipeline_cancelled",
            Self::FatalError { .. } => "fatal_error",
            Self::OutOfSpace { .. } => "out_of_space",
            Self::PipelineQueueWarning { .. } => "pipeline_queue_warning",
            Self::PipelineQueueCritical { .. } => "pipeline_queue_critical",
            Self::SystemStartup { .. } => "system_startup",
            Self::SystemShutdown { .. } => "system_shutdown",
            Self::Credential { event } => event.event_name(),
        }
    }

    /// Get a human-readable title for this event.
    pub fn title(&self) -> String {
        match self {
            Self::StreamOnline { streamer_name, .. } => {
                format!("🔴 {} is now live!", streamer_name)
            }
            Self::StreamOffline { streamer_name, .. } => {
                format!("⚫ {} went offline", streamer_name)
            }
            Self::DownloadStarted { streamer_name, .. } => {
                format!("⬇️ Started recording {}", streamer_name)
            }
            Self::DownloadCompleted { streamer_name, .. } => {
                format!("✅ Finished recording {}", streamer_name)
            }
            Self::DownloadError { streamer_name, .. } => {
                format!("❌ Download error for {}", streamer_name)
            }
            Self::SegmentStarted {
                streamer_name,
                segment_index,
                ..
            } => {
                format!("📼 Segment {} started for {}", segment_index, streamer_name)
            }
            Self::SegmentCompleted {
                streamer_name,
                segment_index,
                ..
            } => {
                format!(
                    "✅ Segment {} completed for {}",
                    segment_index, streamer_name
                )
            }
            Self::DownloadCancelled { streamer_name, .. } => {
                format!("⏹️ Download cancelled for {}", streamer_name)
            }
            Self::DownloadRejected { streamer_name, .. } => {
                format!("🚫 Download rejected for {}", streamer_name)
            }
            Self::ConfigUpdated { streamer_name, .. } => {
                format!("⚙️ Config updated for {}", streamer_name)
            }
            Self::PipelineStarted { job_type, .. } => {
                format!("⚙️ Started {} job", job_type)
            }
            Self::PipelineCompleted { job_type, .. } => {
                format!("✅ Completed {} job", job_type)
            }
            Self::PipelineFailed { job_type, .. } => {
                format!("❌ Failed {} job", job_type)
            }
            Self::PipelineCancelled { job_type, .. } => {
                format!("⚪ Cancelled {} job", job_type)
            }
            Self::FatalError {
                streamer_name,
                error_type,
                ..
            } => {
                format!("🚨 Fatal error for {}: {}", streamer_name, error_type)
            }
            Self::OutOfSpace { path, .. } => {
                format!("💾 Low disk space on {}", path)
            }
            Self::PipelineQueueWarning { queue_depth, .. } => {
                format!("⚠️ Pipeline queue warning: {} jobs", queue_depth)
            }
            Self::PipelineQueueCritical { queue_depth, .. } => {
                format!("🚨 Pipeline queue critical: {} jobs", queue_depth)
            }
            Self::SystemStartup { version, .. } => {
                format!("🚀 System started (v{})", version)
            }
            Self::SystemShutdown { reason, .. } => {
                format!("🛑 System shutting down: {}", reason)
            }
            Self::Credential { event } => match event {
                CredentialEvent::Refreshed {
                    platform, scope, ..
                } => format!(
                    "🔐 {} credentials refreshed ({})",
                    platform,
                    scope.describe()
                ),
                CredentialEvent::RefreshFailed {
                    platform,
                    scope,
                    requires_relogin,
                    ..
                } => {
                    if *requires_relogin {
                        format!(
                            "🔐 {} refresh failed (re-login required) ({})",
                            platform,
                            scope.describe()
                        )
                    } else {
                        format!("🔐 {} refresh failed ({})", platform, scope.describe())
                    }
                }
                CredentialEvent::Invalid {
                    platform, scope, ..
                } => {
                    format!("🔐 {} credentials invalid ({})", platform, scope.describe())
                }
                CredentialEvent::ExpiringSoon {
                    platform, scope, ..
                } => {
                    format!(
                        "🔐 {} credentials expiring soon ({})",
                        platform,
                        scope.describe()
                    )
                }
            },
        }
    }

    /// Get a detailed description of this event.
    pub fn description(&self) -> String {
        match self {
            Self::StreamOnline {
                title, category, ..
            } => match category {
                Some(cat) => format!("{} ({})", title, cat),
                None => title.clone(),
            },
            Self::StreamOffline { duration_secs, .. } => match duration_secs {
                Some(secs) => format!("Stream duration: {}", format_duration(*secs)),
                None => "Stream ended".to_string(),
            },
            Self::DownloadStarted { session_id, .. } => {
                format!("Session: {}", session_id)
            }
            Self::DownloadCompleted {
                file_size_bytes,
                duration_secs,
                ..
            } => {
                format!(
                    "Size: {}, Duration: {}",
                    format_bytes(*file_size_bytes),
                    format_duration(*duration_secs)
                )
            }
            Self::DownloadError {
                error_message,
                recoverable,
                ..
            } => {
                if *recoverable {
                    format!("{} (will retry)", error_message)
                } else {
                    error_message.clone()
                }
            }
            Self::SegmentStarted { segment_path, .. } => {
                format!("Path: {}", segment_path)
            }
            Self::SegmentCompleted {
                segment_path,
                size_bytes,
                duration_secs,
                ..
            } => {
                format!(
                    "Path: {}, Size: {}, Duration: {}",
                    segment_path,
                    format_bytes(*size_bytes),
                    format_duration(*duration_secs)
                )
            }
            Self::DownloadCancelled { session_id, .. } => {
                format!("Session: {}", session_id)
            }
            Self::DownloadRejected { reason, .. } => reason.clone(),
            Self::ConfigUpdated { update_type, .. } => {
                format!("Update type: {}", update_type)
            }
            Self::PipelineStarted { job_id, .. } => {
                format!("Job ID: {}", job_id)
            }
            Self::PipelineCompleted {
                output_path,
                duration_secs,
                ..
            } => match output_path {
                Some(path) => format!("Output: {} ({})", path, format_duration(*duration_secs)),
                None => format!("Completed in {}", format_duration(*duration_secs)),
            },
            Self::PipelineFailed { error_message, .. } => error_message.clone(),
            Self::PipelineCancelled {
                job_id,
                pipeline_id,
                ..
            } => match pipeline_id {
                Some(pid) => format!("Job {} cancelled (pipeline: {})", job_id, pid),
                None => format!("Job {} cancelled", job_id),
            },
            Self::FatalError { message, .. } => message.clone(),
            Self::OutOfSpace {
                available_bytes,
                threshold_bytes,
                ..
            } => {
                format!(
                    "Available: {} (threshold: {})",
                    format_bytes(*available_bytes),
                    format_bytes(*threshold_bytes)
                )
            }
            Self::PipelineQueueWarning {
                queue_depth,
                threshold,
                ..
            } => {
                format!(
                    "Queue depth {} exceeds warning threshold {}",
                    queue_depth, threshold
                )
            }
            Self::PipelineQueueCritical {
                queue_depth,
                threshold,
                ..
            } => {
                format!(
                    "Queue depth {} exceeds critical threshold {}",
                    queue_depth, threshold
                )
            }
            Self::SystemStartup { version, .. } => {
                format!("System initialized successfully (v{})", version)
            }
            Self::SystemShutdown { reason, .. } => reason.clone(),
            Self::Credential { event } => event.to_message(),
        }
    }

    /// Get the timestamp of this event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::StreamOnline { timestamp, .. }
            | Self::StreamOffline { timestamp, .. }
            | Self::DownloadStarted { timestamp, .. }
            | Self::DownloadCompleted { timestamp, .. }
            | Self::DownloadError { timestamp, .. }
            | Self::SegmentStarted { timestamp, .. }
            | Self::SegmentCompleted { timestamp, .. }
            | Self::DownloadCancelled { timestamp, .. }
            | Self::DownloadRejected { timestamp, .. }
            | Self::ConfigUpdated { timestamp, .. }
            | Self::PipelineStarted { timestamp, .. }
            | Self::PipelineCompleted { timestamp, .. }
            | Self::PipelineFailed { timestamp, .. }
            | Self::PipelineCancelled { timestamp, .. }
            | Self::FatalError { timestamp, .. }
            | Self::OutOfSpace { timestamp, .. }
            | Self::PipelineQueueWarning { timestamp, .. }
            | Self::PipelineQueueCritical { timestamp, .. }
            | Self::SystemStartup { timestamp, .. }
            | Self::SystemShutdown { timestamp, .. } => *timestamp,
            Self::Credential { event } => match event {
                CredentialEvent::Refreshed { timestamp, .. }
                | CredentialEvent::RefreshFailed { timestamp, .. }
                | CredentialEvent::Invalid { timestamp, .. }
                | CredentialEvent::ExpiringSoon { timestamp, .. } => *timestamp,
            },
        }
    }

    /// Get the streamer ID if this event is related to a streamer.
    pub fn streamer_id(&self) -> Option<&str> {
        match self {
            Self::StreamOnline { streamer_id, .. }
            | Self::StreamOffline { streamer_id, .. }
            | Self::DownloadStarted { streamer_id, .. }
            | Self::DownloadCompleted { streamer_id, .. }
            | Self::DownloadError { streamer_id, .. }
            | Self::SegmentStarted { streamer_id, .. }
            | Self::SegmentCompleted { streamer_id, .. }
            | Self::DownloadCancelled { streamer_id, .. }
            | Self::DownloadRejected { streamer_id, .. }
            | Self::ConfigUpdated { streamer_id, .. }
            | Self::PipelineStarted { streamer_id, .. }
            | Self::FatalError { streamer_id, .. } => Some(streamer_id),
            _ => None,
        }
    }

    pub fn event_type_info(event_type: &str) -> Option<NotificationEventTypeInfo> {
        let canonical = canonicalize_subscription_event_name(event_type)?;
        notification_event_types()
            .iter()
            .copied()
            .find(|e| e.event_type == canonical)
    }
}

/// Format bytes into human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format duration in seconds into human-readable string.
fn format_duration(secs: f64) -> String {
    let total_secs = secs as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_priority_ordering() {
        assert!(NotificationPriority::Low < NotificationPriority::Normal);
        assert!(NotificationPriority::Normal < NotificationPriority::High);
        assert!(NotificationPriority::High < NotificationPriority::Critical);
    }

    #[test]
    fn test_notification_priority_display() {
        assert_eq!(NotificationPriority::Low.to_string(), "low");
        assert_eq!(NotificationPriority::Critical.to_string(), "critical");
    }

    #[test]
    fn test_stream_online_event() {
        let event = NotificationEvent::StreamOnline {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            title: "Playing Games".to_string(),
            category: Some("Gaming".to_string()),
            timestamp: Utc::now(),
        };

        assert_eq!(event.priority(), NotificationPriority::Normal);
        assert_eq!(event.event_type(), "stream_online");
        assert!(event.title().contains("TestStreamer"));
        assert!(event.description().contains("Playing Games"));
        assert_eq!(event.streamer_id(), Some("123"));
    }

    #[test]
    fn test_fatal_error_priority() {
        let event = NotificationEvent::FatalError {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            error_type: "NotFound".to_string(),
            message: "Streamer not found".to_string(),
            timestamp: Utc::now(),
        };

        assert_eq!(event.priority(), NotificationPriority::Critical);
    }

    #[test]
    fn test_download_error_priority() {
        let recoverable = NotificationEvent::DownloadError {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            error_message: "Network error".to_string(),
            recoverable: true,
            timestamp: Utc::now(),
        };
        assert_eq!(recoverable.priority(), NotificationPriority::Normal);

        let non_recoverable = NotificationEvent::DownloadError {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            error_message: "Fatal error".to_string(),
            recoverable: false,
            timestamp: Utc::now(),
        };
        assert_eq!(non_recoverable.priority(), NotificationPriority::High);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(3661.0), "1h 1m 1s");
    }

    #[test]
    fn test_segment_events() {
        let start_event = NotificationEvent::SegmentStarted {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            session_id: "session_1".to_string(),
            segment_path: "/path/to/segment/1.ts".to_string(),
            segment_index: 1,
            timestamp: Utc::now(),
        };

        assert_eq!(start_event.priority(), NotificationPriority::Low);
        assert_eq!(start_event.event_type(), "segment_started");
        assert!(start_event.title().contains("Segment 1 started"));
        assert!(start_event.description().contains("/path/to/segment/1.ts"));

        let complete_event = NotificationEvent::SegmentCompleted {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            session_id: "session_1".to_string(),
            segment_path: "/path/to/segment/1.ts".to_string(),
            segment_index: 1,
            size_bytes: 1024 * 1024,
            duration_secs: 10.0,
            timestamp: Utc::now(),
        };

        assert_eq!(complete_event.priority(), NotificationPriority::Low);
        assert_eq!(complete_event.event_type(), "segment_completed");
        assert!(complete_event.title().contains("Segment 1 completed"));
        assert!(complete_event.description().contains("Size: 1.00 MB"));
    }

    #[test]
    fn test_config_update_event() {
        let event = NotificationEvent::ConfigUpdated {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            update_type: "Cookies".to_string(),
            timestamp: Utc::now(),
        };

        assert_eq!(event.priority(), NotificationPriority::Low);
        assert_eq!(event.event_type(), "config_updated");
        assert!(event.title().contains("Config updated"));
        assert!(event.description().contains("Cookies"));
    }

    #[test]
    fn test_download_cancellation_rejection() {
        let cancel_event = NotificationEvent::DownloadCancelled {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            session_id: "session_1".to_string(),
            timestamp: Utc::now(),
        };
        assert_eq!(cancel_event.priority(), NotificationPriority::Normal);
        assert_eq!(cancel_event.event_type(), "download_cancelled");

        let reject_event = NotificationEvent::DownloadRejected {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            session_id: "session_1".to_string(),
            reason: "Circuit breaker open".to_string(),
            timestamp: Utc::now(),
        };
        assert_eq!(reject_event.priority(), NotificationPriority::High);
        assert_eq!(reject_event.event_type(), "download_rejected");
        assert!(reject_event.description().contains("Circuit breaker open"));
    }
}
