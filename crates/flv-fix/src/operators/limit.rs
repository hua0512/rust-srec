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
//! ## Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use kanal;
//! use crate::context::StreamerContext;
//! use crate::operators::limit::{LimitOperator, LimitConfig};
//!
//! async fn example() {
//!     let context = Arc::new(StreamerContext::default());
//!     let config = LimitConfig {
//!         max_size_bytes: Some(10_000_000), // 10MB per segment
//!         max_duration_ms: Some(300_000),   // 5 minutes per segment
//!         ..Default::default()
//!     };
//!     let mut operator = LimitOperator::with_config(context, config);
//!
//!     // Create channels for the pipeline
//!     let (input_tx, input_rx) = kanal::bounded_async(32);
//!     let (output_tx, output_rx) = kanal::bounded_async(32);
//!
//!     // Process stream in background task
//!     tokio::spawn(async move {
//!         operator.process(input_rx, output_tx).await;
//!     });
//!
//!     // Input data via input_tx
//!     // Process output from output_rx
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
use crate::operators::FlvOperator;
use bytes::{Bytes, BytesMut};
use flv::data::FlvData;
use flv::error::FlvError;
use flv::header::FlvHeader;
use flv::tag::{FlvTag, FlvTagType, FlvUtil};
use kanal::{AsyncReceiver, AsyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

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
pub type SplitCallback = Box<dyn Fn(SplitReason, u64, u32) + Send + Sync>;

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
            use_retrospective_splitting: false, // Changed to false to disable by default
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
                println!(
                    "{} Size limit exceeded: {} bytes (max: {} bytes)",
                    self.context.name, self.state.accumulated_size, max_size
                );
                return true;
            }
        }

        // Check duration limit if configured
        if let Some(max_duration) = self.config.max_duration_ms {
            let current_duration = self.state.current_duration();
            if current_duration >= max_duration {
                println!(
                    "{} Duration limit exceeded: {} ms (max: {} ms)",
                    self.context.name, current_duration, max_duration
                );
                return true;
            }
        }

        false
    }

    async fn split_stream(&mut self, output: &AsyncSender<Result<FlvData, FlvError>>) -> bool {
        info!(
            "{} Splitting stream at size={} bytes, duration={}ms (segment #{})",
            self.context.name,
            self.state.accumulated_size,
            self.state.current_duration(),
            self.state.split_count + 1
        );

        // Helper macro to reduce repetition when sending tags with Arc
        macro_rules! send_item {
            ($item:expr, $transform:expr, $msg:expr) => {
                if let Some(item) = &$item {
                    debug!("{} {}", self.context.name, $msg);
                    let data = $transform(item.clone());
                    if output.send(Ok(data)).await.is_err() {
                        return false;
                    }
                }
            };
        }

        // Send each item with Arc cloning instead of full data cloning
        send_item!(
            self.state.header,
            |h: FlvHeader| FlvData::Header(h),
            "re-emit header after split"
        );
        send_item!(
            self.state.metadata,
            |t: FlvTag| FlvData::Tag(t),
            "re-emit metadata after split"
        );
        send_item!(
            self.state.video_sequence_tag,
            |t: FlvTag| FlvData::Tag(t),
            "re-emit video sequence tag after split"
        );
        send_item!(
            self.state.audio_sequence_tag,
            |t: FlvTag| FlvData::Tag(t),
            "re-emit audio sequence tag after split"
        );

        // Reset accumulated counters for the new segment
        self.state.reset_counters();
        self.last_split_time = Instant::now();

        true
    }
}

impl FlvOperator for LimitOperator {
    fn context(&self) -> &Arc<StreamerContext> {
        &self.context
    }

