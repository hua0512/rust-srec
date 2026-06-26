use std::io::ErrorKind;

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
            FlvDownloadError::Decoder(FlvError::Io(e))
                if e.kind() == ErrorKind::ConnectionAborted =>
            {
                DownloadError::StreamNetwork {
                    reason: e.to_string(),
                }
            }
            FlvDownloadError::Decoder(e) => DownloadError::FlvDecode { source: e },
            FlvDownloadError::AllSourcesFailed(msg) => DownloadError::source_exhausted(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_aborted_decoder_io_maps_to_stream_network_error() {
        let error = FlvDownloadError::Decoder(FlvError::Io(std::io::Error::new(
            ErrorKind::ConnectionAborted,
            "stream reset",
        )));

        assert!(matches!(
            DownloadError::from(error),
            DownloadError::StreamNetwork { reason } if reason.contains("stream reset")
        ));
    }

    #[test]
    fn invalid_decoder_data_maps_to_decode_error() {
        let error = FlvDownloadError::Decoder(FlvError::InvalidHeader);

        assert!(matches!(
            DownloadError::from(error),
            DownloadError::FlvDecode { .. }
        ));
    }
}
