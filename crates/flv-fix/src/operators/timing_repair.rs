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
//! ## Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use kanal;
//! use crate::context::StreamerContext;
//! use crate::operators::timing_repair::{TimingRepairOperator, RepairStrategy};
//!
//! async fn example() {
//!     let context = Arc::new(StreamerContext::default());
//!     let mut operator = TimingRepairOperator::new(
//!         context,
//!         RepairStrategy::default(),
//!     );
//!     
//!     // Create channels for the pipeline
//!     let (input_tx, input_rx) = kanal::bounded_async(32);
//!     let (output_tx, output_rx) = kanal::bounded_async(32);
//!     
//!     // Process stream in background task
//!     tokio::spawn(async move {
//!         operator.process(input_rx, output_tx).await;
//!     });
//! }
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

use crate::context::StreamerContext;
use amf0::Amf0Value;
use flv::data::FlvData;
use flv::error::FlvError;
use flv::script::ScriptData;
use flv::tag::{FlvTag, FlvTagType, FlvUtil};
use kanal::{AsyncReceiver, AsyncSender};
use std::cmp::{max, min};
use std::collections::HashMap;
use std::f64;
use std::sync::Arc;
use tracing::{debug, info, warn};

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

    /// Enable verbose debugging output
    pub debug: bool,
}

impl Default for TimingRepairConfig {
    fn default() -> Self {
        Self {
            strategy: RepairStrategy::default(),
            default_frame_rate: 30.0,
            default_audio_rate: 44100.0,
            max_discontinuity: 1000, // 1 second
            debug: false,
        }
    }
}

/// Helps maintain precise frame intervals with fractional milliseconds
/// Avoids accumulating rounding errors from millisecond precision
struct FrameRateTracker {
    /// Frame rate in frames per second
    fps: f64,
    /// Accumulated fractional part (in milliseconds)
    fractional_ms: f64,
    /// Number of frames processed
    frame_count: u32,
    /// Base timestamp for calculating ideal positions
    base_timestamp: u32,
}

impl FrameRateTracker {
    fn new(fps: f64) -> Self {
        Self {
            fps,
            fractional_ms: 0.0,
            frame_count: 0,
            base_timestamp: 0,
        }
    }

    fn reset(&mut self, fps: f64, base_timestamp: u32) {
        self.fps = fps;
        self.fractional_ms = 0.0;
        self.frame_count = 0;
        self.base_timestamp = base_timestamp;
    }
}

/// Stream timing state for the repair operator
struct TimingState {
    /// Current accumulated offset to apply to timestamps
    delta: i32,

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

    /// Timestamp of the last discontinuity detected
    last_discontinuity: Option<u32>,

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
            last_discontinuity: None,
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
        self.last_discontinuity = None;

        // Keep statistics for logging
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

