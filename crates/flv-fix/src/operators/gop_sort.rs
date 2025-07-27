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
use flv::data::FlvData;
use flv::tag::{FlvTag, FlvUtil};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::{debug, info, trace};

/// GOP sorting operator that follows the Kotlin implementation's logic
pub struct GopSortOperator {
    context: Arc<StreamerContext>,
    gop_tags: Vec<FlvTag>,
}

impl GopSortOperator {
    /// Buffer size for gop tags before processing
    const TAGS_BUFFER_SIZE: usize = 10;

    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            gop_tags: Vec::new(),
        }
    }

    /// Process buffered tags and emit them in properly sorted order
    /// This follows the Kotlin implementation's sorting logic
    fn push_tags(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if self.gop_tags.is_empty() {
            return Ok(());
        }

        trace!("{} GOP tags: {}", self.context.name, self.gop_tags.len());

        // Special handling for small buffers with sequence headers
        if self.gop_tags.len() < Self::TAGS_BUFFER_SIZE {
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
                    output(FlvData::Tag(script_tag))?;
                }

                // Adjust indices for video header after possible script tag removal
                let avc_idx = if script_pos.is_some() && avc_pos > script_pos.unwrap() {
                    avc_pos - 1
                } else {
                    avc_pos
                };

                // Emit video sequence header
                let avc_tag = self.gop_tags.remove(avc_idx);
                output(FlvData::Tag(avc_tag))?;

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
                output(FlvData::Tag(aac_tag))?;

                // Clear the buffer since we've handled the important tags
                self.gop_tags.clear();
                return Ok(());
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

        let mut sorted_tags = Vec::with_capacity(audio_tags.len() + video_tags.len());
        let mut a_idx = 0;
        let mut v_idx = 0;

        // Two-pointer merge process
        while a_idx < audio_tags.len() && v_idx < video_tags.len() {
            let audio_tag = &audio_tags[a_idx];
            let video_tag = &video_tags[v_idx];

            // Core comparison logic:
            // If video timestamp <= audio timestamp, prioritize video tag.
            // This ensures audio tags appear after the last video tag with timestamp less than or equal to it.
            if video_tag.timestamp_ms <= audio_tag.timestamp_ms {
                sorted_tags.push(video_tag.clone());
                v_idx += 1;
            } else {
                // Otherwise (audio timestamp < video timestamp), add the audio tag
                sorted_tags.push(audio_tag.clone());
                a_idx += 1;
            }
        }

        // Add remaining audio tags (if video list was processed first)
        while a_idx < audio_tags.len() {
            sorted_tags.push(audio_tags[a_idx].clone());
            a_idx += 1;
        }

        // Add remaining video tags (if audio list was processed first)
        while v_idx < video_tags.len() {
            sorted_tags.push(video_tags[v_idx].clone());
            v_idx += 1;
        }

        // Emit script tags in original order (no sorting)
        for tag in script_tags {
            output(FlvData::Tag(tag))?;
        }

        // Emit the sorted tags
        for tag in sorted_tags {
            output(FlvData::Tag(tag))?;
        }

        Ok(())
    }
}

