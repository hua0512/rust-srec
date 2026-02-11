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
    ContinuityMode, DefragmentOperator, DuplicateTagFilterConfig, DuplicateTagFilterOperator,
    GopSortOperator, HeaderCheckOperator, LimitConfig, LimitOperator, RepairStrategy,
    ScriptFillerConfig, ScriptFilterOperator, ScriptKeyframesFillerOperator,
    SequenceHeaderChangeMode, SplitOperator, TimeConsistencyOperator, TimingRepairConfig,
    TimingRepairOperator,
};
use flv::data::FlvData;
use flv::error::FlvError;
use futures::stream::Stream;
use pipeline_common::config::PipelineConfig;
use pipeline_common::{ChannelPipeline, PipelineProvider, StreamerContext};
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for a boxed stream of FLV data with error handling
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, FlvError>> + Send>>;

/// Configuration options for the FLV processing pipeline
#[derive(Debug, Clone)]
pub struct FlvPipelineConfig {
    /// Whether to filter duplicate tags
    pub duplicate_tag_filtering: bool,

    /// Configuration for duplicate media-tag filtering (used when
    /// `duplicate_tag_filtering` is enabled).
    pub duplicate_tag_filter_config: DuplicateTagFilterConfig,

    /// How to detect audio/video sequence-header changes that trigger a split.
    pub sequence_header_change_mode: SequenceHeaderChangeMode,

    /// Whether to drop semantically duplicate audio/video sequence headers.
    ///
    /// When enabled, the pipeline will suppress repeated AAC/AVC/HEVC sequence
    /// headers that carry the same codec configuration. This can reduce player
    /// stutter caused by redundant decoder re-initialization signals, but may
    /// reduce "mid-stream join" friendliness for live pipelines.
    pub drop_duplicate_sequence_headers: bool,

    /// Strategy for timestamp repair
    pub repair_strategy: RepairStrategy,

    /// Mode for timeline continuity
    pub continuity_mode: ContinuityMode,

    /// Configuration for keyframe index injection
    pub keyframe_index_config: Option<ScriptFillerConfig>,

    pub enable_low_latency: bool,

    pub pipe_mode: bool,
}

