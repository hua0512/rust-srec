use flv::error::FlvError;
use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download cancelled")]
    Cancelled,

    #[error("invalid URL `{input}`: {reason}")]
    InvalidUrl { input: String, reason: String },

    #[error("unsupported protocol `{protocol}`")]
    UnsupportedProtocol { protocol: String },

    #[error("failed to detect protocol for URL `{url}`")]
    ProtocolDetectionFailed { url: String },

    #[error("proxy configuration error: {reason}")]
    ProxyConfiguration { reason: String },

    #[error("HTTP request failed: {source}")]
    Network {
        #[from]
        source: reqwest::Error,
    },

    #[error("request failed with HTTP {status} during {operation} for {url}")]
    HttpStatus {
        status: StatusCode,
        url: String,
        operation: &'static str,
    },

    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    #[error("cache error: {reason}")]
    Cache { reason: String },

    #[error("playlist error: {reason}")]
    Playlist { reason: String },

    #[error("segment fetch error: {reason}")]
    SegmentFetch { reason: String, retryable: bool },

    #[error("segment processing error: {reason}")]
    SegmentProcess { reason: String },

    #[error("decryption error: {reason}")]
    Decryption { reason: String },

    #[error("invalid content for {protocol}: {reason}")]
    InvalidContent {
        protocol: &'static str,
        reason: String,
    },

    #[error("configuration error: {reason}")]
    Configuration { reason: String },

    #[error("operation timed out: {reason}")]
    Timeout { reason: String },

    #[error("resource not found: {resource}")]
    NotFound { resource: String },

    #[error("all download sources failed: {reason}")]
    SourceExhausted { reason: String },

    #[error("FLV decode error: {source}")]
    FlvDecode {
        #[from]
        source: FlvError,
    },

    #[error("protocol error: {reason}")]
    Protocol { reason: String },

    #[error("internal error: {reason}")]
    Internal { reason: String },
}

impl DownloadError {
    pub fn invalid_url(input: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidUrl {
            input: input.into(),
            reason: reason.into(),
        }
    }

    pub fn proxy_configuration(reason: impl Into<String>) -> Self {
        Self::ProxyConfiguration {
            reason: reason.into(),
        }
    }

    pub fn http_status(
        status: StatusCode,
        url: impl Into<String>,
        operation: &'static str,
    ) -> Self {
        Self::HttpStatus {
            status,
            url: url.into(),
            operation,
        }
    }

    pub fn source_exhausted(reason: impl Into<String>) -> Self {
        Self::SourceExhausted {
            reason: reason.into(),
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Cancelled => false,
            Self::InvalidUrl { .. }
            | Self::UnsupportedProtocol { .. }
            | Self::ProtocolDetectionFailed { .. }
            | Self::ProxyConfiguration { .. }
            | Self::InvalidContent { .. }
            | Self::Configuration { .. }
            | Self::NotFound { .. } => false,
            Self::HttpStatus { status, .. } => {
                status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS
            }
            Self::SegmentFetch { retryable, .. } => *retryable,
            Self::Network { .. }
            | Self::Io { .. }
            | Self::Cache { .. }
            | Self::Playlist { .. }
            | Self::SegmentProcess { .. }
            | Self::Decryption { .. }
            | Self::SourceExhausted { .. }
            | Self::FlvDecode { .. }
            | Self::Protocol { .. }
            | Self::Timeout { .. }
            | Self::Internal { .. } => true,
        }
    }

    pub fn is_non_recoverable_source_error(&self) -> bool {
        match self {
            Self::HttpStatus { status, .. } => status.is_client_error(),
            Self::InvalidUrl { .. }
            | Self::UnsupportedProtocol { .. }
            | Self::ProtocolDetectionFailed { .. }
            | Self::InvalidContent { .. }
            | Self::NotFound { .. } => true,
            Self::SegmentFetch { retryable, .. } => !retryable,
            _ => false,
        }
    }
}

impl From<DownloadError> for FlvError {
    fn from(err: DownloadError) -> Self {
        FlvError::Io(std::io::Error::other(format!("Download error: {err}")))
    }
}
