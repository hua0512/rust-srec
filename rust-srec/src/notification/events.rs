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
    /// Default priority level (integer, Gotify-compatible 0-10 scale).
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
        event_type: "output_path_inaccessible",
        label: "Output Path Inaccessible",
        priority: NotificationPriority::Critical,
        aliases: &[
            "output_path_inaccessible",
            "output.path_inaccessible",
            "OutputPathInaccessible",
        ],
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
        /// via [`crate::downloader::engine::traits::IoErrorKindSer::as_str`].
        error_kind: String,
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
            Self::OutputPathInaccessible { .. } => NotificationPriority::Critical,
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
            Self::OutputPathInaccessible { .. } => "output_path_inaccessible",
            Self::PipelineQueueWarning { .. } => "pipeline_queue_warning",
            Self::PipelineQueueCritical { .. } => "pipeline_queue_critical",
            Self::SystemStartup { .. } => "system_startup",
            Self::SystemShutdown { .. } => "system_shutdown",
            Self::Credential { event } => event.event_name(),
        }
    }

    /// Get a human-readable title for this event.
    ///
    /// Localized via [`crate::t_str!`]. The active locale is picked at
    /// startup from `RUST_SREC_LOCALE` (see [`crate::i18n`]); currently
    /// `en` and `zh-CN` are supported.
    ///
    /// Numeric placeholders (`segment_index`, `queue_depth`, etc.) are
    /// stringified with `.to_string()` before passing to the macro because
    /// `rust_i18n` placeholders take `&str`. The YAML stays free of
    /// Rust-specific formatting.
    pub fn title(&self) -> String {
        match self {
            Self::StreamOnline { streamer_name, .. } => crate::t_str!(
                "notification.stream_online.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::StreamOffline { streamer_name, .. } => crate::t_str!(
                "notification.stream_offline.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadStarted { streamer_name, .. } => crate::t_str!(
                "notification.download_started.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadCompleted { streamer_name, .. } => crate::t_str!(
                "notification.download_completed.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadError { streamer_name, .. } => crate::t_str!(
                "notification.download_error.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::SegmentStarted {
                streamer_name,
                segment_index,
                ..
            } => crate::t_str!(
                "notification.segment_started.title",
                streamer_name = streamer_name.as_str(),
                segment_index = segment_index.to_string().as_str(),
            ),
            Self::SegmentCompleted {
                streamer_name,
                segment_index,
                ..
            } => crate::t_str!(
                "notification.segment_completed.title",
                streamer_name = streamer_name.as_str(),
                segment_index = segment_index.to_string().as_str(),
            ),
            Self::DownloadCancelled { streamer_name, .. } => crate::t_str!(
                "notification.download_cancelled.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::DownloadRejected { streamer_name, .. } => crate::t_str!(
                "notification.download_rejected.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::ConfigUpdated { streamer_name, .. } => crate::t_str!(
                "notification.config_updated.title",
                streamer_name = streamer_name.as_str(),
            ),
            Self::PipelineStarted { job_type, .. } => crate::t_str!(
                "notification.pipeline_started.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineCompleted { job_type, .. } => crate::t_str!(
                "notification.pipeline_completed.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineFailed { job_type, .. } => crate::t_str!(
                "notification.pipeline_failed.title",
                job_type = job_type.as_str(),
            ),
            Self::PipelineCancelled { job_type, .. } => crate::t_str!(
                "notification.pipeline_cancelled.title",
                job_type = job_type.as_str(),
            ),
            Self::FatalError {
                streamer_name,
                error_type,
                ..
            } => crate::t_str!(
                "notification.fatal_error.title",
                streamer_name = streamer_name.as_str(),
                error_type = error_type.as_str(),
            ),
            Self::OutOfSpace { path, .. } => {
                crate::t_str!("notification.out_of_space.title", path = path.as_str(),)
            }
            Self::OutputPathInaccessible { path, .. } => crate::t_str!(
                "notification.output_path_inaccessible.title",
                path = path.as_str(),
            ),
            Self::PipelineQueueWarning { queue_depth, .. } => crate::t_str!(
                "notification.pipeline_queue_warning.title",
                queue_depth = queue_depth.to_string().as_str(),
            ),
            Self::PipelineQueueCritical { queue_depth, .. } => crate::t_str!(
                "notification.pipeline_queue_critical.title",
                queue_depth = queue_depth.to_string().as_str(),
            ),
            Self::SystemStartup { version, .. } => crate::t_str!(
                "notification.system_startup.title",
                version = version.as_str(),
            ),
            Self::SystemShutdown { reason, .. } => crate::t_str!(
                "notification.system_shutdown.title",
                reason = reason.as_str(),
            ),
            Self::Credential { event } => credential_title(event),
        }
    }

    /// Get a detailed description of this event.
    pub fn description(&self) -> String {
        match self {
            Self::StreamOnline {
                title, category, ..
            } => match category {
                Some(cat) => crate::t_str!(
                    "notification.stream_online.description.with_category",
                    title = title.as_str(),
                    category = cat.as_str(),
                ),
                None => crate::t_str!(
                    "notification.stream_online.description.plain",
                    title = title.as_str(),
                ),
            },
            Self::StreamOffline { duration_secs, .. } => match duration_secs {
                Some(secs) => crate::t_str!(
                    "notification.stream_offline.description.with_duration",
                    duration = format_duration(*secs).as_str(),
                ),
                None => crate::t_str!("notification.stream_offline.description.plain"),
            },
            Self::DownloadStarted { session_id, .. } => crate::t_str!(
                "notification.download_started.description",
                session_id = session_id.as_str(),
            ),
            Self::DownloadCompleted {
                file_size_bytes,
                duration_secs,
                ..
            } => crate::t_str!(
                "notification.download_completed.description",
                size = format_bytes(*file_size_bytes).as_str(),
                duration = format_duration(*duration_secs).as_str(),
            ),
            Self::DownloadError {
                error_message,
                recoverable,
                ..
            } => {
                let key = if *recoverable {
                    "notification.download_error.description.recoverable"
                } else {
                    "notification.download_error.description.unrecoverable"
                };
                crate::t_str!(key, error_message = error_message.as_str())
            }
            Self::SegmentStarted { segment_path, .. } => crate::t_str!(
                "notification.segment_started.description",
                segment_path = segment_path.as_str(),
            ),
            Self::SegmentCompleted {
                segment_path,
                size_bytes,
                duration_secs,
                ..
            } => crate::t_str!(
                "notification.segment_completed.description",
                segment_path = segment_path.as_str(),
                size = format_bytes(*size_bytes).as_str(),
                duration = format_duration(*duration_secs).as_str(),
            ),
            Self::DownloadCancelled { session_id, .. } => crate::t_str!(
                "notification.download_cancelled.description",
                session_id = session_id.as_str(),
            ),
            Self::DownloadRejected { reason, .. } => crate::t_str!(
                "notification.download_rejected.description",
                reason = reason.as_str(),
            ),
            Self::ConfigUpdated { update_type, .. } => crate::t_str!(
                "notification.config_updated.description",
                update_type = update_type.as_str(),
            ),
            Self::PipelineStarted { job_id, .. } => crate::t_str!(
                "notification.pipeline_started.description",
                job_id = job_id.as_str(),
            ),
            Self::PipelineCompleted {
                output_path,
                duration_secs,
                ..
            } => {
                let duration = format_duration(*duration_secs);
                match output_path {
                    Some(path) => crate::t_str!(
                        "notification.pipeline_completed.description.with_output",
                        output_path = path.as_str(),
                        duration = duration.as_str(),
                    ),
                    None => crate::t_str!(
                        "notification.pipeline_completed.description.without_output",
                        duration = duration.as_str(),
                    ),
                }
            }
            Self::PipelineFailed { error_message, .. } => crate::t_str!(
                "notification.pipeline_failed.description",
                error_message = error_message.as_str(),
            ),
            Self::PipelineCancelled {
                job_id,
                pipeline_id,
                ..
            } => match pipeline_id {
                Some(pid) => crate::t_str!(
                    "notification.pipeline_cancelled.description.with_pipeline",
                    job_id = job_id.as_str(),
                    pipeline_id = pid.as_str(),
                ),
                None => crate::t_str!(
                    "notification.pipeline_cancelled.description.plain",
                    job_id = job_id.as_str(),
                ),
            },
            Self::FatalError { message, .. } => crate::t_str!(
                "notification.fatal_error.description",
                message = message.as_str(),
            ),
            Self::OutOfSpace {
                available_bytes,
                threshold_bytes,
                ..
            } => crate::t_str!(
                "notification.out_of_space.description",
                available = format_bytes(*available_bytes).as_str(),
                threshold = format_bytes(*threshold_bytes).as_str(),
            ),
            Self::OutputPathInaccessible {
                path, error_kind, ..
            } => {
                // Map the kind string to a per-kind i18n description; falls
                // back to the `other` key if we don't have a dedicated
                // branch. The kind strings here MUST stay in sync with
                // `IoErrorKindSer::as_str` (covered by a unit test in
                // traits.rs).
                let key = match error_kind.as_str() {
                    "not_found" => "notification.output_path_inaccessible.description.not_found",
                    "storage_full" => {
                        "notification.output_path_inaccessible.description.storage_full"
                    }
                    "permission_denied" => {
                        "notification.output_path_inaccessible.description.permission_denied"
                    }
                    "read_only" => "notification.output_path_inaccessible.description.read_only",
                    "timed_out" => "notification.output_path_inaccessible.description.timed_out",
                    _ => "notification.output_path_inaccessible.description.other",
                };
                crate::t_str!(key, path = path.as_str(), kind = error_kind.as_str())
            }
            Self::PipelineQueueWarning {
                queue_depth,
                threshold,
                ..
            } => crate::t_str!(
                "notification.pipeline_queue_warning.description",
                queue_depth = queue_depth.to_string().as_str(),
                threshold = threshold.to_string().as_str(),
            ),
            Self::PipelineQueueCritical {
                queue_depth,
                threshold,
                ..
            } => crate::t_str!(
                "notification.pipeline_queue_critical.description",
                queue_depth = queue_depth.to_string().as_str(),
                threshold = threshold.to_string().as_str(),
            ),
            Self::SystemStartup { version, .. } => crate::t_str!(
                "notification.system_startup.description",
                version = version.as_str(),
            ),
            Self::SystemShutdown { reason, .. } => crate::t_str!(
                "notification.system_shutdown.description",
                reason = reason.as_str(),
            ),
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
            | Self::OutputPathInaccessible { timestamp, .. }
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

/// Build the localized title for a [`CredentialEvent`] when wrapped in a
/// [`NotificationEvent::Credential`]. Split out because the credential
/// variant has two nested match layers (`CredentialEvent` variant +
/// `requires_relogin` branching in `RefreshFailed`); inlining it would
/// have made the main `title()` match unreadable.
fn credential_title(event: &CredentialEvent) -> String {
    match event {
        CredentialEvent::Refreshed {
            platform, scope, ..
        } => crate::t_str!(
            "notification.credential.refreshed.title",
            platform = platform.as_str(),
            scope = scope.describe().as_str(),
        ),
        CredentialEvent::RefreshFailed {
            platform,
            scope,
            requires_relogin,
            ..
        } => {
            let key = if *requires_relogin {
                "notification.credential.refresh_failed.title.requires_relogin"
            } else {
                "notification.credential.refresh_failed.title.retrying"
            };
            crate::t_str!(
                key,
                platform = platform.as_str(),
                scope = scope.describe().as_str(),
            )
        }
        CredentialEvent::Invalid {
            platform, scope, ..
        } => crate::t_str!(
            "notification.credential.invalid.title",
            platform = platform.as_str(),
            scope = scope.describe().as_str(),
        ),
        CredentialEvent::ExpiringSoon {
            platform, scope, ..
        } => crate::t_str!(
            "notification.credential.expiring_soon.title",
            platform = platform.as_str(),
            scope = scope.describe().as_str(),
        ),
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
    fn test_notification_priority_as_int() {
        assert_eq!(NotificationPriority::Low.as_int(), 2);
        assert_eq!(NotificationPriority::Normal.as_int(), 5);
        assert_eq!(NotificationPriority::High.as_int(), 8);
        assert_eq!(NotificationPriority::Critical.as_int(), 10);
    }

    #[test]
    fn test_notification_priority_from_int() {
        assert_eq!(
            NotificationPriority::from_int(0),
            Some(NotificationPriority::Low)
        );
        assert_eq!(
            NotificationPriority::from_int(2),
            Some(NotificationPriority::Low)
        );
        assert_eq!(
            NotificationPriority::from_int(3),
            Some(NotificationPriority::Low)
        );
        assert_eq!(
            NotificationPriority::from_int(4),
            Some(NotificationPriority::Normal)
        );
        assert_eq!(
            NotificationPriority::from_int(5),
            Some(NotificationPriority::Normal)
        );
        assert_eq!(
            NotificationPriority::from_int(7),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            NotificationPriority::from_int(8),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            NotificationPriority::from_int(10),
            Some(NotificationPriority::Critical)
        );
        assert_eq!(
            NotificationPriority::from_int(255),
            Some(NotificationPriority::Critical)
        );
    }

    #[test]
    fn test_notification_priority_int_roundtrip() {
        for p in [
            NotificationPriority::Low,
            NotificationPriority::Normal,
            NotificationPriority::High,
            NotificationPriority::Critical,
        ] {
            assert_eq!(NotificationPriority::from_int(p.as_int()), Some(p));
        }
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

    /// `rust_i18n::set_locale` mutates a process global, so locale-sensitive
    /// tests must serialize on this lock to avoid racing the i18n module's
    /// own tests (and each other).
    static OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn output_path_inaccessible_basic_metadata() {
        let event = NotificationEvent::OutputPathInaccessible {
            path: "/rec".to_string(),
            error_kind: "not_found".to_string(),
            timestamp: Utc::now(),
        };
        assert_eq!(event.priority(), NotificationPriority::Critical);
        assert_eq!(event.event_type(), "output_path_inaccessible");
        assert_eq!(event.streamer_id(), None, "infra event has no streamer_id");
    }

    #[test]
    fn output_path_inaccessible_localizes_to_english() {
        let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::i18n::set_locale("en");
        let event = NotificationEvent::OutputPathInaccessible {
            path: "/rec".to_string(),
            error_kind: "not_found".to_string(),
            timestamp: Utc::now(),
        };
        let title = event.title();
        let description = event.description();
        assert!(title.contains("/rec"), "title: {}", title);
        assert!(
            title.contains("Output path inaccessible"),
            "title: {}",
            title
        );
        assert!(description.contains("/rec"), "description: {}", description);
        assert!(
            description.contains("BaoTa"),
            "description should mention BaoTa for the not_found stale-mount case: {}",
            description
        );
        assert!(
            description.contains("restart"),
            "description should mention container restart: {}",
            description
        );
    }

    #[test]
    fn output_path_inaccessible_localizes_to_chinese() {
        let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::i18n::set_locale("zh-CN");
        let event = NotificationEvent::OutputPathInaccessible {
            path: "/rec".to_string(),
            error_kind: "not_found".to_string(),
            timestamp: Utc::now(),
        };
        let title = event.title();
        let description = event.description();
        assert!(title.contains("/rec"), "title: {}", title);
        assert!(title.contains("输出路径"), "title: {}", title);
        assert!(
            description.contains("宝塔"),
            "description should mention 宝塔 for the not_found stale-mount case: {}",
            description
        );
        assert!(
            description.contains("重启容器"),
            "description should mention container restart: {}",
            description
        );
        crate::i18n::set_locale("en");
    }

    #[test]
    fn output_path_inaccessible_all_error_kinds_resolve() {
        // Every IoErrorKindSer::as_str() value must map to a real i18n key,
        // not the literal key string. Defends against silent misalignment
        // between the YAML files and the description() match arms.
        let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::i18n::set_locale("en");
        for kind in [
            "not_found",
            "storage_full",
            "permission_denied",
            "read_only",
            "timed_out",
            "other",
        ] {
            let event = NotificationEvent::OutputPathInaccessible {
                path: "/rec".to_string(),
                error_kind: kind.to_string(),
                timestamp: Utc::now(),
            };
            let description = event.description();
            assert!(
                description.contains("/rec"),
                "kind={} description={:?}",
                kind,
                description
            );
            assert!(
                !description.starts_with("notification."),
                "kind={} returned untranslated key: {:?}",
                kind,
                description
            );
        }
    }

    #[test]
    fn output_path_inaccessible_in_event_type_registry() {
        let info = NotificationEvent::event_type_info("output_path_inaccessible")
            .expect("event type should be registered");
        assert_eq!(info.event_type, "output_path_inaccessible");
        assert_eq!(info.priority, NotificationPriority::Critical);

        // Aliases resolve back to the canonical type
        let from_camel = NotificationEvent::event_type_info("OutputPathInaccessible");
        assert!(from_camel.is_some());
        let from_dotted = NotificationEvent::event_type_info("output.path_inaccessible");
        assert!(from_dotted.is_some());
    }

    // ========== Full-notification i18n round-trip (Phase 2) ==========

    /// One plausible instance of every `NotificationEvent` variant.
    /// Field values are picked so the variant-specific placeholders are
    /// present and checkable after localization.
    ///
    /// New variants added to `NotificationEvent` MUST be added here or
    /// `all_notification_variants_localize` will catch the omission via
    /// the exhaustive count assertion.
    fn sample_events() -> Vec<NotificationEvent> {
        use crate::credentials::{CredentialEvent, CredentialScope};
        let now = Utc::now();
        vec![
            NotificationEvent::StreamOnline {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                title: "Test title".into(),
                category: Some("Gaming".into()),
                timestamp: now,
            },
            NotificationEvent::StreamOffline {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                duration_secs: Some(1234.0),
                timestamp: now,
            },
            NotificationEvent::DownloadStarted {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                session_id: "sess-1".into(),
                timestamp: now,
            },
            NotificationEvent::DownloadCompleted {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                session_id: "sess-1".into(),
                file_size_bytes: 1024 * 1024 * 100,
                duration_secs: 3600.0,
                timestamp: now,
            },
            NotificationEvent::DownloadError {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                error_message: "timeout".into(),
                recoverable: true,
                timestamp: now,
            },
            NotificationEvent::SegmentStarted {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                session_id: "sess-1".into(),
                segment_path: "/rec/seg-0001.mp4".into(),
                segment_index: 1,
                timestamp: now,
            },
            NotificationEvent::SegmentCompleted {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                session_id: "sess-1".into(),
                segment_path: "/rec/seg-0001.mp4".into(),
                segment_index: 1,
                size_bytes: 1024 * 1024,
                duration_secs: 10.0,
                timestamp: now,
            },
            NotificationEvent::DownloadCancelled {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                session_id: "sess-1".into(),
                timestamp: now,
            },
            NotificationEvent::DownloadRejected {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                session_id: "sess-1".into(),
                reason: "circuit breaker open".into(),
                timestamp: now,
            },
            NotificationEvent::ConfigUpdated {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                update_type: "Cookies".into(),
                timestamp: now,
            },
            NotificationEvent::PipelineStarted {
                job_id: "job-1".into(),
                job_type: "remux".into(),
                streamer_id: "s1".into(),
                timestamp: now,
            },
            NotificationEvent::PipelineCompleted {
                job_id: "job-1".into(),
                job_type: "remux".into(),
                output_path: Some("/out/final.mp4".into()),
                duration_secs: 42.0,
                timestamp: now,
            },
            NotificationEvent::PipelineFailed {
                job_id: "job-1".into(),
                job_type: "remux".into(),
                error_message: "ffmpeg exited with code 1".into(),
                timestamp: now,
            },
            NotificationEvent::PipelineCancelled {
                job_id: "job-1".into(),
                job_type: "remux".into(),
                pipeline_id: Some("pipeline-xyz".into()),
                timestamp: now,
            },
            NotificationEvent::FatalError {
                streamer_id: "s1".into(),
                streamer_name: "TestStreamer".into(),
                error_type: "ProtocolError".into(),
                message: "connection reset".into(),
                timestamp: now,
            },
            NotificationEvent::OutOfSpace {
                path: "/rec".into(),
                available_bytes: 1024 * 1024,
                threshold_bytes: 1024 * 1024 * 1024,
                timestamp: now,
            },
            NotificationEvent::OutputPathInaccessible {
                path: "/rec".into(),
                error_kind: "not_found".into(),
                timestamp: now,
            },
            NotificationEvent::PipelineQueueWarning {
                queue_depth: 120,
                threshold: 100,
                timestamp: now,
            },
            NotificationEvent::PipelineQueueCritical {
                queue_depth: 500,
                threshold: 200,
                timestamp: now,
            },
            NotificationEvent::SystemStartup {
                version: "0.2.1".into(),
                timestamp: now,
            },
            NotificationEvent::SystemShutdown {
                reason: "SIGTERM".into(),
                timestamp: now,
            },
            NotificationEvent::Credential {
                event: CredentialEvent::Refreshed {
                    scope: CredentialScope::Platform {
                        platform_id: "bilibili".into(),
                        platform_name: "bilibili".into(),
                    },
                    platform: "bilibili".into(),
                    expires_at: Some(now),
                    timestamp: now,
                },
            },
            NotificationEvent::Credential {
                event: CredentialEvent::RefreshFailed {
                    scope: CredentialScope::Platform {
                        platform_id: "bilibili".into(),
                        platform_name: "bilibili".into(),
                    },
                    platform: "bilibili".into(),
                    error: "401 Unauthorized".into(),
                    requires_relogin: true,
                    failure_count: 3,
                    timestamp: now,
                },
            },
            NotificationEvent::Credential {
                event: CredentialEvent::RefreshFailed {
                    scope: CredentialScope::Platform {
                        platform_id: "bilibili".into(),
                        platform_name: "bilibili".into(),
                    },
                    platform: "bilibili".into(),
                    error: "429 Rate Limited".into(),
                    requires_relogin: false,
                    failure_count: 1,
                    timestamp: now,
                },
            },
            NotificationEvent::Credential {
                event: CredentialEvent::Invalid {
                    scope: CredentialScope::Platform {
                        platform_id: "bilibili".into(),
                        platform_name: "bilibili".into(),
                    },
                    platform: "bilibili".into(),
                    reason: "token revoked".into(),
                    error_code: Some(-412),
                    timestamp: now,
                },
            },
            NotificationEvent::Credential {
                event: CredentialEvent::ExpiringSoon {
                    scope: CredentialScope::Platform {
                        platform_id: "bilibili".into(),
                        platform_name: "bilibili".into(),
                    },
                    platform: "bilibili".into(),
                    expires_at: now,
                    days_remaining: 5,
                    timestamp: now,
                },
            },
        ]
    }

    /// For every variant, assert that `title()` and `description()` produce
    /// non-empty strings that are NOT the raw key literal (which is what
    /// rust-i18n returns when a key is missing from every locale). Runs
    /// against both `en` and `zh-CN` so missing translations in either
    /// locale are caught at CI time.
    #[test]
    fn all_notification_variants_localize_in_both_locales() {
        let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        let events = sample_events();
        // If a new variant is added to NotificationEvent without extending
        // sample_events, we want a visible signal. This assert is a
        // self-doc; bump it alongside the match arms in title/description.
        assert_eq!(
            events.len(),
            26,
            "sample_events is out of sync with NotificationEvent; add a sample for the new variant so its localization is covered"
        );

        for locale in ["en", "zh-CN"] {
            crate::i18n::set_locale(locale);
            for (i, event) in events.iter().enumerate() {
                let title = event.title();
                let description = event.description();
                assert!(
                    !title.is_empty(),
                    "locale={} variant #{} ({}): empty title",
                    locale,
                    i,
                    event.event_type(),
                );
                assert!(
                    !description.is_empty(),
                    "locale={} variant #{} ({}): empty description",
                    locale,
                    i,
                    event.event_type(),
                );
                assert!(
                    !title.starts_with("notification."),
                    "locale={} variant #{} ({}): title returned the raw key ({:?}) — translation missing",
                    locale,
                    i,
                    event.event_type(),
                    title,
                );
                assert!(
                    !description.starts_with("notification."),
                    "locale={} variant #{} ({}): description returned the raw key ({:?}) — translation missing",
                    locale,
                    i,
                    event.event_type(),
                    description,
                );
            }
        }

        crate::i18n::set_locale("en");
    }

    /// Spot-check that a few high-visibility variants render recognizable
    /// Chinese text when the locale is zh-CN. Cheap but catches accidental
    /// copy-paste of English into the Chinese YAML.
    #[test]
    fn zh_cn_has_actual_chinese_text() {
        let _g = OUTPUT_PATH_INACCESSIBLE_LOCALE_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());

        crate::i18n::set_locale("zh-CN");

        let online = NotificationEvent::StreamOnline {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            title: "Test title".into(),
            category: None,
            timestamp: Utc::now(),
        };
        assert!(
            online.title().contains("开播"),
            "StreamOnline title should contain '开播', got: {}",
            online.title()
        );

        let fatal = NotificationEvent::FatalError {
            streamer_id: "s1".into(),
            streamer_name: "TestStreamer".into(),
            error_type: "Protocol".into(),
            message: "boom".into(),
            timestamp: Utc::now(),
        };
        assert!(
            fatal.title().contains("致命错误"),
            "FatalError title should contain '致命错误', got: {}",
            fatal.title()
        );

        let startup = NotificationEvent::SystemStartup {
            version: "0.2.1".into(),
            timestamp: Utc::now(),
        };
        assert!(
            startup.title().contains("系统已启动"),
            "SystemStartup title should contain '系统已启动', got: {}",
            startup.title()
        );

        crate::i18n::set_locale("en");
    }
}
