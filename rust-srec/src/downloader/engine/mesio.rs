//! Mesio native download engine module.
//!
//! This module contains the Mesio download engine and its supporting components:
//!
//! - `MesioEngine` - The main engine that coordinates protocol detection and delegation
//! - `HlsDownloader` - HLS-specific download orchestrator
//! - `DashDownloader` - MPEG-DASH download orchestrator, writing through the
//!   HLS fMP4 path
//! - `FlvDownloader` - FLV-specific download orchestrator
//! - `config` - Configuration mapping utilities for mesio protocol configs
//!
//! # FLV Fix Configuration
//!
//! When FLV processing is enabled, Mesio can further tune the `flv-fix` pipeline
//! via `MesioEngineConfig.flv_fix` (e.g. split detection mode and duplicate-tag
//! filtering parameters). Defaults preserve the legacy behavior (split on raw
//! sequence-header CRC32 changes).
//!
//! # Architecture
//!
//! The `MesioEngine` acts as a thin coordinator that:
//! 1. Starts protocol sessions through `mesio::MesioDownloader`
//! 2. Delegates to the appropriate downloader (`HlsDownloader`, `DashDownloader`
//!    or `FlvDownloader`)
//! 3. Propagates results back to the caller
//!
//! Download telemetry is available from mesio session events. The rust-srec
//! engine wrappers keep user-visible recording progress on the writer pipeline
//! so byte accounting follows the actual persisted output.

pub mod config;
mod dash_downloader;
mod engine;
mod flv_downloader;
mod helpers;
mod hls_downloader;

pub use engine::MesioEngine;
pub use helpers::DownloadStats;

use crate::downloader::engine::traits::DownloadFailureKind;
use mesio::DownloadError;
use reqwest::StatusCode;

/// Classify a `mesio::DownloadError` into a `DownloadFailureKind`.
///
/// This is the classification boundary — mesio types stay inside
/// the mesio engine wrappers and do not leak into the shared traits.
pub(super) fn classify_download_error(err: &DownloadError) -> DownloadFailureKind {
    match err {
        DownloadError::HttpStatus { status, .. } => classify_http_status(*status),
        // Mesio already waited the server's rate limiting out as long as it
        // allows; a 503 with Retry-After is rate limiting too.
        DownloadError::RateLimited { .. } => DownloadFailureKind::RateLimited,
        DownloadError::Network { .. }
        | DownloadError::StreamNetwork { .. }
        | DownloadError::Timeout { .. } => DownloadFailureKind::Network,
        DownloadError::Io { .. } => DownloadFailureKind::Io,
        DownloadError::NotFound { .. } | DownloadError::SourceExhausted { .. } => {
            DownloadFailureKind::SourceUnavailable
        }
        DownloadError::InvalidUrl { .. }
        | DownloadError::UnsupportedProtocol { .. }
        | DownloadError::ProtocolDetectionFailed { .. }
        | DownloadError::ProxyConfiguration { .. }
        | DownloadError::Configuration { .. }
        | DownloadError::InvalidContent { .. }
        | DownloadError::Unsupported { .. } => DownloadFailureKind::Configuration,
        DownloadError::FlvDecode { .. }
        | DownloadError::SegmentProcess { .. }
        | DownloadError::Decryption { .. }
        | DownloadError::Protocol { .. } => DownloadFailureKind::Processing,
        DownloadError::SegmentFetch { retryable, .. } => {
            if *retryable {
                DownloadFailureKind::Network
            } else {
                DownloadFailureKind::SourceUnavailable
            }
        }
        DownloadError::Cancelled => DownloadFailureKind::Cancelled,
        DownloadError::Cache { .. }
        | DownloadError::Playlist { .. }
        | DownloadError::Manifest { .. }
        | DownloadError::Internal { .. } => DownloadFailureKind::Network,
    }
}

fn classify_http_status(status: StatusCode) -> DownloadFailureKind {
    let code = status.as_u16();
    if code == 429 {
        DownloadFailureKind::RateLimited
    } else if status.is_client_error() {
        DownloadFailureKind::HttpClientError { status: code }
    } else if status.is_server_error() {
        DownloadFailureKind::HttpServerError { status: code }
    } else {
        DownloadFailureKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_network_error_classifies_as_network() {
        let err = DownloadError::StreamNetwork {
            reason: "stream reset".to_string(),
        };

        assert_eq!(classify_download_error(&err), DownloadFailureKind::Network);
    }

    #[test]
    fn manifest_error_classifies_as_retryable_like_playlist_errors() {
        let manifest = DownloadError::Manifest {
            reason: "SegmentTimeline overlaps".to_string(),
        };
        let playlist = DownloadError::Playlist {
            reason: "bad playlist".to_string(),
        };

        let kind = classify_download_error(&manifest);
        assert_eq!(kind, classify_download_error(&playlist));
        assert!(kind.is_recoverable());
    }

    #[test]
    fn rate_limiting_classifies_as_recoverable_rate_limited() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let err = DownloadError::rate_limited(
                status,
                "https://cdn.example/live.m3u8",
                "playlist refresh",
                std::time::Duration::from_secs(30),
            );
            let kind = classify_download_error(&err);
            assert_eq!(kind, DownloadFailureKind::RateLimited, "{status}");
            assert!(kind.is_recoverable());
        }
    }

    #[test]
    fn unsupported_feature_classifies_as_non_retryable_configuration() {
        let err = DownloadError::Unsupported {
            protocol: "dash",
            feature: "content protection (DRM)".to_string(),
        };

        let kind = classify_download_error(&err);
        assert_eq!(kind, DownloadFailureKind::Configuration);
        assert!(!kind.is_recoverable());
        assert!(!kind.affects_circuit_breaker());
    }
}
