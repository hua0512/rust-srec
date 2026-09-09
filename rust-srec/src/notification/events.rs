//! Notification events.
//!
//! Defines the events that can trigger notifications and their priority levels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::credentials::CredentialEvent;

mod catalog;
pub(crate) mod mapping;
mod render;

pub use catalog::{
    NotificationEventTypeInfo, canonicalize_subscription_event_name, notification_event_types,
};
pub(crate) use render::RenderCache;
pub use render::RenderedEvent;

/// Priority level for notifications.
///
/// Serializes as an integer (Gotify-compatible 0-10 scale):
/// Low = 2, Normal = 5, High = 8, Critical = 10.
///
/// Deserializes from either an integer or a string label (backward compat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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

impl utoipa::PartialSchema for NotificationPriority {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{Object, SchemaType, Type};
        Object::builder()
            .schema_type(SchemaType::Type(Type::Integer))
            .description(Some(
                "Priority level (integer, 0-10 scale): 2=Low, 5=Normal, 8=High, 10=Critical",
            ))
            .enum_values(Some([2i32, 5, 8, 10]))
            .build()
            .into()
    }
}

impl utoipa::ToSchema for NotificationPriority {}

impl Serialize for NotificationPriority {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.as_int())
    }
}

impl<'de> Deserialize<'de> for NotificationPriority {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        struct PriorityVisitor;

        impl<'de> de::Visitor<'de> for PriorityVisitor {
            type Value = NotificationPriority;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an integer (0-10) or a string (low/normal/high/critical)")
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                let val = u8::try_from(value).map_err(|_| {
                    de::Error::invalid_value(de::Unexpected::Unsigned(value), &self)
                })?;
                NotificationPriority::from_int(val)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Unsigned(value), &self))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                let val = u8::try_from(value)
                    .map_err(|_| de::Error::invalid_value(de::Unexpected::Signed(value), &self))?;
                NotificationPriority::from_int(val)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Signed(value), &self))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                match value.trim().to_ascii_lowercase().as_str() {
                    "low" => Ok(NotificationPriority::Low),
                    "normal" => Ok(NotificationPriority::Normal),
                    "high" => Ok(NotificationPriority::High),
                    "critical" => Ok(NotificationPriority::Critical),
                    _ => Err(de::Error::unknown_variant(
                        value,
                        &["low", "normal", "high", "critical"],
                    )),
                }
            }
        }

        deserializer.deserialize_any(PriorityVisitor)
    }
}

impl NotificationPriority {
    /// Integer representation aligned with Gotify's 0-10 priority scale.
    pub fn as_int(&self) -> u8 {
        match self {
            Self::Low => 2,
            Self::Normal => 5,
            Self::High => 8,
            Self::Critical => 10,
        }
    }

    /// Parse from integer value.
    ///
    /// Maps ranges to the closest priority level:
    /// 0-3 → Low, 4-6 → Normal, 7-9 → High, 10+ → Critical
    pub fn from_int(value: u8) -> Option<Self> {
        match value {
            0..=3 => Some(Self::Low),
            4..=6 => Some(Self::Normal),
            7..=9 => Some(Self::High),
            10..=u8::MAX => Some(Self::Critical),
        }
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
    /// Recording output path is unwritable (caught at the filesystem boundary
    /// by the output-root write gate). Distinct from `OutOfSpace`, which is a
    /// proactive disk-pressure warning; this fires when the gate has actually
    /// blocked downloads because `create_dir_all` / mid-stream writes failed.
    /// Emitted exactly once per `Healthy → Degraded` transition.
    OutputPathInaccessible {
        /// Resolved root path that the gate is guarding (e.g. `/rec`).
        path: String,
        /// Stable string identifying the underlying io error kind. Maps to the
        /// `notification.output_path_inaccessible.description.<kind>` i18n key
        /// via [`crate::downloader::IoErrorKindSer::as_str`].
        error_kind: String,
        timestamp: DateTime<Utc>,
    },
    /// GPU became unavailable to the container (caught by the GPU health
    /// monitor probing nvidia-smi). Most often the NVIDIA Container Toolkit
    /// and cgroup v2 reconciliation pattern: the host's
    /// systemd reloads the device cgroup and silently drops the
    /// container's GPU access while `/dev/nvidia*` nodes remain visible.
    ///
    /// Emitted exactly once per `Healthy → Unhealthy` transition.
    GpuUnavailable {
        /// Stable string identifying the failure kind. Maps to the
        /// `notification.gpu_unavailable.description.<error_kind>` i18n
        /// key via [`crate::metrics::gpu_health::GpuErrorKind::as_str`].
        error_kind: String,
        /// Truncated stderr / diagnostic line from the failed probe (≤ 256
        /// chars). Useful for ops; not used for routing.
        message: String,
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

    // ========== External Tool Events ==========
    /// Automatic Baidu Netdisk re-login with stored credentials was
    /// rejected. Emitted by `crate::baidupcs::StoredLoginAuthenticator`
    /// after a real replay attempt only — its failure cooldown bounds this
    /// to at most one event per window per config dir. Uploads keep
    /// failing until fresh credentials are provided via the login dialog.
    BaiduPcsReloginFailed {
        /// Config-dir key of the affected session (`"default"` when the
        /// CLI's default location is used).
        config_dir: String,
        /// Scrubbed, truncated BaiduPCS-Go output tail.
        message: String,
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
            Self::OutputPathInaccessible { .. } => NotificationPriority::Critical,
            Self::GpuUnavailable { .. } => NotificationPriority::Critical,
            Self::PipelineQueueWarning { .. } => NotificationPriority::High,
            Self::PipelineQueueCritical { .. } => NotificationPriority::Critical,
            Self::SystemStartup { .. } => NotificationPriority::Normal,
            Self::SystemShutdown { .. } => NotificationPriority::Normal,

            // Credential events
            Self::Credential { event } => event.severity(),

            // External tool events
            Self::BaiduPcsReloginFailed { .. } => NotificationPriority::High,
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
            Self::OutputPathInaccessible { .. } => "output_path_inaccessible",
            Self::GpuUnavailable { .. } => "gpu_unavailable",
            Self::PipelineQueueWarning { .. } => "pipeline_queue_warning",
            Self::PipelineQueueCritical { .. } => "pipeline_queue_critical",
            Self::SystemStartup { .. } => "system_startup",
            Self::SystemShutdown { .. } => "system_shutdown",
            Self::Credential { event } => event.event_name(),
            Self::BaiduPcsReloginFailed { .. } => "baidupcs_relogin_failed",
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
            | Self::OutputPathInaccessible { timestamp, .. }
            | Self::GpuUnavailable { timestamp, .. }
            | Self::PipelineQueueWarning { timestamp, .. }
            | Self::PipelineQueueCritical { timestamp, .. }
            | Self::SystemStartup { timestamp, .. }
            | Self::SystemShutdown { timestamp, .. }
            | Self::BaiduPcsReloginFailed { timestamp, .. } => *timestamp,
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

#[cfg(test)]
mod tests;
