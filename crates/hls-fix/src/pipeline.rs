use std::sync::Arc;

use hls::HlsData;
use pipeline_common::{ChannelPipeline, PipelineProvider, StreamerContext, config::PipelineConfig};

use crate::operators::{DefragmentOperator, SegmentLimiterOperator, SegmentSplitOperator};

#[derive(Debug, Clone)]
pub struct HlsPipelineConfig {
    pub defragment: bool,
    pub split_segments: bool,
    pub segment_limiter: bool,
}

impl Default for HlsPipelineConfig {
    fn default() -> Self {
        Self {
            defragment: true,
            split_segments: true,
            segment_limiter: true,
        }
    }
}

impl HlsPipelineConfig {
    /// Create a new HLS pipeline configuration
    pub fn builder() -> HlsPipelineConfigBuilder {
        HlsPipelineConfigBuilder::new()
    }
}

pub struct HlsPipelineConfigBuilder {
    config: HlsPipelineConfig,
}

impl HlsPipelineConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: HlsPipelineConfig::default(),
        }
    }

    pub fn build(self) -> HlsPipelineConfig {
        self.config
    }
}

impl Default for HlsPipelineConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HlsPipeline {
    context: Arc<StreamerContext>,
    config: HlsPipelineConfig,
    common_config: PipelineConfig,
}

impl PipelineProvider for HlsPipeline {
    type Item = HlsData;
    type Config = HlsPipelineConfig;

    fn with_config(
        context: Arc<StreamerContext>,
        common_config: &PipelineConfig,
        config: Self::Config,
    ) -> Self {
        Self {
            context,
            config,
            common_config: common_config.clone(),
        }
    }

    fn build_pipeline(&self) -> ChannelPipeline<Self::Item> {
        let mut sync_pipeline = pipeline_common::Pipeline::new(self.context.clone());

        if self.config.defragment {
            sync_pipeline =
                sync_pipeline.add_processor(DefragmentOperator::new(self.context.clone()));
        }

        if self.config.split_segments {
            sync_pipeline =
                sync_pipeline.add_processor(SegmentSplitOperator::new(self.context.clone()));
        }

        if self.config.segment_limiter {
            sync_pipeline = sync_pipeline.add_processor(SegmentLimiterOperator::new(
                self.common_config.max_duration,
                Some(self.common_config.max_file_size),
            ));
        }

        ChannelPipeline::new(self.context.clone()).add_processor(sync_pipeline)
    }
}
