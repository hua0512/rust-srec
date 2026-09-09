use super::NotificationPriority;

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
        event_type: "gpu_unavailable",
        label: "GPU Unavailable",
        priority: NotificationPriority::Critical,
        aliases: &["gpu_unavailable", "gpu.unavailable", "GpuUnavailable"],
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
    // ========== External Tool Events ==========
    NotificationEventTypeInfo {
        event_type: "baidupcs_relogin_failed",
        label: "Baidu Netdisk Re-login Failed",
        priority: NotificationPriority::High,
        aliases: &[
            "baidupcs_relogin_failed",
            "baidupcs.relogin_failed",
            "BaiduPcsReloginFailed",
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
