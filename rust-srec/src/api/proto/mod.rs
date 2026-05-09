//! Protocol Buffer types for WebSocket API.
//!
//! This module contains the generated protobuf types and conversion traits
//! for the download progress WebSocket API.

use crate::downloader::engine::DownloadInfo;
use crate::downloader::queue::PendingEntry as QueuePendingEntry;

// Include the generated protobuf code
pub mod download_progress {
    include!(concat!(env!("OUT_DIR"), "/download_progress.rs"));
}

// Log event protobuf types
pub mod log_event {
    include!(concat!(env!("OUT_DIR"), "/log_event.rs"));
}

// Re-export commonly used types
pub use download_progress::{
    ClientMessage, DownloadCancelled, DownloadCompleted, DownloadDequeued, DownloadFailed,
    DownloadMeta, DownloadMetrics, DownloadQueued, DownloadRejected, DownloadSnapshot,
    DownloadState, ErrorPayload, EventType, SegmentCompleted, StreamerCheckRecorded,
    SubscribeRequest, UnsubscribeRequest, WsMessage,
};

impl From<&DownloadInfo> for DownloadMeta {
    fn from(info: &DownloadInfo) -> Self {
        Self {
            download_id: info.id.clone(),
            streamer_id: info.streamer_id.clone(),
            session_id: info.session_id.clone(),
            engine_type: info.engine_type.as_str().to_string(),
            started_at_ms: info.started_at.timestamp_millis(),
            // Snapshot meta is immutable, so updated_at tracks started_at.
            updated_at_ms: info.started_at.timestamp_millis(),
            cdn_host: crate::utils::url::extract_host(&info.url).unwrap_or_default(),
            download_url: info.url.clone(),
        }
    }
}

impl From<&DownloadInfo> for DownloadMetrics {
    fn from(info: &DownloadInfo) -> Self {
        Self {
            download_id: info.id.clone(),
            status: info.status.as_str().to_string(),
            bytes_downloaded: info.progress.bytes_downloaded,
            duration_secs: info.progress.duration_secs,
            speed_bytes_per_sec: safe_speed(
                info.progress.bytes_downloaded,
                info.progress.duration_secs,
            ),
            segments_completed: info.progress.segments_completed,
            media_duration_secs: info.progress.media_duration_secs,
            playback_ratio: safe_playback_ratio(
                info.progress.media_duration_secs,
                info.progress.duration_secs,
            ),
        }
    }
}

/// Safely compute speed, returning 0 if elapsed time is zero to avoid division errors.
fn safe_speed(bytes: u64, duration_secs: f64) -> u64 {
    if duration_secs <= 0.0 {
        0
    } else {
        (bytes as f64 / duration_secs) as u64
    }
}

/// Safely compute playback ratio, returning 0.0 if elapsed time is zero to avoid division errors.
fn safe_playback_ratio(media_duration_secs: f64, elapsed_secs: f64) -> f64 {
    if elapsed_secs <= 0.0 {
        0.0
    } else {
        media_duration_secs / elapsed_secs
    }
}

/// Create a snapshot message from a list of download infos plus the
/// list of currently-queued pending acquires (downloads that have
/// emitted `DownloadQueued` but not yet received their slot).
pub fn create_snapshot_message(
    downloads: Vec<DownloadInfo>,
    queued: Vec<QueuePendingEntry>,
) -> WsMessage {
    let states: Vec<DownloadState> = downloads
        .iter()
        .map(|d| DownloadState {
            meta: Some(DownloadMeta::from(d)),
            metrics: Some(DownloadMetrics::from(d)),
        })
        .collect();

    let queued_msgs: Vec<DownloadQueued> = queued
        .into_iter()
        .map(|q| DownloadQueued {
            streamer_id: q.streamer_id,
            session_id: q.session_id,
            streamer_name: q.streamer_name,
            engine_type: q.engine_type.as_str().to_string(),
            queued_at_ms: q.queued_at_ms,
            is_high_priority: q.priority.is_high(),
        })
        .collect();

    WsMessage {
        event_type: EventType::Snapshot as i32,
        payload: Some(download_progress::ws_message::Payload::Snapshot(
            DownloadSnapshot {
                downloads: states,
                queued: queued_msgs,
            },
        )),
    }
}

