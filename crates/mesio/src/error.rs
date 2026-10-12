use std::sync::Arc;

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

    /// A manifest (e.g. a DASH MPD) is malformed or inconsistent. A later
    /// refresh may publish a valid one, so this is retryable like
    /// [`Self::Playlist`].
    #[error("manifest error: {reason}")]
    Manifest { reason: String },

    /// The stream uses a feature this engine deliberately does not support
    /// (e.g. DRM). Retrying cannot succeed.
    #[error("unsupported {protocol} feature: {feature}")]
    Unsupported {
        protocol: &'static str,
        feature: String,
    },

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
            | Self::Unsupported { .. }
            | Self::NotFound { .. } => true,
            Self::StreamNetwork { .. } => false,
            Self::SegmentFetch { retryable, .. } => !retryable,
            _ => false,
        }
    }
}

/// A [`DownloadError`] delivered to several consumers, such as every track of
/// a stream whose manifest source failed. Each takes its own copy with
/// [`Self::to_error`]. Two are equal when they are the same shared error.
#[derive(Debug, Clone)]
pub struct SharedError(Arc<DownloadError>);

impl SharedError {
    pub fn new(error: DownloadError) -> Self {
        Self(Arc::new(error))
    }

    pub fn get(&self) -> &DownloadError {
        &self.0
    }

    /// An owned error of the same variant and message. A source that cannot
    /// be copied keeps its classification: a `reqwest` failure becomes
    /// [`DownloadError::StreamNetwork`], an I/O error keeps its
    /// [`std::io::ErrorKind`].
    pub fn to_error(&self) -> DownloadError {
        use DownloadError as E;
        match self.get() {
            E::Cancelled => E::Cancelled,
            E::InvalidUrl { input, reason } => E::InvalidUrl {
                input: input.clone(),
                reason: reason.clone(),
            },
            E::UnsupportedProtocol { protocol } => E::UnsupportedProtocol {
                protocol: protocol.clone(),
            },
            E::ProtocolDetectionFailed { url } => E::ProtocolDetectionFailed { url: url.clone() },
            E::ProxyConfiguration { reason } => E::ProxyConfiguration {
                reason: reason.clone(),
            },
            E::Network { source } => E::StreamNetwork {
                reason: source.to_string(),
            },
            E::StreamNetwork { reason } => E::StreamNetwork {
                reason: reason.clone(),
            },
            E::HttpStatus {
                status,
                url,
                operation,
            } => E::HttpStatus {
                status: *status,
                url: url.clone(),
                operation,
            },
            E::Io { source } => E::Io {
                source: std::io::Error::new(source.kind(), source.to_string()),
            },
            E::Cache { reason } => E::Cache {
                reason: reason.clone(),
            },
            E::Playlist { reason } => E::Playlist {
                reason: reason.clone(),
            },
            E::Manifest { reason } => E::Manifest {
                reason: reason.clone(),
            },
            E::Unsupported { protocol, feature } => E::Unsupported {
                protocol,
                feature: feature.clone(),
            },
            E::SegmentFetch { reason, retryable } => E::SegmentFetch {
                reason: reason.clone(),
                retryable: *retryable,
            },
            E::SegmentProcess { reason } => E::SegmentProcess {
                reason: reason.clone(),
            },
            E::Decryption { reason } => E::Decryption {
                reason: reason.clone(),
            },
            E::InvalidContent { protocol, reason } => E::InvalidContent {
                protocol,
                reason: reason.clone(),
            },
            E::Configuration { reason } => E::Configuration {
                reason: reason.clone(),
            },
            E::Timeout { reason } => E::Timeout {
                reason: reason.clone(),
            },
            E::NotFound { resource } => E::NotFound {
                resource: resource.clone(),
            },
            E::SourceExhausted { reason } => E::SourceExhausted {
                reason: reason.clone(),
            },
            E::FlvDecode { source } => E::FlvDecode {
                source: match source {
                    FlvError::InvalidHeader => FlvError::InvalidHeader,
                    FlvError::Io(error) => {
                        FlvError::Io(std::io::Error::new(error.kind(), error.to_string()))
                    }
                    FlvError::IncompleteData => FlvError::IncompleteData,
                    FlvError::TagParseError(reason) => FlvError::TagParseError(reason.clone()),
                    FlvError::ResyncFailed => FlvError::ResyncFailed,
                    FlvError::InvalidTagType(tag) => FlvError::InvalidTagType(*tag),
                    FlvError::TagTooLarge(size) => FlvError::TagTooLarge(*size),
                },
            },
            E::Protocol { reason } => E::Protocol {
                reason: reason.clone(),
            },
            E::Internal { reason } => E::Internal {
                reason: reason.clone(),
            },
        }
    }
}

impl PartialEq for SharedError {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SharedError {}

impl std::fmt::Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
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
    fn shared_errors_copy_to_the_same_variant() {
        let http = SharedError::new(DownloadError::http_status(
            StatusCode::NOT_FOUND,
            "https://e.com/a.mpd",
            "MPD fetch",
        ));
        assert!(matches!(
            http.to_error(),
            DownloadError::HttpStatus { status, operation: "MPD fetch", .. } if status == StatusCode::NOT_FOUND
        ));
        let unsupported = SharedError::new(DownloadError::Unsupported {
            protocol: "dash",
            feature: "DRM".to_owned(),
        });
        assert!(unsupported.to_error().is_non_recoverable_source_error());
        let io = SharedError::new(DownloadError::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "slow",
        )));
        assert!(matches!(
            io.to_error(),
            DownloadError::Io { source } if source.kind() == std::io::ErrorKind::TimedOut
        ));
        assert_eq!(http.to_error().to_string(), http.to_string());
        assert_eq!(http, http.clone());
        assert_ne!(
            http,
            SharedError::new(DownloadError::Manifest {
                reason: String::new()
            })
        );
    }

    #[tokio::test]
    async fn shared_network_errors_stay_network_errors() {
        // Nothing listens on port 9 (discard) of the loopback address.
        let client = crate::downloader::create_client(&Default::default()).unwrap();
        let error = client
            .get("http://127.0.0.1:9/")
            .send()
            .await
            .expect_err("connection refused");
        let shared = SharedError::new(DownloadError::from(error));
        assert!(matches!(
            shared.to_error(),
            DownloadError::StreamNetwork { reason } if reason == shared.get().to_string().trim_start_matches("HTTP request failed: ")
        ));
    }

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
