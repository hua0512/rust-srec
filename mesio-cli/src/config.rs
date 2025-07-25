use flv_fix::FlvPipelineConfig;
use hls_fix::HlsPipelineConfig;
use mesio_engine::{flv::FlvConfig, hls::HlsConfig};
use pipeline_common::config::PipelineConfig;

/// Configuration for the entire program
#[derive(Debug, Clone)]
pub struct ProgramConfig {
    /// Pipeline configuration
    pub pipeline_config: PipelineConfig,

    /// Pipeline configuration for FLV processing
    pub flv_pipeline_config: FlvPipelineConfig,

    /// Pipeline configuration for HLS processing
    pub hls_pipeline_config: HlsPipelineConfig,

    /// FLV-specific configuration
    pub flv_config: Option<FlvConfig>,

    /// HLS-specific configuration
    pub hls_config: Option<HlsConfig>,

    /// Whether to enable processing pipeline (vs raw download)
    pub enable_processing: bool,
}

impl ProgramConfig {
    /// Create a new builder for ProgramConfig
    #[inline]
    pub const fn builder() -> ProgramConfigBuilder {
        ProgramConfigBuilder::new()
    }
}

/// Builder for ProgramConfig
#[derive(Debug, Default)]
pub struct ProgramConfigBuilder {
    pipeline_config: Option<PipelineConfig>,
    flv_pipeline_config: Option<FlvPipelineConfig>,
    hls_pipeline_config: Option<HlsPipelineConfig>,
    flv_config: Option<FlvConfig>,
    hls_config: Option<HlsConfig>,
    enable_processing: bool,
}

impl ProgramConfigBuilder {
    /// Create a new builder with default values
    #[inline]
    pub const fn new() -> Self {
        Self {
            pipeline_config: None,
            flv_pipeline_config: None,
            hls_pipeline_config: None,
            flv_config: None,
            hls_config: None,
            enable_processing: true,
        }
    }

    /// Set the pipeline configuration
    #[inline]
    pub fn pipeline_config(mut self, config: PipelineConfig) -> Self {
        self.pipeline_config = Some(config);
        self
    }

    /// Set the FLV pipeline configuration
    #[inline]
    pub fn flv_pipeline_config(mut self, config: FlvPipelineConfig) -> Self {
        self.flv_pipeline_config = Some(config);
        self
    }

    /// Set the HLS pipeline configuration
    #[inline]
    pub fn hls_pipeline_config(mut self, config: HlsPipelineConfig) -> Self {
        self.hls_pipeline_config = Some(config);
        self
    }

    /// Set the FLV-specific configuration
    #[inline]
    pub fn flv_config(mut self, config: FlvConfig) -> Self {
        self.flv_config = Some(config);
        self
    }

    /// Set the HLS-specific configuration
    #[inline]
    pub fn hls_config(mut self, config: HlsConfig) -> Self {
        self.hls_config = Some(config);
        self
    }

    /// Set whether to enable processing pipeline
    #[inline]
    pub fn enable_processing(mut self, enable: bool) -> Self {
        self.enable_processing = enable;
        self
    }

    /// Build the ProgramConfig
    pub fn build(self) -> Result<ProgramConfig, &'static str> {
        let pipeline_config = self.pipeline_config.ok_or("pipeline_config is required")?;
        let flv_pipeline_config = self
            .flv_pipeline_config
            .ok_or("flv_pipeline_config is required")?;
        let hls_pipeline_config = self
            .hls_pipeline_config
            .ok_or("hls_pipeline_config is required")?;

        Ok(ProgramConfig {
            pipeline_config,
            flv_pipeline_config,
            hls_pipeline_config,
            flv_config: self.flv_config,
            hls_config: self.hls_config,
            enable_processing: self.enable_processing,
        })
    }
}
