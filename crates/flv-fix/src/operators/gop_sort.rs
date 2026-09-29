//! # GOP Sorting Operator
//!
//! This operator interleaves FLV audio and video tags by timestamp while preserving
//! arrival order within each stream. Timestamp repair is handled by later stages.
//! Implementation matches the Kotlin version's logic for consistent behavior across platforms.
//!
//! ## Features
//!
//! - Buffers tags until a keyframe is encountered
//! - Handles sequence headers specially for small buffers
//! - Partitions tags by type using reusable buffers
//! - Maintains proper interleaving of audio and video tags
//! - Preserves script tags in their original order
//!
//! ## Algorithm
//!
//! 1. Buffer tags until a keyframe is encountered
//! 2. For small buffers with sequence headers, emit them directly
//! 3. Partition the remaining tags by type, preserving per-stream order
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
use std::sync::Arc;

use flv::data::FlvData;
use flv::tag::FlvTag;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use tracing::{debug, info, trace, warn};

/// GOP sorting operator that follows the Kotlin implementation's logic
pub struct GopSortOperator {
    context: Arc<StreamerContext>,
    gop_tags: Vec<FlvTag>,
    // Reuse partition storage across GOPs; payloads are drained on each flush.
    audio_tags: Vec<FlvTag>,
    video_tags: Vec<FlvTag>,
    has_video: bool,
}

impl GopSortOperator {
    /// Buffer size for gop tags before processing
    const TAGS_BUFFER_SIZE: usize = 10;

    /// Hard cap on `gop_tags` regardless of codec. Normally a GOP is flushed at
    /// the next keyframe (`FlvTag::is_key_frame_nalu`); if the stream's video
    /// never matches that predicate (unknown codec, malformed tags), the buffer
    /// would otherwise grow for the lifetime of the stream. Flushing a partial
    /// GOP is safe: `push_tags` preserves per-stream arrival order.
    const MAX_GOP_TAGS: usize = 8192;

    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            gop_tags: Vec::new(),
            audio_tags: Vec::new(),
            video_tags: Vec::new(),
            has_video: false,
        }
    }

    /// Emit script tags, then merge audio/video without reordering either stream.
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
                let avc_idx = if script_pos.is_some_and(|pos| avc_pos > pos) {
                    avc_pos - 1
                } else {
                    avc_pos
                };

                // Emit video sequence header
                let avc_tag = self.gop_tags.remove(avc_idx);
                output(FlvData::Tag(avc_tag))?;

                // Adjust indices for audio header after script and video header removal
                let aac_idx = if script_pos.is_some_and(|pos| aac_pos > pos) {
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

                // The remaining buffered tags (e.g. audio frames that arrived
                // between a split and the first keyframe) are real media and
                // must not be discarded — fall through to the partition/merge
                // path below to emit them in timestamp order.
            }
        }

        let result = (|| {
            for tag in self.gop_tags.drain(..) {
                if tag.is_script_tag() {
                    // All script tags precede media, preserving arrival order.
                    output(FlvData::Tag(tag))?;
                } else if tag.is_video_tag() {
                    self.video_tags.push(tag);
                } else if tag.is_audio_tag() {
                    self.audio_tags.push(tag);
                }
            }

            let mut audio_iter = self.audio_tags.drain(..).peekable();
            let mut video_iter = self.video_tags.drain(..).peekable();

            // Preserve per-stream order, with video first on timestamp ties.
            while let (Some(audio), Some(video)) = (audio_iter.peek(), video_iter.peek()) {
                let next = if video.timestamp_ms <= audio.timestamp_ms {
                    video_iter.next()
                } else {
                    audio_iter.next()
                };
                if let Some(tag) = next {
                    output(FlvData::Tag(tag))?;
                }
            }
            for tag in audio_iter.chain(video_iter) {
                output(FlvData::Tag(tag))?;
            }
            Ok(())
        })();

        // An error while emitting script tags can leave media in the partitions.
        // Release those payloads as well, retaining only the reusable capacity.
        self.audio_tags.clear();
        self.video_tags.clear();
        result
    }
}

