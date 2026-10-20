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
    fn mesio_failures_sort_into_retry_and_circuit_breaker_classes() {
        let rate_limited = |status| {
            DownloadError::rate_limited(
                status,
                "https://cdn.example/live.m3u8",
                "playlist refresh",
                std::time::Duration::from_secs(30),
            )
        };
        let reason = || "x".to_string();
        let cases = [
            (
                DownloadError::Playlist { reason: reason() },
                DownloadFailureKind::Network,
            ),
            (
                DownloadError::Manifest { reason: reason() },
                DownloadFailureKind::Network,
            ),
            (
                DownloadError::StreamNetwork { reason: reason() },
                DownloadFailureKind::Network,
            ),
            (
                DownloadError::http_status(StatusCode::TOO_MANY_REQUESTS, "https://cdn", "fetch"),
                DownloadFailureKind::RateLimited,
            ),
            (
                rate_limited(StatusCode::SERVICE_UNAVAILABLE),
                DownloadFailureKind::RateLimited,
            ),
            (
                DownloadError::http_status(StatusCode::BAD_GATEWAY, "https://cdn", "fetch"),
                DownloadFailureKind::HttpServerError { status: 502 },
            ),
            (
                DownloadError::Unsupported {
                    protocol: "dash",
                    feature: "content protection (DRM)".to_string(),
                },
                DownloadFailureKind::Configuration,
            ),
            (
                DownloadError::InvalidContent {
                    protocol: "hls",
                    reason: reason(),
                },
                DownloadFailureKind::Configuration,
            ),
            (
                DownloadError::SegmentFetch {
                    reason: reason(),
                    retryable: true,
                },
                DownloadFailureKind::Network,
            ),
            (
                DownloadError::SegmentFetch {
                    reason: reason(),
                    retryable: false,
                },
                DownloadFailureKind::SourceUnavailable,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(classify_download_error(&error), expected, "{error}");
        }

        for retried in [
            DownloadFailureKind::Network,
            DownloadFailureKind::RateLimited,
        ] {
            assert!(retried.is_recoverable() && retried.affects_circuit_breaker());
        }
        assert!(!DownloadFailureKind::Configuration.is_recoverable());
        assert!(!DownloadFailureKind::Configuration.affects_circuit_breaker());
    }
}