impl Default for FlvPipelineConfig {
    fn default() -> Self {
        Self {
            duplicate_tag_filtering: true,
            duplicate_tag_filter_config: DuplicateTagFilterConfig::default(),
            sequence_header_change_mode: SequenceHeaderChangeMode::Crc32,
            drop_duplicate_sequence_headers: false,
            repair_strategy: RepairStrategy::Strict,
            continuity_mode: ContinuityMode::Reset,
            keyframe_index_config: Some(ScriptFillerConfig::default()),
            enable_low_latency: true,
            pipe_mode: false,
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

    pub fn duplicate_tag_filter_config(
        mut self,
        duplicate_tag_filter_config: DuplicateTagFilterConfig,
    ) -> Self {
        self.config.duplicate_tag_filter_config = duplicate_tag_filter_config;
        self
    }

    pub fn sequence_header_change_mode(
        mut self,
        sequence_header_change_mode: SequenceHeaderChangeMode,
    ) -> Self {
        self.config.sequence_header_change_mode = sequence_header_change_mode;
        self
    }

    pub fn drop_duplicate_sequence_headers(
        mut self,
        drop_duplicate_sequence_headers: bool,
    ) -> Self {
        self.config.drop_duplicate_sequence_headers = drop_duplicate_sequence_headers;
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

    pub fn enable_low_latency(mut self, enable_low_latency: bool) -> Self {
        self.config.enable_low_latency = enable_low_latency;
        self
    }

    /// Set pipe mode for the keyframe index config.
    /// When true, AMF0 processing is skipped since keyframe injection is not needed for pipe output.
    pub fn pipe_mode(mut self, pipe_mode: bool) -> Self {
        self.config.pipe_mode = pipe_mode;
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
    common_config: PipelineConfig,
}

impl PipelineProvider for FlvPipeline {
    type Item = FlvData;
    type Config = FlvPipelineConfig;

    fn with_config(
        context: Arc<StreamerContext>,
        common_config: &PipelineConfig,
        config: FlvPipelineConfig,
    ) -> Self {
        Self {
            context,
            config,
            common_config: common_config.clone(),
        }
    }

    /// Create and configure the pipeline with all necessary operators
    fn build_pipeline(&self) -> ChannelPipeline<FlvData> {
        let context = Arc::clone(&self.context);
        let config = self.config.clone();

        // Create all operators with adapters
        let defrag_operator = DefragmentOperator::new(context.clone());
        let header_check_operator = HeaderCheckOperator::new(context.clone(), true, true);

        // Configure the limit operator
        let limit_config = LimitConfig {
            max_size_bytes: if self.common_config.max_file_size > 0 {
                Some(self.common_config.max_file_size)
            } else {
                None
            },
            max_duration_ms: self
                .common_config
                .max_duration
                .map(|d| d.as_millis() as u32),
            split_at_keyframes_only: true,
            on_split: None,
        };
        let limit_operator = LimitOperator::with_config(context.clone(), limit_config);

        // Create remaining operators
        let gop_sort_operator = GopSortOperator::new(context.clone());
        let timing_repair_operator =
            TimingRepairOperator::new(context.clone(), TimingRepairConfig::default());
        let split_operator = SplitOperator::with_config(
            context.clone(),
            config.sequence_header_change_mode,
            config.drop_duplicate_sequence_headers,
        );

        let duplicate_tag_filter_operator = if config.duplicate_tag_filtering {
            Some(DuplicateTagFilterOperator::with_config(
                context.clone(),
                config.duplicate_tag_filter_config.clone(),
            ))
        } else {
            None
        };
        let time_consistency_operator =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);
        let time_consistency_operator_2 =
            TimeConsistencyOperator::new(context.clone(), config.continuity_mode);

        // Determine if we're in pipe mode - skip script-related operators
        // In pipe mode, AMF0 metadata modification is unnecessary overhead
        let is_pipe_mode = config.pipe_mode;

        // Create the KeyframeIndexInjector operator if enabled and not in pipe mode
        let keyframe_index_operator = if !is_pipe_mode && config.keyframe_index_config.is_some() {
            config
                .keyframe_index_config
                .map(|c| ScriptKeyframesFillerOperator::new(context.clone(), c))
        } else {
            None
        };

        // Create the ScriptFilter operator only if not in pipe mode
        let script_filter_operator = if !is_pipe_mode {
            Some(ScriptFilterOperator::new(context.clone()))
        } else {
            None
        };

        // Build the synchronous pipeline
        let mut sync_pipeline = pipeline_common::Pipeline::new(context.clone())
            .add_processor(defrag_operator)
            .add_processor(header_check_operator)
            .add_processor(split_operator)
            .add_processor(gop_sort_operator);

        if let Some(op) = duplicate_tag_filter_operator {
            sync_pipeline = sync_pipeline.add_processor(op);
        }

        sync_pipeline = sync_pipeline
            .add_processor(time_consistency_operator)
            .add_processor(timing_repair_operator)
            .add_processor(limit_operator)
            .add_processor(time_consistency_operator_2);

        // Add keyframe filler
        if let Some(keyframe_op) = keyframe_index_operator {
            sync_pipeline = sync_pipeline.add_processor(keyframe_op);
        }

        // Add script filter
        let sync_pipeline = if let Some(script_filter_op) = script_filter_operator {
            sync_pipeline.add_processor(script_filter_op)
        } else {
            sync_pipeline
        };

        // Wrap it in a ChannelPipeline to offload processing to a dedicated thread
        ChannelPipeline::new(context).add_processor(sync_pipeline)
    }
}

#[cfg(test)]
/// Tests for the FLV processing pipeline
mod test {
    use super::*;
    use crate::writer::FlvWriter;
    use crate::writer_task::FlvWriterConfig;

    use flv::data::FlvData;
    use flv::parser_async::FlvDecoderStream;
    use futures::StreamExt;
    use pipeline_common::{
        CancellationToken, PipelineError, ProtocolWriter, WriterError, WriterStats,
        init_test_tracing,
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
        let context = Arc::new(StreamerContext::new(CancellationToken::new()));

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

        // Use tokio channel for input to allow blocking_recv which is Sync friendly
        let (sender, mut receiver) =
            tokio::sync::mpsc::channel::<Result<FlvData, PipelineError>>(8);

        let (output_tx, output_rx) =
            tokio::sync::mpsc::channel::<Result<FlvData, PipelineError>>(8);

        let process_task = Some(tokio::task::spawn_blocking(move || {
            let pipeline = pipeline.build_pipeline();

            let input =
                std::iter::from_fn(move || receiver.blocking_recv().map(Some).unwrap_or(None));

            let mut output = |result: Result<FlvData, PipelineError>| {
                if output_tx.blocking_send(result).is_err() {
                    tracing::warn!("Output channel closed, stopping processing");
                }
            };

            if let Err(err) = pipeline.run(input, &mut output)
                && !matches!(err, PipelineError::Cancelled)
            {
                output_tx
                    .blocking_send(Err(PipelineError::Strategy(Box::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Pipeline error: {err}"),
                    )))))
                    .ok();
            }
        }));

        // Run the writer task with the receiver
        let writer_handle = tokio::task::spawn_blocking(move || {
            let mut writer_task = FlvWriter::new(FlvWriterConfig {
                output_dir,
                base_name,
                enable_low_latency: true,
            });

            let stats = writer_task.run(output_rx)?;

            Ok::<_, WriterError>(stats)
        });

        // Ensure the forwarding task completes
        while let Some(result) = decoder_stream.next().await {
            if sender
                .send(result.map_err(|e| PipelineError::Strategy(Box::new(e))))
                .await
                .is_err()
            {
                break;
            }
        }
        drop(sender); // Close the channel to signal completion

        let stats: WriterStats = writer_handle.await??;

        // Wait for the processing task to finish
        if let Some(p) = process_task {
            p.await?;
        }

        let elapsed = start_time.elapsed();

        info!(
            duration = ?elapsed,
            total_tags = stats.items_written,
            files_written = stats.files_created,
            "Pipeline finished processing"
        );

        // Basic assertions (optional, but good for tests)
        assert!(
            stats.files_created > 0,
            "Expected at least one output file to be created"
        );
        assert!(stats.items_written > 0, "Expected tags to be processed");

        Ok(())
    }
}
