//! Protocol Buffer types for WebSocket API.
//!
//! This module contains the generated protobuf types and conversion traits
//! for the download progress WebSocket API.

use std::collections::HashMap;

use crate::downloader::engine::DownloadInfo;
use crate::downloader::queue::PendingEntry as QueuePendingEntry;
use crate::pipeline::{ActiveJobInfo, JobPhase, JobProgressSnapshot};

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
    DownloadState, EventType, JobEnded, JobStarted, JobState, SegmentCompleted,
    StreamerCheckRecorded, UploadProgress, UploadQueue, UploadStarted, UploadState, UploadTerminal,
    UploadTerminalStatus, WsMessage,
};

impl From<&DownloadInfo> for DownloadMeta {
    fn from(info: &DownloadInfo) -> Self {
        Self {
            manual_split: Some(manual_split_to_proto(&info.manual_split)),
            download_id: info.id.clone(),
            streamer_id: info.streamer_id.clone(),
            session_id: info.session_id.clone(),
            engine_type: info.engine_type.as_str().to_string(),
            started_at_ms: info.started_at.timestamp_millis(),
            // Identity metadata is immutable; manual_split has its own revision.
            updated_at_ms: info.started_at.timestamp_millis(),
            cdn_host: crate::utils::url::extract_host(&info.url).unwrap_or_default(),
            download_url: info.url.clone(),
        }
    }
}

pub fn manual_split_to_proto(
    state: &pipeline_common::ManualSplitSnapshot,
) -> download_progress::ManualSplitState {
    download_progress::ManualSplitState {
        supported: state.supported,
        unavailable_reason: state.unavailable_reason.to_string(),
        request_id: state.request_id,
        revision: state.revision,
        status: state.status.as_str().to_string(),
        expiry_reason: state
            .expiry_reason
            .map(|reason| reason.as_str().to_string())
            .unwrap_or_default(),
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

/// Wire shape of one progress update. Shared between the live
/// `UPLOAD_PROGRESS` and `JOB_PROGRESS` events and the snapshot slices so
/// every path serializes a `JobProgressSnapshot` identically.
pub fn upload_progress_to_proto(
    job_id: &str,
    streamer_id: Option<&str>,
    snapshot: &JobProgressSnapshot,
) -> UploadProgress {
    UploadProgress {
        job_id: job_id.to_string(),
        streamer_id: streamer_id.unwrap_or_default().to_string(),
        percent: snapshot.percent,
        bytes_done: snapshot.bytes_done,
        bytes_total: snapshot.bytes_total,
        speed_bytes_per_sec: snapshot.speed_bytes_per_sec,
        eta_secs: snapshot.eta_secs,
        updated_at_ms: snapshot.updated_at.timestamp_millis(),
        files_done: snapshot.files_done.map(saturating_u32),
        files_total: snapshot.files_total.map(saturating_u32),
        out_time_ms: snapshot.out_time_ms,
        duration_ms: snapshot.duration_ms,
        speed_x: snapshot.speed_x,
        phase: snapshot.phase.map(|phase| job_phase_to_proto(phase) as i32),
        current_file: snapshot.current_file.clone(),
        attempt: snapshot.attempt,
        max_attempts: snapshot.max_attempts,
    }
}

fn job_phase_to_proto(phase: JobPhase) -> download_progress::JobPhase {
    match phase {
        JobPhase::Finalizing => download_progress::JobPhase::Finalizing,
        JobPhase::Retrying => download_progress::JobPhase::Retrying,
        JobPhase::Waiting => download_progress::JobPhase::Waiting,
    }
}

fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Job state carried by a snapshot: upload and long jobs being processed
/// plus how many uploads are waiting for a worker.
#[derive(Default)]
pub struct SnapshotJobs {
    pub active: Vec<ActiveJobInfo>,
    pub pending_uploads: u32,
    /// Avatar URL by streamer id, for the streamers in `active`.
    pub streamer_avatars: HashMap<String, String>,
}

/// Create a snapshot message from a list of download infos plus the
/// list of currently-queued pending acquires (downloads that have
/// emitted `DownloadQueued` but not yet received their slot) and the
/// jobs shown live.
pub fn create_snapshot_message(
    downloads: Vec<DownloadInfo>,
    queued: Vec<QueuePendingEntry>,
    jobs: SnapshotJobs,
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

    // Each entry is what a client would have assembled from the start event
    // plus the latest progress event.
    let mut upload_msgs = Vec::new();
    let mut job_msgs = Vec::new();
    for info in jobs.active {
        let streamer_id = info.streamer_id.unwrap_or_default();
        let streamer_avatar = jobs
            .streamer_avatars
            .get(&streamer_id)
            .cloned()
            .unwrap_or_default();
        let progress = info
            .progress
            .as_ref()
            .map(|s| upload_progress_to_proto(&info.job_id, Some(&streamer_id), s));
        let started_at_ms = info
            .started_at
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_default();
        let streamer_name = info.streamer_name.unwrap_or_default();
        match info.uploader {
            Some(uploader) => upload_msgs.push(UploadState {
                started: Some(UploadStarted {
                    job_id: info.job_id,
                    streamer_id,
                    session_id: info.session_id.unwrap_or_default(),
                    uploader: uploader.to_string(),
                    files_total: info.files_total,
                    started_at_ms,
                    streamer_name,
                    streamer_avatar,
                }),
                progress,
            }),
            None => job_msgs.push(JobState {
                started: Some(JobStarted {
                    job_id: info.job_id,
                    streamer_id,
                    job_type: info.job_type,
                    started_at_ms,
                    streamer_name,
                    streamer_avatar,
                }),
                progress,
            }),
        }
    }

    WsMessage {
        event_type: EventType::Snapshot as i32,
        payload: Some(download_progress::ws_message::Payload::Snapshot(
            DownloadSnapshot {
                downloads: states,
                queued: queued_msgs,
                uploads: upload_msgs,
                pending_uploads: jobs.pending_uploads,
                jobs: job_msgs,
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
            manual_split: Default::default(),
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
    fn upload_progress_carries_the_phase_current_file_and_attempt() {
        let mut snapshot = JobProgressSnapshot::new(crate::pipeline::ProgressKind::Ffmpeg);
        snapshot.phase = Some(JobPhase::Finalizing);
        snapshot.current_file = Some("seg_002.flv".to_string());
        let wire = upload_progress_to_proto("job", None, &snapshot);
        assert_eq!(
            (wire.phase(), wire.current_file.as_deref()),
            (download_progress::JobPhase::Finalizing, Some("seg_002.flv"))
        );

        snapshot.phase = Some(JobPhase::Retrying);
        snapshot.attempt = Some(2);
        snapshot.max_attempts = Some(3);
        let wire = upload_progress_to_proto("job", None, &snapshot);
        assert_eq!(
            (wire.phase(), wire.attempt, wire.max_attempts),
            (download_progress::JobPhase::Retrying, Some(2), Some(3))
        );
    }

    #[test]
    fn test_create_snapshot_message() {
        let downloads = vec![create_test_download_info()];
        let msg = create_snapshot_message(downloads, Vec::new(), SnapshotJobs::default());

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
        let msg = create_snapshot_message(downloads, queued, SnapshotJobs::default());

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
}
