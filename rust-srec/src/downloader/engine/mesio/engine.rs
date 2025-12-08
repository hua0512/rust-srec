//! Mesio native download engine implementation.
//!
//! This engine uses the `mesio` crate's factory pattern for protocol detection
//! and stream downloading. It supports HLS and FLV formats through the
//! `MesioDownloaderFactory` API.
//!
//! The engine acts as a thin coordinator that:
//! 1. Detects the protocol type from the URL
//! 2. Delegates to `HlsDownloader` or `FlvDownloader` for actual download logic
//!
//! Progress tracking is handled by the writers internally via `WriterTask` and
//! `WriterState`, eliminating the need for duplicate tracking in the engine.

use async_trait::async_trait;
use mesio::flv::FlvProtocolConfig;
use mesio::{FlvProtocolBuilder, HlsProtocolBuilder, MesioDownloaderFactory, ProtocolType};
use std::sync::Arc;
use tracing::{debug, error, info};

use super::flv_downloader::FlvDownloader;
use super::hls_downloader::HlsDownloader;
use crate::Result;
use crate::database::models::engine::MesioEngineConfig;
use crate::downloader::engine::traits::{DownloadEngine, DownloadHandle, EngineType, SegmentEvent};

/// Native Mesio download engine.
///
/// This engine uses the `mesio` crate's `MesioDownloaderFactory` for
/// protocol detection and delegates to specialized downloaders for
/// HLS and FLV formats.
///
/// The engine is a thin coordinator that:
/// - Detects protocol type from URL
/// - Creates appropriate downloader (HlsDownloader or FlvDownloader)
/// - Delegates download execution and propagates results
pub struct MesioEngine {
    /// Whether the engine is available.
    available: bool,
    /// Engine version.
    version: String,
    /// Engine configuration.
    config: MesioEngineConfig,
    /// Default HLS configuration.
    hls_config: Option<mesio::hls::HlsConfig>,
    /// Default FLV configuration.
    flv_config: Option<FlvProtocolConfig>,
}

impl MesioEngine {
    /// Create a new Mesio engine with default configurations.
    pub fn new() -> Self {
        Self::with_config(MesioEngineConfig::default())
    }

    /// Create with a custom configuration.
    pub fn with_config(config: MesioEngineConfig) -> Self {
        Self {
            available: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
            config,
            hls_config: Some(HlsProtocolBuilder::new().get_config()),
            flv_config: Some(FlvProtocolBuilder::new().get_config()),
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

    /// Detect the protocol type from a URL using the mesio factory.
    ///
    /// Returns the detected `ProtocolType` (HLS or FLV) based on URL patterns.
    pub fn detect_protocol(url: &str) -> Result<ProtocolType> {
        MesioDownloaderFactory::detect_protocol(url)
            .map_err(|e| crate::Error::Other(format!("Protocol detection failed: {}", e)))
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

    async fn start(&self, handle: Arc<DownloadHandle>) -> Result<()> {
        info!(
            "Starting mesio download for streamer {}",
            handle.config.streamer_id
        );

        // Detect protocol type using MesioDownloaderFactory
        let protocol_type = Self::detect_protocol(&handle.config.url)?;

        debug!(
            "Detected protocol {:?} for URL: {}",
            protocol_type, handle.config.url
        );

        // Delegate to appropriate downloader based on protocol type
        let download_result = match protocol_type {
            ProtocolType::Hls => {
                let downloader = HlsDownloader::new(
                    handle.config.clone(),
                    self.config.clone(),
                    handle.event_tx.clone(),
                    handle.cancellation_token.clone(),
                    self.hls_config.clone(),
                );
                downloader.run().await.map(|_| ())
            }
            ProtocolType::Flv => {
                let downloader = FlvDownloader::new(
                    handle.config.clone(),
                    self.config.clone(),
                    handle.event_tx.clone(),
                    handle.cancellation_token.clone(),
                    self.flv_config.clone(),
                );
                downloader.run().await.map(|_| ())
            }
            _ => {
                let error_msg = format!("Unsupported protocol type: {:?}", protocol_type);
                error!("{}", error_msg);
                let _ = handle
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: error_msg.clone(),
                        recoverable: false,
                    })
                    .await;
                return Err(crate::Error::Other(error_msg));
            }
        };

        // Log any errors (downloaders emit their own events internally)
        if let Err(e) = &download_result {
            error!(
                "Mesio download failed for {}: {}",
                handle.config.streamer_id, e
            );
        }

        download_result
    }

    async fn stop(&self, handle: &DownloadHandle) -> Result<()> {
        info!(
            "Stopping mesio download for streamer {}",
            handle.config.streamer_id
        );
        handle.cancel();
        Ok(())
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn version(&self) -> Option<String> {
        Some(self.version.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_type() {
        let engine = MesioEngine::new();
        assert_eq!(engine.engine_type(), EngineType::Mesio);
    }

    #[test]
    fn test_is_available() {
        let engine = MesioEngine::new();
        assert!(engine.is_available());
    }

    #[test]
    fn test_version() {
        let engine = MesioEngine::new();
        assert!(engine.version().is_some());
    }

    #[test]
    fn test_default() {
        let engine = MesioEngine::default();
        assert!(engine.is_available());
    }

    #[test]
    fn test_with_config() {
        let config = MesioEngineConfig::default();
        let engine = MesioEngine::with_config(config);
        assert!(engine.is_available());
    }

    #[test]
    fn test_detect_protocol_hls() {
        let result = MesioEngine::detect_protocol("https://example.com/stream.m3u8");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProtocolType::Hls);
    }

    #[test]
    fn test_detect_protocol_flv() {
        let result = MesioEngine::detect_protocol("https://example.com/stream.flv");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ProtocolType::Flv);
    }
}
