use flv_fix::PipelineConfig;
use mesio_engine::{flv::FlvConfig, hls::HlsConfig};

use crate::output::provider::OutputFormat;

/// Configuration for the entire program
#[derive(Debug, Clone)]
pub struct ProgramConfig {
    /// Pipeline configuration for FLV processing
    pub pipeline_config: PipelineConfig,

    /// FLV-specific configuration
    pub flv_config: Option<FlvConfig>,

    /// HLS-specific configuration
    pub hls_config: Option<HlsConfig>,

    /// Whether to enable processing pipeline (vs raw download)
    pub enable_processing: bool,

    /// Size of internal processing channels
    pub channel_size: usize,

    /// Output format to use
    pub output_format: Option<OutputFormat>,
}
