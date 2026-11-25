//! Monitor support modules for live status detection, batch precheck,
//! Bigo live notifications, rate limiting, and check history recording.

mod batch_precheck;
mod bigo_notifications;
pub(crate) mod check_history_writer;
mod detector;
mod events;
mod rate_limiter;
mod types;

pub use crate::domain::streamer::FatalErrorType;
pub(crate) use batch_precheck::{BatchAnswer, BatchPrecheck, GroupKey, Poll};
pub(crate) use bigo_notifications::{BigoCheckTrigger, BigoNotificationDeps};
pub use bigo_notifications::{
    BigoNotificationStatus, BigoNotifications, ListenerState, MonitoringMethod, PollingReason,
    StreamerNotificationMethod,
};
pub(crate) use check_history_writer::{CheckHistoryBroadcaster, CheckHistoryWriter};
pub(crate) use detector::{CheckContext, DetectionResult};
pub use detector::{FilterReason, LiveStatus, StreamDetector, StreamInfo};
pub use events::MonitorEvent;
pub use rate_limiter::{RateLimiter, RateLimiterConfig, RateLimiterManager};
pub(crate) use types::batch_detection_enabled;
pub use types::{InfraBlockReason, StreamMonitorConfig};
