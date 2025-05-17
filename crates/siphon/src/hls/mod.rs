// Main module for the new HLS downloader implementation

pub mod config;
pub mod coordinator;
pub mod decryption;
pub mod error;
pub mod events;
pub mod fetcher;
pub mod hls_downloader;
pub mod output;
pub mod playlist;
pub mod processor;
pub mod scheduler;
pub(crate) mod segment_utils;

// Re-exports for easier access
pub use config::HlsConfig;
pub use coordinator::HlsStreamCoordinator;
pub use error::HlsDownloaderError;
pub use events::HlsStreamEvent;
pub use hls_downloader::HlsDownloader;
