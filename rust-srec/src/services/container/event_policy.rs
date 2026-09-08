//! Decisions shared by service event subscriptions.

use tracing::{debug, warn};

use crate::downloader::engine::DownloadProgress;

pub(super) fn should_end_stream_on_danmu_stream_closed(
    platform_specific_config: Option<&str>,
) -> bool {
    platform_specific_config
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|value| {
            value
                .get("end_stream_on_danmu_stream_closed")
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(true)
}

pub(super) const RECOVERY_PROGRESS_MIN_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn has_transient_error_state(metadata: &crate::streamer::StreamerMetadata) -> bool {
    metadata.consecutive_error_count > 0
        || metadata.disabled_until.is_some()
        || metadata.last_error.is_some()
}

pub(super) fn should_record_recovery_from_progress(progress: &DownloadProgress) -> bool {
    progress.segments_completed > 0
        || (progress.bytes_downloaded >= RECOVERY_PROGRESS_MIN_BYTES
            && progress.speed_bytes_per_sec > 0)
}

pub(super) fn broadcast_error_is_recoverable(
    subscriber: &'static str,
    error: tokio::sync::broadcast::error::RecvError,
) -> bool {
    match error {
        tokio::sync::broadcast::error::RecvError::Lagged(skipped) => {
            warn!(
                subscriber,
                skipped, "Broadcast subscriber lagged; continuing from the newest available event"
            );
            true
        }
        tokio::sync::broadcast::error::RecvError::Closed => {
            debug!(subscriber, "Broadcast channel closed");
            false
        }
    }
}
