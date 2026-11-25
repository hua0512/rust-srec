//! Stream Monitor module for detecting live status.
//!
//! The Stream Monitor is responsible for:
//! - Checking individual streamer live status
//! - Filter evaluation (time, keyword, category)
//! - Rate limiting to prevent API abuse
//! - State transitions and session management
//! - Emitting events for the notification system

mod batch_precheck;
mod bigo_notifications;
pub(crate) mod check_history_writer;
mod detector;
mod events;
mod rate_limiter;
mod service;

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
pub(crate) use events::MonitorEventDelivery;
pub(crate) use events::runtime_instance_id;
pub use events::{MonitorEvent, MonitorEventBroadcaster};
pub use rate_limiter::{RateLimiter, RateLimiterConfig, RateLimiterManager};
pub use service::{
    CredentialCheckPurpose, InfraBlockReason, ProcessStatusResult, ProcessStatusSuppression,
    StreamMonitor, StreamMonitorConfig,
};
pub(crate) use service::{StreamMonitorRuntimeConfig, batch_detection_enabled};
