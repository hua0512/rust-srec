use flv::error::FlvError;
use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download cancelled")]
    Cancelled,

    /// `input` is redacted (see [`crate::redact`]).
    #[error("invalid URL `{input}`: {reason}")]
    InvalidUrl { input: String, reason: String },

    #[error("unsupported protocol `{protocol}`")]
    UnsupportedProtocol { protocol: String },

    #[error("failed to detect protocol for URL `{url}`")]
    ProtocolDetectionFailed { url: String },

    #[error("proxy configuration error: {reason}")]
    ProxyConfiguration { reason: String },

    /// Construct via `From`, which redacts the URL in reqwest's message.
    #[error("HTTP request failed: {source}")]
    Network { source: reqwest::Error },

    #[error("stream network error: {reason}")]
    StreamNetwork { reason: String },

    /// `url` is redacted (see [`crate::redact`]).
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
            input: crate::redact::redact_url_str(&input.into()),
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
            url: crate::redact::redact_url_str(&url.into()),
            operation,
        }
    }

    pub fn source_exhausted(reason: impl Into<String>) -> Self {
        Self::SourceExhausted {
            reason: reason.into(),
        }
    }

    pub fn is_non_recoverable_source_error(&self) -> bool {
        match self {
            Self::HttpStatus { status, .. } => is_permanent_client_error(*status),
            Self::InvalidUrl { .. }
            | Self::UnsupportedProtocol { .. }
            | Self::ProtocolDetectionFailed { .. }
            | Self::InvalidContent { .. }
            | Self::NotFound { .. } => true,
            Self::StreamNetwork { .. } => false,
            Self::SegmentFetch { retryable, .. } => !retryable,
            _ => false,
        }
    }
}

/// A 4xx that means the URL itself is invalid. Statuses that can clear
/// without the URL changing are excluded: expired signed URLs (401/403) get
/// refreshed, a live playlist may not be published yet (404), and timeouts or
/// rate limits (408/425/429) pass.
pub(crate) fn is_permanent_client_error(status: StatusCode) -> bool {
    status.is_client_error()
        && !matches!(
            status,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::NOT_FOUND
                | StatusCode::REQUEST_TIMEOUT
                | StatusCode::TOO_EARLY
                | StatusCode::TOO_MANY_REQUESTS
        )
}

impl From<reqwest::Error> for DownloadError {
    fn from(source: reqwest::Error) -> Self {
        Self::Network {
            source: crate::redact::redact_reqwest(source),
        }
    }
}

impl From<DownloadError> for FlvError {
    fn from(err: DownloadError) -> Self {
        FlvError::Io(std::io::Error::other(format!("Download error: {err}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_permanent_client_errors_are_non_recoverable() {
        let error = |status| DownloadError::http_status(status, "https://e.com/a", "test");
        for status in [StatusCode::BAD_REQUEST, StatusCode::GONE] {
            assert!(error(status).is_non_recoverable_source_error(), "{status}");
        }
        for status in [
            StatusCode::NOT_FOUND,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
        ] {
            assert!(!error(status).is_non_recoverable_source_error(), "{status}");
        }
    }

    #[test]
    fn invalid_url_errors_do_not_echo_signed_url_tokens() {
        // Unparseable, so no caller can redact it as a `Url` first.
        let error = DownloadError::invalid_url(
            "https://user:pw@[bad-host/live.flv?token=secret",
            "invalid IPv6 address",
        );

        let message = error.to_string();
        assert!(!message.contains("secret"), "{message}");
        assert!(!message.contains("pw"), "{message}");
        assert!(message.contains("live.flv?token=***"), "{message}");
    }
}
