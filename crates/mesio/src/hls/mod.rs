// HLS downloader: reactor-based engine (see HLS_ENGINE_ARCHITECTURE.md).

pub mod config;
pub mod engine;
pub mod error;
pub mod events;
mod hls_downloader;
mod metrics;
mod playlist;
mod segment_utils;
mod twitch_processor;

// Re-exports for easier access
pub use config::{BufferLimits, GapSkipStrategy, HlsConfig, HlsEngineConfig, IdentityPolicyConfig};
pub use error::HlsDownloaderError;
pub use events::{GapSkipReason, HlsStreamEvent};
pub use hls_downloader::HlsDownloader;
pub use metrics::{MetricsSnapshot, PerformanceMetrics};
