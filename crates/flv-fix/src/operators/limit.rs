//! # LimitOperator
//!
//! The `LimitOperator` implements size and duration limits for FLV streams, automatically
//! splitting the stream when configured thresholds are reached.
//!
//! ## Purpose
//!
//! Long media streams often need to be segmented for:
//! - Limiting file sizes to avoid filesystem limitations
//! - Creating manageable chunks for storage and processing
//! - Enabling time-based segmentation for archives
//! - Implementing recording limits
//!
//! This operator monitors stream size and duration, automatically splitting the stream
//! and re-injecting appropriate headers when limits are reached.
//!
//! ## Operation
//!
//! The operator:
//! - Tracks accumulated byte size of all emitted tags
//! - Monitors the maximum timestamp seen in the stream
//! - Triggers splits when size or duration thresholds are exceeded
//! - Re-injects stream headers after each split
//! - Supports optional callbacks when splits occur
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
use flv::header::FlvHeader;
use flv::tag::{FlvTag, FlvUtil};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

/// Reason for a stream split
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitReason {
    /// Split due to size limit being reached
    SizeLimit,
    /// Split due to duration limit being reached
    DurationLimit,
    /// Split due to both limits being reached
    BothLimits,
}

/// Optional callback for when a stream split occurs
pub type SplitCallback = Box<dyn Fn(SplitReason, u64, u32)>;

/// Configuration options for the limit operator
pub struct LimitConfig {
    /// Maximum size in bytes before splitting (None = no limit)
    pub max_size_bytes: Option<u64>,

    /// Maximum duration in milliseconds before splitting (None = no limit)
    pub max_duration_ms: Option<u32>,

    /// Whether to split at keyframes only (may exceed limits slightly)
    pub split_at_keyframes_only: bool,

    /// Whether to use retrospective splitting (split at last keyframe when limit is reached)
    pub use_retrospective_splitting: bool,

    /// Optional callback when a split occurs, receives:
    /// - The reason for the split
    /// - The accumulated size in bytes
    /// - The duration in milliseconds
    pub on_split: Option<SplitCallback>,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: None,
            max_duration_ms: None,
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: None,
        }
    }
}

// Store stream state for re-emission after splits
struct StreamState {
    header: Option<FlvHeader>,
    metadata: Option<FlvTag>,
    audio_sequence_tag: Option<FlvTag>,
    video_sequence_tag: Option<FlvTag>,
    accumulated_size: u64,
    start_timestamp: u32,
    max_timestamp: u32,
    last_keyframe_position: Option<(u64, u32)>, // (size, timestamp) at last keyframe
    split_count: u32,
    first_content_tag_seen: bool,
}

impl StreamState {
    fn new() -> Self {
        Self {
            header: None,
            metadata: None,
            audio_sequence_tag: None,
            video_sequence_tag: None,
            accumulated_size: 0,
            start_timestamp: 0,
            max_timestamp: 0,
            last_keyframe_position: None,
            split_count: 0,
            first_content_tag_seen: false,
        }
    }

    fn reset_counters(&mut self) {
        self.accumulated_size = 0;
        self.start_timestamp = self.max_timestamp;
        self.last_keyframe_position = None;
        self.split_count += 1;
    }

    fn current_duration(&self) -> u32 {
        self.max_timestamp.saturating_sub(self.start_timestamp)
    }
}

/// Operator that limits FLV streams by size and/or duration
pub struct LimitOperator {
    context: Arc<StreamerContext>,
    config: LimitConfig,
    state: StreamState,
    last_split_time: Instant,
}

