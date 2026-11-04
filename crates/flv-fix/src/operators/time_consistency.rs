//! # TimeConsistencyOperator
//!
//! The `TimeConsistencyOperator` ensures timestamp continuity in FLV streams
//! that have been split due to parameter changes.
//!
//! ## Purpose
//!
//! When stream splits occur (due to resolution, codec, or other parameter changes),
//! reinjected initialization data can cause timestamp discontinuities. This operator:
//!
//! 1. Detects split points in the stream (header reinjection)
//! 2. Calculates appropriate timestamp offsets for each segment
//! 3. Adjusts timestamps to maintain a continuous timeline
//! 4. Preserves relative timing within each segment
//!
//! ## Operation
//!
//! The operator:
//! - Tracks FLV headers to identify stream split points
//! - Records the last timestamp before a split and first timestamp after
//! - Calculates appropriate offsets to maintain timeline continuity
//! - Applies these offsets to all subsequent tags until the next split
//! - Re-bases the offset when the source restarts its timeline well before the
//!   segment's start, so the output keeps advancing instead of sticking at 0
//!
//! ## Configuration
//!
//! The operator supports two timeline continuity modes:
//! - `Continuous`: Maintains an ever-increasing timeline across all segments
//! - `Reset`: Resets the timeline to zero at each split point
//!
//! ## Usage in Pipeline
//!
//! This operator provides basic timestamp continuity across stream splits, but does not
//! handle more complex timing issues like rebounds, discontinuities, or A/V sync problems.
//! For complete timing correction, use this operator first, followed by `TimingRepairOperator`.
//!
//! Example pipeline: first apply `TimeConsistencyOperator` to handle split points, then apply
//! `TimingRepairOperator` for comprehensive timing fixes.
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use flv::data::FlvData;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// How far before the segment's base a media tag may land and still be clamped
/// to 0. GOP sorting upstream merges audio and video in timestamp order within
/// each group it releases, so ordinary A/V interleaving does not lag the base;
/// only muxing jitter does. A tag further below the base means the source
/// restarted its timeline there; clamping it would pin every following tag to
/// 0 until the source caught up again, so the timeline is re-based instead.
///
/// Media sent before the first keyframe is released as a group of its own, so
/// a first keyframe more than this far behind such tags is also re-based.
const MAX_CLAMPED_LAG_MS: i64 = 100;

/// Frame steps outside this range are not trusted to predict the next frame.
const PLAUSIBLE_STEP_MS: std::ops::RangeInclusive<u32> = 10..=100;

/// Steps assumed when a track has no plausible observed step: one frame at
/// 30 fps, and one 1024-sample AAC frame at 44.1 kHz.
const FALLBACK_VIDEO_STEP_MS: u32 = 33;
const FALLBACK_AUDIO_STEP_MS: u32 = 23;

/// The last emitted media timestamp of one track in the current segment, and
/// the last plausible step observed in this segment.
#[derive(Default)]
struct TrackClock {
    last_timestamp: Option<u32>,
    /// The last step within `PLAUSIBLE_STEP_MS`, or `None` until the segment
    /// has two plausibly spaced frames. A duplicate or metadata packet (0 ms)
    /// or a glitch step does not replace an earlier good measurement.
    last_step: Option<u32>,
}

impl TrackClock {
    fn record(&mut self, timestamp: u32) {
        if let Some(last) = self.last_timestamp {
            let step = timestamp.saturating_sub(last);
            if PLAUSIBLE_STEP_MS.contains(&step) {
                self.last_step = Some(step);
            }
        }
        self.last_timestamp = Some(timestamp);
    }

    /// When this track's next frame is due, if it has emitted one.
    fn next_expected(&self, fallback_step: u32) -> Option<i64> {
        let step = self.last_step.unwrap_or(fallback_step);
        self.last_timestamp
            .map(|last| i64::from(last) + i64::from(step))
    }
}

/// Defines how timestamps should be handled across stream splits
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ContinuityMode {
    /// Maintain a continuous timeline across all segments
    #[default]
    Continuous,

    /// Reset timeline to zero after each split
    Reset,
}

/// State tracking for timestamp correction
struct TimelineState {
    /// Whether we've seen a header and are starting a new segment
    new_segment: bool,

    /// Tracking of segments for debugging
    segment_count: u32,

    /// Last timestamp seen in the previous segment
    last_timestamp: Option<u32>,

    /// First media timestamp of the current segment, read as signed
    first_timestamp_in_segment: Option<i32>,

    /// Current timestamp offset to apply
    timestamp_offset: i64,