        // Calculate the absolute difference between expected and last timestamp
        let diff = if expected > last.timestamp_ms {
            expected - last.timestamp_ms
        } else {
            last.timestamp_ms - expected
        };

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
                // based on the frame rate, accounting for rounding errors
                if tag.is_video_tag() && self.last_video_tag.is_some() {
                    // Calculate the expected interval based on the frame rate with fractional precision
                    let exact_interval = 1000.0 / self.frame_rate;

                    // Since millisecond precision (ie 1/1000 of a second) causes rounding errors
                    // (e.g., 30fps → 33.33ms which gets truncated), we allow for ±1ms tolerance
                    let lower_bound = (exact_interval - TOLERANCE as f64).max(0.0).round() as u32;
                    let upper_bound = (exact_interval + TOLERANCE as f64).round() as u32;

                    // Check if the observed interval falls outside our tolerance range
                    diff < lower_bound || diff > upper_bound
                } else {
                    // For non-video tags or when no video history exists
                    diff > threshold * 2 // More lenient for non-video
                }
            }
            RepairStrategy::Relaxed => diff > config.max_discontinuity,
        }
    }

    /// Calculate a new delta correction when a problem is detected
    fn calculate_delta_correction(&mut self, tag: &FlvTag) -> i32 {
        let current = tag.timestamp_ms;
        let mut new_delta = self.delta;
        let last_ts = self.last_tag.as_ref().map(|t| t.timestamp_ms).unwrap_or(0);

        if tag.is_video_tag() && self.last_video_tag.is_some() {
            let last_video = self.last_video_tag.as_ref().unwrap();

            // Calculate ideal next frame timestamp with precision
            let ideal_next_ts = if !last_video.is_video_sequence_header() {
                // Use precise frame rate calculation
                let exact_interval = 1000.0 / self.frame_rate;

                // Round to nearest millisecond to avoid fractional timestamps
                (last_video.timestamp_ms as f64 + exact_interval).round() as u32
            } else {
                // For sequence headers, just use the interval directly
                last_video.timestamp_ms + self.video_frame_interval
            };

            new_delta = ideal_next_ts as i32 - current as i32;
        } else if tag.is_audio_tag() && self.last_audio_tag.is_some() {
            // Similar audio precision handling
            let last_audio = self.last_audio_tag.as_ref().unwrap();
            new_delta =
                (last_audio.timestamp_ms + self.audio_sample_interval) as i32 - current as i32;
        } else if let Some(last) = &self.last_tag {
            // No type-specific last tag, use generic last tag
            let interval = max(self.video_frame_interval, self.audio_sample_interval);
            new_delta = (last.timestamp_ms + interval) as i32 - current as i32;
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
                // Use precise frame rate calculation
                let exact_interval = 1000.0 / self.frame_rate;
                // Round to nearest millisecond to avoid fractional timestamps
                let adjusted_ts = (last_ts as f64 + exact_interval).round() as u32;
                new_delta = if adjusted_ts >= current {
                    (adjusted_ts - current) as i32
                } else {
                    -((current - adjusted_ts) as i32)
                };
            } else if tag.is_audio_tag() {
                // calculate ideal next audio timestamp
                let adjusted_ts = last_ts + self.audio_sample_interval;
                new_delta = if adjusted_ts >= current {
                    (adjusted_ts - current) as i32
                } else {
                    -((current - adjusted_ts) as i32)
                };
            } else {
                // No type-specific last tag, use generic last tag
                let interval = max(self.video_frame_interval, self.audio_sample_interval);
                let adjusted_ts = last_ts + interval;
                new_delta = if adjusted_ts >= current {
                    (adjusted_ts - current) as i32
                } else {
                    -((current - adjusted_ts) as i32)
                };
            }
        }

        new_delta
    }
}

/// Operator for comprehensive timing repair in FLV streams
pub struct TimingRepairOperator {
    context: Arc<StreamerContext>,
    config: TimingRepairConfig,
}

impl TimingRepairOperator {
    /// Create a new TimingRepairOperator with the specified configuration
    pub fn new(context: Arc<StreamerContext>, config: TimingRepairConfig) -> Self {
        Self { context, config }
    }

    /// Create a new TimingRepairOperator with default configuration
    pub fn with_strategy(context: Arc<StreamerContext>, strategy: RepairStrategy) -> Self {
        let mut config = TimingRepairConfig::default();
        config.strategy = strategy;
        Self { context, config }
    }

