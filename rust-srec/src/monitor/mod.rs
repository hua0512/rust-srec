//! Stream Monitor module for detecting live status.
//!
//! The Stream Monitor is responsible for:
//! - Checking individual streamer live status
//! - Batch detection for supported platforms
//! - Filter evaluation (time, keyword, category)
//! - Rate limiting to prevent API abuse
//! - State transitions and session management
//! - Emitting events for the notification system

mod batch_detector;
mod detector;
mod events;
mod rate_limiter;
mod service;

pub use batch_detector::BatchDetector;
pub use detector::{FilterReason, LiveStatus, StreamDetector};
pub use events::{FatalErrorType, MonitorEvent, MonitorEventBroadcaster};
pub use rate_limiter::{RateLimiter, RateLimiterConfig};
pub use service::{StreamMonitor, StreamMonitorConfig};