    /// Per-track media clocks of the current segment, which decide where a
    /// restarted source timeline resumes.
    audio_clock: TrackClock,
    video_clock: TrackClock,

    /// Whether we need to calculate a new offset
    needs_offset_calculation: bool,
}

impl TimelineState {
    fn new() -> Self {
        Self {
            new_segment: true,
            segment_count: 0,
            last_timestamp: None,
            first_timestamp_in_segment: None,
            timestamp_offset: 0,
            audio_clock: TrackClock::default(),
            video_clock: TrackClock::default(),
            needs_offset_calculation: false,
        }
    }

    fn reset(&mut self) {
        self.segment_count += 1;
        self.new_segment = true;
        self.first_timestamp_in_segment = None;
        self.needs_offset_calculation = true;
        self.timestamp_offset = 0;
        self.audio_clock = TrackClock::default();
        self.video_clock = TrackClock::default();
    }
}

/// Operator that corrects timestamp discontinuities after stream splits
pub struct TimeConsistencyOperator {
    context: Arc<StreamerContext>,
    continuity_mode: ContinuityMode,
    state: TimelineState,
}

impl TimeConsistencyOperator {
    /// Create a new TimeConsistencyOperator
    pub fn new(context: Arc<StreamerContext>, continuity_mode: ContinuityMode) -> Self {
        Self {
            context,
            continuity_mode,
            state: TimelineState::new(),
        }
    }

    /// Shift a signed source timestamp into the output timeline, which never
    /// goes below zero.
    fn apply_offset(timestamp_ms: i32, offset: i64) -> u32 {
        let expected = i64::from(timestamp_ms) + offset;
        u32::try_from(expected.max(0)).unwrap_or(u32::MAX)
    }

    /// Calculate timestamp offset based on continuity mode and current state
    fn calculate_timestamp_offset(&mut self) {
        if let (Some(last), Some(first)) = (
            self.state.last_timestamp,
            self.state.first_timestamp_in_segment,
        ) {
            match self.continuity_mode {
                ContinuityMode::Continuous => {
                    // Make current segment continue from where the previous one ended
                    self.state.timestamp_offset = i64::from(last) - i64::from(first);
                    debug!(
                        "{} Maintaining continuous timeline: offset = {}ms",
                        self.context.name, self.state.timestamp_offset
                    );
                }
                ContinuityMode::Reset => {
                    // Reset timeline - this means applying a negative offset to bring timestamps to zero
                    self.state.timestamp_offset = -i64::from(first);
                    debug!(
                        "{} Resetting timeline to zero: offset = {}ms",
                        self.context.name, self.state.timestamp_offset
                    );
                }
            }
        }
        self.state.needs_offset_calculation = false;
    }
}

