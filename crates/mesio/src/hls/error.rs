use std::sync::Arc;

#[derive(Debug, thiserror::Error, Clone)]
pub enum HlsDownloaderError {
    #[error("Playlist error: {0}")]
    PlaylistError(String),
    #[error("Segment fetch error: {0}")]
    SegmentFetchError(String),
    #[error("Segment processing error: {0}")]
    SegmentProcessError(String),
    #[error("Decryption error: {0}")]
    DecryptionError(String),
    #[error("Cache error: {0}")]
    CacheError(String),
    #[error("Network error: {source}")]
    NetworkError {
        #[from]
        source: Arc<reqwest::Error>,
    },
    #[error("I/O error: {source}")]
    IoError {
        #[from]
        source: Arc<std::io::Error>,
    },
    #[error("Internal error: {0}")]
    InternalError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Operation timed out: {0}")]
    TimeoutError(String),
    #[error("Resource not found: {0}")]
    NotFoundError(String),
    #[error("Operation cancelled")]
    Cancelled,
}

// Manual implementation of From<reqwest::Error> for HlsDownloaderError
// because of the Arc wrapping.
impl From<reqwest::Error> for HlsDownloaderError {
    fn from(err: reqwest::Error) -> Self {
        HlsDownloaderError::NetworkError {
            source: Arc::new(err),
        }
    }
}

// Manual implementation of From<std::io::Error> for HlsDownloaderError
impl From<std::io::Error> for HlsDownloaderError {
    fn from(err: std::io::Error) -> Self {
        HlsDownloaderError::IoError {
            source: Arc::new(err),
        }
    }
}
