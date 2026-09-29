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
    GopSortOperator, HeaderCheckOperator, LimitConfig, LimitOperator,
    MIN_INTERVAL_BETWEEN_KEYFRAMES_MS, RepairStrategy, ScriptFillerConfig, ScriptFilterOperator,
    ScriptKeyframesFillerOperator, SequenceHeaderChangeMode, SplitOperator,
    TimeConsistencyOperator, TimingRepairConfig, TimingRepairOperator,
};
use flv::data::FlvData;
use flv::error::FlvError;
use futures::stream::Stream;
use pipeline_common::config::PipelineConfig;
use pipeline_common::{Pipeline, PipelineProvider, StreamerContext};
use std::pin::Pin;
use std::sync::Arc;

/// Type alias for a boxed stream of FLV data with error handling
pub type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, FlvError>> + Send>>;

/// Configuration options for the FLV processing pipeline
#[derive(Debug, Clone)]
pub struct FlvPipelineConfig {
    /// Whether to filter duplicate media tags. Disabled by default.
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

    pub pipe_mode: bool,
}

impl Default for FlvPipelineConfig {
    fn default() -> Self {
        Self {
            duplicate_tag_filtering: false,
            duplicate_tag_filter_config: DuplicateTagFilterConfig::default(),
            sequence_header_change_mode: SequenceHeaderChangeMode::Crc32,
            drop_duplicate_sequence_headers: false,
            repair_strategy: RepairStrategy::Relaxed,
            continuity_mode: ContinuityMode::Reset,
            keyframe_index_config: Some(ScriptFillerConfig::default()),
            pipe_mode: false,
        }
    }
}

impl FlvPipelineConfig {
    /// Create a new builder for FlvPipelineConfig
    pub fn builder() -> FlvPipelineConfigBuilder {
        FlvPipelineConfigBuilder::new()
    }

    fn timing_repair_config(&self) -> TimingRepairConfig {
        TimingRepairConfig {
            strategy: self.repair_strategy,
            ..TimingRepairConfig::default()
        }
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
    fn build_pipeline(&self) -> Pipeline<FlvData> {
        let context = Arc::clone(&self.context);
        let config = self.config.clone();

        // Create all operators with adapters
        let defrag_operator = DefragmentOperator::new(context.clone());
        let header_check_operator = HeaderCheckOperator::new(context.clone(), true, true);

        // Configure the limit operator
        let max_duration_ms = self
            .common_config
            .max_duration
            .map(|duration| u32::try_from(duration.as_millis()).unwrap_or(u32::MAX));
        let limit_config = LimitConfig {
            max_size_bytes: if self.common_config.max_file_size > 0 {
                Some(self.common_config.max_file_size)
            } else {
                None
            },
            max_duration_ms,
            split_at_keyframes_only: true,
            on_split: None,
        };
        let limit_operator = LimitOperator::with_config(context.clone(), limit_config);

        // Create remaining operators
        let gop_sort_operator = GopSortOperator::new(context.clone());
        let timing_repair_operator =
            TimingRepairOperator::new(context.clone(), config.timing_repair_config());
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
            config.keyframe_index_config.map(|mut filler_config| {
                if let Some(max_duration_ms) = max_duration_ms {
                    filler_config.keyframe_duration_ms =
                        max_duration_ms.max(MIN_INTERVAL_BETWEEN_KEYFRAMES_MS);
                }
                ScriptKeyframesFillerOperator::new(context.clone(), filler_config)
            })
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
        if let Some(script_filter_op) = script_filter_operator {
            sync_pipeline.add_processor(script_filter_op)
        } else {
            sync_pipeline
        }
    }
}

#[cfg(test)]
/// Tests for the FLV processing pipeline
mod test {
    use super::*;

    fn aac_silence(timestamp: u32) -> FlvData {
        // AAC-LC stereo silence, one 1024-sample raw_data_block at 44.1 kHz.
        // Second AAC packet from FFmpeg: -f lavfi -i anullsrc=r=44100:cl=stereo
        // -frames:a 2 -c:a aac -f flv silence.flv
        FlvData::Tag(flv::FlvTag::new(
            timestamp,
            0,
            flv::FlvTagType::Audio,
            false,
            bytes::Bytes::from_static(b"\xaf\x01\x21\x10\x04\x60\x8c\x1c"),
        ))
    }