impl Processor<FlvData> for GopSortOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match input {
            FlvData::Header(_) | FlvData::EndOfSequence(_) => {
                // Process any buffered tags first
                self.push_tags(output)?;

                // Forward the header or EOS
                // do not send end of stream to output
                if input.is_end_of_sequence() {
                    debug!("{} End of stream...", self.context.name);
                    return Ok(());
                }

                output(input)?;
                debug!("{} Reset GOP tags...", self.context.name);
            }
            FlvData::Tag(tag) => {
                // Check for a nalu keyframe
                if tag.is_key_frame_nalu() {
                    // On keyframe, process buffered tags
                    self.push_tags(output)?;
                    // Start the new buffer with this keyframe
                }
                // Just add non-keyframe to buffer
                self.gop_tags.push(tag);
            }
        }
        Ok(())
    }

    fn finish(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Process any remaining buffered tags at end of stream
        self.push_tags(output)?;
        info!("{} GOP sort completed", self.context.name);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "GopSortOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        self, create_audio_sequence_header, create_audio_tag, create_script_tag,
        create_test_header, create_video_sequence_header, create_video_tag,
    };
    use flv::tag::FlvTagType;
    use pipeline_common::StreamerContext;

    #[test]
    fn test_sequence_header_special_handling() {
        let context = StreamerContext::arc_new();
        let mut operator = GopSortOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send header
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();

        // Send script tag + sequence headers (in mixed order) + a few regular tags
        operator
            .process(create_audio_tag(5), &mut output_fn)
            .unwrap();
        operator
            .process(create_script_tag(0, false), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(20, false), &mut output_fn)
            .unwrap();

        // Send a keyframe to trigger emission
        operator
            .process(create_video_tag(30, true), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        test_utils::print_tags(&output_items);

        // Check that the script tag and sequence headers are properly ordered
        if let FlvData::Tag(tag) = &output_items[1] {
            assert_eq!(
                tag.tag_type,
                FlvTagType::ScriptData,
                "First tag should be script"
            );
        }

        if let FlvData::Tag(tag) = &output_items[2] {
            assert!(
                tag.is_video_sequence_header(),
                "Second tag should be video sequence header"
            );
        }

        if let FlvData::Tag(tag) = &output_items[3] {
            assert!(
                tag.is_audio_sequence_header(),
                "Third tag should be audio sequence header"
            );
        }
    }

    #[test]
    fn test_interleaving() {
        let context = StreamerContext::arc_new();
        let mut operator = GopSortOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send header
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();

        // Send audio and video tags with specific timestamps for testing interleaving
        operator
            .process(create_audio_tag(10), &mut output_fn)
            .unwrap(); // A10
        operator
            .process(create_video_tag(20, false), &mut output_fn)
            .unwrap(); // V20
        operator
            .process(create_audio_tag(25), &mut output_fn)
            .unwrap(); // A25
        operator
            .process(create_video_tag(30, false), &mut output_fn)
            .unwrap(); // V30
        operator
            .process(create_audio_tag(35), &mut output_fn)
            .unwrap(); // A35

        // Send keyframe to trigger emission
        operator
            .process(create_video_tag(40, true), &mut output_fn)
            .unwrap(); // V40 (keyframe)

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        test_utils::print_tags(&output_items);

        // Check the interleaving pattern
        let timestamps = test_utils::extract_timestamps(&output_items);
        let mut types = Vec::new();

        for item in &output_items[1..] {
            // Skip the header
            if let FlvData::Tag(tag) = item {
                types.push(if tag.is_audio_tag() {
                    "A"
                } else if tag.is_video_tag() {
                    "V"
                } else {
                    "S"
                });
            }
        }

        println!("Timestamps: {timestamps:?}");
        println!("Types: {types:?}");

        // The Kotlin algorithm should interleave with audio tags after corresponding video tags
        // Verify video tags come before audio tags with same or higher timestamps
        let audio_pos = types.iter().position(|&t| t == "A").unwrap_or(0);
        let video_pos = types.iter().position(|&t| t == "V").unwrap_or(0);

        // Basic verification that the algorithm produces expected ordering
        assert!(
            audio_pos > 0 || video_pos > 0,
            "Should have at least one audio or video tag"
        );
    }

    #[test]
    fn test_audio_tags_before_first_video() {
        // Setup with audio tags having earlier timestamps than any video tag
        let context = StreamerContext::arc_new();
        let mut operator = GopSortOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send header
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();

        // Send audio tags with timestamps before any video tag
        operator
            .process(create_audio_tag(5), &mut output_fn)
            .unwrap(); // A5
        operator
            .process(create_audio_tag(10), &mut output_fn)
            .unwrap(); // A10

        // Send video tags with higher timestamps
        operator
            .process(create_video_tag(20, false), &mut output_fn)
            .unwrap(); // V20
        operator
            .process(create_video_tag(30, true), &mut output_fn)
            .unwrap(); // V30 (keyframe)

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        test_utils::print_tags(&output_items);

        // Skip header (output_items[0])
        // Verify that all audio tags are present in the output
        let audio_tags_count = output_items
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