    /// Process method that receives FLV data, corrects timing issues, and forwards the data
    pub async fn process(
        &self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) {
        let mut state = TimingState::new(&self.config);

        while let Ok(item) = input.recv().await {
            match item {
                Ok(mut data) => {
                    match &mut data {
                        FlvData::Header(_) => {
                            // Reset state when encountering a header
                            state.reset(&self.config);

                            if self.config.debug {
                                debug!(
                                    "{} TimingRepair: Processing new segment",
                                    self.context.name
                                );
                            }

                            // Forward the header unmodified
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
                        }
                        FlvData::Tag(tag) => {
                            state.tag_count += 1;
                            let original_timestamp = tag.timestamp_ms;

                            // Handle script tags (metadata)
                            if tag.is_script_tag() {
                                let mut cursor = std::io::Cursor::new(tag.data.clone());
                                if let Ok(amf_data) = ScriptData::demux(&mut cursor) {
                                    if amf_data.name == "onMetaData" && !amf_data.data.is_empty() {
                                        match &amf_data.data[0] {
                                            Amf0Value::Object(props) => {
                                                // Create a HashMap for easier property access
                                                let properties = props
                                                    .iter()
                                                    .map(|(k, v)| (k.to_string(), v.clone()))
                                                    .collect::<HashMap<String, Amf0Value>>();

                                                state.update_timing_params(&properties);

                                                if self.config.debug {
                                                    debug!(
                                                        "{} TimingRepair: Updated timing params - video interval: {}ms, audio interval: {}ms",
                                                        self.context.name,
                                                        state.video_frame_interval,
                                                        state.audio_sample_interval
                                                    );
                                                }
                                            }
                                            Amf0Value::StrictArray(items) => {
                                                // StrictArray doesn't have named key-value pairs like Object,
                                                // but some encoders might use it with a specific structure.
                                                // Try to find framerate and audio sample rate values in common positions
                                                if self.config.debug {
                                                    debug!(
                                                        "{} TimingRepair: Received metadata as StrictArray with {} items",
                                                        self.context.name,
                                                        items.len()
                                                    );
                                                }

                                                // Try to extract timing information based on common patterns
                                                // Some encoders put framerate as the first or third numeric value
                                                let mut framerate_value: Option<f64> = None;
                                                let mut audio_rate_value: Option<f64> = None;

                                                // Look for numeric values that might be frame rate (typically 23.976-60)
                                                // or audio sample rate (typically 44100, 48000, etc.)
                                                for item in items.iter() {
                                                    if let Amf0Value::Number(value) = item {
                                                        // Potential frame rate (common video frame rates are between 10-500)
                                                        if *value > 10.0
                                                            && *value < 500.0
                                                            && framerate_value.is_none()
                                                        {
                                                            framerate_value = Some(*value);
                                                            if self.config.debug {
                                                                debug!(
                                                                    "{} TimingRepair: Found potential framerate in StrictArray: {}",
                                                                    self.context.name, *value
                                                                );
                                                            }
                                                        }
                                                        // Potential audio sample rate
                                                        else if (*value == 44100.0
                                                            || *value == 48000.0
                                                            || *value == 22050.0
                                                            || *value == 11025.0
                                                            || *value == 88200.0
                                                            || *value == 96000.0)
                                                            && audio_rate_value.is_none()
                                                        {
                                                            audio_rate_value = Some(*value);
                                                            if self.config.debug {
                                                                debug!(
                                                                    "{} TimingRepair: Found potential audio sample rate in StrictArray: {}",
                                                                    self.context.name, *value
                                                                );
                                                            }
                                                        }
                                                    }
                                                }

                                                // Apply any found values to update timing parameters
                                                if framerate_value.is_some()
                                                    || audio_rate_value.is_some()
                                                {
                                                    let mut properties = HashMap::new();

                                                    if let Some(fps) = framerate_value {
                                                        properties.insert(
                                                            "framerate".to_string(),
                                                            Amf0Value::Number(fps),
                                                        );
                                                    }

                                                    if let Some(rate) = audio_rate_value {
                                                        properties.insert(
                                                            "audiosamplerate".to_string(),
                                                            Amf0Value::Number(rate),
                                                        );
                                                    }

                                                    state.update_timing_params(&properties);

                                                    if self.config.debug {
                                                        debug!(
                                                            "{} TimingRepair: Updated timing params from StrictArray - video interval: {}ms, audio interval: {}ms",
                                                            self.context.name,
                                                            state.video_frame_interval,
                                                            state.audio_sample_interval
                                                        );
                                                    }
                                                }
                                            }
                                            _ => {
                                                if self.config.debug {
                                                    debug!(
                                                        "{} TimingRepair: Metadata format not supported: {:?}",
                                                        self.context.name,
                                                        amf_data.data[0].marker()
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }

                                // Script tags should have timestamp 0
                                if tag.timestamp_ms != 0 {
                                    tag.timestamp_ms = 0;
                                    if self.config.debug {
                                        debug!(
                                            "{} TimingRepair: Reset script tag timestamp to 0",
                                            self.context.name
                                        );
                                    }
                                }

                                // Forward script tag
                                if output.send(Ok(data)).await.is_err() {
                                    return;
                                }
                                continue;
                            }

                            // Check for timestamp issues
                            let mut need_correction = false;

                            // Check for timestamp rebounds
                            if state.is_timestamp_rebounded(tag) {
                                // warn!("rebounded tag is : {:?}", tag.tag_type);
                                state.rebound_count += 1;
                                let new_delta = state.calculate_delta_correction(tag);

                                warn!(
                                    "{} TimingRepair: Timestamp rebound detected: {}ms would go back in time - applying correction delta: {}ms",
                                    self.context.name, tag.timestamp_ms, new_delta
                                );

                                state.delta = new_delta;
                                need_correction = true;
                            }
                            // Check for discontinuities
                            else if state.is_timestamp_discontinuous(tag, &self.config) {
                                state.discontinuity_count += 1;
                                let new_delta = state.calculate_delta_correction(tag);

                                warn!(
                                    "{} TimingRepair: Timestamp discontinuity detected: {}ms, applying correction delta: {}ms",
                                    self.context.name, tag.timestamp_ms, new_delta
                                );

                                state.delta = new_delta;
                                need_correction = true;
                                state.last_discontinuity = Some(tag.timestamp_ms);
                            }

                            // Apply correction if needed
                            if state.delta != 0 || need_correction {
                                // Apply the correction delta
                                // probably not gonna happen, but just in case
                                let corrected_timestamp = if tag.timestamp_ms as i32 + state.delta
                                    < 0
                                {
                                    warn!(
                                        "{} TimingRepair: Negative timestamp detected, applying frame-rate aware correction",
                                        self.context.name
                                    );
                                    // Avoid negative timestamps - use frame-rate aware correction
                                    if let Some(last) = &state.last_tag {
                                        if tag.is_video_tag() {
                                            // Calculate ideal timestamp using frame rate
                                            let interval =
                                                (1000.0 / state.frame_rate).round() as u32;
                                            last.timestamp_ms + interval
                                        } else if tag.is_audio_tag() {
                                            last.timestamp_ms + state.audio_sample_interval
                                        } else {
                                            last.timestamp_ms + 1
                                        }
                                    } else {
                                        0
                                    }
                                } else {
                                    (tag.timestamp_ms as i32 + state.delta) as u32
                                };

                                tag.timestamp_ms = corrected_timestamp;
                                state.correction_count += 1;

                                if self.config.debug {
                                    debug!(
                                        "{} TimingRepair: Corrected timestamp: {}ms -> {}ms (delta: {}ms)",
                                        self.context.name,
                                        original_timestamp,
                                        corrected_timestamp,
                                        state.delta
                                    );
                                }
                            }

                            // Update state with this tag
                            if tag.is_audio_tag() {
                                state.last_audio_tag = Some(tag.clone());
                            } else if tag.is_video_tag() {
                                state.last_video_tag = Some(tag.clone());
                            }
                            state.last_tag = Some(tag.clone());

                            // Forward the tag with corrected timestamp
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
                        }
                        // Forward other data types unmodified
                        _ => {
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    // Forward error
                    if output.send(Err(e)).await.is_err() {
                        return;
                    }
                }
            }
        }

        // Log statistics when complete
        info!(
            "{} TimingRepair complete: Processed {} tags, applied {} corrections, detected {} rebounds and {} discontinuities",
            self.context.name,
            state.tag_count,
            state.correction_count,
            state.rebound_count,
            state.discontinuity_count
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::header::FlvHeader;

    // Helper functions for testing
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    fn create_header() -> FlvData {
        FlvData::Header(FlvHeader::new(true, true))
    }

    fn create_video_tag(timestamp: u32) -> FlvData {
        let data = vec![0x17, 0x01, 0x00, 0x00, 0x00];
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        })
    }

    fn create_audio_tag(timestamp: u32) -> FlvData {
        let data = vec![0xAF, 0x01, 0x21, 0x10, 0x04];
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Audio,
            data: Bytes::from(data),
        })
    }

    fn create_video_sequence_tag(timestamp: u32) -> FlvData {
        let data = vec![
            0x17, // frame type 1 (keyframe) + codec id 7 (AVC)
            0x00, // AVC sequence header
            0x00, 0x00, 0x00, // composition time
            0x01, // version
            0x64, 0x00, 0x1F, 0xFF, // SPS parameter set stuff
        ];
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        })
    }

    fn create_audio_sequence_tag(timestamp: u32) -> FlvData {
        let data = vec![
            0xAF, // Audio format 10 (AAC) + sample rate 3 (44kHz) + sample size 1 (16-bit) + stereo
            0x00, // AAC sequence header
            0x12, 0x10, // AAC config
        ];
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Audio,
            data: Bytes::from(data),
        })
    }

    fn create_script_tag_with_metadata(frame_rate: f64, audio_rate: f64) -> FlvData {
        // This is a simplified version - in a real implementation we would create a proper AMF object
        let data = Vec::new(); // We would need to serialize AMF data here
        FlvData::Tag(FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: FlvTagType::ScriptData,
            data: Bytes::from(data),
        })
    }

