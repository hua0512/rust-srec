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

impl FlvConfig {
    /// Create a new builder for FlvConfig
    pub fn builder() -> FlvConfigBuilder {
        FlvConfigBuilder::new()
    }
}

/// Builder for FlvConfig
#[derive(Debug, Clone)]
pub struct FlvConfigBuilder {
    base: DownloaderConfig,
    buffer_size: usize,
}

impl FlvConfigBuilder {
    /// Create a new FlvConfigBuilder with default values
    pub fn new() -> Self {
        Self {
            base: DownloaderConfig::default(),
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }

    /// Set the base downloader configuration
    pub fn with_base_config(mut self, base: DownloaderConfig) -> Self {
        self.base = base;
        self
    }

    /// Set the buffer size for download chunks
    pub fn buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Build the FlvConfig
    pub fn build(self) -> FlvConfig {
        FlvConfig {
            base: self.base,
            buffer_size: self.buffer_size,
        }
    }
}

impl Default for FlvConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}
