use flv_fix::PipelineConfig;
use siphon_engine::{DownloaderConfig, flv::FlvConfig, hls::HlsConfig};

use crate::output::output::OutputFormat;

/// Configuration for the entire program
#[derive(Debug, Clone)]
pub struct ProgramConfig {
    /// Pipeline configuration for FLV processing
    pub pipeline_config: PipelineConfig,

    /// FLV-specific configuration
    pub flv_config: Option<FlvConfig>,

    /// HLS-specific configuration
    pub hls_config: Option<HlsConfig>,

    /// Common downloader configuration
    pub download_config: Option<DownloaderConfig>,

    /// Whether to enable processing pipeline (vs raw download)
    pub enable_processing: bool,

    /// Size of internal processing channels
    pub channel_size: usize,

    /// Output format to use
    pub output_format: Option<OutputFormat>,
}
