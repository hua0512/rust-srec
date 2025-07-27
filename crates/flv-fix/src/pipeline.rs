//! # FLV Processing Pipeline
//!
//! This module implements a processing pipeline for fixing and optimizing FLV (Flash Video) streams.
//! The pipeline consists of multiple operators that can transform, validate, and repair FLV data
//! to ensure proper playability and standards compliance.
//!
//! ## Pipeline Architecture
//!
//! Input → Defragment → HeaderCheck → Split → GopSort → TimeConsistency →
//!        TimingRepair → Limit → TimeConsistency2 → ScriptKeyframesFiller → ScriptFilter → Output
//!
//! Each operator addresses specific issues that can occur in FLV streams:
//!
//! - **Defragment**: Handles fragmented streams by buffering and validating segments
//! - **HeaderCheck**: Ensures streams begin with a valid FLV header
//! - **Split**: Divides content at appropriate points for better playability
//! - **GopSort**: Ensures video tags are properly ordered by GOP (Group of Pictures)
//! - **TimeConsistency**: Maintains consistent timestamps throughout the stream
//! - **TimingRepair**: Fixes timestamp anomalies like negative values or jumps
//! - **Limit**: Enforces file size and duration limits
//! - **ScriptKeyframesFiller**: Prepares metadata for proper seeking by adding keyframe placeholders
//! - **ScriptFilter**: Removes or modifies problematic script tags

use crate::operators::{
    ContinuityMode, DefragmentOperator, GopSortOperator, HeaderCheckOperator, LimitConfig,
    LimitOperator, RepairStrategy, ScriptFillerConfig, ScriptFilterOperator,
    ScriptKeyframesFillerOperator, SplitOperator, TimeConsistencyOperator, TimingRepairConfig,
    TimingRepairOperator,
};
use flv::data::FlvData;
use flv::error::FlvError;
use futures::stream::Stream;
use pipeline_common::config::PipelineConfig;
use pipeline_common::{Pipeline, PipelineProvider, StreamerContext};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Type alias for a boxed stream of FLV data with error handling
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, FlvError>> + Send>>;

/// Configuration options for the FLV processing pipeline
#[derive(Debug, Clone)]
pub struct FlvPipelineConfig {
    /// Whether to filter duplicate tags
    pub duplicate_tag_filtering: bool,

    /// Strategy for timestamp repair
    pub repair_strategy: RepairStrategy,

    /// Mode for timeline continuity
    pub continuity_mode: ContinuityMode,

    /// Configuration for keyframe index injection
    pub keyframe_index_config: Option<ScriptFillerConfig>,
}

impl Default for FlvPipelineConfig {
    fn default() -> Self {
        Self {
            duplicate_tag_filtering: true,
            repair_strategy: RepairStrategy::Strict,
            continuity_mode: ContinuityMode::Reset,
            keyframe_index_config: Some(ScriptFillerConfig::default()),
        }
    }
}

impl FlvPipelineConfig {
    /// Create a new builder for FlvPipelineConfig
    pub fn builder() -> FlvPipelineConfigBuilder {
        FlvPipelineConfigBuilder::new()
    }
}

pub struct FlvPipelineConfigBuilder {
    config: FlvPipelineConfig,
}

impl FlvPipelineConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: FlvPipelineConfig::default(),
        }
    }

    pub fn duplicate_tag_filtering(mut self, duplicate_tag_filtering: bool) -> Self {
        self.config.duplicate_tag_filtering = duplicate_tag_filtering;
        self
    }

    pub fn repair_strategy(mut self, repair_strategy: RepairStrategy) -> Self {
        self.config.repair_strategy = repair_strategy;
        self
    }

    pub fn continuity_mode(mut self, continuity_mode: ContinuityMode) -> Self {
        self.config.continuity_mode = continuity_mode;
        self
    }

    pub fn keyframe_index_config(
        mut self,
        keyframe_index_config: Option<ScriptFillerConfig>,
    ) -> Self {
        self.config.keyframe_index_config = keyframe_index_config;
        self
    }

    pub fn build(self) -> FlvPipelineConfig {
        self.config
    }
}

impl Default for FlvPipelineConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Main pipeline for processing FLV streams
pub struct FlvPipeline {
    context: Arc<StreamerContext>,
    config: FlvPipelineConfig,

    max_file_size: u64,
    max_duration: Option<Duration>,
}

impl PipelineProvider for FlvPipeline {
    type Item = FlvData;
    type Config = FlvPipelineConfig;

    fn with_config(
        context: StreamerContext,
        common_config: &PipelineConfig,
        config: FlvPipelineConfig,
    ) -> Self {
        Self {
            context: Arc::new(context),
            config,
            max_file_size: common_config.max_file_size,
            max_duration: common_config.max_duration,
        }
    }

