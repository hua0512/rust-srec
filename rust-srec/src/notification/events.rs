//! Notification events.
//!
//! Defines the events that can trigger notifications and their priority levels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Priority level for notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NotificationPriority {
    /// Low priority - informational only.
    Low,
    /// Normal priority - standard notifications.
    Normal,
    /// High priority - important events.
    High,
    /// Critical priority - requires immediate attention.
    Critical,
}

impl Default for NotificationPriority {
    fn default() -> Self {
        Self::Normal
    }
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

            // Pipeline events
            Self::PipelineStarted { .. } => NotificationPriority::Low,
            Self::PipelineCompleted { .. } => NotificationPriority::Low,
            Self::PipelineFailed { .. } => NotificationPriority::High,

            // System events
            Self::FatalError { .. } => NotificationPriority::Critical,
            Self::OutOfSpace { .. } => NotificationPriority::Critical,
            Self::PipelineQueueWarning { .. } => NotificationPriority::High,
            Self::PipelineQueueCritical { .. } => NotificationPriority::Critical,
            Self::SystemStartup { .. } => NotificationPriority::Normal,
            Self::SystemShutdown { .. } => NotificationPriority::Normal,
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
            Self::PipelineStarted { .. } => "pipeline_started",
            Self::PipelineCompleted { .. } => "pipeline_completed",
            Self::PipelineFailed { .. } => "pipeline_failed",
            Self::FatalError { .. } => "fatal_error",
            Self::OutOfSpace { .. } => "out_of_space",
            Self::PipelineQueueWarning { .. } => "pipeline_queue_warning",
            Self::PipelineQueueCritical { .. } => "pipeline_queue_critical",
            Self::SystemStartup { .. } => "system_startup",
            Self::SystemShutdown { .. } => "system_shutdown",
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
            Self::PipelineStarted { job_type, .. } => {
                format!("⚙️ Started {} job", job_type)
            }
            Self::PipelineCompleted { job_type, .. } => {
                format!("✅ Completed {} job", job_type)
            }
            Self::PipelineFailed { job_type, .. } => {
                format!("❌ Failed {} job", job_type)
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
            Self::SystemStartup { .. } => "System initialized successfully".to_string(),
            Self::SystemShutdown { reason, .. } => reason.clone(),
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
            | Self::PipelineStarted { timestamp, .. }
            | Self::PipelineCompleted { timestamp, .. }
            | Self::PipelineFailed { timestamp, .. }
            | Self::FatalError { timestamp, .. }
            | Self::OutOfSpace { timestamp, .. }
            | Self::PipelineQueueWarning { timestamp, .. }
            | Self::PipelineQueueCritical { timestamp, .. }
            | Self::SystemStartup { timestamp, .. }
            | Self::SystemShutdown { timestamp, .. } => *timestamp,
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
            | Self::PipelineStarted { streamer_id, .. }
            | Self::FatalError { streamer_id, .. } => Some(streamer_id),
            _ => None,
        }
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
        assert_eq!(format_duration(30.0), "30s");
        assert_eq!(format_duration(90.0), "1m 30s");
        assert_eq!(format_duration(3661.0), "1h 1m 1s");
    }
}
