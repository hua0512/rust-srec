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

pub use batch_detector::{BatchDetector, BatchFailure, BatchResult};
pub use detector::{FilterReason, LiveStatus, StreamDetector, StreamInfo};
pub use events::{FatalErrorType, MonitorEvent, MonitorEventBroadcaster};
pub use rate_limiter::{RateLimiter, RateLimiterConfig, RateLimiterManager};
pub use service::{StreamMonitor, StreamMonitorConfig};