    /// Create and configure the pipeline with all necessary operators
    fn build_pipeline(&self) -> Pipeline<FlvData> {
        let context = Arc::clone(&self.context);
        let config = self.config.clone();

        // Create all operators with adapters
        let defrag_operator = DefragmentOperator::new(context.clone());
        let header_check_operator = HeaderCheckOperator::new(context.clone());

        // Configure the limit operator
        let limit_config = LimitConfig {
            max_size_bytes: if self.max_file_size > 0 {
                Some(self.max_file_size)
            } else {
                None
            },
            max_duration_ms: if self.max_duration.is_some() {
                Some(self.max_duration.unwrap().as_millis() as u32)
            } else {
                None
            },
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: None,
        };
        let limit_operator = LimitOperator::with_config(context.clone(), limit_config);

        // Create remaining operators
        let gop_sort_operator = GopSortOperator::new(context.clone());
        let script_filter_operator = ScriptFilterOperator::new(context.clone());
        let timing_repair_operator =
            TimingRepairOperator::new(context.clone(), TimingRepairConfig::default());
        let split_operator = SplitOperator::new(context.clone());
        let time_consistency_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);
        let time_consistency_2_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);

        // Create the KeyframeIndexInjector operator if enabled
        let keyframe_index_operator = if let Some(keyframe_config) = config.keyframe_index_config {
            ScriptKeyframesFillerOperator::new(context.clone(), keyframe_config)
        } else {
            ScriptKeyframesFillerOperator::new(context.clone(), ScriptFillerConfig::default())
        };

        // Build the pipeline using the generic Pipeline implementation
        Pipeline::new(context)
            .add_processor(defrag_operator)
            .add_processor(header_check_operator)
            .add_processor(split_operator)
            .add_processor(gop_sort_operator)
            .add_processor(time_consistency_operator)
            .add_processor(timing_repair_operator)
            .add_processor(limit_operator)
            .add_processor(time_consistency_2_operator)
            .add_processor(keyframe_index_operator)
            .add_processor(script_filter_operator)
    }
}

#[cfg(test)]
/// Tests for the FLV processing pipeline
mod test {
    use super::*;
    use crate::{FlvStrategyError, writer::FlvWriter};

    use flv::data::FlvData;
    use flv::parser_async::FlvDecoderStream;
    use futures::StreamExt;
    use pipeline_common::{
        PipelineError, ProgressEvent, ProtocolWriter, WriterError, init_test_tracing,
    };

    use std::path::Path;
    use tracing::info;

    #[tokio::test]
    #[ignore]
    async fn test_process() -> Result<(), Box<dyn std::error::Error>> {
        init_test_tracing!();

        // Source and destination paths
        let input_path = Path::new("D:/test/999/16_02_26-福州~ 主播恋爱脑！！！.flv");

        // Skip if test file doesn't exist
        if !input_path.exists() {
            info!(path = %input_path.display(), "Test file not found, skipping test");
            return Ok(());
        }

        let output_dir = input_path.parent().ok_or("Invalid input path")?.join("fix");
        tokio::fs::create_dir_all(&output_dir)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = ?e, "Output directory creation failed or already exists");
            });
        let base_name = input_path
            .file_stem()
            .ok_or("No file stem")?
            .to_string_lossy()
            .to_string();

        let start_time = std::time::Instant::now(); // Start timer
        info!(path = %input_path.display(), "Starting FLV processing pipeline test");

        // Create the context
        let context = StreamerContext::default();

        // Create the pipeline with default configuration
        let pipeline = FlvPipeline::with_config(
            context,
            &PipelineConfig::default(),
            FlvPipelineConfig::default(),
        );

        // Start a task to parse the input file using async Decoder
        let file_reader = tokio::io::BufReader::new(tokio::fs::File::open(input_path).await?);
        let mut decoder_stream = FlvDecoderStream::with_capacity(
            file_reader,
            32 * 1024, // Input buffer capacity
        );

        let (sender, receiver) = crossbeam_channel::bounded::<Result<FlvData, PipelineError>>(8);

        let (output_tx, output_rx) =
            crossbeam_channel::bounded::<Result<FlvData, PipelineError>>(8);

        let process_task = Some(tokio::task::spawn_blocking(move || {
            let pipeline = pipeline.build_pipeline();

            let input = std::iter::from_fn(|| receiver.recv().map(Some).unwrap_or(None));

            let mut output = |result: Result<FlvData, PipelineError>| {
                if output_tx.send(result).is_err() {
                    tracing::warn!("Output channel closed, astopping processing");
                }
            };

            if let Err(err) = pipeline.process(input, &mut output) {
                output_tx
                    .send(Err(PipelineError::Processing(format!(
                        "Pipeline error: {err}"
                    ))))
                    .ok();
            }
        }));

        // Run the writer task with the receiver
        let writer_handle = tokio::task::spawn_blocking(move || {
            let mut writer_task =
                FlvWriter::<fn(ProgressEvent)>::new(output_dir, base_name, "flv".to_string(), None);

            writer_task.run(output_rx)?;

            Ok::<_, WriterError<FlvStrategyError>>((
                writer_task.get_state().items_written_total,
                writer_task.get_state().file_sequence_number,
            ))
        });

        // Ensure the forwarding task completes
        while let Some(result) = decoder_stream.next().await {
            sender
                .send(result.map_err(|e| PipelineError::Processing(e.to_string())))
                .unwrap();
        }
        drop(sender); // Close the channel to signal completion

        let (total_tags_written, files_created) = writer_handle.await??;

        // Wait for the processing task to finish
        if let Some(p) = process_task {
            p.await?;
        }

        let elapsed = start_time.elapsed();

        info!(
            duration = ?elapsed,
            total_tags = total_tags_written,
            files_written = files_created,
            "Pipeline finished processing"
        );

        // Basic assertions (optional, but good for tests)
        assert!(
            files_created > 0,
            "Expected at least one output file to be created"
        );
        assert!(total_tags_written > 0, "Expected tags to be processed");

        Ok(())
    }
}
