use std::collections::HashMap;

use crate::danmu::DanmuStatistics;
use crate::domain::DanmuStatisticsConfig;
use crate::error::Error;

/// Everything needed to start one danmu collection.
#[derive(Debug, Clone)]
pub struct CollectionSpec {
    pub session_id: String,
    pub streamer_id: String,
    pub streamer_url: String,
    pub cookies: Option<String>,
    pub extras: Option<HashMap<String, String>>,
    pub statistics: DanmuStatisticsConfig,
}

/// Why the service asked a collection to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectionStopReason {
    SessionEnded,
    ServiceShutdown,
}

/// Why a collection runner finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CollectionExitReason {
    SessionStopped,
    StreamClosed,
    ServiceShutdown,
    CommandChannelClosed,
    Cancelled,
    Failed,
}

impl CollectionExitReason {
    /// Interrupted collections may resume later, so their checkpoints must stay.
    pub(crate) fn retains_checkpoint(self) -> bool {
        matches!(
            self,
            Self::ServiceShutdown | Self::CommandChannelClosed | Self::Cancelled
        )
    }
}

impl From<CollectionStopReason> for CollectionExitReason {
    fn from(reason: CollectionStopReason) -> Self {
        match reason {
            CollectionStopReason::SessionEnded => Self::SessionStopped,
            CollectionStopReason::ServiceShutdown => Self::ServiceShutdown,
        }
    }
}

/// Complete result produced by one collection task.
#[derive(Debug)]
pub(crate) struct CollectionOutcome {
    pub statistics: DanmuStatistics,
    pub reason: CollectionExitReason,
    pub error: Option<Error>,
    pub cleanup_errors: Vec<Error>,
}
