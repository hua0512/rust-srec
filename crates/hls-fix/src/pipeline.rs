//! # HLS Processing Pipeline
//!
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use hls::segment::HlsData;
use pipeline_common::{Pipeline, PipelineError, StreamerContext};
use std::sync::Arc;

use crate::operators::SegmentLimiterOperator;

/// Configuration options for the HLS processing pipeline
#[derive(Debug, Clone, Default)]
pub struct PipelineConfig {
    /// Maximum duration of segments to include (in seconds)
    pub max_duration: Option<u64>,

    /// Maximum total size of segments to include (in bytes)
    pub max_size: Option<u64>,
}

/// HLS processing pipeline
pub struct HlsPipeline {
    context: Arc<StreamerContext>,
    config: PipelineConfig,
}

impl HlsPipeline {
    /// Create a new pipeline with default configuration
    pub fn new(name: impl Into<String>) -> Self {
        let context = StreamerContext::with_name(name);

        Self {
            context: Arc::new(context),
            config: PipelineConfig::default(),
        }
    }

    /// Create a new pipeline with custom configuration
    pub fn with_config(name: impl Into<String>, config: PipelineConfig) -> Self {
        let context = StreamerContext::with_name(name);

        Self {
            context: Arc::new(context),
            config,
        }
    }

    /// Create and configure the pipeline with all necessary operators
    pub fn build_pipeline(&self) -> Pipeline<HlsData> {
        let context = Arc::clone(&self.context);
        let config = self.config.clone();

        // Convert duration from seconds to Duration type
        let max_duration = config.max_duration.map(std::time::Duration::from_secs);
        let segment_limiter = SegmentLimiterOperator::new(max_duration, config.max_size);

        // Build the pipeline
        Pipeline::new(context).add_processor(segment_limiter)
    }

    /// Process a stream of HLS data
    pub fn process(
        &self,
        input: impl Iterator<Item = Result<HlsData, PipelineError>>,
        output: &mut impl FnMut(Result<HlsData, PipelineError>),
    ) -> Result<(), PipelineError> {
        // Build the pipeline
        let pipeline = self.build_pipeline();

        // Run the pipeline and convert any errors
        pipeline.process(input, output)
    }
}
