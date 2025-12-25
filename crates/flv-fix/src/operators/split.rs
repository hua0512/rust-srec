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
    buffered_metadata: bool,
    buffered_audio_sequence_tag: bool,
    buffered_video_sequence_tag: bool,
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
            buffered_metadata: false,
            buffered_audio_sequence_tag: false,
            buffered_video_sequence_tag: false,
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
        self.buffered_metadata = false;
        self.buffered_audio_sequence_tag = false;
        self.buffered_video_sequence_tag = false;
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
        self.state.buffered_metadata = false;
        self.state.buffered_audio_sequence_tag = false;
        self.state.buffered_video_sequence_tag = false;
        info!("{} Stream split", self.context.name);
        Ok(())
    }

    fn flush_buffered_tags_if_pending(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if !self.state.changed {
            return Ok(());
        }

        // We intentionally do NOT inject a new header here to avoid creating an empty segment (and
        // triggering writer rotation) when the stream ends before the next media tag arrives.
        if self.state.buffered_metadata
            && let Some(metadata) = self.state.metadata.take()
        {
            output(FlvData::Tag(metadata))?;
        }
        if self.state.buffered_video_sequence_tag
            && let Some(video_seq) = self.state.video_sequence_tag.take()
        {
            output(FlvData::Tag(video_seq))?;
        }
        if self.state.buffered_audio_sequence_tag
            && let Some(audio_seq) = self.state.audio_sequence_tag.take()
        {
            output(FlvData::Tag(audio_seq))?;
        }

        self.state.changed = false;
        self.state.buffered_metadata = false;
        self.state.buffered_audio_sequence_tag = false;
        self.state.buffered_video_sequence_tag = false;
        Ok(())
    }
}

