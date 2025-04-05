//! # GOP Sorting Operator
//!
//! This operator sorts FLV tags to ensure proper ordering based on timestamp and type.
//! Implementation matches the Kotlin version's logic for consistent behavior across platforms.
//!
//! ## Features
//!
//! - Buffers tags until a keyframe is encountered
//! - Handles sequence headers specially for small buffers
//! - Partitions and sorts tags by type for larger buffers
//! - Maintains proper interleaving of audio and video tags
//! - Preserves script tags in their original order
//!
//! ## Algorithm
//!
//! 1. Buffer tags until a keyframe is encountered
//! 2. For small buffers with sequence headers, emit them directly
//! 3. For larger buffers, partition by type and sort
//! 4. Interleave audio and video tags based on timestamp
//! 5. Emit in the correct order to ensure proper playback
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
use flv::tag::{FlvTag, FlvTagType, FlvUtil};
use kanal::{AsyncReceiver, AsyncSender};
use std::sync::Arc;
use tracing::{debug, info, trace};

/// Threshold for special handling of small tag buffers
const TAGS_BUFFER_SIZE: usize = 10;

/// GOP sorting operator that follows the Kotlin implementation's logic
pub struct GopSortOperator {
    context: Arc<StreamerContext>,
    gop_tags: Vec<FlvTag>,
}

impl GopSortOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            gop_tags: Vec::new(),
        }
    }

    /// Process buffered tags and emit them in properly sorted order
    /// This follows the Kotlin implementation's sorting logic
    async fn push_tags(&mut self, output: &AsyncSender<Result<FlvData, FlvError>>) -> bool {
        if self.gop_tags.is_empty() {
            return true;
        }

        trace!("{} GOP tags: {}", self.context.name, self.gop_tags.len());

        // Special handling for small buffers with sequence headers
        if self.gop_tags.len() < TAGS_BUFFER_SIZE {
            let avc_header_pos = self
                .gop_tags
                .iter()
                .position(|tag| tag.is_video_sequence_header());
            let aac_header_pos = self
                .gop_tags
                .iter()
                .position(|tag| tag.is_audio_sequence_header());

            // If we have both sequence headers, emit them directly
            if let (Some(avc_pos), Some(aac_pos)) = (avc_header_pos, aac_header_pos) {
                // Find first script tag
                let script_pos = self.gop_tags.iter().position(|tag| tag.is_script_tag());

                debug!(
                    "{} AVC header position: {:?}, AAC header position: {:?}",
                    self.context.name, avc_header_pos, aac_header_pos
                );

                // Emit script tag first if present
                if let Some(script_pos) = script_pos {
                    let script_tag = self.gop_tags.remove(script_pos);
                    if output.send(Ok(FlvData::Tag(script_tag))).await.is_err() {
                        return false;
                    }
                }

                // Adjust indices for video header after possible script tag removal
                let avc_idx = if script_pos.is_some() && avc_pos > script_pos.unwrap() {
                    avc_pos - 1
                } else {
                    avc_pos
                };

                // Emit video sequence header
                let avc_tag = self.gop_tags.remove(avc_idx);
                if output.send(Ok(FlvData::Tag(avc_tag))).await.is_err() {
                    return false;
                }

                // Adjust indices for audio header after script and video header removal
                let aac_idx = if script_pos.is_some() && aac_pos > script_pos.unwrap() {
                    aac_pos - 1
                } else {
                    aac_pos
                };
                let aac_idx = if avc_idx < aac_idx {
                    aac_idx - 1
                } else {
                    aac_idx
                };

                // Emit audio sequence header
                let aac_tag = self.gop_tags.remove(aac_idx);
                debug!(
                    "{} Emitting audio sequence header: {:?}",
                    self.context.name, aac_tag
                );
                if output.send(Ok(FlvData::Tag(aac_tag))).await.is_err() {
                    return false;
                }

                // Clear the buffer since we've handled the important tags
                self.gop_tags.clear();
                return true;
            }
        }

        // Partition tags by type (script, video, audio)
        let mut script_tags = Vec::new();
        let mut video_tags = Vec::new();
        let mut audio_tags = Vec::new();

        for tag in std::mem::take(&mut self.gop_tags) {
            if tag.is_script_tag() {
                script_tags.push(tag);
            } else if tag.is_video_tag() {
                video_tags.push(tag);
            } else if tag.is_audio_tag() {
                audio_tags.push(tag);
            }
        }

        // Apply the special interleaving algorithm from Kotlin implementation
        // where audio tags are inserted after video tags with timestamps >= the video timestamp

        // Reverse video tags since we'll process from highest to lowest timestamp
        video_tags.reverse();

        // Sorted output buffer
        let mut sorted_tags = Vec::new();
        let mut audio_idx = audio_tags.len() as i32 - 1;

        // Process video tags and interleave audio
        for video_tag in &video_tags {
            // Add video tag to start of sorted list (building in reverse)
            sorted_tags.insert(0, video_tag.clone());

            // Add audio tags with timestamp >= video tag timestamp
            while audio_idx >= 0
                && audio_tags[audio_idx as usize].timestamp_ms >= video_tag.timestamp_ms
            {
                sorted_tags.insert(1, audio_tags[audio_idx as usize].clone());
                audio_idx -= 1;
            }
        }

        // add any remaining audio tags at the beginning
        while audio_idx >= 0 {
            sorted_tags.insert(0, audio_tags[audio_idx as usize].clone());
            audio_idx -= 1;
        }

        // Emit script tags in original order (no sorting)
        for tag in script_tags {
            if output.send(Ok(FlvData::Tag(tag))).await.is_err() {
                return false;
            }
        }

        // Emit the sorted tags
        for tag in sorted_tags {
            if output.send(Ok(FlvData::Tag(tag))).await.is_err() {
                return false;
            }
        }

        true
    }
}

