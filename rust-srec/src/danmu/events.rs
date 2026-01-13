//! Danmu service events and commands.
//!
//! This module defines the events emitted by the danmu service and the
//! internal commands used to control collection sessions.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::danmu::{DanmuControlEvent, DanmuStatistics};

/// Events emitted by the danmu service.
///
/// These events can be subscribed to via `DanmuService::subscribe()` to
/// monitor the progress and status of danmu collection.
#[derive(Debug, Clone)]
pub enum DanmuEvent {
    /// Collection started for a session
    CollectionStarted {
        session_id: String,
        streamer_id: String,
    },
    /// Collection stopped for a session
    CollectionStopped {
        session_id: String,
        statistics: DanmuStatistics,
    },
    /// Segment file started
    SegmentStarted {
        session_id: String,
        streamer_id: String,
        segment_id: String,
        output_path: PathBuf,
        /// The start time of this segment (for danmu timestamp offset calculation).
        start_time: DateTime<Utc>,
    },
    /// Segment file completed
    SegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_id: String,
        output_path: PathBuf,
        message_count: u64,
    },
    /// Platform control event (best-effort signal derived from danmu stream).
    ///
    /// When a provider emits `DanmuControlEvent::StreamClosed`, the runner will shut down
    /// gracefully, which may be followed by `SegmentCompleted` (if a segment is active) and then
    /// `CollectionStopped`.
    Control {
        session_id: String,
        streamer_id: String,
        platform: String,
        control: DanmuControlEvent,
    },
    /// Connection lost and reconnecting
    Reconnecting { session_id: String, attempt: u32 },
    /// Reconnection failed
    ReconnectFailed { session_id: String, error: String },
    /// Error during collection
    Error { session_id: String, error: String },
}

/// Commands sent to the collection task.
///
/// These are internal commands used to control segment file writing
/// and stop collection from the `CollectionHandle`.
#[derive(Debug)]
pub(crate) enum CollectionCommand {
    /// Start a new segment file
    StartSegment {
        segment_id: String,
        output_path: PathBuf,
        /// The start time of this segment (for danmu timestamp offset calculation).
        start_time: DateTime<Utc>,
    },
    /// End the current segment file
    EndSegment { segment_id: String },
    /// Stop collection entirely
    Stop,
}
