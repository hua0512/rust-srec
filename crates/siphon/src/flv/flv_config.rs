//! # FLV Protocol Configuration
//!
//! This module defines the configuration options specific to FLV downloads.

use crate::DownloaderConfig;
use crate::media_protocol::ProtocolConfig;
use std::fmt::Debug;

/// Configuration for FLV downloads
#[derive(Debug, Clone)]
pub struct FlvConfig {
    /// Base downloader configuration
    pub base: DownloaderConfig,
    /// Buffer size for download chunks (in bytes)
    pub buffer_size: usize,
}

const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB default buffer size

impl Default for FlvConfig {
    fn default() -> Self {
        Self {
            base: DownloaderConfig::default(),
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

impl ProtocolConfig for FlvConfig {}

impl From<DownloaderConfig> for FlvConfig {
    fn from(base: DownloaderConfig) -> Self {
        Self {
            base,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}
