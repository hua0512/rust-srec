use flv::error::FlvError;

use crate::DownloadError;

/// Error types specific to FLV downloads
#[derive(Debug, thiserror::Error)]
pub enum FlvDownloadError {
    #[error("Failed to download FLV: {0}")]
    Download(#[from] DownloadError),

    #[error("Failed to create FLV decoder: {0}")]
    Decoder(#[from] FlvError),

    #[error("All sources failed: {0}")]
    AllSourcesFailed(String),
}

// Add implementation for converting FlvDownloadError back to DownloadError
// This helps with error propagation across module boundaries
impl From<FlvDownloadError> for DownloadError {
    fn from(err: FlvDownloadError) -> Self {
        match err {
            FlvDownloadError::Download(e) => e,
            FlvDownloadError::Decoder(e) => DownloadError::FlvError(e.to_string()),
            FlvDownloadError::AllSourcesFailed(msg) => DownloadError::NoSource(msg),
        }
    }
}
