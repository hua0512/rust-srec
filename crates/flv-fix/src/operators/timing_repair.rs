//! # TimingRepairOperator
//!
//! The `TimingRepairOperator` provides advanced timestamp correction for FLV streams.
//! It solves numerous timing issues that can affect playback quality, seeking, and player compatibility.
//!
//! ## Purpose
//!
//! Even after basic timestamp continuity is established, FLV streams can suffer from various
//! timing problems that affect playback. This operator provides comprehensive fixes for:
//!
//! 1. Timestamp rebounds (sudden backwards jumps in time)
//! 2. Timestamp discontinuities (unreasonable gaps)
//! 3. Inconsistent frame intervals (jittery playback)
//! 4. Audio/video synchronization issues (drift between streams)
//! 5. Metadata timing issues (incorrect durations/timestamps)
//! 6. Stream concatenation artifacts (glitches at boundaries)
//! 7. Player compatibility issues (timing patterns that break specific players)
//! 8. Recording and seeking problems (improper indexing due to bad timing)
//!
//! ## Operation
//!
//! The operator:
//! - Tracks audio and video streams separately
//! - Extracts frame rate and audio sample rate from metadata
//! - Calculates expected intervals between frames and audio samples
//! - Detects timestamps that violate expected patterns
//! - Applies carefully calculated corrections to maintain proper timing
//! - Maintains A/V sync while correcting individual streams
//!
//! ## Configuration
//!
//! The operator supports various configuration options:
//! - Repair strategies (strict vs. relaxed)
//! - Default frame rates when metadata is missing
//! - Tolerance thresholds for discontinuity detection
//! - Debug modes for timing issue diagnosis
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use amf0::Amf0Value;
use flv::data::FlvData;
use flv::script::ScriptData;
use flv::tag::{FlvTag, FlvTagType, FlvUtil};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::cmp::max;
use std::collections::HashMap;
use std::f64;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};

/// The tolerance for timestamp correction due to floating point conversion errors
/// Since millisecond precision (1/1000 of a second) can't exactly represent many common frame rates
/// (e.g., 30fps = 33.33ms), we allow +/- 1ms tolerance to account for rounding
const TOLERANCE: u32 = 1;

/// Defines the strategy for timestamp repair
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RepairStrategy {
    /// Strict mode enforces exact frame intervals and corrects any deviation
    Strict,

    /// Relaxed mode only fixes severe timing issues, allowing minor variations
    Relaxed,
}

impl Default for RepairStrategy {
    fn default() -> Self {
        Self::Relaxed
    }
}

/// Configuration options for the TimingRepairOperator
#[derive(Debug, Clone)]
pub struct TimingRepairConfig {
    /// Strategy to use for repairing timestamps
    pub strategy: RepairStrategy,

    /// Default video frame rate if not specified in metadata (fps)
    pub default_frame_rate: f64,

    /// Default audio sample rate if not specified in metadata (Hz)
    pub default_audio_rate: f64,

    /// Maximum allowed discontinuity in timestamps (ms)
    pub max_discontinuity: u32,
}

impl Default for TimingRepairConfig {
    fn default() -> Self {
        Self {
            strategy: RepairStrategy::default(),
            default_frame_rate: 30.0,
            default_audio_rate: 44100.0,
            max_discontinuity: 1000, // 1 second
        }
    }
}

/// Stream timing state for the repair operator
struct TimingState {
    /// Current accumulated offset to apply to timestamps
    delta: i64,

    /// Last tag processed (any type)
    last_tag: Option<FlvTag>,

    /// Last audio tag processed
    last_audio_tag: Option<FlvTag>,

    /// Last video tag processed
    last_video_tag: Option<FlvTag>,

    /// Video frame rate in frames per second
    frame_rate: f64,

    /// Audio sample rate in Hz
    audio_rate: f64,

    /// Expected interval between video frames in ms
    video_frame_interval: u32,

    /// Expected interval between audio samples in ms
    audio_sample_interval: u32,

    /// Count of corrections applied (for statistics)
    correction_count: u32,

    /// Number of timestamp rebounds detected
    rebound_count: u32,

    /// Number of discontinuities detected
    discontinuity_count: u32,

    /// Number of tags processed
    tag_count: u32,
}

