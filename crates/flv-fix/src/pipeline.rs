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
    use std::time::Duration;

    use bytes::Bytes;
    use flv::{FlvHeader, FlvTag, FlvTagType, SplitReason};
    use pipeline_common::{CancellationToken, PipelineError};

    use super::*;
    use crate::test_utils::create_audio_sequence_header;
    use crate::test_utils::fixtures::{
        AAC_LC_SILENCE, AV1_CODEC_CONFIG, AV1_KEYFRAME, AVC_CONFIG_16X16, AVC_IDR_16X16,
        AVC_P_16X16,
    };

    /// Duration of one 1024-sample AAC frame at 44.1 kHz, rounded down.
    const AAC_FRAME_MS: u32 = 23;

    fn tag(timestamp: u32, kind: FlvTagType, data: &'static [u8]) -> FlvData {
        FlvData::Tag(FlvTag::new(
            timestamp,
            0,
            kind,
            false,
            Bytes::from_static(data),
        ))
    }

    fn aac_silence(timestamp: u32) -> FlvData {
        tag(timestamp, FlvTagType::Audio, AAC_LC_SILENCE)
    }

    fn run_pipeline(
        common: &PipelineConfig,
        config: FlvPipelineConfig,
        input: impl IntoIterator<Item = FlvData>,
    ) -> Vec<FlvData> {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let pipeline = FlvPipeline::with_config(context, common, config).build_pipeline();
        let mut output = Vec::new();
        pipeline
            .run(input.into_iter().map(Ok::<_, PipelineError>), &mut |item| {
                output.push(item.unwrap())
            })
            .unwrap();
        output
    }

    #[test]
    fn duplicate_filtering_is_disabled_until_explicitly_enabled() {
        let mut unique = vec![FlvData::Header(FlvHeader::new(true, false))];
        unique.extend((0..12).map(|frame| aac_silence(frame * AAC_FRAME_MS)));
        let mut input = vec![unique[0].clone()];
        for tag in &unique[1..] {
            input.extend([tag.clone(), tag.clone()]);
        }
        let enabled = FlvPipelineConfig::builder()
            .duplicate_tag_filtering(true)
            .build();
        for (config, expected) in [
            (FlvPipelineConfig::default(), input.clone()),
            (enabled, unique),
        ] {
            assert_eq!(
                run_pipeline(&PipelineConfig::default(), config, input.clone()),
                expected
            );
        }
    }

    #[test]
    fn replay_removal_follows_stream_time_across_delivery_pauses() {
        // 10 fps video with one PCM audio packet per frame and a keyframe every
        // 10 frames. GOP sorting upstream releases each GOP when the next
        // keyframe arrives, so the filter receives the stream in bursts.
        const FRAMES_PER_GOP: u16 = 10;
        const TAGS_PER_GOP: usize = 2 * FRAMES_PER_GOP as usize;
        const FRAME_MS: u32 = 100;
        // Output timestamps start at 0, so expectations omit this source offset.
        const SOURCE_START_MS: u32 = 10_000;

        let frame = |timestamp, index: u16| {
            let video = if index.is_multiple_of(FRAMES_PER_GOP) {
                AVC_IDR_16X16
            } else {
                AVC_P_16X16
            };
            let [lo, hi] = index.to_le_bytes();
            [
                tag(timestamp, FlvTagType::Video, video),
                FlvData::Tag(FlvTag::new(
                    timestamp,
                    0,
                    FlvTagType::Audio,
                    false,
                    Bytes::from(vec![0x3f, lo, hi, lo, hi]),
                )),
            ]
        };
        let frames = |start: u32, indexes: std::ops::Range<u16>| {
            indexes
                .flat_map(move |i| frame(start + u32::from(i) * FRAME_MS, i))
                .collect::<Vec<_>>()
        };
        let init = vec![
            FlvData::Header(FlvHeader::new(true, true)),
            tag(0, FlvTagType::Video, AVC_CONFIG_16X16),
        ];
        // Three GOPs, the same three GOPs replayed from timestamp 0, then a
        // fourth GOP of new content.
        let original = 0..3 * FRAMES_PER_GOP;
        let new_content = original.end..original.end + FRAMES_PER_GOP;
        // The source has sent two replayed GOPs. GOP sorting still holds the
        // second until the next keyframe, so the pause falls between the
        // filter receiving replayed GOPs one and two.
        let pause_before = init.len() + 5 * TAGS_PER_GOP;
        let input = [
            init.clone(),
            frames(SOURCE_START_MS, original.clone()),
            frames(0, original),
            frames(SOURCE_START_MS, new_content.clone()),
        ]
        .concat();
        let expected = [init, frames(0, 0..new_content.end)].concat();

        let config = FlvPipelineConfig::builder()
            .duplicate_tag_filtering(true)
            .duplicate_tag_filter_config(DuplicateTagFilterConfig {
                enable_replay_offset_matching: true,
                ..Default::default()
            })
            .build();
        let input = input.into_iter().enumerate().map(|(index, item)| {
            if index == pause_before {
                // Longer than the filter's 500 ms replay gap, measured on the
                // wall clock. The replay must survive because stream time is
                // continuous.
                std::thread::sleep(Duration::from_millis(600));
            }
            item
        });
        assert_eq!(
            run_pipeline(&PipelineConfig::default(), config, input),
            expected
        );
    }

    #[test]
    fn repeated_sequence_headers_preserve_media_timestamps() {
        // A resent header carries timestamp 0 after media has advanced. It
        // must not be treated as a timestamp jump that shifts later frames.
        let frame_ts = |frame: u32| frame * 1_024_000 / 44_100;
        let input = [
            vec![
                FlvData::Header(FlvHeader::new(true, false)),
                create_audio_sequence_header(0, 0x12),
            ],
            (0..10).map(|frame| aac_silence(frame_ts(frame))).collect(),
            vec![create_audio_sequence_header(0, 0x12)],
            (10..12).map(|frame| aac_silence(frame_ts(frame))).collect(),
        ]
        .concat();
        assert_eq!(
            run_pipeline(
                &PipelineConfig::default(),
                FlvPipelineConfig::default(),
                input.clone()
            ),
            input
        );
    }

    #[test]
    fn duration_splits_reinject_av1_mpeg2_configuration() {
        const SPLIT_MS: u32 = 1000;
        let common = PipelineConfig {
            max_duration: Some(Duration::from_millis(SPLIT_MS.into())),
            ..PipelineConfig::default()
        };
        // AV1 sequence start (packet type 5) with the MPEG-2 descriptor wrapper
        // (tag 0x80, length 4) that FFmpeg/libaom-av1 emits before the av1C record.
        let sequence_start = [&b"\x95av01\x80\x04"[..], AV1_CODEC_CONFIG].concat();
        let init = vec![
            FlvData::Header(FlvHeader::new(false, true)),
            FlvData::Tag(FlvTag::new(
                0,
                0,
                FlvTagType::Video,
                false,
                Bytes::from(sequence_start),
            )),
        ];
        let mut input = init.clone();
        let mut expected = init.clone();
        for timestamp in (0..=SPLIT_MS + 200).step_by(100) {
            input.push(tag(timestamp, FlvTagType::Video, AV1_KEYFRAME));
            if timestamp == SPLIT_MS {
                expected.push(FlvData::Split(SplitReason::DurationLimit));
                expected.extend(init.clone());
            }
            // The second file restarts its timeline at the split.
            let output_ts = if timestamp >= SPLIT_MS {
                timestamp - SPLIT_MS
            } else {
                timestamp
            };
            expected.push(tag(output_ts, FlvTagType::Video, AV1_KEYFRAME));
        }
        assert_eq!(
            run_pipeline(&common, FlvPipelineConfig::default(), input),
            expected
        );
    }

    #[test]
    fn repair_strategy_defaults_to_relaxed_and_forwards_overrides() {
        let mut input = vec![
            FlvData::Header(FlvHeader::new(true, false)),
            create_audio_sequence_header(0, 0x12),
        ];
        let before_gap: Vec<u32> = (0..10).map(|frame| frame * AAC_FRAME_MS).collect();
        let last_before_gap = before_gap[9];
        input.extend(before_gap.into_iter().map(aac_silence));
        input.extend([500, 523].map(aac_silence));

        // Relaxed repair keeps the ~300 ms gap; strict repair closes it so the
        // next frame follows the last one by one AAC frame.
        let closed = [1, 2].map(|n| last_before_gap + n * AAC_FRAME_MS);
        let strict = FlvPipelineConfig::builder()
            .repair_strategy(RepairStrategy::Strict)
            .build();
        for (config, tail) in [(FlvPipelineConfig::default(), [500, 523]), (strict, closed)] {
            let mut expected = input[..input.len() - 2].to_vec();
            expected.extend(tail.map(aac_silence));
            assert_eq!(
                run_pipeline(&PipelineConfig::default(), config, input.clone()),
                expected
            );
        }
    }
}
