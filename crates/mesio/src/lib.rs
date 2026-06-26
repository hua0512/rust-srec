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
//! - Protocol-neutral session and event API
//! - Protocol auto-detection from URLs

pub mod builder;
pub mod bytes_stream;
pub mod cache;
pub mod config;
pub mod downloader;
pub mod error;
pub mod flv;
pub mod hls;
pub mod protocol_builder;
pub mod proxy;
pub mod session;
pub mod source;

/// A boxed async media stream.
pub type BoxMediaStream<D, E> = std::pin::Pin<Box<dyn futures::Stream<Item = Result<D, E>> + Send>>;

pub use config::DEFAULT_USER_AGENT;

pub use builder::DownloaderConfigBuilder;
pub use cache::{CacheConfig, CacheManager};
pub use config::{DownloaderConfig, HttpVersionPreference};
pub use error::DownloadError;

// Re-export protocol builders
pub use protocol_builder::{FlvProtocolBuilder, HlsProtocolBuilder, ProtocolBuilder};
pub use source::{ContentSource, SourceManager, SourceSelectionStrategy};

// Re-export downloader utilities
pub use downloader::create_client;

// Re-export session/event API
pub use session::{
    DownloadEvent, DownloadEventStream, DownloadHandle, DownloadOptions, DownloadRequest,
    DownloadSession, DownloadTerminal, DownloaderSession, EventSink, FlvReconnect,
    FlvRequestOptions, HlsRequestOptions, MediaEngine, MesioConfig, MesioDownloader,
    ProtocolSelection, ProtocolType, ResourceId,
};

// Re-export proxy utilities
pub use proxy::{ProxyAuth, ProxyConfig, ProxyType};
