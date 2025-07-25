//! # Mesio
//!
//! A library for downloading media content from various sources.
//! Supports FLV, HLS, and other streaming formats with efficient
//! processing pipeline integration.
//!
//! ## Features
//!
//! - Multiple protocol support (HLS, FLV)
//! - Efficient download management with caching
//! - Source selection with fallback capabilities
//! - Factory pattern for protocol instantiation
//! - Protocol auto-detection from URLs

pub mod builder;
pub mod bytes_stream;
pub mod cache;
pub mod config;
pub mod downloader;
pub mod error;
pub mod factory;
pub mod flv;
pub mod hls;
pub mod media_protocol;
pub mod protocol_builder;
pub mod proxy;
pub mod source;

pub use builder::DownloaderConfigBuilder;
pub use cache::{CacheConfig, CacheManager};
pub use config::DownloaderConfig;
pub use error::DownloadError;

// Re-export legacy protocol traits for backward compatibility
pub use media_protocol::{BoxMediaStream, ProtocolConfig};

// Re-export new capability-based traits
pub use media_protocol::{
    Cacheable,
    Download,
    MultiSource,
    // Base traits
    ProtocolBase,
    RawDownload,
    RawResumable,
    Resumable,
    download_raw_with_resume,
    // Utility functions
    download_with_resume,
    download_with_sources,
    download_with_sources_and_cache,
};

// Re-export protocol builders
pub use protocol_builder::{FlvProtocolBuilder, HlsProtocolBuilder, ProtocolBuilder};
pub use source::{ContentSource, SourceManager, SourceSelectionStrategy};

// Re-export downloader utilities
pub use downloader::{DownloadManager, DownloadManagerConfig, create_client};

// Re-export factory types
pub use factory::{DownloadStream, DownloaderInstance, MesioDownloaderFactory, ProtocolType};

// Re-export proxy utilities
pub use proxy::{ProxyAuth, ProxyConfig, ProxyType};
