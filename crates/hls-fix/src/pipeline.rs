use std::sync::Arc;

use hls::HlsData;
use pipeline_common::{Pipeline, StreamerContext};

use crate::{
    SegmentLimiterOperator, SegmentSplitOperator, operators::defragment::DefragmentOperator,
};

pub struct HlsPipelineConfig {
    pub max_segment_duration: Option<u64>,
    pub max_segments: Option<u64>,
}

pub struct HlsPipeline {
    context: Arc<StreamerContext>,
    config: HlsPipelineConfig,
}

impl Default for HlsPipelineConfig {
    fn default() -> Self {
        Self {
            max_segment_duration: None,
            max_segments: None,
        }
    }
}

impl HlsPipeline {
    pub fn new(context: Arc<StreamerContext>, config: HlsPipelineConfig) -> Self {
        Self { context, config }
    }

    pub fn build_pipeline(&self) -> Pipeline<HlsData> {
        let context = self.context.clone();

        let defrag_operator = DefragmentOperator::new(context.clone());
        let limit_operator = SegmentLimiterOperator::new(None, None);
        let split_operator = SegmentSplitOperator::new(context.clone());

        let pipeline = Pipeline::new(context.clone())
            // .add_processor(defrag_operator)
            .add_processor(split_operator)
            .add_processor(limit_operator);

        return pipeline;
    }
}
