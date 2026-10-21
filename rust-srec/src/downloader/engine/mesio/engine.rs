//! Mesio native download engine implementation.
//!
//! This engine uses the `mesio` crate's session API for protocol detection and
//! stream downloading.
//!
//! The engine acts as a thin coordinator that:
//! 1. Takes the protocol from the download configuration, or detects it from
//!    the URL when the platform did not label the stream
//! 2. Delegates to `HlsDownloader`, `DashDownloader` or `FlvDownloader` for
//!    actual download logic
//!
//! Progress tracking is handled by the writers internally via `WriterTask` and
//! `WriterState`, eliminating the need for duplicate tracking in the engine.

use async_trait::async_trait;
use axum::http::HeaderMap;
use mesio::flv::FlvProtocolConfig;
use mesio::{
    DashConfig, DashProtocolBuilder, FlvProtocolBuilder, HlsProtocolBuilder, MesioDownloader,
    ProtocolType,
};
use std::sync::Arc;
use tracing::{debug, error, info};

use super::dash_downloader::DashDownloader;
use super::flv_downloader::FlvDownloader;
use super::hls_downloader::HlsDownloader;
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadProtocol,
    EngineStartError, EngineType,
};
use crate::downloader::engine::utils::sanitize_engine_message;

/// Native Mesio download engine.
///
/// This engine uses the `mesio` crate's session API for protocol detection and
/// delegates to specialized downloaders for HLS, DASH and FLV formats.
///
/// The engine is a thin coordinator that:
/// - Resolves the protocol type (see `resolve_protocol`)
/// - Creates appropriate downloader (HlsDownloader, DashDownloader or FlvDownloader)
/// - Delegates download execution and propagates results
pub struct MesioEngine {
    /// Whether the engine is available.
    available: bool,
    /// Engine configuration.
    config: MesioEngineConfig,
    /// Default HLS configuration.
    hls_config: Option<mesio::hls::HlsConfig>,
    /// Default FLV configuration.
    flv_config: Option<FlvProtocolConfig>,
    /// Default DASH configuration.
    dash_config: Option<DashConfig>,
}

impl MesioEngine {
    /// Create a new Mesio engine with default configurations.
    pub fn new() -> Self {
        Self::with_config(MesioEngineConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: MesioEngineConfig) -> Self {
        let buffer_size = config.buffer_size;
        Self {
            available: true,
            config,
            hls_config: Some(
                HlsProtocolBuilder::new()
                    // override default headers
                    .headers(HeaderMap::new())
                    .get_config(),
            ),
            flv_config: Some(
                FlvProtocolBuilder::new()
                    // override default headers
                    .headers(HeaderMap::new())
                    .buffer_size(buffer_size)
                    .get_config(),
            ),
            dash_config: Some(
                DashProtocolBuilder::new()
                    // override default headers
                    .headers(HeaderMap::new())
                    .get_config(),
            ),
        }
    }

    /// Create a new Mesio engine with custom HLS configuration built from HlsProtocolBuilder.
    pub fn with_hls_config(mut self, config: mesio::hls::HlsConfig) -> Self {
        self.hls_config = Some(config);
        self
    }

    /// Create a new Mesio engine with custom FLV configuration.
    pub fn with_flv_config(mut self, config: FlvProtocolConfig) -> Self {
        self.flv_config = Some(config);
        self
    }

    /// The protocol to record `config` with.
    ///
    /// A DASH label always wins: an MPD URL need not end in `.mpd` (e.g.
    /// `/manifest?id=…`), and URL detection would otherwise take it for HLS.
    /// For other streams the URL decides, as it always has, and an HLS or FLV
    /// label is used only when the URL is not recognised.
    fn resolve_protocol(
        config: &DownloadConfig,
    ) -> std::result::Result<ProtocolType, EngineStartError> {
        let labelled = match config.protocol {
            DownloadProtocol::Hls => Some(ProtocolType::Hls),
            DownloadProtocol::Flv => Some(ProtocolType::Flv),
            DownloadProtocol::Dash => Some(ProtocolType::Dash),
            DownloadProtocol::Unknown => None,
        };
        let detected = MesioDownloader::detect_protocol(&config.url);
        if let (Some(labelled), Ok(detected)) = (labelled, &detected)
            && labelled != *detected
        {
            debug!(
                streamer_id = %config.streamer_id,
                labelled = ?labelled,
                detected = ?detected,
                "Stream protocol label differs from the URL"
            );
        }
        match (labelled, detected) {
            (Some(ProtocolType::Dash), _) => Ok(ProtocolType::Dash),
            (_, Ok(detected)) => Ok(detected),
            (Some(labelled), Err(_)) => Ok(labelled),
            (None, Err(e)) => Err(EngineStartError::new(
                DownloadFailureKind::Configuration,
                sanitize_engine_message(
                    &format!("Protocol detection failed: {}", e),
                    config.managed_credentials,
                ),
            )),
        }
    }
}

impl Default for MesioEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DownloadEngine for MesioEngine {
    fn engine_type(&self) -> EngineType {
        EngineType::Mesio
    }