impl Processor<FlvData> for TimeConsistencyOperator {
    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        match input {
            FlvData::Header(_) => {
                // Headers indicate stream splits (except the first one)
                if self.state.segment_count > 0 {
                    debug!(
                        "{} Detected stream split, preparing timestamp correction",
                        self.context.name
                    );
                }
                self.state.reset();

                // Forward the header unmodified
                output(input)
            }
            FlvData::Tag(mut tag) => {
                // Source timestamps are signed (SI32): a timeline may start
                // slightly below zero.
                let original_timestamp = tag.signed_timestamp_ms();

                // For normal media tags, handle timestamp adjustment
                if self.state.new_segment {
                    // For sequence headers, always set timestamp to 0
                    if tag.is_video_sequence_header() || tag.is_audio_sequence_header() {
                        if original_timestamp != 0 {
                            debug!(
                                "{} Reset sequence header timestamp from {}ms to 0ms",
                                self.context.name, original_timestamp
                            );
                            // Set timestamp to 0
                            tag.timestamp_ms = 0;
                        }

                        return output(FlvData::Tag(tag));
                    } else if tag.is_script_tag() {
                        // Keep script metadata at timestamp 0 for maximum player compatibility.
                        if original_timestamp != 0 {
                            debug!(
                                "{} Reset script data timestamp: {}ms -> 0ms",
                                self.context.name, original_timestamp
                            );
                        }
                        tag.timestamp_ms = 0;
                        return output(FlvData::Tag(tag));
                    }

                    if self.state.first_timestamp_in_segment.is_none() {
                        // Record the first timestamp in this segment
                        self.state.first_timestamp_in_segment = Some(original_timestamp);
                        debug!(
                            "{} First timestamp in segment {}: {}ms",
                            self.context.name, self.state.segment_count, original_timestamp
                        );

                        if self.state.segment_count > 1 && self.state.needs_offset_calculation {
                            self.calculate_timestamp_offset();
                        } else if self.state.segment_count == 1 {
                            // Reset starts the timeline at the first tag.
                            // Continuous keeps the source timeline, moved up
                            // only as far as needed to start at zero.
                            let to_zero = -i64::from(original_timestamp);
                            self.state.timestamp_offset = match self.continuity_mode {
                                ContinuityMode::Reset => to_zero,
                                ContinuityMode::Continuous => to_zero.max(0),
                            };
                        }
                    }
                    self.state.new_segment = false;
                }

                // Sequence headers and script tags carry no media time, so a
                // stale timestamp on them is clamped rather than moving the
                // timeline.
                let is_media = (tag.is_audio_tag() && !tag.is_audio_sequence_header())
                    || (tag.is_video_tag() && !tag.is_video_sequence_header());
                if is_media
                    && i64::from(original_timestamp) + self.state.timestamp_offset
                        < -MAX_CLAMPED_LAG_MS
                {
                    // The re-basing tag lands where the later of the two
                    // tracks expects its next frame, so it stays on that
                    // track's frame grid. The offset is shared, so both tracks
                    // keep their relative timing from the source; timing
                    // repair downstream corrects a track that still lands
                    // before its own previous tag.
                    let resume_at = self
                        .state
                        .video_clock
                        .next_expected(FALLBACK_VIDEO_STEP_MS)
                        .max(self.state.audio_clock.next_expected(FALLBACK_AUDIO_STEP_MS))
                        .unwrap_or(0);
                    let new_offset = resume_at - i64::from(original_timestamp);
                    warn!(
                        streamer = %self.context.name,
                        segment = self.state.segment_count,
                        source_timestamp_ms = original_timestamp,
                        resume_at_ms = resume_at,
                        old_offset_ms = self.state.timestamp_offset,
                        new_offset_ms = new_offset,
                        "Source timeline restarted before the segment start; re-basing timestamps"
                    );
                    self.state.timestamp_offset = new_offset;
                }

                // Applied even with a zero offset, so a tag below zero is
                // clamped rather than written as a negative timestamp.
                tag.timestamp_ms =
                    Self::apply_offset(original_timestamp, self.state.timestamp_offset);
                if i64::from(tag.timestamp_ms) != i64::from(original_timestamp) {
                    trace!(
                        "{} Adjusted timestamp: {}ms -> {}ms",
                        self.context.name, original_timestamp, tag.timestamp_ms
                    );
                }

                // Remember the last timestamp we've seen
                self.state.last_timestamp = Some(tag.timestamp_ms);
                if is_media {
                    let clock = if tag.is_video_tag() {
                        &mut self.state.video_clock
                    } else {
                        &mut self.state.audio_clock
                    };
                    clock.record(tag.timestamp_ms);
                }

                // Forward the tag with possibly adjusted timestamp
                output(FlvData::Tag(tag))
            }
            // Forward other data types unmodified
            _ => output(input),
        }
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} Time consistency operator completed", self.context.name);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "TimeConsistencyOperator"
    }
}

#[cfg(test)]
mod tests {

    use crate::test_utils::{create_audio_tag, create_test_header, create_video_tag};
    use pipeline_common::{CancellationToken, StreamerContext, init_test_tracing};

    use super::*;