    async fn process(
        &mut self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) {
        while let Ok(item) = input.recv().await {
            match item {
                Ok(data) => {
                    match &data {
                        FlvData::Header(header) => {
                            // Reset state for a new stream
                            self.state = StreamState::new();
                            self.state.header = Some(header.clone());
                            self.last_split_time = Instant::now();

                            // Forward the header
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
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
                                debug!(
                                    "{} Video sequence header detected, resetting timestamp.",
                                    self.context.name
                                );
                            } else if tag.is_audio_sequence_header() {
                                let mut tag = tag.clone();
                                tag.timestamp_ms = 0; // Reset timestamp for audio sequence header
                                self.state.audio_sequence_tag = Some(tag);
                                debug!(
                                    "{} Audio sequence header detected, resetting timestamp.",
                                    self.context.name
                                );
                            }

                            // Track keyframes for optimal split points
                            if tag.is_key_frame() {
                                self.state.last_keyframe_position =
                                    Some((self.state.accumulated_size, self.state.max_timestamp));
                            }

                            // Check if any limit is exceeded
                            let should_split = self.check_limits();

                            // Inside the process method where split decisions are made
                            if should_split
                                && (!self.config.split_at_keyframes_only || tag.is_key_frame())
                            {
                                // Direct splitting - no retrospective logic
                                let split_reason = self.determine_split_reason();

                                // Report the split with current stats
                                if let Some(callback) = &self.config.on_split {
                                    let duration = self.state.current_duration();
                                    (callback)(split_reason, self.state.accumulated_size, duration);
                                }

                                // Perform the split
                                if !self.split_stream(&output).await {
                                    return;
                                }

                                // Emit current tag after the split if it's a keyframe
                                if tag.is_key_frame() || !self.config.split_at_keyframes_only {
                                    if output.send(Ok(data.clone())).await.is_err() {
                                        return;
                                    }
                                }
                            } else if should_split
                                && self.config.split_at_keyframes_only
                                && self.config.use_retrospective_splitting
                                && self.state.last_keyframe_position.is_some()
                            {
                                // Retrospective splitting logic - only used if enabled
                                let split_reason = self.determine_split_reason();

                                // We're not at a keyframe but need to split at a keyframe
                                // Emit this tag then trigger split
                                if output.send(Ok(data.clone())).await.is_err() {
                                    return;
                                }

                                if let Some((size, timestamp)) = self.state.last_keyframe_position {
                                    // Use the position of the last keyframe for stats
                                    if let Some(callback) = &self.config.on_split {
                                        let duration =
                                            timestamp.saturating_sub(self.state.start_timestamp);
                                        (callback)(split_reason, size, duration);
                                    }
                                }

                                // Perform the split
                                if !self.split_stream(&output).await {
                                    return;
                                }
                            } else {
                                // No split needed, just forward the tag
                                if output.send(Ok(data)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        _ => {
                            // Forward other data types
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    // Forward errors
                    if output.send(Err(e)).await.is_err() {
                        return;
                    }
                }
            }
        }

        debug!("{} completed.", self.context.name);
    }

    fn name(&self) -> &'static str {
        "LimitOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::tag::{FlvTag, FlvTagType};
    use kanal;
    use std::sync::{Arc, Mutex};

    // Helper function to create a test context
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    // Helper function to create a FlvHeader for testing
    fn create_test_header() -> FlvData {
        FlvData::Header(FlvHeader::new(true, true))
    }

    // Helper function to create a video tag for testing
    fn create_video_tag(timestamp: u32, is_keyframe: bool, size: usize) -> FlvData {
        // First byte: 4 bits frame type (1=keyframe, 2=inter), 4 bits codec id (7=AVC)
        let frame_type = if is_keyframe { 1 } else { 2 };
        let first_byte = (frame_type << 4) | 7; // AVC codec

        // Create a data array of the requested size
        let mut data = vec![0u8; size];
        data[0] = first_byte;

        // Add AVC packet type (1 = NALU)
        if data.len() > 1 {
            data[1] = 1;
        }

        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        })
    }

    // Helper function to create a video sequence header
    fn create_video_sequence_header(timestamp: u32) -> FlvData {
        // Format header for AVC sequence header
        let mut data = vec![
            0x17, // frame type 1 (keyframe) + codec id 7 (AVC)
            0x00, // AVC sequence header
            0x00, 0x00,
            0x00, // composition time
                  // ... rest of AVC config data
        ];

        // Pad to some reasonable size
        data.resize(30, 0);

        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        })
    }