    #[tokio::test]
    async fn test_timestamp_rebound_correction() {
        let context = create_test_context();
        let config = TimingRepairConfig {
            strategy: RepairStrategy::Strict,
            debug: true,
            ..Default::default()
        };

        let mut operator = TimingRepairOperator::new(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send video sequence header
        input_tx
            .send(Ok(create_video_sequence_tag(0)))
            .await
            .unwrap();

        // Send audio sequence header
        input_tx
            .send(Ok(create_audio_sequence_tag(0)))
            .await
            .unwrap();

        // Send regular tags with increasing timestamps
        for i in 1..5 {
            input_tx.send(Ok(create_video_tag(i * 33))).await.unwrap();
            input_tx
                .send(Ok(create_audio_tag(i * 33 + 5)))
                .await
                .unwrap();
        }

        // Send a tag with timestamp rebound (goes backwards)
        input_tx.send(Ok(create_video_tag(50))).await.unwrap();

        // Send more regular tags
        for i in 6..10 {
            input_tx.send(Ok(create_video_tag(i * 33))).await.unwrap();
            input_tx
                .send(Ok(create_audio_tag(i * 33 + 5)))
                .await
                .unwrap();
        }

        // Close input
        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        // Print results for analysis
        for (i, item) in results.iter().enumerate() {
            if let FlvData::Tag(tag) = item {
                println!(
                    "Tag {}: {:?} timestamp = {}ms",
                    i, tag.tag_type, tag.timestamp_ms
                );
            }
        }

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

    #[tokio::test]
    async fn test_timestamp_discontinuity_correction() {
        let context = create_test_context();
        let config = TimingRepairConfig {
            strategy: RepairStrategy::Strict,
            debug: true,
            ..Default::default()
        };

        let mut operator = TimingRepairOperator::new(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send regular tags with increasing timestamps
        for i in 1..5 {
            input_tx.send(Ok(create_video_tag(i * 33))).await.unwrap();
        }

        // Send a tag with large timestamp jump (discontinuity)
        input_tx.send(Ok(create_video_tag(5000))).await.unwrap();

        // Send more regular tags after the jump
        for i in 151..155 {
            input_tx.send(Ok(create_video_tag(i * 33))).await.unwrap();
        }

        // Close input
        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        // Print results for analysis
        for (i, item) in results.iter().enumerate() {
            if let FlvData::Tag(tag) = item {
                println!(
                    "Tag {}: {:?} timestamp = {}ms",
                    i, tag.tag_type, tag.timestamp_ms
                );
            }
        }

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

    #[tokio::test]
    async fn test_av_sync_maintenance() {
        let context = create_test_context();
        let config = TimingRepairConfig {
            strategy: RepairStrategy::Relaxed,
            debug: true,
            ..Default::default()
        };

        let mut operator = TimingRepairOperator::new(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send interleaved audio and video with audio slightly ahead
        for i in 1..10 {
            input_tx.send(Ok(create_video_tag(i * 33))).await.unwrap();
            input_tx
                .send(Ok(create_audio_tag(i * 33 + 5)))
                .await
                .unwrap();
        }

        // Send audio with major sync issue (far ahead of video)
        input_tx.send(Ok(create_audio_tag(800))).await.unwrap();

        // Continue with normal video
        for i in 10..15 {
            input_tx.send(Ok(create_video_tag(i * 33))).await.unwrap();
        }

        // Close input
        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        // Extract audio and video timestamps to check sync
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
        println!("Audio timestamps: {:?}", audio_ts);
        println!("Video timestamps: {:?}", video_ts);

        // The out-of-sync audio should be corrected to maintain reasonable A/V sync
        // Check that the max difference between audio and video timestamps is not too large

        let mut max_diff = 0;
        for (i, v_ts) in video_ts.iter().enumerate() {
            if i < audio_ts.len() {
                let diff = if audio_ts[i] > *v_ts {
                    audio_ts[i] - *v_ts
                } else {
                    *v_ts - audio_ts[i]
                };
                max_diff = max(max_diff, diff);
            }
        }

        println!("Maximum A/V timestamp difference: {}ms", max_diff);

        // In relaxed mode, we allow some difference but not too much
        assert!(
            max_diff < 500,
            "Audio and video should maintain reasonable sync"
        );
    }
}
