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
    state: StreamState,
}

impl SplitOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            state: StreamState::new(),
        }
    }

    /// Calculate CRC32 for a byte slice using crc32fast
    fn calculate_crc32(data: &[u8]) -> u32 {
        crc32fast::hash(data)
    }

    // Split stream and re-inject header+sequence data
    fn split_stream(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Note on timestamp handling:
        // When we split the stream, we re-inject the header and sequence information
        // using the original timestamps from when they were first encountered.
        // This maintains timestamp consistency within the stream segments
        // but does not reset the timeline. Downstream components or players
        // may need to handle potential timestamp discontinuities at split points.
        if let Some(header) = &self.state.header {
            output(FlvData::Header(header.clone()))?;
        }
        if let Some(metadata) = &self.state.metadata {
            output(FlvData::Tag(metadata.clone()))?;
        }
        if let Some(video_seq) = &self.state.video_sequence_tag {
            output(FlvData::Tag(video_seq.clone()))?;
        }
        if let Some(audio_seq) = &self.state.audio_sequence_tag {
            output(FlvData::Tag(audio_seq.clone()))?;
        }
        self.state.changed = false;
        info!("{} Stream split", self.context.name);
        Ok(())
    }
}

impl Processor<FlvData> for SplitOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match &input {
            FlvData::Header(header) => {
                // Reset state when a new header is encountered
                self.state.reset();
                self.state.header = Some(header.clone());
                output(input)
            }
            FlvData::Tag(tag) => {
                // Process different tag types
                if tag.is_script_tag() {
                    debug!("{} Metadata detected", self.context.name);
                    self.state.metadata = Some(tag.clone());
                } else if tag.is_video_sequence_header() {
                    debug!("{} Video sequence tag detected", self.context.name);

                    // Calculate CRC for comparison
                    let crc = Self::calculate_crc32(&tag.data);

                    // Compare with cached CRC if available
                    if let Some(prev_crc) = self.state.video_crc {
                        if prev_crc != crc {
                            info!(
                                "{} Video sequence header changed (CRC: {:x} -> {:x}), marking for split",
                                self.context.name, prev_crc, crc
                            );
                            self.state.changed = true;
                        }
                    }
                    // Update sequence tag
                    self.state.video_sequence_tag = Some(tag.clone());
                    self.state.video_crc = Some(crc);
                } else if tag.is_audio_sequence_header() {
                    debug!("{} Audio sequence tag detected", self.context.name);

                    let crc = Self::calculate_crc32(&tag.data);
                    // Compare with cached CRC if available
                    if let Some(prev_crc) = self.state.audio_crc {
                        if prev_crc != crc {
                            info!(
                                "{} Audio parameters changed: {:x} -> {:x}",
                                self.context.name, prev_crc, crc
                            );
                            self.state.changed = true;
                        }
                    }
                    // Update sequence tag
                    self.state.audio_sequence_tag = Some(tag.clone());
                    self.state.audio_crc = Some(crc);
                } else if self.state.changed {
                    // If parameters have changed and this is a regular tag,
                    // it's time to split the stream
                    self.split_stream(output)?;
                }
                output(input)
            }
            _ => output(input),
        }
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} completed.", self.context.name);
        self.state.reset();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SplitOperator"
    }
}

#[cfg(test)]
mod tests {
    use pipeline_common::StreamerContext;

    use super::*;
    use crate::test_utils::{
        create_audio_sequence_header, create_audio_tag, create_test_header,
        create_video_sequence_header, create_video_tag,
    };

    #[test]
    fn test_video_codec_change_detection() {
        let context = StreamerContext::arc_new();
        let mut operator = SplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and first video sequence header (version 1)
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_sequence_header(1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(create_video_tag(i * 100, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Add a different video sequence header (version 2) - should trigger a split
        operator
            .process(create_video_sequence_header(2), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(create_video_tag(i * 100, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // The header count indicates how many splits occurred
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 2 headers: initial + 1 after codec change
        assert_eq!(
            header_count, 2,
            "Should detect video codec change and inject new header"
        );
    }

    #[test]
    fn test_audio_codec_change_detection() {
        let context = StreamerContext::arc_new();
        let mut operator = SplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and first audio sequence header
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_sequence_header(1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // Add a different audio sequence header - should trigger a split
        operator
            .process(create_audio_sequence_header(2), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // The header count indicates how many splits occurred
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 2 headers: initial + 1 after codec change
        assert_eq!(
            header_count, 2,
            "Should detect audio codec change and inject new header"
        );
    }

    #[test]
    fn test_no_codec_change() {
        let context = StreamerContext::arc_new();
        let mut operator = SplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and codec headers
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_sequence_header(1), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_sequence_header(1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(create_video_tag(i * 100, i % 3 == 0), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // Add identical codec headers again - should NOT trigger a split
        operator
            .process(create_video_sequence_header(1), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_sequence_header(1), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(create_video_tag(i * 100, i % 3 == 0), &mut output_fn)
                .unwrap();
            operator
                .process(create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // The header count indicates how many splits occurred
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have only 1 header (the initial one)
        assert_eq!(
            header_count, 1,
            "Should not split when codec doesn't change"
        );
    }

    #[test]
    fn test_multiple_codec_changes() {
        let context = StreamerContext::arc_new();
        let mut operator = SplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // First segment
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_sequence_header(1), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_sequence_header(1), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Second segment (video codec change)
        operator
            .process(create_video_sequence_header(2), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(200, true), &mut output_fn)
            .unwrap();

        // Third segment (audio codec change)
        operator
            .process(create_audio_sequence_header(2), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_tag(300), &mut output_fn)
            .unwrap();

        // Fourth segment (both codecs change)
        operator
            .process(create_video_sequence_header(3), &mut output_fn)
            .unwrap();
        operator
            .process(create_audio_sequence_header(3), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(400, true), &mut output_fn)
            .unwrap();

        // The header count indicates how many segments we have
        let header_count = output_items
            .iter()
            .filter(|item| matches!(item, FlvData::Header(_)))
            .count();

        // Should have 4 headers: initial + 3 after codec changes
        assert_eq!(
            header_count, 4,
            "Should detect all codec changes and inject new headers"
        );
    }
}
