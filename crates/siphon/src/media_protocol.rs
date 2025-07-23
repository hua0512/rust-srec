//! # Media Protocol Trait
//!
//! This module defines the core traits for media protocol implementations.
//! All media protocols (FLV, HLS, etc.) should implement these traits.

use futures::Stream;
use std::future::Future;
use std::pin::Pin;
use std::{fmt::Debug, sync::Arc};

use crate::{
    DownloadError, cache::CacheManager, flv::FlvConfig, hls::HlsConfig, source::SourceManager,
};

/// A type alias for a boxed media stream
pub type BoxMediaStream<D, E> = Pin<Box<dyn Stream<Item = Result<D, E>> + Send>>;

/// Protocol configuration trait
pub trait ProtocolConfig: Debug + Clone {}

pub enum Protocol {
    /// FLV protocol
    Flv(Box<FlvConfig>),
    /// HLS protocol
    Hls(Box<HlsConfig>),
}

/// Shared protocol configuration trait
pub trait ProtocolBase: Send + Sync + 'static {
    /// Protocol-specific configuration
    type Config: ProtocolConfig;

    /// Create a new protocol implementation with the given configuration
    fn new(config: Self::Config) -> Result<Self, DownloadError>
    where
        Self: Sized;
}

/// Base download capability
///
/// Provides the core ability to download media from a URL.
pub trait Download: ProtocolBase {
    /// The type of data produced by this protocol
    type Data: Send + 'static;

    /// The error type for this protocol
    type Error: Send + std::error::Error + Into<DownloadError> + 'static;

    /// The stream type returned by this protocol
    type Stream: Stream<Item = Result<Self::Data, Self::Error>> + Send + 'static;

    /// Download media content from a URL
    fn download(
        &self,
        url: &str,
    ) -> impl Future<Output = Result<Self::Stream, DownloadError>> + Send;
}

/// Optional resumable download capability
///
/// Enables downloading from a specific byte position.
pub trait Resumable: Download {
    /// Resume a download from a specified byte range
    fn resume(
        &self,
        url: &str,
        range: (u64, Option<u64>),
    ) -> impl Future<Output = Result<Self::Stream, DownloadError>> + Send;
}

/// Optional multi-source download capability
///
/// Enables downloading with fallback sources and source management.
pub trait MultiSource: Download {
    /// Download media content with support for multiple sources
    fn download_with_sources(
        &self,
        url: &str,
        source_manager: &mut SourceManager,
    ) -> impl Future<Output = Result<Self::Stream, DownloadError>> + Send;
}

/// Optional caching capability
///
/// Enables content caching for more efficient downloads.
pub trait Cacheable: Download {
    /// Download media content with caching support
    fn download_with_cache(
        &self,
        url: &str,
        cache_manager: Arc<CacheManager>,
    ) -> impl Future<Output = Result<Self::Stream, DownloadError>> + Send;
}

/// Optional raw download capability
///
/// Enables downloading as raw bytes without protocol-specific parsing.
pub trait RawDownload: ProtocolBase {
    /// The error type for raw downloads
    type Error: Send + std::error::Error + Into<DownloadError> + 'static;

    /// The stream type returned for raw downloads
    type RawStream: Stream<Item = Result<bytes::Bytes, Self::Error>> + Send + 'static;

    /// Download media content as a raw byte stream
    fn download_raw(
        &self,
        url: &str,
    ) -> impl Future<Output = Result<Self::RawStream, DownloadError>> + Send;
}

/// Optional raw resumable capability
///
/// Enables resuming raw downloads from a specific byte position.
pub trait RawResumable: RawDownload {
    /// Resume a raw download from a specified byte range
    fn resume_raw(
        &self,
        url: &str,
        range: (u64, Option<u64>),
    ) -> impl Future<Output = Result<Self::RawStream, DownloadError>> + Send;
}

//-----------------------------------------------------------------------------
// Utility functions for working with capabilities
//-----------------------------------------------------------------------------

/// Download content with automatic resume support if available
pub async fn download_with_resume<P>(
    protocol: &P,
    url: &str,
    range: Option<(u64, u64)>,
) -> Result<P::Stream, DownloadError>
where
    P: Download + Resumable,
{
    if let Some(range) = range {
        protocol.resume(url, (range.0, Some(range.1))).await
    } else {
        protocol.download(url).await
    }
}

/// Download content with source management support if available
pub async fn download_with_sources<P>(
    protocol: &P,
    url: &str,
    source_manager: &mut SourceManager,
) -> Result<P::Stream, DownloadError>
where
    P: Download + MultiSource,
{
    // If no sources are configured yet, add the main URL
    if !source_manager.has_sources() {
        source_manager.add_url(url, 0);
    }

    protocol.download_with_sources(url, source_manager).await
}

/// Download content with combined source management and cache support
pub async fn download_with_sources_and_cache<P>(
    protocol: &P,
    url: &str,
    source_manager: &mut SourceManager,
    cache_manager: Arc<CacheManager>,
) -> Result<P::Stream, DownloadError>
where
    P: Download + MultiSource + Cacheable,
{
    // First check cache
    match protocol.download_with_cache(url, cache_manager).await {
        Ok(stream) => Ok(stream),
        Err(_) => {
            // Cache miss, try sources
            download_with_sources(protocol, url, source_manager).await
        }
    }
}

/// Download raw content with resume support if needed
pub async fn download_raw_with_resume<P>(
    protocol: &P,
    url: &str,
    range: Option<(u64, u64)>,
) -> Result<P::RawStream, DownloadError>
where
    P: RawDownload + RawResumable,
{
    if let Some(range) = range {
        protocol.resume_raw(url, (range.0, Some(range.1))).await
    } else {
        protocol.download_raw(url).await
    }
}
