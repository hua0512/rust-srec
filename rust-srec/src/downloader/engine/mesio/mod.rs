//! Mesio native download engine module.
//!
//! This module contains the Mesio download engine and its supporting components:
//!
//! - `MesioEngine` - The main engine that coordinates protocol detection and delegation
//! - `HlsDownloader` - HLS-specific download orchestrator
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
//! 1. Detects the protocol type from the URL (HLS or FLV)
//! 2. Delegates to the appropriate downloader (`HlsDownloader` or `FlvDownloader`)
//! 3. Propagates results back to the caller
//!
//! Progress tracking is handled by the writers internally via `WriterTask` and
//! `WriterState`, eliminating the need for duplicate tracking in the engine.

pub mod config;
mod engine;
mod flv_downloader;
mod hls_downloader;

pub use engine::MesioEngine;
pub use flv_downloader::FlvDownloader;
pub use hls_downloader::{DownloadStats, HlsDownloader};

use crate::downloader::engine::traits::DownloadFailureKind;
use mesio::DownloadError;
use mesio::flv::error::FlvDownloadError;
use reqwest::StatusCode;

/// Classify a `mesio::DownloadError` into a `DownloadFailureKind`.
///
/// This is the classification boundary — mesio types stay inside
/// the mesio engine wrappers and do not leak into the shared traits.
pub(super) fn classify_download_error(err: &DownloadError) -> DownloadFailureKind {
    match err {
        DownloadError::HttpStatus { status, .. } => classify_http_status(*status),
        DownloadError::Network { .. } | DownloadError::Timeout { .. } => {
            DownloadFailureKind::Network
        }
        DownloadError::Io { .. } => DownloadFailureKind::Io,
        DownloadError::NotFound { .. } | DownloadError::SourceExhausted { .. } => {
            DownloadFailureKind::SourceUnavailable
        }
        DownloadError::InvalidUrl { .. }
        | DownloadError::UnsupportedProtocol { .. }
        | DownloadError::ProtocolDetectionFailed { .. }
        | DownloadError::ProxyConfiguration { .. }
        | DownloadError::Configuration { .. }
        | DownloadError::InvalidContent { .. } => DownloadFailureKind::Configuration,
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
        | DownloadError::Internal { .. } => DownloadFailureKind::Network,
    }
}

/// Classify a `FlvDownloadError` into a `DownloadFailureKind`.
pub(super) fn classify_flv_error(err: &FlvDownloadError) -> DownloadFailureKind {
    match err {
        FlvDownloadError::Download(e) => classify_download_error(e),
        FlvDownloadError::Decoder(_) => DownloadFailureKind::Processing,
        FlvDownloadError::AllSourcesFailed(_) => DownloadFailureKind::SourceUnavailable,
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