    #[test]
    fn test_normal_flow() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = TimeConsistencyOperator::new(context.clone(), ContinuityMode::Reset);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header followed by some tags
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(0, true), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(10), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(20, false), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(30), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&context, &mut output_fn).unwrap();

        // Validate tags have correct timestamps
        assert_eq!(output_items.len(), 5);

        // Extract tags and verify timestamps
        let timestamps: Vec<u32> = output_items
            .iter()
            .filter_map(|item| {
                if let FlvData::Tag(tag) = item {
                    Some(tag.timestamp_ms)
                } else {
                    None
                }
            })
            .collect();

        // Original timestamps should be preserved in normal flow
        assert_eq!(timestamps, vec![0, 10, 20, 30]);
    }

    #[test]
    fn test_decreasing_timestamp_handling() {
        init_test_tracing!();
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = TimeConsistencyOperator::new(context.clone(), ContinuityMode::Reset);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process tags with non-monotonic timestamps (decreasing)
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(1000, true), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(1010), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(1020, false), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(990), &mut output_fn)
            .unwrap(); // Decreasing timestamp
        operator
            .process(&context, create_video_tag(1030, false), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&context, &mut output_fn).unwrap();

        // Extract tags and verify timestamps
        let timestamps: Vec<u32> = output_items
            .iter()
            .filter_map(|item| {
                if let FlvData::Tag(tag) = item {
                    Some(tag.timestamp_ms)
                } else {
                    None
                }
            })
            .collect();

        // In Reset mode, first timestamp (1000) is used to calculate offset (-1000)
        // So input timestamps [1000, 1010, 1020, 990, 1030] should become
        // output timestamps [0, 10, 20, 0, 30]

        // Verify all timestamps individually
        assert_eq!(timestamps[0], 0, "First timestamp should be reset to 0");
        assert_eq!(timestamps[1], 10, "Second timestamp should be 10");
        assert_eq!(timestamps[2], 20, "Third timestamp should be 20");

        // The decreasing timestamp (990) should be adjusted to 0 since:
        // 990 + (-1000) = -10, which gets clamped to 0 by the max(0, ...) call
        assert_eq!(
            timestamps[3], 0,
            "Decreasing timestamp should be adjusted to 0 after applying offset"
        );

        assert_eq!(timestamps[4], 30, "Last timestamp should be 30");

        // Complete expected sequence
        assert_eq!(
            timestamps,
            vec![0, 10, 20, 0, 30],
            "The sequence of timestamps should match expected values"
        );
    }

    #[test]
    fn test_multiple_splits() {
        init_test_tracing!();
        let context = StreamerContext::arc_new(CancellationToken::new());

        // Test with Continuous mode
        {
            let mut operator =
                TimeConsistencyOperator::new(context.clone(), ContinuityMode::Continuous);
            let mut output_items = Vec::new();

            let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
                output_items.push(item);
                Ok(())
            };

            // First segment: starts at 1000
            operator
                .process(&context, create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(1000, true), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_audio_tag(1010), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(1020, false), &mut output_fn)
                .unwrap();

            // Second segment: starts at 500
            operator
                .process(&context, create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(500, true), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_audio_tag(510), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(520, false), &mut output_fn)
                .unwrap();

            // Third segment: starts at 200
            operator
                .process(&context, create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(200, true), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_audio_tag(210), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(220, false), &mut output_fn)
                .unwrap();

            // Finish processing
            operator.finish(&context, &mut output_fn).unwrap();

            // Extract tags and verify timestamps
            let timestamps: Vec<u32> = output_items
                .iter()
                .filter_map(|item| {
                    if let FlvData::Tag(tag) = item {
                        Some(tag.timestamp_ms)
                    } else {
                        None
                    }
                })
                .collect();

            // With Continuous mode:
            // 1. First segment: [1000, 1010, 1020]
            // 2. Second segment: offset = 1020-500 = 520, so [500+520, 510+520, 520+520] = [1020, 1030, 1040]
            // 3. Third segment: offset = 1040-200 = 840, so [200+840, 210+840, 220+840] = [1040, 1050, 1060]
            assert_eq!(
                timestamps,
                vec![1000, 1010, 1020, 1020, 1030, 1040, 1040, 1050, 1060],
                "Timestamps should maintain continuous sequence across multiple splits"
            );
        }

        // Test with Reset mode
        {
            let mut operator = TimeConsistencyOperator::new(context.clone(), ContinuityMode::Reset);
            let mut output_items = Vec::new();

            let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
                output_items.push(item);
                Ok(())
            };

            // First segment: starts at 1000
            operator
                .process(&context, create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(1000, true), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_audio_tag(1010), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(1020, false), &mut output_fn)
                .unwrap();

            // Second segment: starts at 500
            operator
                .process(&context, create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(500, true), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_audio_tag(510), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(520, false), &mut output_fn)
                .unwrap();

            // Third segment: starts at 200
            operator
                .process(&context, create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(200, true), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_audio_tag(210), &mut output_fn)
                .unwrap();
            operator
                .process(&context, create_video_tag(220, false), &mut output_fn)
                .unwrap();

            // Finish processing
            operator.finish(&context, &mut output_fn).unwrap();

            // Extract tags and verify timestamps
            let timestamps: Vec<u32> = output_items
                .iter()
                .filter_map(|item| {
                    if let FlvData::Tag(tag) = item {
                        Some(tag.timestamp_ms)
                    } else {
                        None
                    }
                })
                .collect();

            // With Reset mode:
            // 1. First segment: offset = -1000, so [0, 10, 20]
            // 2. Second segment: offset = -500, so [0, 10, 20]
            // 3. Third segment: offset = -200, so [0, 10, 20]
            assert_eq!(
                timestamps,
                vec![0, 10, 20, 0, 10, 20, 0, 10, 20],
                "Timestamps should reset to zero at the beginning of each segment"
            );
        }
    }
}