    async fn run(&self, handle: Arc<DownloadHandle>) -> std::result::Result<(), EngineStartError> {
        let config_snapshot = handle.config_snapshot();
        let streamer_id = config_snapshot.streamer_id.clone();

        info!("Starting mesio download for streamer {}", streamer_id);

        let protocol_type = Self::resolve_protocol(&config_snapshot)?;

        debug!(
            "Using protocol {:?} for URL: {}",
            protocol_type,
            if config_snapshot.managed_credentials {
                "[managed URL redacted]"
            } else {
                &config_snapshot.url
            }
        );

        // Delegate to appropriate downloader based on protocol type
        let download_result = match protocol_type {
            ProtocolType::Hls => {
                HlsDownloader::new(handle, self.config.clone(), self.hls_config.clone())
                    .run()
                    .await
                    .map(|_| ())
            }
            ProtocolType::Dash => {
                DashDownloader::new(handle, self.config.clone(), self.dash_config.clone())
                    .run()
                    .await
                    .map(|_| ())
            }
            ProtocolType::Flv => {
                FlvDownloader::new(handle, self.config.clone(), self.flv_config.clone())
                    .run()
                    .await
                    .map(|_| ())
            }
            _ => {
                let error_msg = format!("Unsupported protocol type: {:?}", protocol_type);
                error!("{}", error_msg);
                return Err(EngineStartError::new(
                    DownloadFailureKind::Configuration,
                    error_msg,
                ));
            }
        };

        let download_result = download_result.map_err(|mut error| {
            error.message =
                sanitize_engine_message(&error.message, config_snapshot.managed_credentials);
            error
        });

        // Log any errors (downloaders emit their own events internally)
        if let Err(e) = &download_result {
            error!("Mesio download failed for {}: {}", streamer_id, e);
        }

        download_result
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn version(&self) -> Option<String> {
        Some(mesio::VERSION.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download_config(url: &str, protocol: DownloadProtocol) -> DownloadConfig {
        DownloadConfig::new(url, "/tmp", "streamer", "streamer", "session").with_protocol(protocol)
    }

    #[test]
    fn a_dash_label_wins_over_url_detection() {
        let resolve =
            |url, protocol| MesioEngine::resolve_protocol(&download_config(url, protocol)).unwrap();
        assert_eq!(
            resolve(
                "https://cdn.example/live/manifest?id=1",
                DownloadProtocol::Dash
            ),
            ProtocolType::Dash
        );
        assert_eq!(
            resolve("https://cdn.example/live/stream", DownloadProtocol::Dash),
            ProtocolType::Dash
        );
    }

    #[test]
    fn hls_and_flv_labels_only_apply_when_the_url_is_not_recognised() {
        let resolve =
            |url, protocol| MesioEngine::resolve_protocol(&download_config(url, protocol)).unwrap();
        // The URL wins, as before labels were consulted.
        assert_eq!(
            resolve("https://cdn.example/live/stream.flv", DownloadProtocol::Hls),
            ProtocolType::Flv
        );
        assert_eq!(
            resolve(
                "https://cdn.example/live/stream.m3u8",
                DownloadProtocol::Flv
            ),
            ProtocolType::Hls
        );
        // Unrecognised URL: the label decides instead of failing.
        assert_eq!(
            resolve("https://cdn.example/live/stream", DownloadProtocol::Hls),
            ProtocolType::Hls
        );
        assert_eq!(
            resolve("https://cdn.example/live/stream", DownloadProtocol::Flv),
            ProtocolType::Flv
        );
    }

    /// With no label, a URL mesio cannot recognise is a configuration error,
    /// which fails the attempt once instead of retrying it as a network
    /// failure against the circuit breaker.
    #[test]
    fn an_unlabelled_protocol_is_detected_from_the_url() {
        let resolve =
            |url| MesioEngine::resolve_protocol(&download_config(url, DownloadProtocol::Unknown));
        assert_eq!(
            resolve("https://cdn.example/live.mpd").unwrap(),
            ProtocolType::Dash
        );
        assert_eq!(
            resolve("https://cdn.example/live.m3u8").unwrap(),
            ProtocolType::Hls
        );
        let error = resolve("https://cdn.example/clip.mp4").unwrap_err();
        assert_eq!(error.kind, DownloadFailureKind::Configuration);
    }

    #[test]
    fn flv_downloads_use_the_configured_buffer_size() {
        let engine = MesioEngine::with_config(MesioEngineConfig {
            buffer_size: 256 * 1024,
            ..MesioEngineConfig::default()
        });
        assert_eq!(
            engine.flv_config.as_ref().map(|config| config.buffer_size),
            Some(256 * 1024)
        );
    }
}
