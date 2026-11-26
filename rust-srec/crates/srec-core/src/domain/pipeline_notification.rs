//! Notifications from the recording control plane to the pipeline engine.

use std::path::PathBuf;

/// Post-commit notifications forwarded from the recording control plane to the
/// pipeline engine.
///
/// Video and danmu segment rows and session-end state are already durably
/// committed before these notifications are delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineNotification {
    VideoSegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        path: PathBuf,
        auto_thumbnail_candidate: bool,
    },
    DanmuCollectionStarted {
        session_id: String,
        streamer_id: String,
    },
    DanmuSegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        path: PathBuf,
        message_count: u64,
    },
    DanmuCollectionStopped {
        session_id: String,
        total_count: u64,
    },
    SessionEnded {
        session_id: String,
        streamer_id: String,
        /// Name of the cause that ended the session, for logs.
        cause: String,
        should_run_session_complete: bool,
    },
}