impl TimingState {
    fn new(config: &TimingRepairConfig) -> Self {
        let video_frame_interval = Self::calculate_video_frame_interval(config.default_frame_rate);
        let audio_sample_interval =
            Self::calculate_audio_sample_interval(config.default_audio_rate);

        Self {
            delta: 0,
            last_tag: None,
            last_audio_tag: None,
            last_video_tag: None,
            frame_rate: config.default_frame_rate,
            audio_rate: config.default_audio_rate,
            video_frame_interval,
            audio_sample_interval,
            correction_count: 0,
            rebound_count: 0,
            discontinuity_count: 0,
            tag_count: 0,
        }
    }

    /// Reset the timing state
    fn reset(&mut self, config: &TimingRepairConfig) {
        self.delta = 0;
        self.last_tag = None;
        self.last_audio_tag = None;
        self.last_video_tag = None;
        self.frame_rate = config.default_frame_rate;
        self.video_frame_interval = Self::calculate_video_frame_interval(config.default_frame_rate);
        self.audio_rate = config.default_audio_rate;
        self.audio_sample_interval =
            Self::calculate_audio_sample_interval(config.default_audio_rate);
    }

    /// Calculate the video frame interval in milliseconds based on frame rate
    fn calculate_video_frame_interval(fps: f64) -> u32 {
        if fps <= 0.0 {
            return 33; // Default ~30fps
        }
        // Round to nearest millisecond
        f64::ceil(1000.0 / fps) as u32
    }

    /// Calculate the audio sample interval in milliseconds based on sample rate
    fn calculate_audio_sample_interval(rate: f64) -> u32 {
        if rate <= 0.0 {
            return 23; // Default for 44.1kHz
        }
        f64::ceil(1000.0 / (rate / 1000.0)) as u32
    }

    /// Update timing parameters based on metadata
    fn update_timing_params(&mut self, properties: &HashMap<String, Amf0Value>) {
        // Extract frame rate
        let fps = properties
            .get("fps")
            .or_else(|| properties.get("framerate"));

        if let Some(fps_value) = fps {
            match fps_value {
                Amf0Value::Number(value) => {
                    if *value > 0.0 {
                        self.frame_rate = *value;
                        self.video_frame_interval = Self::calculate_video_frame_interval(*value);
                    }
                }
                Amf0Value::String(value) => {
                    if let Ok(fps_float) = value.parse::<f64>() {
                        if fps_float > 0.0 {
                            self.frame_rate = fps_float;
                            self.video_frame_interval =
                                Self::calculate_video_frame_interval(fps_float);
                        }
                    }
                }
                _ => {}
            }
        }

        // Extract audio sample rate
        let audio_rate = properties.get("audiosamplerate");
        if let Some(Amf0Value::Number(rate)) = audio_rate {
            if *rate > 0.0 {
                self.audio_rate = *rate;
                // Convert from Hz to kHz for interval calculation
                self.audio_sample_interval = Self::calculate_audio_sample_interval(*rate);
            }
        }
    }

