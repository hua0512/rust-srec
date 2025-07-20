use std::{sync::Arc, time::Duration};

use hls::HlsData;
use pipeline_common::{Pipeline, StreamerContext};

use crate::operators::{DefragmentOperator, SegmentLimiterOperator, SegmentSplitOperator};

#[derive(Debug, Default)]
pub struct HlsPipelineConfig {
    pub max_duration_limit: Option<u64>,
    pub max_file_size: u64,
}

pub struct HlsPipeline {
    context: Arc<StreamerContext>,
    config: HlsPipelineConfig,
}

impl HlsPipeline {
    pub fn new(context: Arc<StreamerContext>, config: HlsPipelineConfig) -> Self {
        Self { context, config }
    }

    pub fn build_pipeline(&self) -> Pipeline<HlsData> {
        let context = self.context.clone();

        let defrag_operator = DefragmentOperator::new(context.clone());
        let max_duration_limit = self.config.max_duration_limit.map(Duration::from_millis);
        let limit_operator =
            SegmentLimiterOperator::new(max_duration_limit, Some(self.config.max_file_size));
        let split_operator = SegmentSplitOperator::new(context.clone());

        Pipeline::new(context.clone())
            .add_processor(defrag_operator)
            .add_processor(split_operator)
            .add_processor(limit_operator)
    }
}
