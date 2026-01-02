// Main module for the new HLS downloader implementation

mod buffer_pool;
pub mod config;
mod coordinator;
mod decryption;
pub mod error;
pub mod events;
mod fetcher;
mod hls_downloader;
mod metrics;
mod output;
mod playlist;
mod prefetch;
mod processor;
mod scheduler;
mod segment_utils;
mod twitch_processor;

// Re-exports for easier access
pub use buffer_pool::{BufferPool, BufferPoolStats};
pub use config::{BufferLimits, GapSkipStrategy, HlsConfig};
pub use coordinator::HlsStreamCoordinator;
pub use error::HlsDownloaderError;
pub use events::{GapSkipReason, HlsStreamEvent};
pub use hls_downloader::HlsDownloader;
pub use metrics::{MetricsSnapshot, PerformanceMetrics};
pub use prefetch::PrefetchManager;
