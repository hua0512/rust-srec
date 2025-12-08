//! Download Manager module for stream recording.
//!
//! The Download Manager is responsible for:
//! - Managing concurrent downloads with configurable limits
//! - Abstracting download engines (ffmpeg, streamlink, mesio)
//! - Handling segment completion events
//! - Implementing retry logic with circuit breaker pattern
//! - Supporting priority-based download scheduling
//! - Stream selection based on quality, format, and CDN preferences

pub mod engine;

mod manager;
mod resilience;
mod stream_selector;

pub use engine::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadInfo, SegmentEvent, SegmentInfo,
};

pub use manager::{ConfigUpdateType, DownloadManager, DownloadManagerConfig, DownloadManagerEvent};
pub use resilience::{CircuitBreaker, RetryConfig};
pub use stream_selector::{StreamSelectionConfig, StreamSelector};
