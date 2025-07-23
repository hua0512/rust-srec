use crate::{
    BoxMediaStream, DownloadError, DownloadManager, DownloadManagerConfig,
    flv::{FlvConfig, FlvDownloader},
    hls::{HlsConfig, HlsDownloader},
};
use url::Url;

/// Protocol type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolType {
    /// FLV protocol
    Flv,
    /// HLS protocol
    Hls,
    /// Auto-detect from URL
    Auto,
}

/// Mesio downloader factory for creating appropriate download managers
#[derive(Debug, Default)]
pub struct MesioDownloaderFactory {
    /// Base download manager configuration
    download_config: DownloadManagerConfig,
    /// FLV protocol configuration
    flv_config: FlvConfig,
    /// HLS protocol configuration
    hls_config: HlsConfig,
}

impl MesioDownloaderFactory {
    /// Create a new mesio downloader factory with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set download manager configuration
    pub fn with_download_config(mut self, config: DownloadManagerConfig) -> Self {
        self.download_config = config;
        self
    }

    /// Set FLV protocol configuration
    pub fn with_flv_config(mut self, config: FlvConfig) -> Self {
        self.flv_config = config;
        self
    }

    /// Set HLS protocol configuration
    pub fn with_hls_config(mut self, config: HlsConfig) -> Self {
        self.hls_config = config;
        self
    }

    /// Detect protocol type from URL
    pub fn detect_protocol(url: &str) -> Result<ProtocolType, DownloadError> {
        // Parse URL
        let url = Url::parse(url).map_err(|_| {
            DownloadError::UrlError(format!("Failed to parse URL for protocol detection: {url}",))
        })?;

        // Check for HLS indicators first
        let path = url.path().to_lowercase();
        if path.ends_with(".m3u8") || path.ends_with(".m3u") || url.path().contains("playlist") {
            return Ok(ProtocolType::Hls);
        }

        // Check for FLV indicators
        if path.ends_with(".flv") {
            return Ok(ProtocolType::Flv);
        }

        // Check query parameters that might indicate HLS
        if let Some(query) = url.query() {
            let query = query.to_lowercase();
            if query.contains("playlist") || query.contains("manifest") || query.contains("hls") {
                return Ok(ProtocolType::Hls);
            }
        }

        // Default to FLV for rtmp/rtsp URLs
        if url.scheme() == "rtmp" || url.scheme() == "rtmps" || url.scheme() == "rtsp" {
            return Ok(ProtocolType::Flv);
        }

        // If we can't detect, return error
        Err(DownloadError::ProtocolDetectionFailed(url.to_string()))
    }

    /// Create appropriate download manager for the given URL and protocol type
    ///
    /// This uses the factory pattern to avoid dynamic dispatch in hot paths,
    /// returning concrete manager types for better performance.
    pub async fn create_for_url(
        &self,
        url: &str,
        protocol_type: ProtocolType,
    ) -> Result<DownloaderInstance, DownloadError> {
        // Detect protocol if Auto is specified
        let protocol = match protocol_type {
            ProtocolType::Auto => Self::detect_protocol(url)?,
            specific => specific,
        };

        match protocol {
            ProtocolType::Flv => {
                let flv = FlvDownloader::with_config(self.flv_config.clone())?;
                let manager =
                    DownloadManager::with_config(flv, self.download_config.clone()).await?;
                Ok(DownloaderInstance::Flv(Box::new(manager)))
            }
            ProtocolType::Hls => {
                let hls = HlsDownloader::with_config(self.hls_config.clone())?;
                let manager =
                    DownloadManager::with_config(hls, self.download_config.clone()).await?;
                Ok(DownloaderInstance::Hls(Box::new(manager)))
            }
            ProtocolType::Auto => unreachable!(),
        }
    }

    /// Create a download manager for FLV protocol (direct method for when type is known)
    pub async fn create_flv_manager(
        &self,
    ) -> Result<DownloadManager<FlvDownloader>, DownloadError> {
        let protocol = FlvDownloader::with_config(self.flv_config.clone())?;
        DownloadManager::with_config(protocol, self.download_config.clone()).await
    }

    /// Create a download manager for HLS protocol (direct method for when type is known)
    pub async fn create_hls_manager(
        &self,
    ) -> Result<DownloadManager<HlsDownloader>, DownloadError> {
        let protocol = HlsDownloader::with_config(self.hls_config.clone())?;
        DownloadManager::with_config(protocol, self.download_config.clone()).await
    }
}

/// Enum-based unified downloader instance
///
// #[derive(Debug)]
pub enum DownloaderInstance {
    Flv(Box<DownloadManager<FlvDownloader>>),
    Hls(Box<DownloadManager<HlsDownloader>>),
}

impl DownloaderInstance {
    /// Get the protocol type this downloader handles
    pub fn protocol_type(&self) -> ProtocolType {
        match self {
            Self::Flv(_) => ProtocolType::Flv,
            Self::Hls(_) => ProtocolType::Hls,
        }
    }

    /// Add a source URL to the download manager
    pub fn add_source(&mut self, url: impl Into<String>, priority: u8) {
        match self {
            Self::Flv(manager) => manager.add_source(url, priority),
            Self::Hls(manager) => manager.add_source(url, priority),
        }
    }

    /// Download content from the specified URL
    ///
    /// Returns the appropriate stream type for the protocol being used.
    /// Caller should match on the return type to handle the specific data.
    pub async fn download(&self, url: &str) -> Result<DownloadStream, DownloadError> {
        match self {
            Self::Flv(manager) => {
                let stream = manager.download(url).await?;
                Ok(DownloadStream::Flv(stream))
            }
            Self::Hls(manager) => {
                let stream = manager.download(url).await?;
                Ok(DownloadStream::Hls(stream))
            }
        }
    }

    /// Download with fallback sources
    pub async fn download_with_sources(
        &mut self,
        url: &str,
    ) -> Result<DownloadStream, DownloadError> {
        match self {
            Self::Flv(manager) => {
                let stream = manager.download_with_sources(url).await?;
                Ok(DownloadStream::Flv(stream))
            }
            Self::Hls(manager) => {
                let stream = manager.download_with_sources(url).await?;
                Ok(DownloadStream::Hls(stream))
            }
        }
    }
}

/// Enum representing the different stream types that can be returned
/// This avoids runtime type checking while still providing a unified interface
// #[derive(Debug)]
pub enum DownloadStream {
    Flv(BoxMediaStream<flv::data::FlvData, crate::flv::error::FlvDownloadError>),
    Hls(BoxMediaStream<hls::segment::HlsData, crate::hls::error::HlsDownloaderError>),
}

/// Helper macro to process streams with type-specific handling
#[macro_export]
macro_rules! process_stream {
    ($stream:expr, {
        flv($flv_stream:ident) => $flv_block:block,
        hls($hls_stream:ident) => $hls_block:block,
    }) => {
        match $stream {
            DownloadStream::Flv($flv_stream) => $flv_block,
            DownloadStream::Hls($hls_stream) => $hls_block,
        }
    };
}
