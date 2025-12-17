//! Protocol Buffer types for WebSocket API.
//!
//! This module contains the generated protobuf types and conversion traits
//! for the download progress WebSocket API.

use crate::downloader::engine::DownloadInfo;

// Include the generated protobuf code
pub mod download_progress {
    include!(concat!(env!("OUT_DIR"), "/download_progress.rs"));
}

// TODO: REXPORT DOUYIN PROTO
pub mod douyin_proto {
    include!(concat!(env!("OUT_DIR"), "/douyin.rs"));
}

// Log event protobuf types
pub mod log_event {
    include!(concat!(env!("OUT_DIR"), "/log_event.rs"));
}

// Re-export commonly used types
pub use download_progress::{
    ClientMessage, DownloadCancelled, DownloadCompleted, DownloadFailed, DownloadProgress,
    DownloadSnapshot, DownloadStarted, ErrorPayload, EventType, SegmentCompleted, SubscribeRequest,
    UnsubscribeRequest, WsMessage,
};

impl From<&DownloadInfo> for DownloadProgress {
    fn from(info: &DownloadInfo) -> Self {
        Self {
            download_id: info.id.clone(),
            streamer_id: info.streamer_id.clone(),
            session_id: info.session_id.clone(),
            engine_type: info.engine_type.as_str().to_string(),
            status: format!("{:?}", info.status),
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
            started_at_ms: info.started_at.timestamp_millis(),
        }
    }
}

impl From<DownloadInfo> for DownloadProgress {
    fn from(info: DownloadInfo) -> Self {
        DownloadProgress::from(&info)
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

/// Create a snapshot message from a list of download infos.
pub fn create_snapshot_message(downloads: Vec<DownloadInfo>) -> WsMessage {
    let progress_list: Vec<DownloadProgress> = downloads.into_iter().map(Into::into).collect();

    WsMessage {
        event_type: EventType::Snapshot as i32,
        payload: Some(download_progress::ws_message::Payload::Snapshot(
            DownloadSnapshot {
                downloads: progress_list,
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
    fn test_download_info_to_progress_conversion() {
        let info = create_test_download_info();
        let progress: DownloadProgress = info.into();

        assert_eq!(progress.download_id, "download-123");
        assert_eq!(progress.streamer_id, "streamer-456");
        assert_eq!(progress.session_id, "session-789");
        assert_eq!(progress.engine_type, "ffmpeg");
        assert_eq!(progress.status, "Downloading");
        assert_eq!(progress.bytes_downloaded, 1024000);
        assert_eq!(progress.segments_completed, 5);
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
        let msg = create_snapshot_message(downloads);

        assert_eq!(msg.event_type, EventType::Snapshot as i32);
        assert!(msg.payload.is_some());
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
