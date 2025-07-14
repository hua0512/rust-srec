//! # ScriptFilterOperator
//!
//! The `ScriptFilterOperator` removes duplicate script data (metadata) tags from an FLV stream.
//!
//! ## Purpose
//!
//! FLV streams may contain multiple script data tags (metadata), but many players
//! can only properly handle a single metadata tag. This operator ensures that:
//!
//! 1. The first script data tag is preserved (typically containing essential metadata)
//! 2. Subsequent script data tags are discarded
//!
//! This improves compatibility with various players and reduces unnecessary data.
//!
//! ## Operation
//!
//! The operator tracks whether it has already encountered a script tag. Once it has
//! seen the first script tag, it filters out any subsequent ones while passing through
//! all other tag types unmodified.
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
use flv::tag::FlvTagType;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::{debug, info};

/// Operator that filters out script data tags except for the first one
pub struct ScriptFilterOperator {
    context: Arc<StreamerContext>,
    seen_script_tag: bool,
    script_tag_count: u32,
}

impl ScriptFilterOperator {
    /// Create a new ScriptFilterOperator
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            seen_script_tag: false,
            script_tag_count: 0,
        }
    }

    /// Reset the operator state
    pub fn reset(&mut self) {
        self.seen_script_tag = false;
        self.script_tag_count = 0;
    }
}

impl Processor<FlvData> for ScriptFilterOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        match input {
            FlvData::Header(_) => {
                debug!("{} Resetting script tag filter state", self.context.name);
                self.reset();
                // Forward the header
                output(input)
            } // Check if this is a script tag
            FlvData::Tag(tag) if tag.tag_type == FlvTagType::ScriptData => {
                self.script_tag_count += 1;
                if !self.seen_script_tag {
                    // First script tag, keep it and mark as seen
                    self.seen_script_tag = true;
                    debug!("{} Forwarding first script tag", self.context.name);
                    output(FlvData::Tag(tag))
                } else {
                    // Subsequent script tag, discard it
                    debug!(
                        "{} Discarding subsequent script tag #{}",
                        self.context.name, self.script_tag_count
                    );
                    // Skip sending this to output
                    Ok(())
                }
            }
            _ => output(input), // Forward other data types unmodified
        }
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if self.script_tag_count > 1 {
            info!(
                "{} Filtered out {} excess script tags",
                self.context.name,
                self.script_tag_count - 1
            );
        }
        debug!("{} Script filter operator completed", self.context.name);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "ScriptFilterOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{create_script_tag, create_test_header, create_video_tag};
    use pipeline_common::create_test_context;
    use pipeline_common::init_test_tracing;

    #[test]
    fn test_filter_script_tags() {
        init_test_tracing!();
        let context = create_test_context();
        let mut operator = ScriptFilterOperator::new(context);
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

        // Send first script tag - should be kept
        operator
            .process(create_script_tag(0, true), &mut output_fn)
            .unwrap();

        // Send some video tags
        operator
            .process(create_video_tag(10, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(20, false), &mut output_fn)
            .unwrap();

        // Send another script tag - should be discarded
        operator
            .process(create_script_tag(30, false), &mut output_fn)
            .unwrap();

        // Send more video tags
        operator
            .process(create_video_tag(40, true), &mut output_fn)
            .unwrap();

        // Send a third script tag - should be discarded
        operator
            .process(create_script_tag(50, false), &mut output_fn)
            .unwrap();

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check we have the correct number of items (header + 1 script tag + 3 video tags = 5)
        assert_eq!(
            output_items.len(),
            5,
            "Expected 5 items, got {}",
            output_items.len()
        );

        // Verify the order and types of tags
        let mut tag_types = Vec::new();
        for item in &output_items {
            match item {
                FlvData::Header(_) => tag_types.push("Header"),
                FlvData::Tag(tag) => match tag.tag_type {
                    FlvTagType::ScriptData => tag_types.push("ScriptData"),
                    FlvTagType::Video => tag_types.push("Video"),
                    FlvTagType::Audio => tag_types.push("Audio"),
                    _ => tag_types.push("Unknown"),
                },
                _ => tag_types.push("Other"),
            }
        }

        // We expect: Header, ScriptData, Video, Video, Video
        assert_eq!(
            tag_types,
            vec!["Header", "ScriptData", "Video", "Video", "Video"]
        );
    }

    #[test]
    fn test_multiple_headers_reset_filtering() {
        init_test_tracing!();
        let context = create_test_context();
        let mut operator = ScriptFilterOperator::new(context);
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
            .process(create_script_tag(0, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_video_tag(10, true), &mut output_fn)
            .unwrap();
        operator
            .process(create_script_tag(20, false), &mut output_fn)
            .unwrap(); // Should be discarded

        // Second segment (new header should reset filtering)
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(create_script_tag(0, true), &mut output_fn)
            .unwrap(); // Should be kept
        operator
            .process(create_video_tag(10, false), &mut output_fn)
            .unwrap();
        operator
            .process(create_script_tag(20, false), &mut output_fn)
            .unwrap(); // Should be discarded

        // Finish processing
        operator.finish(&mut output_fn).unwrap();

        // Check we have the correct number of items (2 headers + 2 script tags + 2 video tags = 6)
        assert_eq!(
            output_items.len(),
            6,
            "Expected 6 items, got {}",
            output_items.len()
        );

        // Verify each segment has exactly one script tag
        let mut first_segment_script_count = 0;
        let mut second_segment_script_count = 0;
        // first element should be the header

        assert!(matches!(output_items[0], FlvData::Header(_)));
        // The first segment is everything after the first header and before the second header
        let mut in_first_segment = true;

        // Iterate over the results starting from the second item
        for item in &output_items[1..] {
            match item {
                FlvData::Header(_) => {
                    in_first_segment = false; // Switch to second segment after seeing second header
                }
                FlvData::Tag(tag) => {
                    if tag.tag_type == FlvTagType::ScriptData {
                        if in_first_segment {
                            first_segment_script_count += 1;
                        } else {
                            second_segment_script_count += 1;
                        }
                    }
                }
                _ => {}
            }
        }

        assert_eq!(
            first_segment_script_count, 1,
            "First segment should have 1 script tag"
        );
        assert_eq!(
            second_segment_script_count, 1,
            "Second segment should have 1 script tag"
        );
    }
}
