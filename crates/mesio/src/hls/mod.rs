// Main module for the new HLS downloader implementation

pub mod buffer_pool;
pub mod config;
pub mod coordinator;
pub mod decryption;
pub mod error;
pub mod events;
pub mod fetcher;
pub mod hls_downloader;
pub mod metrics;
pub mod output;
pub mod playlist;
pub mod prefetch;
pub mod processor;
pub mod scheduler;
pub(crate) mod segment_utils;
pub mod twitch_processor;

// Re-exports for easier access
pub use buffer_pool::{BufferPool, BufferPoolStats};
pub use config::{BufferLimits, GapSkipStrategy, HlsConfig};
pub use coordinator::HlsStreamCoordinator;
pub use error::HlsDownloaderError;
pub use events::{GapSkipReason, HlsStreamEvent};
pub use hls_downloader::HlsDownloader;
pub use metrics::{MetricsSnapshot, PerformanceMetrics};
pub use prefetch::PrefetchManager;
