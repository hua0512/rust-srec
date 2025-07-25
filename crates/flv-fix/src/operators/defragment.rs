//! # Defragment Operator
//!
//! The Defragment Operator is responsible for detecting and handling fragmented FLV streams.
//!
//! ## Purpose
//!
//! When recording live streams, the connection may be interrupted or the stream might start
//! mid-transmission. This can result in incomplete or fragmented FLV data that needs special
//! handling to ensure the output file is valid and playable.
//!
//! ## How it Works
//!
//! The operator uses a buffering strategy:
//!
//! 1. When an FLV header is detected, it starts "gathering" mode
//! 2. It buffers tags until a minimum threshold is reached
//! 3. If enough tags are gathered, the stream is considered valid and all buffered tags are emitted
//! 4. If another header is detected before gathering enough tags, the existing buffer is discarded
//!    (considered a fragmented/invalid segment)
//!
//! This approach helps filter out small, broken segments while preserving complete, valid ones.
//!
//! ## Configuration
//!
//! The operator requires a minimum number of tags (`MIN_TAGS_NUM`) to consider a segment valid.
//! This value can be adjusted based on stream characteristics.
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
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::{debug, warn};

/// An operator that buffers and validates FLV stream fragments to ensure continuity and validity.
///
/// The DefragmentOperator helps manage fragmented streams by:
/// - Buffering a minimum number of tags after each header
/// - Only emitting complete segments with enough tags
/// - Discarding small fragments that might be incomplete
/// - Handling errors appropriately throughout the process
pub struct DefragmentOperator {
    context: Arc<StreamerContext>,
    is_gathering: bool,
    buffer: Vec<FlvData>,
}

impl DefragmentOperator {
    /// Creates a new DefragmentOperator with the given context.
    ///
    /// # Arguments
    ///
    /// * `context` - The shared StreamerContext containing configuration and state
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            is_gathering: false,
            buffer: Vec::with_capacity(Self::MIN_TAGS_NUM),
        }
    }

    // The minimum number of tags required to consider a segment valid.
    const MIN_TAGS_NUM: usize = 10;

    // Resets the operator state, clearing the buffer and stopping gathering mode.
    fn reset(&mut self) {
        self.is_gathering = false;
        self.buffer.clear();
    }

    // Handle a new header detection
    fn handle_new_header(&mut self) {
        if !self.buffer.is_empty() {
            warn!(
                "{} Discarded {} items, total size: {}",
                self.context.name,
                self.buffer.len(),
                self.buffer.iter().map(|d| d.size()).sum::<usize>()
            );
            self.reset();
        }
        self.is_gathering = true;
        debug!("{} Start gathering...", self.context.name);
    }

    // Emit all buffered items and reset the buffer
    fn emit_buffer(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!(
            "{} Gathered {} items, total size: {}",
            self.context.name,
            self.buffer.len(),
            self.buffer.iter().map(|d| d.size()).sum::<usize>()
        );

        // Emit all buffered items
        for tag in self.buffer.drain(..) {
            output(tag)?;
        }

        self.is_gathering = false;
        debug!(
            "{} Not a fragmented sequence, stopped checking...",
            self.context.name
        );

        Ok(())
    }
}

impl Processor<FlvData> for DefragmentOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Handle new header detection
        if input.is_header() {
            self.handle_new_header();
        }

        if self.is_gathering {
            self.buffer.push(input);

            // Check if we've gathered enough tags
            if self.buffer.len() >= Self::MIN_TAGS_NUM {
                self.emit_buffer(output)?;
            }
        } else {
            // Not gathering, emit directly
            output(input)?;
        }

        Ok(())
    }

    fn finish(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Handle remaining data at end of stream
        if !self.buffer.is_empty() {
            if self.buffer.len() >= Self::MIN_TAGS_NUM {
                self.emit_buffer(output)?;
            } else {
                warn!(
                    "{} End of stream with only {} items in buffer, discarding",
                    self.context.name,
                    self.buffer.len()
                );
                self.reset();
            }
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "DefragmentOperator"
    }
}

#[cfg(test)]
mod tests {
    use pipeline_common::StreamerContext;

    use super::*;
    use crate::test_utils::{self, create_video_tag};

    #[test]
    fn test_normal_flow_with_enough_tags() {
        let context = StreamerContext::arc_new();
        let mut operator = DefragmentOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header followed by enough tags
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        for i in 0..11 {
            operator
                .process(create_video_tag(i, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Should emit all 12 items (header + 11 tags)
        assert_eq!(output_items.len(), 12);
    }

    #[test]
    fn test_header_reset() {
        let context = StreamerContext::arc_new();
        let mut operator = DefragmentOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send a header followed by some tags (but not enough to emit)
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        for i in 0..5 {
            operator
                .process(create_video_tag(i, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Send another header (should discard previous tags)
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        for i in 0..11 {
            operator
                .process(create_video_tag(i, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Send a regular tag after gathering enough
        operator
            .process(create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Should emit 13 items (header + 11 tags from second batch + 1 regular tag)
        assert_eq!(output_items.len(), 13);
    }

    #[test]
    fn test_end_of_stream_with_enough_tags() {
        let context = StreamerContext::arc_new();
        let mut operator = DefragmentOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send a header followed by exactly MIN_TAGS_NUM tags
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        for i in 0..10 {
            operator
                .process(create_video_tag(i, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Finish processing (should emit buffer as it has enough tags)
        operator.finish(&mut output_fn).unwrap();

        // Should receive all 11 items (header + 10 tags)
        assert_eq!(output_items.len(), 11);
    }

    #[test]
    fn test_end_of_stream_with_insufficient_tags() {
        let context = StreamerContext::arc_new();
        let mut operator = DefragmentOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send a header followed by fewer than MIN_TAGS_NUM tags
        operator
            .process(test_utils::create_test_header(), &mut output_fn)
            .unwrap();
        for i in 0..5 {
            operator
                .process(create_video_tag(i, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Finish processing (should discard buffer as it doesn't have enough tags)
        operator.finish(&mut output_fn).unwrap();

        // No items should be emitted
        assert_eq!(output_items.len(), 0);
    }
}