    // Helper function to create an audio sequence header
    fn create_audio_sequence_header(timestamp: u32) -> FlvData {
        let data = vec![
            0xAF, // Audio format 10 (AAC) + sample rate 3 (44kHz) + sample size 1 (16-bit) + stereo
            0x00, // AAC sequence header
                  // ... rest of AAC config data
        ];

        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Audio,
            data: Bytes::from(data),
        })
    }

    // Helper function to create metadata tag
    fn create_metadata_tag() -> FlvData {
        // Simple script tag for metadata
        let data = vec![
            0x02, 0x00, 0x0A, b'o', b'n', b'M', b'e', b't', b'a', b'D', b'a', b't', b'a',
        ];

        FlvData::Tag(FlvTag {
            timestamp_ms: 0,
            stream_id: 0,
            tag_type: FlvTagType::ScriptData,
            data: Bytes::from(data),
        })
    }

    #[tokio::test]
    async fn test_size_limit() {
        let context = create_test_context();

        // Track split events
        let split_count = Arc::new(Mutex::new(0));
        let split_count_clone = Arc::clone(&split_count);

        // Configure with a 1000 byte size limit
        let config = LimitConfig {
            max_size_bytes: Some(1000),
            max_duration_ms: None,
            split_at_keyframes_only: false, // For simplicity in testing
            on_split: Some(Box::new(move |reason, size, _| {
                assert_eq!(reason, SplitReason::SizeLimit);
                assert!(size >= 1000, "Size should be at least 1000 bytes at split");
                *split_count_clone.lock().unwrap() += 1;
            })),
            use_retrospective_splitting: false, // Explicitly disable (matches new default)
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a stream with sequence headers
        input_tx.send(Ok(create_test_header())).await.unwrap();
        input_tx.send(Ok(create_metadata_tag())).await.unwrap();
        input_tx
            .send(Ok(create_video_sequence_header(0)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_audio_sequence_header(0)))
            .await
            .unwrap();

        // Send enough data to trigger multiple splits
        for i in 0..10 {
            // Each tag is around 300 bytes, so every ~3 tags should trigger a split
            let tag = create_video_tag(i * 100, i % 3 == 0, 300);
            input_tx.send(Ok(tag)).await.unwrap();
        }

        // Close the input
        drop(input_tx);

        // Collect the output
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // We should have had multiple splits
        let final_split_count = *split_count.lock().unwrap();
        assert!(
            final_split_count >= 2,
            "Expected at least 2 splits, got {}",
            final_split_count
        );

        // There should be multiple headers in the output
        let header_count = received_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        assert!(
            header_count > 1,
            "Expected multiple headers from splits, got {}",
            header_count
        );
    }

    #[tokio::test]
    async fn test_duration_limit() {
        let context = create_test_context();

        // Track split events
        let split_count = Arc::new(Mutex::new(0));
        let split_count_clone = Arc::clone(&split_count);

        // Configure with a 500ms duration limit
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: Some(500),
            split_at_keyframes_only: false, // For simplicity in testing
            on_split: Some(Box::new(move |reason, _, duration| {
                assert_eq!(reason, SplitReason::DurationLimit);
                assert!(
                    duration >= 500,
                    "Duration should be at least 500ms at split"
                );
                *split_count_clone.lock().unwrap() += 1;
            })),
            use_retrospective_splitting: false, // Explicitly disable (matches new default)
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a stream with sequence headers
        input_tx.send(Ok(create_test_header())).await.unwrap();
        input_tx.send(Ok(create_metadata_tag())).await.unwrap();
        input_tx
            .send(Ok(create_video_sequence_header(0)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_audio_sequence_header(0)))
            .await
            .unwrap();

        // Send tags with increasing timestamps to trigger duration splits
        for i in 0..10 {
            // Timestamp increases by 200ms each tag
            let timestamp = i * 200;
            let tag = create_video_tag(timestamp, i % 2 == 0, 100);
            input_tx.send(Ok(tag)).await.unwrap();
        }

        // Close the input
        drop(input_tx);

        // Collect the output
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // We should have had multiple splits based on duration
        let final_split_count = *split_count.lock().unwrap();
        assert!(
            final_split_count >= 3,
            "Expected at least 3 duration splits, got {}",
            final_split_count
        );
    }

    #[tokio::test]
    async fn test_split_at_keyframes_only() {
        let context = create_test_context();

        // Track the timestamps at which splits occur
        let split_timestamps = Arc::new(Mutex::new(Vec::new()));
        let split_timestamps_clone = Arc::clone(&split_timestamps);

        // Configure with a 300ms duration limit but only split at keyframes
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: Some(300),
            split_at_keyframes_only: true,
            on_split: Some(Box::new(move |reason, _, duration| {
                println!("Split reason: {:?}, duration: {}ms", reason, duration);
                split_timestamps_clone.lock().unwrap().push(duration);
            })),
            use_retrospective_splitting: false, // Explicitly disable
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a stream with sequence headers
        input_tx
            .send(Ok(create_video_tag(0, true, 100)))
            .await
            .unwrap(); // Keyframe at 0
        input_tx
            .send(Ok(create_video_tag(100, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(200, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(300, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(400, true, 100)))
            .await
            .unwrap(); // Keyframe at 400 - should split here
        input_tx
            .send(Ok(create_video_tag(500, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(600, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(700, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(800, true, 100)))
            .await
            .unwrap(); // Keyframe at 800 - should split here
        input_tx
            .send(Ok(create_video_tag(900, false, 100)))
            .await
            .unwrap();

        // Close the input
        drop(input_tx);

        // Collect the output
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // Check split timestamps - should be at keyframes after limit is reached
        let timestamps = split_timestamps.lock().unwrap().clone();
        assert_eq!(timestamps.len(), 2, "Expected 2 splits at keyframes");

        // First split should be at or after 300ms
        assert!(
            timestamps[0] >= 300,
            "First split should be at or after 300ms"
        );
    }

    #[tokio::test]
    async fn test_retrospective_splitting() {
        let context = create_test_context();

        // Track split events and positions
        let split_positions = Arc::new(Mutex::new(Vec::new()));
        let split_positions_clone = Arc::clone(&split_positions);

        // Configure with retrospective splitting enabled
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: Some(300),
            split_at_keyframes_only: true,
            on_split: Some(Box::new(move |_, size, duration| {
                split_positions_clone.lock().unwrap().push((size, duration));
            })),
            use_retrospective_splitting: true, // Explicitly enable retrospective splitting
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a stream with sequence headers
        input_tx.send(Ok(create_test_header())).await.unwrap();
        input_tx.send(Ok(create_metadata_tag())).await.unwrap();
        input_tx
            .send(Ok(create_video_sequence_header(0)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_audio_sequence_header(0)))
            .await
            .unwrap();

        // Send a keyframe at 0ms
        input_tx
            .send(Ok(create_video_tag(0, true, 100)))
            .await
            .unwrap();

        // Send non-keyframes at 100ms, 200ms, 300ms (this will exceed limit)
        input_tx
            .send(Ok(create_video_tag(100, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(200, false, 100)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(350, false, 100)))
            .await
            .unwrap(); // Exceeds 300ms limit

        // Send another keyframe at 400ms
        input_tx
            .send(Ok(create_video_tag(400, true, 100)))
            .await
            .unwrap();

        // Close the input
        drop(input_tx);

        // Collect the output
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // With retrospective splitting enabled, should have split at the last keyframe position
        let positions = split_positions.lock().unwrap().clone();
        assert!(!positions.is_empty(), "Expected at least one split");

        // The split should have happened at the keyframe at 0ms, not at the point where
        // the limit was exceeded (at 350ms)
        if let Some((size, duration)) = positions.first() {
            assert!(
                *duration < 300,
                "With retrospective splitting, duration should be at the last keyframe before limit"
            );
        }
    }

    #[tokio::test]
    async fn test_sequential_splits() {
        let context = create_test_context();

        // Track split count
        let split_count = Arc::new(Mutex::new(0));
        let split_count_clone = Arc::clone(&split_count);

        // Configure with both size and duration limits
        let config = LimitConfig {
            max_size_bytes: Some(500),
            max_duration_ms: Some(300),
            split_at_keyframes_only: false,
            on_split: Some(Box::new(move |_, _, _| {
                *split_count_clone.lock().unwrap() += 1;
            })),
            use_retrospective_splitting: false,
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a stream with sequence headers
        input_tx.send(Ok(create_test_header())).await.unwrap();
        input_tx.send(Ok(create_metadata_tag())).await.unwrap();
        input_tx
            .send(Ok(create_video_sequence_header(0)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_audio_sequence_header(0)))
            .await
            .unwrap();

        // First send tags that should trigger size limit
        for i in 0..3 {
            input_tx
                .send(Ok(create_video_tag(i * 50, i % 2 == 0, 200)))
                .await
                .unwrap();
        }

        // Then send tags that should trigger duration limit
        for i in 0..4 {
            input_tx
                .send(Ok(create_video_tag(i * 100 + 300, i % 2 == 0, 50)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Collect output
        while let Ok(_) = output_rx.recv().await {}

        // Check split count
        let final_split_count = *split_count.lock().unwrap();
        assert!(
            final_split_count >= 2,
            "Expected at least 2 splits (one for size, one for duration), got {}",
            final_split_count
        );
    }

    #[tokio::test]
    async fn test_split_with_interleaved_audio_video() {
        let context = create_test_context();

        // Track split timestamps
        let split_timestamps = Arc::new(Mutex::new(Vec::new()));
        let split_timestamps_clone = Arc::clone(&split_timestamps);

        // Configure with duration limit
        let config = LimitConfig {
            max_size_bytes: None,
            max_duration_ms: Some(400),
            split_at_keyframes_only: true,
            on_split: Some(Box::new(move |_, _, duration| {
                split_timestamps_clone.lock().unwrap().push(duration);
            })),
            use_retrospective_splitting: false,
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Helper function to create audio tag
        let create_audio_tag = |timestamp: u32, size: usize| -> FlvData {
            let data = vec![0xAF, 0x01]; // AAC raw data

            FlvData::Tag(FlvTag {
                timestamp_ms: timestamp,
                stream_id: 0,
                tag_type: FlvTagType::Audio,
                data: Bytes::from(data),
            })
        };

        // Send initial headers
        input_tx.send(Ok(create_test_header())).await.unwrap();
        input_tx.send(Ok(create_metadata_tag())).await.unwrap();
        input_tx
            .send(Ok(create_video_sequence_header(0)))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_audio_sequence_header(0)))
            .await
            .unwrap();

        // Interleaved audio and video with keyframes at 0ms and 500ms
        input_tx
            .send(Ok(create_video_tag(0, true, 100)))
            .await
            .unwrap(); // Keyframe
        input_tx.send(Ok(create_audio_tag(50, 20))).await.unwrap();
        input_tx.send(Ok(create_audio_tag(100, 20))).await.unwrap();
        input_tx
            .send(Ok(create_video_tag(150, false, 100)))
            .await
            .unwrap(); // Non-keyframe
        input_tx.send(Ok(create_audio_tag(200, 20))).await.unwrap();
        input_tx.send(Ok(create_audio_tag(250, 20))).await.unwrap();
        input_tx
            .send(Ok(create_video_tag(300, false, 100)))
            .await
            .unwrap(); // Non-keyframe
        input_tx.send(Ok(create_audio_tag(350, 20))).await.unwrap();
        input_tx.send(Ok(create_audio_tag(400, 20))).await.unwrap(); // Should exceed duration limit (400ms)
        input_tx
            .send(Ok(create_video_tag(450, false, 100)))
            .await
            .unwrap(); // Non-keyframe
        input_tx
            .send(Ok(create_video_tag(500, true, 100)))
            .await
            .unwrap(); // Keyframe - split should happen here

        // Close the input
        drop(input_tx);

        // Collect output
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // Check that split happened at the keyframe
        let timestamps = split_timestamps.lock().unwrap().clone();
        assert_eq!(timestamps.len(), 1, "Expected 1 split at the keyframe");
        assert!(
            timestamps[0] >= 400,
            "Split should occur at or after duration limit was reached"
        );

        // Count headers in output to verify split happened
        let header_count = received_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();
        assert_eq!(
            header_count, 2,
            "Expected original header plus one from split"
        );
    }

    #[tokio::test]
    async fn test_empty_stream_no_splits() {
        let context = create_test_context();

        // Track split events
        let split_count = Arc::new(Mutex::new(0));
        let split_count_clone = Arc::clone(&split_count);

        // Configure with size limit
        let config = LimitConfig {
            max_size_bytes: Some(1000),
            max_duration_ms: None,
            split_at_keyframes_only: false,
            on_split: Some(Box::new(move |_, _, _| {
                *split_count_clone.lock().unwrap() += 1;
            })),
            use_retrospective_splitting: false,
        };

        let mut operator = LimitOperator::with_config(context, config);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Process in background
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send just a header with no actual content
        input_tx.send(Ok(create_test_header())).await.unwrap();

        // Close the input immediately
        drop(input_tx);

        // Collect output
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // Verify no splits occurred
        let final_split_count = *split_count.lock().unwrap();
        assert_eq!(
            final_split_count, 0,
            "No splits should occur in empty stream"
        );

        // Verify we got just the header back
        assert_eq!(
            received_items.len(),
            1,
            "Expected only the header in output"
        );
        assert!(
            matches!(received_items[0], FlvData::Header(_)),
            "Expected only a header in the output"
        );
    }
}