    #[test]
    fn duplicate_filtering_is_disabled_until_explicitly_enabled() {
        use pipeline_common::{CancellationToken, PipelineError};

        let mut unique = vec![FlvData::Header(flv::FlvHeader::new(true, false))];
        unique.extend((0..12).map(|frame| aac_silence(frame * 23)));
        let mut input = vec![unique[0].clone()];
        for tag in &unique[1..] {
            input.extend([tag.clone(), tag.clone()]);
        }
        for (config, expected) in [
            (FlvPipelineConfig::default(), input.clone()),
            (
                FlvPipelineConfig::builder()
                    .duplicate_tag_filtering(true)
                    .build(),
                unique,
            ),
        ] {
            let context = StreamerContext::arc_new(CancellationToken::new());
            let pipeline = FlvPipeline::with_config(context, &PipelineConfig::default(), config)
                .build_pipeline();
            let mut output = Vec::new();
            pipeline
                .run(
                    input.iter().cloned().map(Ok::<_, PipelineError>),
                    &mut |item| output.push(item.unwrap()),
                )
                .unwrap();
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn repeated_sequence_headers_preserve_media_timestamps() {
        use pipeline_common::{CancellationToken, PipelineError};

        use crate::test_utils::create_audio_sequence_header;

        let context = StreamerContext::arc_new(CancellationToken::new());
        let pipeline = FlvPipeline::with_config(
            context,
            &PipelineConfig::default(),
            FlvPipelineConfig::default(),
        )
        .build_pipeline();
        let mut input = vec![
            FlvData::Header(flv::FlvHeader::new(true, false)),
            create_audio_sequence_header(0, 0x12),
        ];
        // 1024 samples per AAC packet, with timestamps rounded to milliseconds.
        input.extend((0..130).map(|frame| aac_silence(frame * 1_024_000 / 44_100)));
        input.push(create_audio_sequence_header(0, 0x12));
        input.push(aac_silence(130 * 1_024_000 / 44_100));
        input.push(aac_silence(131 * 1_024_000 / 44_100));
        let expected = input.clone();
        let mut output = Vec::new();
        pipeline
            .run(input.into_iter().map(Ok::<_, PipelineError>), &mut |item| {
                output.push(item.unwrap());
            })
            .unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn duration_splits_reinject_av1_mpeg2_configuration() {
        use std::time::Duration;

        use bytes::Bytes;
        use flv::{FlvHeader, FlvTag, FlvTagType};
        use pipeline_common::{CancellationToken, PipelineError};

        let context = StreamerContext::arc_new(CancellationToken::new());
        let common_config = PipelineConfig {
            max_duration: Some(Duration::from_secs(1)),
            ..PipelineConfig::default()
        };
        let pipeline =
            FlvPipeline::with_config(context, &common_config, FlvPipelineConfig::default())
                .build_pipeline();
        // AV1 config and keyframe from FFmpeg/libaom-av1:
        // -f lavfi -i color=c=black:s=16x16:r=10 -frames:v 1
        // -c:v libaom-av1 -cpu-used 8 -g 1 -crf 40 -f flv black.flv
        // The sequence start uses a MPEG-2 descriptor wrapper (0x80, length 4).
        // FFmpeg is only needed to regenerate these bytes, not to run the test.
        let sequence = FlvTag::new(
            0,
            0,
            FlvTagType::Video,
            false,
            Bytes::from_static(&[
                0x95, b'a', b'v', b'0', b'1', 0x80, 4, 0x81, 0, 0x0c, 0, 0x0a, 0x0a, 0, 0, 0, 1,
                0x9f, 0xf9, 0xb5, 0xf2, 0, 0x80,
            ]),
        );
        let init = vec![
            FlvData::Header(FlvHeader::new(false, true)),
            FlvData::Tag(sequence),
        ];
        let mut input = init.clone();
        let mut expected = init.clone();
        for timestamp in (0..=1200).step_by(100) {
            let mut tag = FlvTag::new(
                timestamp,
                0,
                FlvTagType::Video,
                false,
                Bytes::from_static(b"\x91av01\x12\0\x0a\x0a\0\0\0\x01\x9f\xf9\xb5\xf2\0\x80\x32\x0e\x10\0\xd0\0\0\x02\x80\0\0\0\xa9\x8e\x5e\xd0"),
            );
            input.push(FlvData::Tag(tag.clone()));
            if timestamp == 1000 {
                expected.push(FlvData::Split(flv::SplitReason::DurationLimit));
                expected.extend(init.clone());
            }
            if timestamp >= 1000 {
                tag.timestamp_ms -= 1000;
            }
            expected.push(FlvData::Tag(tag));
        }
        let mut output = Vec::new();
        pipeline
            .run(input.into_iter().map(Ok::<_, PipelineError>), &mut |item| {
                output.push(item.unwrap())
            })
            .unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn repair_strategy_defaults_to_relaxed_and_forwards_overrides() {
        use pipeline_common::{CancellationToken, PipelineError};

        use crate::test_utils::create_audio_sequence_header;

        let mut input = vec![
            FlvData::Header(flv::FlvHeader::new(true, false)),
            create_audio_sequence_header(0, 0x12),
        ];
        input.extend([0, 23, 46, 69, 92, 115, 138, 161, 184, 207, 500, 523].map(aac_silence));
        for (config, expected_tail) in [
            (FlvPipelineConfig::default(), [500, 523]),
            (
                FlvPipelineConfig::builder()
                    .repair_strategy(RepairStrategy::Strict)
                    .build(),
                [230, 253],
            ),
        ] {
            let context = StreamerContext::arc_new(CancellationToken::new());
            let pipeline = FlvPipeline::with_config(context, &PipelineConfig::default(), config)
                .build_pipeline();
            let mut expected = input[..input.len() - 2].to_vec();
            expected.extend(expected_tail.map(aac_silence));
            let mut output = Vec::new();
            pipeline
                .run(
                    input.iter().cloned().map(Ok::<_, PipelineError>),
                    &mut |item| output.push(item.unwrap()),
                )
                .unwrap();
            assert_eq!(output, expected);
        }
    }
}
