use std::{sync::Arc, time::Duration};

use hls::HlsData;
use pipeline_common::{Pipeline, PipelineProvider, StreamerContext, config::PipelineConfig};

use crate::operators::{DefragmentOperator, SegmentLimiterOperator, SegmentSplitOperator};

#[derive(Debug, Clone, Default)]
pub struct HlsPipelineConfig {}

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
    #[allow(dead_code)]
    config: HlsPipelineConfig,
    max_file_size: u64,
    max_duration: Option<Duration>,
}

impl PipelineProvider for HlsPipeline {
    type Item = HlsData;
    type Config = HlsPipelineConfig;

    fn with_config(
        context: StreamerContext,
        common_config: &PipelineConfig,
        config: Self::Config,
    ) -> Self {
        Self {
            context: Arc::new(context),
            config,
            max_file_size: common_config.max_file_size,
            max_duration: common_config.max_duration,
        }
    }

    fn build_pipeline(&self) -> Pipeline<Self::Item> {
        let defrag_operator = DefragmentOperator::new(self.context.clone());
        let limit_operator =
            SegmentLimiterOperator::new(self.max_duration, Some(self.max_file_size));
        let split_operator = SegmentSplitOperator::new(self.context.clone());

        Pipeline::new(self.context.clone())
            .add_processor(defrag_operator)
            .add_processor(split_operator)
            .add_processor(limit_operator)
    }
}
