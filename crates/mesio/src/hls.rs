// HLS downloader: reactor-based engine (see docs/HLS_ENGINE_ARCHITECTURE.md).

pub mod config;
pub mod engine;
pub mod error;
pub mod events;
mod hls_downloader;
mod playlist;
mod soop_processor;
mod twitch_processor;

// Re-exports for easier access
pub use crate::segment_engine::metrics::{MetricsSnapshot, PerformanceMetrics};
pub use config::{BufferLimits, GapSkipStrategy, HlsConfig, HlsEngineConfig, IdentityPolicyConfig};
pub use error::HlsDownloaderError;
pub use events::{GapSkipReason, HlsStreamEvent};
pub use hls_downloader::HlsDownloader;
