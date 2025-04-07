//! # SplitOperator
//!
//! The `SplitOperator` processes FLV (Flash Video) streams and manages stream splitting
//! when video or audio parameters change.
//!
//! ## Purpose
//!
//! Media streams sometimes change encoding parameters mid-stream (resolution, bitrate,
//! codec settings). These changes require re-initialization of decoders, which many
//! players handle poorly. This operator detects such changes and helps maintain
//! proper playback by:
//!
//! 1. Monitoring audio and video sequence headers for parameter changes
//! 2. Re-injecting stream initialization data (headers, metadata) when changes occur
//! 3. Ensuring players can properly handle parameter transitions
//!
//! ## Operation
//!
//! The operator:
//! - Tracks FLV headers, metadata tags, and sequence headers
//! - Calculates CRC32 checksums of sequence headers to detect changes
//! - When changes are detected, marks the stream for splitting
//! - At the next regular media tag, re-injects headers and sequence information
//!
//! ## Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use kanal;
//! use crate::context::StreamerContext;
//! use crate::operators::split::SplitOperator;
//!
//! async fn example() {
//!     let context = Arc::new(StreamerContext::default());
//!     let mut operator = SplitOperator::new(context);
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
use flv::data::FlvData;
use flv::error::FlvError;
use flv::header::FlvHeader;
use flv::tag::{FlvTag, FlvUtil};
use kanal;
use kanal::{AsyncReceiver, AsyncSender};
use std::sync::Arc;
use tracing::{debug, info};

// Store data wrapped in Arc for efficient cloning
struct StreamState {
    header: Option<FlvHeader>,
    metadata: Option<FlvTag>,
    audio_sequence_tag: Option<FlvTag>,
    video_sequence_tag: Option<FlvTag>,
    video_crc: Option<u32>,
    audio_crc: Option<u32>,
    changed: bool,
}

impl StreamState {
    fn new() -> Self {
        Self {
            header: None,
            metadata: None,
            audio_sequence_tag: None,
            video_sequence_tag: None,
            video_crc: None,
            audio_crc: None,
            changed: false,
        }
    }

    fn reset(&mut self) {
        self.header = None;
        self.metadata = None;
        self.audio_sequence_tag = None;
        self.video_sequence_tag = None;
        self.video_crc = None;
        self.audio_crc = None;
        self.changed = false;
    }
}

pub struct SplitOperator {
    context: Arc<StreamerContext>,
}

impl SplitOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self { context }
    }

    /// Calculate CRC32 for a byte slice using crc32fast
    fn calculate_crc32(data: &[u8]) -> u32 {
        crc32fast::hash(data)
    }
}

impl FlvOperator for SplitOperator {
    fn context(&self) -> &Arc<StreamerContext> {
        &self.context
    }

