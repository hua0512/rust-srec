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
//! Example pipeline:
//! ```
//! // 1. First apply TimeConsistencyOperator to handle split points
//! // 2. Then apply TimingRepairOperator for comprehensive timing fixes
//! ```
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
use flv::tag::FlvUtil;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::cmp::max;
use std::sync::Arc;
use tracing::{debug, trace};

/// Defines how timestamps should be handled across stream splits
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContinuityMode {
    /// Maintain a continuous timeline across all segments
    Continuous,

    /// Reset timeline to zero after each split
    Reset,
}

impl Default for ContinuityMode {
    fn default() -> Self {
        Self::Continuous
    }
}

/// State tracking for timestamp correction
struct TimelineState {
    /// Whether we've seen a header and are starting a new segment
    new_segment: bool,

    /// Tracking of segments for debugging
    segment_count: u32,

    /// Last timestamp seen in the previous segment
    last_timestamp: Option<u32>,

    /// First timestamp seen in the current segment
    first_timestamp_in_segment: Option<u32>,

    /// Current timestamp offset to apply
    timestamp_offset: i64,

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
            needs_offset_calculation: false,
        }
    }

    fn reset(&mut self) {
        self.segment_count += 1;
        self.new_segment = true;
        self.first_timestamp_in_segment = None;
        self.needs_offset_calculation = true;
        self.timestamp_offset = 0
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

    /// Calculate timestamp offset based on continuity mode and current state
    fn calculate_timestamp_offset(&mut self) {
        if let (Some(last), Some(first)) = (
            self.state.last_timestamp,
            self.state.first_timestamp_in_segment,
        ) {
            match self.continuity_mode {
                ContinuityMode::Continuous => {
                    // Make current segment continue from where the previous one ended
                    self.state.timestamp_offset = last as i64 - first as i64;
                    debug!(
                        "{} Maintaining continuous timeline: offset = {}ms",
                        self.context.name, self.state.timestamp_offset
                    );
                }
                ContinuityMode::Reset => {
                    // Reset timeline - this means applying a negative offset to bring timestamps to zero
                    self.state.timestamp_offset = -(first as i64);
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
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
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
                let original_timestamp = tag.timestamp_ms;

                // For normal media tags, handle timestamp adjustment
                if self.state.new_segment {
                    // For sequence headers, always set timestamp to 0
                    if tag.is_video_sequence_header() || tag.is_audio_sequence_header() {
                        // Save original timestamp for debugging
                        let original = tag.timestamp_ms;
                        if original != 0 {
                            debug!(
                                "{} Reset sequence header timestamp from {}ms to 0ms",
                                self.context.name, original
                            );
                            // Set timestamp to 0
                            tag.timestamp_ms = 0;
                        }

                        return output(FlvData::Tag(tag));
                    } else if tag.is_script_tag() {
                        // apply delta to script data tags
                        if self.state.timestamp_offset != 0 {
                            tag.timestamp_ms = 0;

                            debug!(
                                "{} Adjusted script data timestamp: {}ms -> {}ms",
                                self.context.name, original_timestamp, 0
                            );
                        }
                        return output(FlvData::Tag(tag));
                    }

                    if self.state.first_timestamp_in_segment.is_none() {
                        // Record the first timestamp in this segment
                        self.state.first_timestamp_in_segment = Some(tag.timestamp_ms);
                        debug!(
                            "{} First timestamp in segment {}: {}ms",
                            self.context.name, self.state.segment_count, tag.timestamp_ms
                        );

                        if self.state.segment_count > 1 && self.state.needs_offset_calculation {
                            self.calculate_timestamp_offset();
                        } else if self.state.segment_count == 1
                            && self.continuity_mode == ContinuityMode::Reset
                        {
                            // use the first timestamp as the delta
                            self.state.timestamp_offset =
                                -(self.state.first_timestamp_in_segment.unwrap() as i64);
                        }
                    }
                    self.state.new_segment = false;
                }

                // Apply timestamp correction if needed
                if self.state.timestamp_offset != 0 {
                    // Calculate the corrected timestamp, ensure it doesn't go negative
                    let expected = tag.timestamp_ms as i64 + self.state.timestamp_offset;
                    let corrected = max(0, expected) as u32;
                    tag.timestamp_ms = corrected;

                    trace!(
                        "{} Adjusted timestamp: {}ms -> {}ms",
                        self.context.name, original_timestamp, corrected
                    );
                }

                // Remember the last timestamp we've seen
                self.state.last_timestamp = Some(tag.timestamp_ms);

                // Forward the tag with possibly adjusted timestamp
                output(FlvData::Tag(tag))
            }
            // Forward other data types unmodified
            _ => output(input),
        }
    }

    fn finish(
        &mut self,
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
    use pipeline_common::{create_test_context, init_test_tracing};

    use super::*;

    #[test]
    fn test_normal_flow() {
        let context = create_test_context();
        let mut operator = TimeConsistencyOperator::new(context, ContinuityMode::Reset);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header followed by some tags
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(0, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(10), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(20, false), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(30), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

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
    fn test_timestamp_reset() {
        let context = create_test_context();
        let mut operator = TimeConsistencyOperator::new(context, ContinuityMode::Reset);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header followed by some tags with increasing timestamps
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(1000, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(1010), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(1020, false), &mut output_fn)
            .unwrap();

        // Send another header (should reset timebase)
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(500, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(510), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(520, false), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

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

        // With Reset mode, second segment should start from its own timestamp without adjustment
        assert_eq!(timestamps, vec![0, 10, 20, 0, 10, 20]);
    }

    #[test]
    fn test_timestamp_continue_mode() {
        let context = create_test_context();
        let mut operator = TimeConsistencyOperator::new(context, ContinuityMode::Continuous);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header followed by some tags with increasing timestamps
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(1000, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(1010), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(1020, false), &mut output_fn)
            .unwrap();

        // Send another header (should continue timing)
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(500, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(510), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(520, false), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

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

        // With Continue mode, second segment should continue from last segment's max timestamp
        // Last timestamp of first segment = 1020
        // First timestamp of second segment = 500
        // Offset = 1020 - 500 = 520
        // Applied to timestamps: 500+520=1020, 510+520=1030, 520+520=1040
        assert_eq!(timestamps, vec![1000, 1010, 1020, 1020, 1030, 1040]);
    }

    #[test]
    fn test_decreasing_timestamp_handling() {
        init_test_tracing!();
        let context = create_test_context();
        let mut operator = TimeConsistencyOperator::new(context, ContinuityMode::Reset);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process tags with non-monotonic timestamps (decreasing)
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(1000, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(1010), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(1020, false), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(990), &mut output_fn)
            .unwrap(); // Decreasing timestamp
        operator
            .process(create_video_tag(1030, false), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

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
        let context = create_test_context();

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
                .process(create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(1000, true), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(1010), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(1020, false), &mut output_fn)
                .unwrap();

            // Second segment: starts at 500
            operator
                .process(create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(500, true), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(510), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(520, false), &mut output_fn)
                .unwrap();

            // Third segment: starts at 200
            operator
                .process(create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(200, true), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(210), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(220, false), &mut output_fn)
                .unwrap();

            // Finish processing
            operator.finish(&mut output_fn).unwrap();

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
                .process(create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(1000, true), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(1010), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(1020, false), &mut output_fn)
                .unwrap();

            // Second segment: starts at 500
            operator
                .process(create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(500, true), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(510), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(520, false), &mut output_fn)
                .unwrap();

            // Third segment: starts at 200
            operator
                .process(create_test_header(), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(200, true), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(210), &mut output_fn)
                .unwrap();
            operator
                .process(create_video_tag(220, false), &mut output_fn)
                .unwrap();

            // Finish processing
            operator.finish(&mut output_fn).unwrap();

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