impl LimitOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self::with_config(context, LimitConfig::default())
    }

    pub fn with_config(context: Arc<StreamerContext>, config: LimitConfig) -> Self {
        Self {
            context,
            config,
            state: StreamState::new(),
            last_split_time: Instant::now(),
        }
    }

    fn determine_split_reason(&self) -> SplitReason {
        let size_exceeded = self
            .config
            .max_size_bytes
            .map(|max| self.state.accumulated_size >= max)
            .unwrap_or(false);

        let duration_exceeded = self
            .config
            .max_duration_ms
            .map(|max| self.state.current_duration() >= max)
            .unwrap_or(false);

        match (size_exceeded, duration_exceeded) {
            (true, true) => SplitReason::BothLimits,
            (true, false) => SplitReason::SizeLimit,
            (false, true) => SplitReason::DurationLimit,
            _ => SplitReason::SizeLimit, // Default to size limit if somehow we get here
        }
    }

    fn check_limits(&self) -> bool {
        // Check size limit if configured
        if let Some(max_size) = self.config.max_size_bytes {
            if self.state.accumulated_size >= max_size {
                debug!(
                    "{} Size limit exceeded: {} bytes (max: {} bytes)",
                    self.context.name, self.state.accumulated_size, max_size
                );
                return true;
            }
        }

        // Check duration limit if configured - only if we've seen at least one content tag
        if let Some(max_duration) = self.config.max_duration_ms {
            if self.state.first_content_tag_seen {
                let current_duration = self.state.current_duration();
                if current_duration >= max_duration {
                    debug!(
                        "{} Duration limit exceeded: {} ms (max: {} ms)",
                        self.context.name, current_duration, max_duration
                    );
                    return true;
                }
            }
        }

        false
    }

    fn split_stream(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        info!(
            "{} Splitting stream at size={} bytes, duration={}ms (segment #{})",
            self.context.name,
            self.state.accumulated_size,
            self.state.current_duration(),
            self.state.split_count + 1
        );

        // Send each item with Arc cloning instead of full data cloning
        if let Some(header) = &self.state.header {
            output(FlvData::Header(header.clone()))?;
            debug!("{} {}", self.context.name, "re-emit header after split");
        }
        if let Some(metadata) = &self.state.metadata {
            output(FlvData::Tag(metadata.clone()))?;
            debug!("{} {}", self.context.name, "re-emit metadata after split");
        }
        if let Some(video_seq) = &self.state.video_sequence_tag {
            output(FlvData::Tag(video_seq.clone()))?;
            debug!(
                "{} {}",
                self.context.name, "re-emit video sequence tag after split"
            );
        }
        if let Some(audio_seq) = &self.state.audio_sequence_tag {
            output(FlvData::Tag(audio_seq.clone()))?;
            debug!(
                "{} {}",
                self.context.name, "re-emit audio sequence tag after split"
            );
        }

        // Reset accumulated counters for the new segment
        self.state.reset_counters();
        self.last_split_time = Instant::now();
        Ok(())
    }
}