impl FlvOperator for GopSortOperator {
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
                        FlvData::Header(_) | FlvData::EndOfSequence(_) => {
                            // Process any buffered tags first
                            if !self.push_tags(&output).await {
                                return;
                            }

                            // Forward the header or EOS
                            // do not send end of stream to output
                            if data.is_end_of_sequence() {
                                debug!("{} End of stream...", self.context.name);
                                return;
                            }

                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }

                            debug!("{} Reset GOP tags...", self.context.name);
                        }
                        FlvData::Tag(tag) => {
                            let tag = tag.clone();

                            // Check for a nalu keyframe
                            if tag.is_key_frame_nalu() {
                                // On keyframe, process buffered tags
                                if !self.push_tags(&output).await {
                                    return;
                                }
                                // Start the new buffer with this keyframe
                                self.gop_tags.push(tag);
                            } else {
                                // Just add non-keyframe to buffer
                                self.gop_tags.push(tag);
                            }
                        }
                    }
                }
                Err(e) => {
                    // Push any buffered tags before forwarding the error
                    self.push_tags(&output).await;

                    // Forward the error
                    if output.send(Err(e)).await.is_err() {
                        return;
                    }
                }
            }
        }

        // Process any remaining buffered tags at end of stream
        self.push_tags(&output).await;

        info!("{} GOP sort completed", self.context.name);
    }

    fn name(&self) -> &'static str {
        "GopSortOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::header::FlvHeader;
    use kanal::bounded_async;

    // Helper functions for testing
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    fn create_header() -> FlvData {
        FlvData::Header(FlvHeader::new(true, true))
    }

    fn create_tag(tag_type: FlvTagType, timestamp: u32, data: Vec<u8>) -> FlvData {
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type,
            data: Bytes::from(data),
        })
    }

    fn create_video_tag(timestamp: u32, is_keyframe: bool) -> FlvData {
        // First byte: 4 bits frame type (1=keyframe, 2=inter), 4 bits codec id (7=AVC)
        let frame_type = if is_keyframe { 1 } else { 2 };
        let first_byte = (frame_type << 4) | 7; // AVC codec
        create_tag(FlvTagType::Video, timestamp, vec![first_byte, 1, 0, 0, 0])
    }

    fn create_audio_tag(timestamp: u32) -> FlvData {
        create_tag(
            FlvTagType::Audio,
            timestamp,
            vec![0xAF, 1, 0x21, 0x10, 0x04],
        )
    }

    fn create_script_tag(timestamp: u32) -> FlvData {
        create_tag(FlvTagType::ScriptData, timestamp, vec![1, 2, 3, 4])
    }

    fn create_video_sequence_header() -> FlvData {
        let data = vec![
            0x17, // Keyframe + AVC
            0x00, // AVC sequence header
            0x00, 0x00, 0x00, // Composition time
            0x01, 0x64, 0x00, 0x28, // AVCC data
        ];
        create_tag(FlvTagType::Video, 0, data)
    }

    fn create_audio_sequence_header() -> FlvData {
        let data = vec![
            0xAF, // AAC audio format
            0x00, // AAC sequence header
            0x12, 0x10, // AAC specific config
        ];
        create_tag(FlvTagType::Audio, 0, data)
    }

    // Print tag information for debugging
    fn print_tags(items: &[FlvData]) {
        println!("Tag sequence:");
        for (i, item) in items.iter().enumerate() {
            match item {
                FlvData::Header(_) => println!("  {}: Header", i),
                FlvData::Tag(tag) => {
                    let type_str = match tag.tag_type {
                        FlvTagType::Audio => {
                            if tag.is_audio_sequence_header() {
                                "Audio (Header)"
                            } else {
                                "Audio"
                            }
                        }
                        FlvTagType::Video => {
                            if tag.is_key_frame() {
                                "Video (Keyframe)"
                            } else if tag.is_video_sequence_header() {
                                "Video (Header)"
                            } else {
                                "Video"
                            }
                        }
                        FlvTagType::ScriptData => "Script",
                        _ => "Unknown",
                    };
                    println!("  {}: {} @ {}ms", i, type_str, tag.timestamp_ms);
                }
                _ => println!("  {}: Other", i),
            }
        }
    }

    #[tokio::test]
    async fn test_sequence_header_special_handling() {
        let context = create_test_context();
        let mut operator = GopSortOperator::new(context);

        let (input_tx, input_rx) = bounded_async(32);
        let (output_tx, output_rx) = bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send script tag + sequence headers (in mixed order) + a few regular tags
        input_tx.send(Ok(create_audio_tag(5))).await.unwrap();
        input_tx.send(Ok(create_script_tag(0))).await.unwrap();
        input_tx
            .send(Ok(create_video_sequence_header()))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_audio_sequence_header()))
            .await
            .unwrap();
        input_tx
            .send(Ok(create_video_tag(20, false)))
            .await
            .unwrap();

        // Send a keyframe to trigger emission
        input_tx.send(Ok(create_video_tag(30, true))).await.unwrap();

        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        print_tags(&results);

        // Check that the script tag and sequence headers are properly ordered
        if let FlvData::Tag(tag) = &results[1] {
            assert_eq!(
                tag.tag_type,
                FlvTagType::ScriptData,
                "First tag should be script"
            );
        }

        if let FlvData::Tag(tag) = &results[2] {
            assert!(
                tag.is_video_sequence_header(),
                "Second tag should be video sequence header"
            );
        }

        if let FlvData::Tag(tag) = &results[3] {
            assert!(
                tag.is_audio_sequence_header(),
                "Third tag should be audio sequence header"
            );
        }
    }

    #[tokio::test]
    async fn test_interleaving() {
        let context = create_test_context();
        let mut operator = GopSortOperator::new(context);

        let (input_tx, input_rx) = bounded_async(32);
        let (output_tx, output_rx) = bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send audio and video tags with specific timestamps for testing interleaving
        input_tx.send(Ok(create_audio_tag(10))).await.unwrap(); // A10
        input_tx
            .send(Ok(create_video_tag(20, false)))
            .await
            .unwrap(); // V20
        input_tx.send(Ok(create_audio_tag(25))).await.unwrap(); // A25
        input_tx
            .send(Ok(create_video_tag(30, false)))
            .await
            .unwrap(); // V30
        input_tx.send(Ok(create_audio_tag(35))).await.unwrap(); // A35

        // Send keyframe to trigger emission
        input_tx.send(Ok(create_video_tag(40, true))).await.unwrap(); // V40 (keyframe)

        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        print_tags(&results);

        // Check the interleaving pattern matches the Kotlin implementation
        let mut timestamps = Vec::new();
        let mut types = Vec::new();

        for item in &results[1..] {
            // Skip the header
            if let FlvData::Tag(tag) = item {
                timestamps.push(tag.timestamp_ms);
                types.push(if tag.is_audio_tag() {
                    "A"
                } else if tag.is_video_tag() {
                    "V"
                } else {
                    "S"
                });
            }
        }

        println!("Timestamps: {:?}", timestamps);
        println!("Types: {:?}", types);

        // The Kotlin algorithm should interleave with audio tags after corresponding video tags
        // (This is a simplified test - exact order depends on the input timestamps)
    }

    #[tokio::test]
    async fn test_audio_tags_before_first_video() {
        // Setup with audio tags having earlier timestamps than any video tag
        let context = create_test_context();
        let mut operator = GopSortOperator::new(context);

        let (input_tx, input_rx) = bounded_async(32);
        let (output_tx, output_rx) = bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send audio tags with timestamps before any video tag
        input_tx.send(Ok(create_audio_tag(5))).await.unwrap(); // A5
        input_tx.send(Ok(create_audio_tag(10))).await.unwrap(); // A10

        // Send video tags with higher timestamps
        input_tx
            .send(Ok(create_video_tag(20, false)))
            .await
            .unwrap(); // V20
        input_tx.send(Ok(create_video_tag(30, true))).await.unwrap(); // V30 (keyframe)

        drop(input_tx);

        // Collect and verify results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        print_tags(&results);

        // Skip header (results[0])
        // Verify that all audio tags are present in the output
        let audio_tags_count = results
            .iter()
            .filter(|item| {
                if let FlvData::Tag(tag) = item {
                    tag.is_audio_tag()
                } else {
                    false
                }
            })
            .count();

        assert_eq!(
            audio_tags_count, 2,
            "All audio tags should be present in output"
        );
    }
}