impl Processor<FlvData> for SplitOperator {
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
            FlvData::Header(header) => {
                // Reset state when a new header is encountered
                self.state.reset();
                self.state.header = Some(header.clone());
                output(FlvData::Header(header))
            }
            FlvData::EndOfSequence(_) => {
                // If we buffered tags for a pending split but never saw a regular media tag,
                // don't drop them on end-of-stream.
                self.flush_buffered_tags_if_pending(output)?;
                output(input)
            }
            FlvData::Tag(tag) => {
                // If we're waiting to split, buffer metadata/sequence headers and only emit once we
                // see the first regular media tag. This prevents duplicate sequence headers around
                // split points and ensures the injected header precedes new codec config.
                if self.state.changed {
                    if tag.is_script_tag() {
                        debug!(
                            "{} Metadata detected while split pending",
                            self.context.name
                        );
                        self.state.metadata = Some(tag);
                        self.state.buffered_metadata = true;
                        return Ok(());
                    }
                    if tag.is_video_sequence_header() {
                        debug!(
                            "{} Video sequence tag detected while split pending",
                            self.context.name
                        );
                        self.state.video_sequence_tag = Some(tag);
                        self.state.buffered_video_sequence_tag = true;
                        self.state.video_crc = self
                            .state
                            .video_sequence_tag
                            .as_ref()
                            .map(|t| Self::calculate_crc32(&t.data));
                        return Ok(());
                    }
                    if tag.is_audio_sequence_header() {
                        debug!(
                            "{} Audio sequence tag detected while split pending",
                            self.context.name
                        );
                        self.state.audio_sequence_tag = Some(tag);
                        self.state.buffered_audio_sequence_tag = true;
                        self.state.audio_crc = self
                            .state
                            .audio_sequence_tag
                            .as_ref()
                            .map(|t| Self::calculate_crc32(&t.data));
                        return Ok(());
                    }

                    // First regular tag after a pending change: split now, then emit the tag.
                    self.split_stream(output)?;
                    return output(FlvData::Tag(tag));
                }

                // Normal operation: track key tags and detect parameter changes.
                if tag.is_script_tag() {
                    debug!("{} Metadata detected", self.context.name);
                    self.state.metadata = Some(tag.clone());
                    return output(FlvData::Tag(tag));
                }

                if tag.is_video_sequence_header() {
                    debug!("{} Video sequence tag detected", self.context.name);
                    let crc = Self::calculate_crc32(&tag.data);
                    if let Some(prev_crc) = self.state.video_crc
                        && prev_crc != crc
                    {
                        info!(
                            "{} Video sequence header changed (CRC: {:x} -> {:x}), marking for split",
                            self.context.name, prev_crc, crc
                        );
                        self.state.changed = true;
                        self.state.buffered_video_sequence_tag = true;
                    }
                    self.state.video_sequence_tag = Some(tag.clone());
                    self.state.video_crc = Some(crc);

                    // If we just detected a change, buffer the new header and wait for the next
                    // regular tag to inject a fresh header+sequence set.
                    if self.state.changed {
                        return Ok(());
                    }

                    return output(FlvData::Tag(tag));
                }

                if tag.is_audio_sequence_header() {
                    debug!("{} Audio sequence tag detected", self.context.name);
                    let crc = Self::calculate_crc32(&tag.data);
                    if let Some(prev_crc) = self.state.audio_crc
                        && prev_crc != crc
                    {
                        info!(
                            "{} Audio parameters changed: {:x} -> {:x}",
                            self.context.name, prev_crc, crc
                        );
                        self.state.changed = true;
                        self.state.buffered_audio_sequence_tag = true;
                    }
                    self.state.audio_sequence_tag = Some(tag.clone());
                    self.state.audio_crc = Some(crc);

                    if self.state.changed {
                        return Ok(());
                    }

                    return output(FlvData::Tag(tag));
                }

                // Regular media tag: if a change was detected earlier, split before emitting.
                if self.state.changed {
                    self.split_stream(output)?;
                }
                output(FlvData::Tag(tag))
            }
        }
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} completed.", self.context.name);
        self.flush_buffered_tags_if_pending(output)?;
        self.state.reset();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SplitOperator"
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use pipeline_common::{CancellationToken, StreamerContext};

    use super::*;
    use crate::test_utils::{
        create_audio_sequence_header, create_audio_tag, create_test_header,
        create_video_sequence_header, create_video_tag,
    };

    #[test]
    fn test_video_codec_change_detection() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and first video sequence header (version 1)
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
        }

        // Add a different video sequence header (version 2) - should trigger a split
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
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
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and first audio sequence header
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // Add a different audio sequence header - should trigger a split
        operator
            .process(&context, create_audio_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
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
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Add a header and codec headers
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add some content tags
        for i in 1..5 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
                .unwrap();
        }

        // Add identical codec headers again - should NOT trigger a split
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();

        // Add more content tags
        for i in 5..10 {
            operator
                .process(
                    &context,
                    create_video_tag(i * 100, i % 3 == 0),
                    &mut output_fn,
                )
                .unwrap();
            operator
                .process(&context, create_audio_tag(i * 100), &mut output_fn)
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
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // First segment
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Second segment (video codec change)
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(200, true), &mut output_fn)
            .unwrap();

        // Third segment (audio codec change)
        operator
            .process(&context, create_audio_sequence_header(0, 2), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(300), &mut output_fn)
            .unwrap();

        // Fourth segment (both codecs change)
        operator
            .process(&context, create_video_sequence_header(0, 3), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 3), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(400, true), &mut output_fn)
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

    #[test]
    fn test_pending_split_flushes_buffered_sequence_headers_on_finish() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Trigger a pending split by changing the video sequence header.
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // No regular media tag arrives; finish must not drop the buffered sequence header.
        operator.finish(&context, &mut output_fn).unwrap();

        let last = output_items
            .iter()
            .rev()
            .find_map(|item| match item {
                FlvData::Tag(tag) => Some(tag),
                _ => None,
            })
            .expect("Expected at least one tag in output");

        assert!(
            last.is_video_sequence_header(),
            "Expected flushed video sequence header at end"
        );
        assert_eq!(last.data[5], 2, "Expected version=2 sequence header");
    }

    #[test]
    fn test_pending_split_flushes_buffered_sequence_headers_on_end_of_sequence() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SplitOperator::new(context.clone());
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_sequence_header(0, 1), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Trigger pending split.
        operator
            .process(&context, create_video_sequence_header(0, 2), &mut output_fn)
            .unwrap();

        // Emit EOS; buffered tags should be flushed before it.
        operator
            .process(
                &context,
                FlvData::EndOfSequence(Bytes::new()),
                &mut output_fn,
            )
            .unwrap();

        let last_tag_idx = output_items
            .iter()
            .rposition(|i| matches!(i, FlvData::Tag(_)))
            .unwrap();
        let eos_idx = output_items
            .iter()
            .rposition(|i| matches!(i, FlvData::EndOfSequence(_)))
            .unwrap();

        assert!(
            last_tag_idx < eos_idx,
            "Expected buffered tags to flush before EndOfSequence"
        );
    }
}
