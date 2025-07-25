//! # Header Check Operator
//!
//! The Header Check Operator ensures that FLV streams begin with a valid header.
//!
//! ## Purpose
//!
//! When processing FLV data, especially from unreliable sources or network
//! streams, the initial header might be missing. This operator validates the
//! stream structure and injects a header if needed.
//!
//! ## How it Works
//!
//! The operator:
//!
//! 1. Examines the first item in the stream
//! 2. If it's already a valid FLV header, passes it through
//! 3. If it's not a header (e.g., starts with a tag), inserts a default header
//!    before passing along the original item
//! 4. For errors, passes them through without inserting a header
//!
//! ## Use Cases
//!
//! This operator is particularly useful when:
//! - Recording streams that might start mid-transmission
//! - Processing incomplete FLV files
//! - Ensuring downstream processors can assume proper FLV structure
//!
//! ## Configuration
//!
//! The operator creates a header with both audio and video flags set to true
//! when insertion is needed.

use flv::data::FlvData;
use flv::header::FlvHeader;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::warn;

/// An operator that validates and ensures FLV streams have a proper header.
///
/// The HeaderCheckOperator inspects the beginning of a stream and inserts
/// a standard FLV header if one is not present. This helps downstream
/// processors by ensuring they always receive well-formed FLV data.
pub struct HeaderCheckOperator {
    context: Arc<StreamerContext>,
    first_item: bool,
}

impl HeaderCheckOperator {
    /// Creates a new HeaderCheckOperator with the given context.
    ///
    /// # Arguments
    ///
    /// * `context` - The shared StreamerContext containing configuration and state
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            first_item: true,
        }
    }
}

impl Processor<FlvData> for HeaderCheckOperator {
    fn process(
        &mut self,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if self.first_item {
            self.first_item = false;

            // If the first item is not a header, insert a default one
            if !input.is_header() {
                warn!(
                    "{} FLV header is missing, inserted a default header",
                    self.context.name
                );
                // Send a default header
                let default_header = FlvHeader::new(true, true);
                output(FlvData::Header(default_header))?;
            }
        }

        // Forward the data
        output(input)
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "HeaderCheckOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{create_test_header, create_video_tag};
    use pipeline_common::StreamerContext;

    #[test]
    fn test_with_header_present() {
        let context = StreamerContext::arc_new();
        let mut operator = HeaderCheckOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Process a header followed by some tags
        operator
            .process(create_test_header(), &mut output_fn)
            .unwrap();
        for i in 0..5 {
            operator
                .process(create_video_tag(i * 100, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Should receive all 6 items (header + 5 tags)
        assert_eq!(output_items.len(), 6);

        // First item should be a header
        assert!(matches!(output_items[0], FlvData::Header(_)));
    }

    #[test]
    fn test_without_header() {
        let context = StreamerContext::arc_new();
        let mut operator = HeaderCheckOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Send tags without a header
        for i in 0..5 {
            operator
                .process(create_video_tag(i * 100, i % 3 == 0), &mut output_fn)
                .unwrap();
        }

        // Should receive 6 items (default header + 5 tags)
        assert_eq!(output_items.len(), 6);

        // First item should be a header
        assert!(matches!(output_items[0], FlvData::Header(_)));
    }
}