impl Processor<FlvData> for LimitOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match input {
            FlvData::Header(header) => {
                // Reset state for a new stream
                self.state = StreamState::new();
                self.state.header = Some(header.clone());
                self.last_split_time = Instant::now();

                // Forward the header
                output(FlvData::Header(header))?;
            }
            FlvData::Tag(tag) => {
                // Update size counter
                let tag_size = tag.size() as u64;
                self.state.accumulated_size += tag_size;

                // Update timestamp tracking
                if tag.timestamp_ms > self.state.max_timestamp {
                    self.state.max_timestamp = tag.timestamp_ms;
                }

                // Track key metadata
                if tag.is_script_tag() {
                    self.state.metadata = Some(tag.clone());
                } else if tag.is_video_sequence_header() {
                    let mut tag = tag.clone();
                    tag.timestamp_ms = 0; // Reset timestamp for video sequence header
                    self.state.video_sequence_tag = Some(tag);
                } else if tag.is_audio_sequence_header() {
                    let mut tag = tag.clone();
                    tag.timestamp_ms = 0; // Reset timestamp for audio sequence header
                    self.state.audio_sequence_tag = Some(tag);
                } else if !self.state.first_content_tag_seen {
                    // This is the first actual content tag (not sequence header or metadata)
                    // Set the start timestamp to this tag's timestamp
                    self.state.start_timestamp = tag.timestamp_ms;
                    self.state.first_content_tag_seen = true;
                    debug!(
                        "{} First content tag detected, setting start timestamp to {}ms.",
                        self.context.name, tag.timestamp_ms
                    );
                }

                // Track keyframes for optimal split points
                if tag.is_key_frame_nalu() {
                    self.state.last_keyframe_position =
                        Some((self.state.accumulated_size, self.state.max_timestamp));
                }

                // Check if any limit is exceeded
                let should_split = self.check_limits();

                // Inside the process method where split decisions are made
                if should_split && (!self.config.split_at_keyframes_only || tag.is_key_frame_nalu())
                {
                    // Direct splitting - no retrospective logic
                    let split_reason = self.determine_split_reason();

                    // Report the split with current stats
                    if let Some(callback) = &self.config.on_split {
                        let duration = self.state.current_duration();
                        (callback)(split_reason, self.state.accumulated_size, duration);
                    }

                    // Perform the split
                    self.split_stream(output)?;

                    // Emit current tag after the split if it's a keyframe
                    if tag.is_key_frame_nalu() || !self.config.split_at_keyframes_only {
                        output(FlvData::Tag(tag))?;
                    }
                } else if should_split
                    && self.config.split_at_keyframes_only
                    && self.config.use_retrospective_splitting
                    && self.state.last_keyframe_position.is_some()
                {
                    // Retrospective splitting logic - only used if enabled
                    // let split_reason = self.determine_split_reason();

                    // We're not at a keyframe but need to split at a keyframe
                    // Emit this tag then trigger split
                    output(FlvData::Tag(tag))?;

                    if let Some((size, timestamp)) = self.state.last_keyframe_position {
                        // Use the position of the last keyframe for stats
                        if let Some(callback) = &self.config.on_split {
                            let duration = timestamp.saturating_sub(self.state.start_timestamp);
                            (callback)(self.determine_split_reason(), size, duration);
                        }
                    }

                    // Perform the split
                    self.split_stream(output)?;
                } else {
                    // No split needed, just forward the tag
                    output(FlvData::Tag(tag))?;
                }
            }
            _ => {
                // 转发其他数据类型
                output(input)?;
            }
        }
        Ok(())
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} completed.", self.context.name);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "LimitOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{self, create_script_tag};
    use bytes::Bytes;
    use flv::tag::{FlvTag, FlvTagType};
    use pipeline_common::test_utils::create_test_context;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_size_limit_with_keyframe_splitting() {
        let context = create_test_context();
        let split_counter = Arc::new(AtomicUsize::new(0));
        let split_counter_clone = split_counter.clone();

        // Configure with a size limit of 100KB and keyframe splitting
        let config = LimitConfig {
            max_size_bytes: Some(100 * 1024),
            max_duration_ms: None,
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: Some(Box::new(move |_, _, _| {
                split_counter.fetch_add(1, Ordering::SeqCst);
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();

        // Send tags with increasing size, starting with a keyframe
        operator
            .process(
                test_utils::create_video_tag_with_size(0, true, 10 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 10KB keyframe
        operator
            .process(
                test_utils::create_video_tag_with_size(100, false, 30 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 30KB P-frame
        operator
            .process(
                test_utils::create_video_tag_with_size(200, false, 40 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 40KB P-frame
        operator
            .process(
                test_utils::create_video_tag_with_size(300, true, 10 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 10KB keyframe
        operator
            .process(
                test_utils::create_video_tag_with_size(400, false, 50 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 50KB P-frame
        operator
            .process(
                test_utils::create_video_tag_with_size(500, false, 60 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 60KB P-frame
        operator
            .process(
                test_utils::create_video_tag_with_size(600, true, 10 * 1024),
                &mut output_fn,
            )
            .unwrap(); // 10KB keyframe

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check that splits occurred (should be at least 1 split)
        assert!(
            split_counter_clone.load(Ordering::SeqCst) > 0,
            "Should have split at least once"
        );

        // Each output segment should start with a header
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 1 initial header + 1 for each split
        assert_eq!(
            header_count,
            split_counter_clone.load(Ordering::SeqCst) + 1,
            "Should have a header for initial segment and each split"
        );
    }

    #[test]
    fn test_duration_limit_with_keyframe_splitting() {
        let context = create_test_context();
        let split_counter = Arc::new(AtomicUsize::new(0));
        let split_counter_clone = split_counter.clone();

        // Configure with a duration limit of 500ms and keyframe splitting
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: Some(500),
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: Some(Box::new(move |_, _, _| {
                split_counter.fetch_add(1, Ordering::SeqCst);
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();

        // Send tags with increasing timestamps
        operator
            .process(test_utils::create_video_tag(0, true), &mut output_fn)
            .unwrap(); // keyframe at 0ms
        operator
            .process(test_utils::create_video_tag(100, false), &mut output_fn)
            .unwrap(); // P-frame at 100ms
        operator
            .process(test_utils::create_video_tag(200, false), &mut output_fn)
            .unwrap(); // P-frame at 200ms
        operator
            .process(test_utils::create_video_tag(400, false), &mut output_fn)
            .unwrap(); // P-frame at 400ms
        operator
            .process(test_utils::create_video_tag(600, true), &mut output_fn)
            .unwrap(); // keyframe at 600ms (should cause split)
        operator
            .process(test_utils::create_video_tag(700, false), &mut output_fn)
            .unwrap(); // P-frame at 700ms
        operator
            .process(test_utils::create_video_tag(800, false), &mut output_fn)
            .unwrap(); // P-frame at 800ms
        operator
            .process(test_utils::create_video_tag(1000, true), &mut output_fn)
            .unwrap(); // keyframe at 1000ms (should cause split)
        operator
            .process(test_utils::create_video_tag(1100, false), &mut output_fn)
            .unwrap(); // P-frame at 1100ms

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check that splits occurred (should be at least 1 split)
        assert!(
            split_counter_clone.load(Ordering::SeqCst) > 0,
            "Should have split at least once"
        );

        // Each output segment should start with a header
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 1 initial header + 1 for each split
        assert_eq!(
            header_count,
            split_counter_clone.load(Ordering::SeqCst) + 1,
            "Should have a header for initial segment and each split"
        );
    }

    #[test]
    fn test_no_limits() {
        let context = create_test_context();
        let split_counter = Arc::new(AtomicUsize::new(0));
        let split_counter_clone = split_counter.clone();

        // Configure with no limits
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: None,
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: Some(Box::new(move |_, _, _| {
                split_counter.fetch_add(1, Ordering::SeqCst);
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();

        // Send several tags
        for i in 0..10 {
            let is_keyframe = i % 3 == 0;
            operator
                .process(
                    test_utils::create_video_tag(i * 100, is_keyframe),
                    &mut output_fn,
                )
                .unwrap();
        }

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // No splits should have occurred
        assert_eq!(
            split_counter_clone.load(Ordering::SeqCst),
            0,
            "Should not have split"
        );

        // Should only have the initial header
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert_eq!(header_count, 1, "Should only have the initial header");

        // Tag count should match input
        let tag_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Tag(_)))
            .count();

        assert_eq!(tag_count, 10, "Should have all input tags");
    }

    #[test]
    fn test_sequential_splits() {
        let context = create_test_context();

        // Track split count
        let split_count = Arc::new(AtomicUsize::new(0));
        let split_count_clone = Arc::clone(&split_count);

        // Configure with both size and duration limits
        let config = LimitConfig {
            max_size_bytes: Some(500),
            max_duration_ms: Some(300),
            split_at_keyframes_only: false,
            use_retrospective_splitting: false,
            on_split: Some(Box::new(move |_, _, _| {
                split_count.fetch_add(1, Ordering::SeqCst);
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send a stream with sequence headers
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_script_tag(0, false), &mut output_fn)
            .unwrap();
        operator
            .process(test_utils::create_video_sequence_header(0), &mut output_fn)
            .unwrap();
        operator
            .process(test_utils::create_audio_sequence_header(0), &mut output_fn)
            .unwrap();

        // First send tags that should trigger size limit
        for i in 0..3 {
            operator
                .process(
                    test_utils::create_video_tag_with_size(i * 50, i % 2 == 0, 200),
                    &mut output_fn,
                )
                .unwrap();
        }

        // Then send tags that should trigger duration limit
        for i in 0..4 {
            operator
                .process(
                    test_utils::create_video_tag_with_size(i * 100 + 300, i % 2 == 0, 50),
                    &mut output_fn,
                )
                .unwrap();
        }

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check split count
        let final_split_count = split_count_clone.load(Ordering::SeqCst);
        assert!(
            final_split_count >= 2,
            "Expected at least 2 splits (one for size, one for duration), got {final_split_count}"
        );

        // Check header count matches split count + initial header
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert_eq!(
            header_count,
            final_split_count + 1,
            "Expected header count to match splits + initial"
        );
    }

    #[test]
    fn test_split_with_interleaved_audio_video() {
        let context = create_test_context();

        // Track split timestamps
        let split_timestamps = Arc::new(Mutex::new(Vec::new()));

        // Configure with duration limit
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: Some(400),
            split_at_keyframes_only: true,
            use_retrospective_splitting: false,
            on_split: Some(Box::new({
                let st_clone = Arc::clone(&split_timestamps);
                move |_, _, duration| {
                    st_clone.lock().unwrap().push(duration);
                }
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Helper function to create audio tag
        let create_audio_tag = |timestamp: u32, size: usize| -> FlvData {
            let data = vec![0u8; size];
            FlvData::Tag(FlvTag {
                timestamp_ms: timestamp,
                stream_id: 0,
                tag_type: FlvTagType::Audio,
                data: Bytes::from(data),
            })
        };

        // Send initial headers
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_script_tag(0, false), &mut output_fn)
            .unwrap();
        operator
            .process(test_utils::create_video_sequence_header(0), &mut output_fn)
            .unwrap();
        operator
            .process(test_utils::create_audio_sequence_header(0), &mut output_fn)
            .unwrap();

        // Interleaved audio and video with keyframes at 0ms and 500ms
        operator
            .process(
                test_utils::create_video_tag_with_size(0, true, 100),
                &mut output_fn,
            )
            .unwrap(); // Keyframe
        operator
            .process(create_audio_tag(50, 20), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(100, 20), &mut output_fn)
            .unwrap();
        operator
            .process(
                test_utils::create_video_tag_with_size(150, false, 100),
                &mut output_fn,
            )
            .unwrap(); // Non-keyframe
        operator
            .process(create_audio_tag(200, 20), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(250, 20), &mut output_fn)
            .unwrap();
        operator
            .process(
                test_utils::create_video_tag_with_size(300, false, 100),
                &mut output_fn,
            )
            .unwrap(); // Non-keyframe
        operator
            .process(create_audio_tag(350, 20), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(400, 20), &mut output_fn)
            .unwrap(); // Should exceed duration limit (400ms)
        operator
            .process(
                test_utils::create_video_tag_with_size(450, false, 100),
                &mut output_fn,
            )
            .unwrap(); // Non-keyframe
        operator
            .process(
                test_utils::create_video_tag_with_size(500, true, 100),
                &mut output_fn,
            )
            .unwrap(); // Keyframe - split should happen here

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check that split happened at the keyframe
        let timestamps = split_timestamps.lock().unwrap().clone();
        assert_eq!(timestamps.len(), 1, "Expected 1 split at the keyframe");
        assert!(
            timestamps[0] >= 400,
            "Split should occur at or after duration limit was reached"
        );

        // Count headers in output to verify split happened
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();
        assert_eq!(
            header_count, 2,
            "Expected original header plus one from split"
        );
    }

    #[test]
    fn test_empty_stream_no_splits() {
        let context = create_test_context();

        // Track split events
        let split_count = Arc::new(AtomicUsize::new(0));
        let split_count_clone = Arc::clone(&split_count);

        // Configure with size limit
        let config = LimitConfig {
            max_size_bytes: Some(1000),
            max_duration_ms: None,
            split_at_keyframes_only: false,
            use_retrospective_splitting: false,
            on_split: Some(Box::new(move |_, _, _| {
                split_count.fetch_add(1, Ordering::SeqCst);
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send just a header with no actual content
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();

        // Finish processing immediately
        operator.finish(&mut output_fn).unwrap();

        // Verify no splits occurred
        let final_split_count = split_count_clone.load(Ordering::SeqCst);
        assert_eq!(
            final_split_count, 0,
            "No splits should occur in empty stream"
        );

        // Verify we got just the header back
        assert_eq!(output_items.len(), 1, "Expected only the header in output");
        assert!(
            matches!(output_items[0], FlvData::Header(_)),
            "Expected only a header in the output"
        );
    }

    #[test]
    fn test_retrospective_splitting() {
        let context = create_test_context();

        // Track split information
        let split_timestamps = Arc::new(Mutex::new(Vec::new()));
        let split_timestamps_clone = Arc::clone(&split_timestamps);

        // Configure with size limit and retrospective splitting
        let config = LimitConfig {
            max_size_bytes: Some(500),
            max_duration_ms: None,
            split_at_keyframes_only: true,
            use_retrospective_splitting: true, // Enable retrospective splitting
            on_split: Some(Box::new({
                let callback_split_timestamps = Arc::clone(&split_timestamps);
                move |_, _, ts| {
                    callback_split_timestamps.lock().unwrap().push(ts);
                }
            })),
        };

        let mut operator = LimitOperator::with_config(context, config);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();

        // Send a sequence with a keyframe followed by normal frames
        operator
            .process(
                test_utils::create_video_tag_with_size(0, true, 200),
                &mut output_fn,
            )
            .unwrap(); // Keyframe
        operator
            .process(
                test_utils::create_video_tag_with_size(100, false, 100),
                &mut output_fn,
            )
            .unwrap();
        operator
            .process(
                test_utils::create_video_tag_with_size(200, false, 100),
                &mut output_fn,
            )
            .unwrap();

        // Send more non-keyframes to exceed the size limit
        operator
            .process(
                test_utils::create_video_tag_with_size(300, false, 200),
                &mut output_fn,
            )
            .unwrap(); // This should trigger size limit
        operator
            .process(
                test_utils::create_video_tag_with_size(400, false, 100),
                &mut output_fn,
            )
            .unwrap();

        // Now send a keyframe - this is where the split should happen
        operator
            .process(
                test_utils::create_video_tag_with_size(500, true, 100),
                &mut output_fn,
            )
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check that a split occurred
        let split_timestamps = split_timestamps_clone.lock().unwrap().clone();
        assert!(
            !split_timestamps.is_empty(),
            "Should have performed at least one split"
        );

        // Verify headers were inserted properly
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert!(header_count > 1, "Should have more than the initial header");
    }
}