    /// Check if a timestamp has rebounded (gone backward in time)
    fn is_timestamp_rebounded(&self, tag: &FlvTag) -> bool {
        let current = tag.timestamp_ms;
        // Handle potential overflow when adding delta to current timestamp
        let expected = if self.delta >= 0 {
            current.saturating_add(self.delta as u32)
        } else {
            current.saturating_sub((-self.delta) as u32)
        };

        if tag.is_audio_tag() {
            if let Some(ref last) = self.last_audio_tag {
                if last.is_audio_sequence_header() {
                    expected < last.timestamp_ms
                } else {
                    let min_expected = last.timestamp_ms;

                    expected <= min_expected
                }
            } else {
                false
            }
        } else if tag.is_video_tag() {
            if let Some(ref last) = self.last_video_tag {
                if last.is_video_sequence_header() {
                    expected < last.timestamp_ms
                } else {
                    let min_expected = last.timestamp_ms;

                    expected <= min_expected
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Check if there's a discontinuity in timestamps
    fn is_timestamp_discontinuous(&self, tag: &FlvTag, config: &TimingRepairConfig) -> bool {
        if self.last_tag.is_none() {
            return false;
        }

        let last = self.last_tag.as_ref().unwrap();
        let current = tag.timestamp_ms;

        let expected = if self.delta >= 0 {
            current.saturating_add(self.delta as u32)
        } else {
            current.saturating_sub((-self.delta) as u32)
        };

        // Calculate the difference between expected and last timestamp
        // Convert to i64 before subtraction to avoid overflow
        let diff: i64 = (expected as i64) - (last.timestamp_ms as i64);

        // Determine threshold based on media type, considering rounding errors
        let base_threshold = match tag.tag_type {
            FlvTagType::Video => self.video_frame_interval,
            FlvTagType::Audio => self.audio_sample_interval,
            _ => max(self.video_frame_interval, self.audio_sample_interval),
        };

        // Add tolerance to account for rounding errors
        let threshold = base_threshold + TOLERANCE;

        // For strict mode, check if difference is significantly different from expected interval
        match config.strategy {
            RepairStrategy::Strict => {
                // In strict mode, we check if the timestamp differs from what we'd expect
                if tag.is_video_tag() && self.last_video_tag.is_some() {
                    diff < 0 || diff > threshold.into()
                } else {
                    // For non-video tags or when no video history exists
                    diff > threshold as i64 * 2 // More lenient for non-video
                }
            }
            RepairStrategy::Relaxed => diff > config.max_discontinuity as i64,
        }
    }

    /// Calculate a new delta correction when a problem is detected
    fn calculate_delta_correction(&mut self, tag: &FlvTag) -> i64 {
        let current = tag.timestamp_ms;
        let mut new_delta = self.delta;
        let last_ts = self.last_tag.as_ref().map(|t| t.timestamp_ms).unwrap_or(0);

        if tag.is_video_tag() && self.last_video_tag.is_some() {
            let last_video = self.last_video_tag.as_ref().unwrap();

            // Calculate ideal next frame timestamp
            let ideal_next_ts = last_video.timestamp_ms + self.video_frame_interval;

            new_delta = ideal_next_ts as i64 - current as i64;
        } else if tag.is_audio_tag() && self.last_audio_tag.is_some() {
            let last_audio = self.last_audio_tag.as_ref().unwrap();
            let ideal_next_ts = last_audio.timestamp_ms + self.audio_sample_interval;
            new_delta = ideal_next_ts as i64 - current as i64;
        } else if let Some(last) = &self.last_tag {
            // No type-specific last tag, use generic last tag
            let interval = max(self.video_frame_interval, self.audio_sample_interval);
            new_delta = (last.timestamp_ms + interval) as i64 - current as i64;
        }

        let expected = if new_delta >= 0 {
            current.saturating_add(new_delta as u32)
        } else {
            current.saturating_sub((-new_delta) as u32)
        };

        if last_ts != 0 && expected <= last_ts {
            // If the expected timestamp is still less than the last one, we need to adjust
            // to avoid negative timestamps or rebounding
            // in those cases, we use the last timestamp as a reference to calculate the delta
            if tag.is_video_tag() {
                let adjusted_ts = last_ts + self.video_frame_interval;
                new_delta = if adjusted_ts >= current {
                    (adjusted_ts - current) as i64
                } else {
                    -((current - adjusted_ts) as i64)
                };
            } else if tag.is_audio_tag() {
                // calculate ideal next audio timestamp
                let adjusted_ts = last_ts + self.audio_sample_interval;
                new_delta = if adjusted_ts >= current {
                    (adjusted_ts - current) as i64
                } else {
                    -((current - adjusted_ts) as i64)
                };
            }
        }

        new_delta
    }

    fn update_last_tags(&mut self, tag: &FlvTag) {
        let tag_clone = tag.clone();
        self.last_tag = Some(tag.clone());
        if tag.is_audio_tag() {
            self.last_audio_tag = Some(tag_clone);
        } else if tag.is_video_tag() {
            self.last_video_tag = Some(tag_clone);
        }
    }
}

/// Operator for comprehensive timing repair in FLV streams
pub struct TimingRepairOperator {
    context: Arc<StreamerContext>,
    config: TimingRepairConfig,
    state: TimingState,
}

impl TimingRepairOperator {
    /// Create a new TimingRepairOperator with the specified configuration
    pub fn new(context: Arc<StreamerContext>, config: TimingRepairConfig) -> Self {
        Self {
            context,
            state: TimingState::new(&config),
            config,
        }
    }

    /// Create a new TimingRepairOperator with default configuration
    pub fn with_strategy(context: Arc<StreamerContext>, strategy: RepairStrategy) -> Self {
        let config = TimingRepairConfig {
            strategy,
            ..Default::default()
        };
        Self {
            context,
            state: TimingState::new(&config),
            config,
        }
    }

    /// Handle script tag (metadata), update timing params, and forward the tag.
    fn handle_script_tag(
        &mut self,
        tag: &mut FlvTag,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let mut cursor = std::io::Cursor::new(tag.data.clone());
        if let Ok(amf_data) = ScriptData::demux(&mut cursor) {
            if amf_data.name == crate::AMF0_ON_METADATA && !amf_data.data.is_empty() {
                match &amf_data.data[0] {
                    Amf0Value::Object(props) => {
                        let properties = props
                            .iter()
                            .map(|(k, v)| (k.as_ref().to_owned(), v.clone()))
                            .collect::<HashMap<String, Amf0Value>>();
                        self.state.update_timing_params(&properties);

                        debug!(
                            "{} TimingRepair: Updated timing params - video interval: {}ms, audio interval: {}ms",
                            self.context.name,
                            self.state.video_frame_interval,
                            self.state.audio_sample_interval
                        );
                    }
                    Amf0Value::StrictArray(items) => {
                        debug!(
                            "{} TimingRepair: Received metadata as StrictArray with {} items",
                            self.context.name,
                            items.len()
                        );

                        let mut framerate_value: Option<f64> = None;
                        let mut audio_rate_value: Option<f64> = None;
                        for item in items.iter() {
                            if let Amf0Value::Number(value) = item {
                                if *value > 10.0 && *value < 500.0 && framerate_value.is_none() {
                                    framerate_value = Some(*value);

                                    debug!(
                                        "{} TimingRepair: Found potential framerate in StrictArray: {}",
                                        self.context.name, *value
                                    );
                                } else if (*value == 44100.0
                                    || *value == 48000.0
                                    || *value == 22050.0
                                    || *value == 11025.0
                                    || *value == 88200.0
                                    || *value == 96000.0)
                                    && audio_rate_value.is_none()
                                {
                                    audio_rate_value = Some(*value);

                                    debug!(
                                        "{} TimingRepair: Found potential audio sample rate in StrictArray: {}",
                                        self.context.name, *value
                                    );
                                }
                            }
                        }
                        if framerate_value.is_some() || audio_rate_value.is_some() {
                            let mut properties = HashMap::new();
                            if let Some(fps) = framerate_value {
                                properties.insert(crate::METADATA_FRAMERATE.to_owned(), Amf0Value::Number(fps));
                            }
                            if let Some(rate) = audio_rate_value {
                                properties
                                    .insert(crate::METADATA_AUDIOSAMPLERATE.to_owned(), Amf0Value::Number(rate));
                            }
                            self.state.update_timing_params(&properties);

                            debug!(
                                "{} TimingRepair: Updated timing params from StrictArray - video interval: {}ms, audio interval: {}ms",
                                self.context.name,
                                self.state.video_frame_interval,
                                self.state.audio_sample_interval
                            );
                        }
                    }
                    _ => {
                        error!(
                            "{} TimingRepair: Metadata format not supported: {:?}",
                            self.context.name,
                            amf_data.data[0].marker()
                        );
                    }
                }
            }
        }
        // Forward script tag
        output(FlvData::Tag(tag.clone()))
    }
}

impl Processor<FlvData> for TimingRepairOperator {
    /// Process method that receives FLV data, corrects timing issues, and forwards the data
    fn process(
        &mut self,
        mut input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match &mut input {
            FlvData::Header(_) => {
                // Reset state when encountering a header
                self.state.reset(&self.config);

                debug!("{} TimingRepair: Processing new segment", self.context.name);

                // Forward the header unmodified
                output(input)
            }
            FlvData::Tag(tag) => {
                self.state.tag_count += 1;
                let original_timestamp = tag.timestamp_ms;

                // Handle script tags (metadata)
                if tag.is_script_tag() {
                    return self.handle_script_tag(tag, output);
                }

                // Check for timestamp issues
                let mut need_correction = false;

                // Check for timestamp rebounds
                if self.state.is_timestamp_rebounded(tag) {
                    self.state.rebound_count += 1;
                    let new_delta = self.state.calculate_delta_correction(tag);

                    warn!(
                        "{} TimingRepair: Timestamp rebound detected: {}ms, last ts: {}ms,  would go back in time - applying correction delta: {}ms",
                        self.context.name,
                        tag.timestamp_ms,
                        self.state.last_tag.as_ref().map_or(0, |t| t.timestamp_ms),
                        new_delta
                    );

                    self.state.delta = new_delta;
                    need_correction = true;
                }
                // Check for discontinuities
                else if self.state.is_timestamp_discontinuous(tag, &self.config) {
                    self.state.discontinuity_count += 1;
                    let new_delta = self.state.calculate_delta_correction(tag);

                    warn!(
                        "{} TimingRepair: Timestamp discontinuity detected: {}ms, last ts: {}ms, applying correction delta: {}ms",
                        self.context.name,
                        tag.timestamp_ms,
                        self.state.last_tag.as_ref().map_or(0, |t| t.timestamp_ms),
                        new_delta
                    );

                    self.state.delta = new_delta;
                    need_correction = true;
                }

                // Apply correction if needed
                if self.state.delta != 0 || need_correction {
                    let corrected_timestamp = if tag.timestamp_ms as i64 + self.state.delta < 0 {
                        warn!(
                            "{} TimingRepair: Negative timestamp detected, applying frame-rate aware correction",
                            self.context.name
                        );
                        if let Some(last) = &self.state.last_tag {
                            if tag.is_video_tag() {
                                last.timestamp_ms + self.state.video_frame_interval
                            } else if tag.is_audio_tag() {
                                last.timestamp_ms + self.state.audio_sample_interval
                            } else {
                                last.timestamp_ms
                                    + max(
                                        self.state.video_frame_interval,
                                        self.state.audio_sample_interval,
                                    )
                            }
                        } else {
                            0
                        }
                    } else {
                        (tag.timestamp_ms as i64 + self.state.delta) as u32
                    };

                    tag.timestamp_ms = corrected_timestamp;
                    self.state.correction_count += 1;

                    trace!(
                        "{} TimingRepair: Corrected timestamp: {}ms -> {}ms (delta: {}ms)",
                        self.context.name,
                        original_timestamp,
                        corrected_timestamp,
                        self.state.delta
                    );
                }

                // Update state with this tag
                self.state.update_last_tags(tag);

                // Forward the tag with corrected timestamp
                output(input)
            }
            // Forward other data types unmodified
            _ => output(input),
        }
    }

    fn finish(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let _ = output;
        // Finalize processing and log statistics
        info!(
            "{} TimingRepair complete: Processed {} tags, applied {} corrections, detected {} rebounds and {} discontinuities",
            self.context.name,
            self.state.tag_count,
            self.state.correction_count,
            self.state.rebound_count,
            self.state.discontinuity_count
        );
        Ok(())
    }

    fn name(&self) -> &'static str {
        "TimingRepairOperator"
    }
}

#[cfg(test)]
mod tests {
    use pipeline_common::create_test_context;

    use super::*;
    use crate::test_utils::{
        create_audio_sequence_header, create_audio_tag, create_test_header,
        create_video_sequence_header, create_video_tag, print_tags,
    };

    fn process_tags_through_operator(
        config: TimingRepairConfig,
        input_tags: Vec<FlvData>,
    ) -> Vec<FlvData> {
        let context = create_test_context();
        let mut operator = TimingRepairOperator::new(context, config);

        // Collect results in a vector
        let mut results = Vec::new();

        // Process each tag through the operator with a closure to collect the output
        for tag in input_tags {
            let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
                results.push(item);
                Ok(())
            };

            // Process the tag
            operator.process(tag, &mut output_fn).unwrap();
        }

        // Finish processing
        let mut finish_output = |item: FlvData| -> Result<(), PipelineError> {
            results.push(item);
            Ok(())
        };
        operator.finish(&mut finish_output).unwrap();

        results
    }

    #[test]
    fn test_timestamp_rebound_correction() {
        // Create input tags
        let mut input_tags = Vec::new();
        input_tags.push(create_test_header());
        input_tags.push(create_video_sequence_header(1));
        input_tags.push(create_audio_sequence_header(1));

        // Regular tags with increasing timestamps
        for i in 1..5 {
            input_tags.push(create_video_tag(i * 33, true));
            input_tags.push(create_audio_tag(i * 33 + 5));
        }

        // Tag with timestamp rebound (going backwards)
        input_tags.push(create_video_tag(50, true));

        // More regular tags
        for i in 6..10 {
            input_tags.push(create_video_tag(i * 33, true));
            input_tags.push(create_audio_tag(i * 33 + 5));
        }

        let config = TimingRepairConfig {
            strategy: RepairStrategy::Strict,
            ..Default::default()
        };

        let results = process_tags_through_operator(config, input_tags);

        // Print results for analysis
        print_tags(&results);

        // The rebounded timestamp should be corrected to maintain forward progress
        // Find the tag at the rebound point and check if it has been properly corrected

        // Find where the video sequence tag is (should be tag 1)
        // Find where the rebounded tag is (should be tag 11)
        // Check that its timestamp was corrected to be after the previous video tag

        if let FlvData::Tag(tag1) = &results[10] {
            if let FlvData::Tag(tag2) = &results[11] {
                if tag2.tag_type == FlvTagType::Video {
                    assert!(
                        tag2.timestamp_ms > tag1.timestamp_ms,
                        "Rebounded timestamp should be corrected to maintain forward progress"
                    );
                }
            }
        }
    }

    #[test]
    fn test_timestamp_discontinuity_correction() {
        // Create input tags
        let mut input_tags = Vec::new();
        input_tags.push(create_test_header());

        // Regular tags with increasing timestamps
        for i in 1..5 {
            input_tags.push(create_video_tag(i * 33, true));
        }

        // Tag with large timestamp jump (discontinuity)
        input_tags.push(create_video_tag(5000, true));

        // More regular tags after the jump
        for i in 151..155 {
            input_tags.push(create_video_tag(i * 33, true));
        }

        let config = TimingRepairConfig {
            strategy: RepairStrategy::Strict,
            ..Default::default()
        };

        let results = process_tags_through_operator(config, input_tags);

        // Print results for analysis
        print_tags(&results);

        // The discontinuity should be smoothed out
        // The tag after the jump should have a reasonable timestamp increase from the previous tag
        if let FlvData::Tag(tag1) = &results[4] {
            if let FlvData::Tag(tag2) = &results[5] {
                let diff = tag2.timestamp_ms - tag1.timestamp_ms;
                assert!(
                    diff < 1000,
                    "Discontinuity should be corrected to a reasonable interval"
                );
            }
        }
    }

    #[test]
    fn test_av_sync_maintenance() {
        // Create input tags
        let mut input_tags = Vec::new();
        input_tags.push(create_test_header());

        // Interleaved audio and video with audio slightly ahead
        for i in 1..10 {
            input_tags.push(create_video_tag(i * 33, true));
            input_tags.push(create_audio_tag(i * 33 + 5));
        }

        // Audio with major sync issue (far ahead of video)
        input_tags.push(create_audio_tag(800));

        // Continue with normal video
        for i in 10..15 {
            input_tags.push(create_video_tag(i * 33, true));
        }

        let config = TimingRepairConfig {
            strategy: RepairStrategy::Relaxed,
            ..Default::default()
        };

        // Collect results
        let results = process_tags_through_operator(config, input_tags);

        // Extract audio and video timestamps
        let mut audio_ts = Vec::new();
        let mut video_ts = Vec::new();

        for item in &results {
            if let FlvData::Tag(tag) = item {
                if tag.tag_type == FlvTagType::Audio {
                    audio_ts.push(tag.timestamp_ms);
                } else if tag.tag_type == FlvTagType::Video {
                    video_ts.push(tag.timestamp_ms);
                }
            }
        }

        // Print the timestamps for analysis
        println!("Audio timestamps: {audio_ts:?}");
        println!("Video timestamps: {video_ts:?}");

        // The out-of-sync audio should be corrected to maintain reasonable A/V sync
        // Check that the max difference between audio and video timestamps is not too large

        let mut max_diff = 0;
        for (i, v_ts) in video_ts.iter().enumerate() {
            if i < audio_ts.len() {
                let diff = audio_ts[i].abs_diff(*v_ts);
                max_diff = max(max_diff, diff);
            }
        }

        println!("Maximum A/V timestamp difference: {max_diff}ms");

        // In relaxed mode, we allow some difference but not too much
        assert!(
            max_diff < 500,
            "Audio and video should maintain reasonable sync"
        );
    }
}