    async fn process(
        &mut self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) {
        let mut state = StreamState::new();

        // Send header and cached tags to rebuild stream after split
        async fn insert_header_and_tags(
            context: &StreamerContext,
            output: &kanal::AsyncSender<Result<FlvData, FlvError>>,
            state: &StreamState,
        ) -> bool {
            // Helper macro to reduce repetition when sending tags
            macro_rules! send_item {
                ($item:expr, $transform:expr, $msg:expr) => {
                    if let Some(item) = &$item {
                        debug!("{} {}", context.name, $msg);
                        let data = $transform(item.clone());
                        if output.send(Ok(data)).await.is_err() {
                            return false;
                        }
                    }
                };
            }

            // Re-inject header and sequence tags
            send_item!(
                state.header,
                |h: FlvHeader| FlvData::Header(h),
                "re-emit header"
            );
            send_item!(
                state.metadata,
                |t: FlvTag| FlvData::Tag(t),
                "re-emit metadata"
            );
            send_item!(
                state.video_sequence_tag,
                |t: FlvTag| FlvData::Tag(t),
                "re-emit video sequence tag"
            );
            send_item!(
                state.audio_sequence_tag,
                |t: FlvTag| FlvData::Tag(t),
                "re-emit audio sequence tag"
            );

            true
        }

        // Split stream and reinject header+sequence data
        async fn split_stream(
            context: &StreamerContext,
            output: &kanal::AsyncSender<Result<FlvData, FlvError>>,
            state: &mut StreamState,
        ) -> bool {
            debug!("{} Splitting stream...", context.name);

            // Note on timestamp handling:
            // When we split the stream, we re-inject the header and sequence information
            // using the original timestamps from when they were first encountered.
            // This maintains timestamp consistency within the stream segments
            // but does not reset the timeline. Downstream components or players
            // may need to handle potential timestamp discontinuities at split points.

            let result = insert_header_and_tags(context, output, state).await;

            if result {
                state.changed = false;
                info!("{} Stream split", context.name);
            }
            result
        }

        // Main processing loop
        while let Ok(item) = input.recv().await {
            match item {
                Ok(data) => {
                    match &data {
                        FlvData::Header(header) => {
                            // Reset state when a new header is encountered
                            state.reset();
                            // Store header in Arc
                            state.header = Some(header.clone());
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
                            continue;
                        }
                        FlvData::Tag(tag) => {
                            // Process different tag types
                            if tag.is_script_tag() {
                                debug!("{} Metadata detected", self.context.name);
                                state.metadata = Some(tag.clone());
                            } else if tag.is_video_sequence_header() {
                                debug!("{} Video sequence tag detected", self.context.name);

                                // Calculate CRC for comparison
                                let crc = Self::calculate_crc32(&tag.data);

                                // Compare with cached CRC if available
                                if let Some(prev_crc) = state.video_crc {
                                    if prev_crc != crc {
                                        info!(
                                            "{} Video sequence header changed (CRC: {:x} -> {:x}), marking for split",
                                            self.context.name, prev_crc, crc
                                        );
                                        state.changed = true;
                                    }
                                }

                                // Update sequence tag
                                state.video_sequence_tag = Some(tag.clone());
                                state.video_crc = Some(crc);
                            } else if tag.is_audio_sequence_header() {
                                debug!("{} Audio sequence tag detected", self.context.name);

                                // Calculate CRC for comparison
                                let crc = Self::calculate_crc32(&tag.data);

                                // Compare with cached CRC if available
                                if let Some(prev_crc) = state.audio_crc {
                                    if prev_crc != crc {
                                        info!(
                                            "{} Audio parameters changed: {:x} -> {:x}",
                                            self.context.name, prev_crc, crc
                                        );
                                        state.changed = true;
                                    }
                                }

                                // Update sequence tag
                                state.audio_sequence_tag = Some(tag.clone());
                                state.audio_crc = Some(crc);
                            } else if state.changed {
                                // If parameters have changed and this is a regular tag,
                                // it's time to split the stream
                                if !split_stream(&self.context, &output, &mut state).await {
                                    return;
                                }
                            }

                            // Forward the tag
                            if output.send(Ok(data)).await.is_err() {
                                return;
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
                    // Forward error
                    if output.send(Err(e)).await.is_err() {
                        return;
                    }
                }
            }
        }

        debug!("{} completed.", self.context.name);
        state.reset(); // Reset state when processing is done
    }

    fn name(&self) -> &'static str {
        "SplitOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::tag::{FlvTag, FlvTagType};
    use kanal;

    // Helper function to create a test context
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    // Helper function to create a FlvHeader for testing
    fn create_test_header() -> FlvData {
        FlvData::Header(FlvHeader::new(true, true))
    }

    // Helper function to create a basic tag for testing
    fn create_basic_tag(tag_type: FlvTagType, timestamp: u32) -> FlvData {
        let data = vec![0x17, 0x01, 0x00, 0x00, 0x00]; // Sample tag data
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type,
            data: Bytes::from(data),
        })
    }

    // Helper function to create a video sequence header tag
    fn create_video_sequence_tag(
        timestamp: u32,
        sps_content: &[u8],
        pps_content: &[u8],
    ) -> FlvData {
        // Format:
        // 1 byte: first 4 bits = frame type (1 for keyframe), last 4 bits = codec id (7 for AVC)
        // 1 byte: AVC packet type (0 for sequence header)
        // 3 bytes: composition time
        // 1 byte: version
        // ... SPS/PPS data with length prefixes

        let mut data = vec![
            0x17, // frame type 1 (keyframe) + codec id 7 (AVC)
            0x00, // AVC sequence header
            0x00,
            0x00,
            0x00, // composition time
            0x01, // version
            // SPS fields
            0x64,
            0x00,
            0x1F,
            0xFF, // SPS parameter set stuff
            0xE1, // 1 SPS
            ((sps_content.len() >> 8) & 0xFF) as u8,
            (sps_content.len() & 0xFF) as u8, // SPS length
        ];

        // Add SPS content
        data.extend_from_slice(sps_content);

        // Add number of PPS
        data.push(0x01); // 1 PPS

        // Add PPS length
        data.push(((pps_content.len() >> 8) & 0xFF) as u8);
        data.push((pps_content.len() & 0xFF) as u8);

        // Add PPS content
        data.extend_from_slice(pps_content);

        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        })
    }

    // Helper function to create an audio sequence header tag
    fn create_audio_sequence_tag(timestamp: u32, content: &[u8]) -> FlvData {
        // Format:
        // 1 byte: first 4 bits = audio format (10 for AAC), + other audio settings
        // 1 byte: AAC packet type (0 for sequence header)
        // ... AAC specific config

        let mut data = vec![
            0xAF, // Audio format 10 (AAC) + sample rate 3 (44kHz) + sample size 1 (16-bit) + stereo
            0x00, // AAC sequence header
            0x12, 0x10, // AAC specific config
        ];

        // Add sequence specific config
        data.extend_from_slice(content);

        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Audio,
            data: Bytes::from(data),
        })
    }

    #[tokio::test]
    async fn test_normal_flow_no_changes() {
        let context = create_test_context();
        let mut operator = SplitOperator::new(context);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Start the process in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a simple stream with no parameter changes
        input_tx.send(Ok(create_test_header())).await.unwrap();

        // Send video sequence header
        let sps = [0x67, 0x42, 0x00, 0x2A, 0x96, 0x35, 0x40]; // Sample SPS
        let pps = [0x68, 0xCE, 0x38, 0x80]; // Sample PPS
        input_tx
            .send(Ok(create_video_sequence_tag(0, &sps, &pps)))
            .await
            .unwrap();

        // Send audio sequence header
        let aac_config = [0x12, 0x10]; // Sample AAC config
        input_tx
            .send(Ok(create_audio_sequence_tag(0, &aac_config)))
            .await
            .unwrap();

        // Send regular tags
        for i in 0..5 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Should receive all 8 items without any extra insertions
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        assert_eq!(received_items.len(), 8);
    }

    #[tokio::test]
    async fn test_video_parameter_change() {
        let context = create_test_context();
        let mut operator = SplitOperator::new(context);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Start the process in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send initial stream setup
        input_tx.send(Ok(create_test_header())).await.unwrap();

        // First video sequence header
        let sps1 = [0x67, 0x42, 0x00, 0x2A, 0x96, 0x35, 0x40]; // Sample SPS
        let pps1 = [0x68, 0xCE, 0x38, 0x80]; // Sample PPS
        input_tx
            .send(Ok(create_video_sequence_tag(0, &sps1, &pps1)))
            .await
            .unwrap();

        // Audio sequence header
        let aac_config1 = [0x12, 0x10]; // Sample AAC config
        input_tx
            .send(Ok(create_audio_sequence_tag(0, &aac_config1)))
            .await
            .unwrap();

        // Send some regular tags
        for i in 0..3 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i + 1)))
                .await
                .unwrap();
        }

        // Send a new video sequence header with different parameters
        let sps2 = [0x67, 0x42, 0x00, 0x2A, 0x96, 0x35, 0x50]; // Changed SPS
        let pps2 = [0x68, 0xCE, 0x38, 0x80]; // Same PPS
        input_tx
            .send(Ok(create_video_sequence_tag(100, &sps2, &pps2)))
            .await
            .unwrap();

        // Send more regular tags - this should trigger stream split
        for i in 0..3 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i + 100)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Collect all received items
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // We should have:
        // 1. Original header
        // 2. First video sequence header
        // 3. Audio sequence header
        // 4-6. Three regular video tags
        // 7. New video sequence header
        // 8-11. Re-inserted header and metadata (header, video seq, audio seq)
        // 11-13. Remaining regular tags
        // Total: 13 items

        // Print each tag with its position and type
        for (i, item) in received_items.iter().enumerate() {
            match item {
                FlvData::Header(_) => println!("Item {}: Header", i),
                FlvData::Tag(tag) => println!(
                    "Item {}: Tag type {:?}, timestamp: {}ms",
                    i, tag.tag_type, tag.timestamp_ms
                ),
                FlvData::EndOfSequence(_) => println!("Item {}: End of sequence", i),
            }
        }

        assert_eq!(
            received_items.len(),
            13,
            "Expected stream split to add additional items"
        );
    }

    #[tokio::test]
    async fn test_audio_parameter_change() {
        let context = create_test_context();
        let mut operator = SplitOperator::new(context);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Start the process in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send initial stream setup
        input_tx.send(Ok(create_test_header())).await.unwrap();

        // Video sequence header
        let sps = [0x67, 0x42, 0x00, 0x2A, 0x96, 0x35, 0x40]; // Sample SPS
        let pps = [0x68, 0xCE, 0x38, 0x80]; // Sample PPS
        input_tx
            .send(Ok(create_video_sequence_tag(0, &sps, &pps)))
            .await
            .unwrap();

        // First audio sequence header
        let aac_config1 = [0x12, 0x10]; // Sample AAC config
        input_tx
            .send(Ok(create_audio_sequence_tag(0, &aac_config1)))
            .await
            .unwrap();

        // Send some regular tags
        for i in 0..3 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i)))
                .await
                .unwrap();
        }

        // Send a new audio sequence header with different parameters
        let aac_config2 = [0x13, 0x90]; // Changed AAC config
        input_tx
            .send(Ok(create_audio_sequence_tag(100, &aac_config2)))
            .await
            .unwrap();

        // Send more regular tags - this should trigger stream split
        for i in 0..3 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i + 100)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Collect all received items
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // We should have more than the original number of items due to the stream split
        assert!(
            received_items.len() > 10,
            "Expected stream split to add additional items"
        );
    }

    // Helper function to create regular audio tag
    fn create_regular_audio_tag(timestamp: u32) -> FlvData {
        let data = vec![0xAF, 0x01, 0x21, 0x10, 0x04]; // Sample AAC audio frame
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Audio,
            data: Bytes::from(data),
        })
    }

    #[tokio::test]
    async fn test_interleaved_parameter_changes() {
        let context = create_test_context();
        let mut operator = SplitOperator::new(context);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, mut output_rx) = kanal::bounded_async(32);

        // Start the operator in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_test_header())).await.unwrap();

        // Send initial codec headers
        let sps1 = [0x67, 0x42, 0x00, 0x2A, 0x96, 0x35, 0x40];
        let pps1 = [0x68, 0xCE, 0x38, 0x80];
        input_tx
            .send(Ok(create_video_sequence_tag(0, &sps1, &pps1)))
            .await
            .unwrap();

        let aac_config1 = [0x12, 0x10];
        input_tx
            .send(Ok(create_audio_sequence_tag(0, &aac_config1)))
            .await
            .unwrap();

        // Send some regular content
        for i in 1..5 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i * 33)))
                .await
                .unwrap();
            input_tx
                .send(Ok(create_regular_audio_tag(i * 33 + 5)))
                .await
                .unwrap();
        }

        // Edge case: send video sequence change immediately followed by audio sequence change
        // This tests handling of multiple parameter changes without regular tags in between
        let sps2 = [0x67, 0x42, 0x00, 0x2B, 0x96, 0x35, 0x41];
        let pps2 = [0x68, 0xCE, 0x38, 0x81];
        input_tx
            .send(Ok(create_video_sequence_tag(200, &sps2, &pps2)))
            .await
            .unwrap();

        // Send audio parameter change immediately after
        let aac_config2 = [0x13, 0x90];
        input_tx
            .send(Ok(create_audio_sequence_tag(200, &aac_config2)))
            .await
            .unwrap();

        // Send more regular content
        for i in 6..10 {
            input_tx
                .send(Ok(create_basic_tag(FlvTagType::Video, i * 33)))
                .await
                .unwrap();
            input_tx
                .send(Ok(create_regular_audio_tag(i * 33 + 5)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Collect all received items
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
        }

        // Look for split points in the output
        let mut split_detected = false;
        let mut stream_contents = Vec::new();

        // Log the content for analysis
        for (i, item) in received_items.iter().enumerate() {
            match item {
                FlvData::Header(_) => {
                    stream_contents.push(format!("Item {}: Header", i));
                }
                FlvData::Tag(tag) => {
                    if tag.is_video_sequence_header() {
                        stream_contents.push(format!(
                            "Item {}: Video Sequence Header ({}ms)",
                            i, tag.timestamp_ms
                        ));
                    } else if tag.is_audio_sequence_header() {
                        stream_contents.push(format!(
                            "Item {}: Audio Sequence Header ({}ms)",
                            i, tag.timestamp_ms
                        ));
                    } else {
                        stream_contents.push(format!(
                            "Item {}: {:?} ({}ms)",
                            i, tag.tag_type, tag.timestamp_ms
                        ));
                    }
                }
                _ => {}
            }

            // Detect split pattern: header followed by sequence headers
            if i >= 2
                && matches!(received_items[i - 2], FlvData::Header(_))
                && matches!(received_items[i-1], FlvData::Tag(ref t) if t.is_video_sequence_header() || t.is_audio_sequence_header())
                && matches!(received_items[i], FlvData::Tag(ref t) if t.is_video_sequence_header() || t.is_audio_sequence_header())
            {
                split_detected = true;
            }
        }

        // Print the stream for analysis
        for line in &stream_contents {
            println!("{}", line);
        }

        // The operator should have triggered a split and re-injected headers
        assert!(
            split_detected,
            "Expected to detect at least one stream split"
        );

        // Verify presence of both video and audio parameter changes
        let video_changes = received_items
            .iter()
            .filter_map(|item| match item {
                FlvData::Tag(tag) if tag.is_video_sequence_header() => Some(tag.timestamp_ms),
                _ => None,
            })
            .collect::<Vec<_>>();

        let audio_changes = received_items
            .iter()
            .filter_map(|item| match item {
                FlvData::Tag(tag) if tag.is_audio_sequence_header() => Some(tag.timestamp_ms),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            video_changes.len() > 1,
            "Expected multiple video sequence headers"
        );
        assert!(
            audio_changes.len() > 1,
            "Expected multiple audio sequence headers"
        );

        println!("Video sequence headers at timestamps: {:?}", video_changes);
        println!("Audio sequence headers at timestamps: {:?}", audio_changes);
    }
}