impl Processor<FlvData> for GopSortOperator {
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
                // Process any buffered tags first
                self.push_tags(output)?;
                self.has_video = header.has_video;
                output(FlvData::Header(header))?;
                debug!("{} Reset GOP tags...", self.context.name);
            }
            FlvData::EndOfSequence(_) => {
                // Flush the buffered GOP, then forward the marker so downstream close
                // handlers (e.g. mesio-cli pipe_flv_strategy) see the segment boundary.
                self.push_tags(output)?;
                debug!("{} End of stream...", self.context.name);
                output(input)?;
            }
            FlvData::Split(_) => {
                self.push_tags(output)?;
                output(input)?;
            }
            FlvData::Tag(tag) => {
                // if we have video, we wait for a keyframe
                if self.has_video && tag.is_key_frame_nalu() {
                    self.push_tags(output)?;
                } else if !self.has_video && self.gop_tags.len() >= Self::TAGS_BUFFER_SIZE {
                    // if we don't have video, we flush the buffer when it's full
                    self.push_tags(output)?;
                } else if self.gop_tags.len() >= Self::MAX_GOP_TAGS {
                    warn!(
                        "{} No keyframe seen in {} tags; flushing partial GOP to bound memory",
                        self.context.name,
                        self.gop_tags.len()
                    );
                    self.push_tags(output)?;
                }
                self.gop_tags.push(tag);
            }
        }
        Ok(())
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Process any remaining buffered tags at end of stream, even if cancelled
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
    use bytes::Bytes;
    use flv::data::SplitReason;
    use flv::header::FlvHeader;
    use pipeline_common::CancellationToken;

    use crate::test_utils::{
        create_audio_sequence_header, create_audio_tag, create_script_tag, create_test_header,
        create_video_sequence_header, create_video_tag,
    };

    use super::*;

    #[test]
    fn video_without_keyframes_flushes_at_hard_cap() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = GopSortOperator::new(context.clone());
        let mut output = Vec::new();
        let mut expected = vec![create_test_header()];
        expected.extend(
            (0..=GopSortOperator::MAX_GOP_TAGS as u32).map(|ts| create_video_tag(ts, false)),
        );
        for item in &expected {
            operator
                .process(&context, item.clone(), &mut |item| {
                    output.push(item);
                    Ok(())
                })
                .unwrap();
        }
        // The tag that triggers the cap belongs to the next buffer.
        assert_eq!(output, expected[..expected.len() - 1]);
        operator
            .finish(&context, &mut |item| {
                output.push(item);
                Ok(())
            })
            .unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn sequence_headers_are_ordered_without_losing_buffered_media() {
        let headers = [
            create_script_tag(0, false),
            create_video_sequence_header(0, 1),
            create_audio_sequence_header(0, 0x12),
        ];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let context = StreamerContext::arc_new(CancellationToken::new());
            let mut operator = GopSortOperator::new(context.clone());
            let mut input = vec![create_test_header(), create_audio_tag(5)];
            input.extend(order.map(|index| headers[index].clone()));
            input.extend([create_video_tag(20, false), create_video_tag(30, true)]);
            let mut output = Vec::new();
            let mut emit = |item| {
                output.push(item);
                Ok(())
            };
            for item in input {
                operator.process(&context, item, &mut emit).unwrap();
            }
            operator.finish(&context, &mut emit).unwrap();
            assert_eq!(
                output,
                vec![
                    create_test_header(),
                    headers[0].clone(),
                    headers[1].clone(),
                    headers[2].clone(),
                    create_audio_tag(5),
                    create_video_tag(20, false),
                    create_video_tag(30, true),
                ],
                "header order {order:?}"
            );
        }
    }

    #[test]
    fn repeated_flushes_preserve_stream_order_and_put_video_first_on_ties() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = GopSortOperator::new(context.clone());
        operator
            .process(&context, create_test_header(), &mut |_| Ok(()))
            .unwrap();
        // Timestamp repair handles backwards jumps; merging preserves stream order.
        for video_tail in [40, 15, 40] {
            let mut output = Vec::new();
            let mut emit = |item| {
                output.push(item);
                Ok(())
            };
            for item in [
                create_audio_tag(10),
                create_audio_tag(20),
                create_video_tag(20, false),
                create_script_tag(0, false),
                create_audio_tag(30),
                create_video_tag(video_tail, false),
                create_script_tag(1, false),
            ] {
                operator.process(&context, item, &mut emit).unwrap();
            }
            let marker = FlvData::EndOfSequence(Bytes::new());
            operator
                .process(&context, marker.clone(), &mut emit)
                .unwrap();
            let mut expected = vec![
                create_script_tag(0, false),
                create_script_tag(1, false),
                create_audio_tag(10),
                create_video_tag(20, false),
            ];
            if video_tail == 15 {
                expected.extend([
                    create_video_tag(15, false),
                    create_audio_tag(20),
                    create_audio_tag(30),
                ]);
            } else {
                expected.extend([
                    create_audio_tag(20),
                    create_audio_tag(30),
                    create_video_tag(40, false),
                ]);
            }
            expected.push(marker);
            assert_eq!(output, expected, "video tail {video_tail}");
        }
    }

    #[test]
    fn boundaries_flush_buffered_tags_before_the_marker() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = GopSortOperator::new(context.clone());
        operator
            .process(&context, create_test_header(), &mut |_| Ok(()))
            .unwrap();
        for marker in [
            create_test_header(),
            FlvData::Split(SplitReason::DurationLimit),
            FlvData::EndOfSequence(Bytes::from_static(b"end")),
        ] {
            let mut output = Vec::new();
            let mut emit = |item| {
                output.push(item);
                Ok(())
            };
            for item in [
                create_audio_tag(10),
                create_video_tag(0, false),
                marker.clone(),
            ] {
                operator.process(&context, item, &mut emit).unwrap();
            }
            operator.finish(&context, &mut emit).unwrap();
            assert_eq!(
                output,
                vec![create_video_tag(0, false), create_audio_tag(10), marker]
            );
        }
    }

    #[test]
    fn audio_only_stream_flushes_at_each_threshold_and_at_finish() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = GopSortOperator::new(context.clone());
        let header = FlvData::Header(FlvHeader::new(true, false));
        let mut output = Vec::new();
        operator
            .process(&context, header.clone(), &mut |item| {
                output.push(item);
                Ok(())
            })
            .unwrap();
        let tags: Vec<_> = (0..25).map(|index| create_audio_tag(index * 10)).collect();
        for (index, tag) in tags.iter().enumerate() {
            operator
                .process(&context, tag.clone(), &mut |item| {
                    output.push(item);
                    Ok(())
                })
                .unwrap();
            if index == 10 || index == 20 {
                assert_eq!(output[0], header);
                assert_eq!(output[1..], tags[..index]);
            }
        }
        operator
            .finish(&context, &mut |item| {
                output.push(item);
                Ok(())
            })
            .unwrap();
        assert_eq!(output[0], header);
        assert_eq!(output[1..], tags);
    }

    #[test]
    fn downstream_errors_propagate_without_retaining_unemitted_payloads() {
        let expected = [
            create_script_tag(0, false),
            create_script_tag(1, false),
            create_audio_tag(10),
            create_video_tag(20, false),
            create_audio_tag(30),
            create_video_tag(40, false),
        ];
        for fail_at in 0..expected.len() {
            let context = StreamerContext::arc_new(CancellationToken::new());
            let mut operator = GopSortOperator::new(context.clone());
            for item in [
                create_test_header(),
                create_audio_tag(10),
                create_video_tag(20, false),
                create_script_tag(0, false),
                create_audio_tag(30),
                create_video_tag(40, false),
                create_script_tag(1, false),
            ] {
                operator.process(&context, item, &mut |_| Ok(())).unwrap();
            }
            let mut output = Vec::new();
            let result = operator.process(
                &context,
                FlvData::EndOfSequence(Bytes::new()),
                &mut |item| {
                    if output.len() == fail_at {
                        return Err(PipelineError::Io(std::io::ErrorKind::BrokenPipe.into()));
                    }
                    output.push(item);
                    Ok(())
                },
            );
            assert!(
                matches!(result, Err(PipelineError::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe)
            );
            assert_eq!(output, expected[..fail_at]);
            output.clear();
            let mut emit = |item| {
                output.push(item);
                Ok(())
            };
            operator
                .process(&context, create_audio_tag(100), &mut emit)
                .unwrap();
            operator.finish(&context, &mut emit).unwrap();
            assert_eq!(output, vec![create_audio_tag(100)], "failure at {fail_at}");
        }
    }
}
