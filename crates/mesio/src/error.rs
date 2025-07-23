use flv::error::FlvError;
use reqwest::StatusCode;
use std::error::Error as StdError;

use crate::hls::HlsDownloaderError;

// Custom error type for download operations
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Invalid URL: {0}")]
    UrlError(String),

    #[error("Server returned status code {0}")]
    StatusCode(StatusCode),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid proxy configuration: {0}")]
    ProxyError(String),

    #[error("No sources available for download: {0}")]
    NoSource(String),

    #[error("Unsupported protocol: {0}")]
    UnsupportedProtocol(String),

    #[error("FLV error: {0}")]
    FlvError(String), // Consider making this From<crate::flv::error::FlvDownloadError>

    #[error("HLS (next) error: {source}")]
    HlsNextError { source: HlsDownloaderError },

    #[error("Protocol error: {0}")]
    ProtocolError(Box<dyn StdError + Send + Sync>), // Generic protocol error

    #[error("Failed to detect protocol for URL: {0}")]
    ProtocolDetectionFailed(String),

    #[error("Generic download error: {0}")]
    Generic(String),
}

impl From<HlsDownloaderError> for DownloadError {
    fn from(err: HlsDownloaderError) -> Self {
        // Wrap it directly
        DownloadError::HlsNextError { source: err }
    }
}

impl From<DownloadError> for FlvError {
    fn from(err: DownloadError) -> Self {
        FlvError::Io(std::io::Error::other(format!("Download error: {err}")))
    }
}