/// Create an error message.
pub fn create_error_message(code: &str, message: &str) -> WsMessage {
    WsMessage {
        event_type: EventType::Error as i32,
        payload: Some(download_progress::ws_message::Payload::Error(
            ErrorPayload {
                code: code.to_string(),
                message: message.to_string(),
            },
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::engine::{
        DownloadProgress as InternalProgress, DownloadStatus, EngineType,
    };
    use chrono::Utc;

    fn create_test_download_info() -> DownloadInfo {
        DownloadInfo {
            id: "download-123".to_string(),
            url: "https://example.com/stream".to_string(),
            streamer_id: "streamer-456".to_string(),
            session_id: "session-789".to_string(),
            engine_type: EngineType::Ffmpeg,
            status: DownloadStatus::Downloading,
            progress: InternalProgress {
                bytes_downloaded: 1024000,
                duration_secs: 60.0,
                speed_bytes_per_sec: 17066,
                segments_completed: 5,
                current_segment: Some("segment_005.ts".to_string()),
                media_duration_secs: 65.0,
                playback_ratio: 1.083,
            },
            started_at: Utc::now(),
        }
    }

    #[test]
    fn test_download_info_to_meta_metrics_conversion() {
        let info = create_test_download_info();
        let meta = DownloadMeta::from(&info);
        let metrics = DownloadMetrics::from(&info);

        assert_eq!(meta.download_id, "download-123");
        assert_eq!(meta.streamer_id, "streamer-456");
        assert_eq!(meta.session_id, "session-789");
        assert_eq!(meta.engine_type, "ffmpeg");
        assert!(!meta.download_url.is_empty());

        assert_eq!(metrics.download_id, "download-123");
        assert_eq!(metrics.status, "downloading");
        assert_eq!(metrics.bytes_downloaded, 1024000);
        assert_eq!(metrics.segments_completed, 5);
    }

    #[test]
    fn test_safe_speed_zero_duration() {
        assert_eq!(safe_speed(1000, 0.0), 0);
        assert_eq!(safe_speed(1000, -1.0), 0);
    }

    #[test]
    fn test_safe_playback_ratio_zero_duration() {
        assert_eq!(safe_playback_ratio(60.0, 0.0), 0.0);
        assert_eq!(safe_playback_ratio(60.0, -1.0), 0.0);
    }

    #[test]
    fn test_create_snapshot_message() {
        let downloads = vec![create_test_download_info()];
        let msg = create_snapshot_message(downloads, Vec::new());

        assert_eq!(msg.event_type, EventType::Snapshot as i32);
        assert!(msg.payload.is_some());
    }

    #[test]
    fn test_create_snapshot_message_with_queued() {
        use crate::downloader::engine::EngineType;
        use crate::downloader::{Priority, queue::PendingEntry};
        let downloads = vec![create_test_download_info()];
        let queued = vec![PendingEntry {
            session_id: "queued-session".to_string(),
            streamer_id: "streamer-q".to_string(),
            streamer_name: "Queued Streamer".to_string(),
            engine_type: EngineType::Ffmpeg,
            priority: Priority::High,
            queued_at_ms: 1234567890,
        }];
        let msg = create_snapshot_message(downloads, queued);

        assert_eq!(msg.event_type, EventType::Snapshot as i32);
        if let Some(download_progress::ws_message::Payload::Snapshot(s)) = msg.payload {
            assert_eq!(s.downloads.len(), 1);
            assert_eq!(s.queued.len(), 1);
            assert_eq!(s.queued[0].streamer_id, "streamer-q");
            assert_eq!(s.queued[0].session_id, "queued-session");
            assert!(s.queued[0].is_high_priority);
            assert_eq!(s.queued[0].engine_type, "ffmpeg");
        } else {
            panic!("expected snapshot payload");
        }
    }

    #[test]
    fn test_create_error_message() {
        let msg = create_error_message("TEST_ERROR", "Test error message");

        assert_eq!(msg.event_type, EventType::Error as i32);
        if let Some(download_progress::ws_message::Payload::Error(err)) = msg.payload {
            assert_eq!(err.code, "TEST_ERROR");
            assert_eq!(err.message, "Test error message");
        } else {
            panic!("Expected error payload");
        }
    }
}
