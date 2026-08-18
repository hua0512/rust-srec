//! Danmu service events and commands.
//!
//! This module defines the events emitted by the danmu service and the
//! internal commands used to control collection sessions.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::danmu::DanmuControlEvent;

use super::lifecycle::CollectionStopReason;

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
    /// Collection stopped for a session.
    ///
    /// Carries only the message total, not the full `DanmuStatistics`: the
    /// statistics are persisted by `persist_statistics` before this is emitted,
    /// and the aggregate vectors would otherwise be cloned into the broadcast
    /// channel for every subscriber when the sole consumer wants the count.
    CollectionStopped {
        session_id: String,
        total_count: u64,
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
    /// Connection lost; a reconnect attempt is scheduled.
    Reconnecting { session_id: String, attempt: u32 },
    /// An item arrived again after `Reconnecting`, ending the outage.
    Reconnected {
        session_id: String,
        /// Reconnect attempts the recovery took.
        attempts: u32,
        /// How long the link was down.
        downtime_secs: u64,
    },
    /// The link has been down long enough to be worth attention. Collection keeps
    /// reconnecting — `CollectionRunner` gives up only when the session ends — so
    /// this is an alert, not a terminal state.
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
    Stop(CollectionStopReason),
}
