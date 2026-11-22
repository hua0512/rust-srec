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

use std::sync::Arc;

use flv::data::FlvData;
use flv::script::ScriptData;
use flv::tag::FlvTagType;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use tracing::{debug, info};

/// Operator that filters out duplicate `onMetaData` / `@setDataFrame` metadata
/// tags while preserving non-metadata script tags (such as `onCuePoint` and
/// `onTextData`).
pub struct ScriptFilterOperator {
    context: Arc<StreamerContext>,
    seen_metadata_tag: bool,
    discarded_metadata_count: u32,
}

impl ScriptFilterOperator {
    /// Create a new ScriptFilterOperator
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            seen_metadata_tag: false,
            discarded_metadata_count: 0,
        }
    }

    /// Reset the operator state
    pub fn reset(&mut self) {
        self.seen_metadata_tag = false;
        self.discarded_metadata_count = 0;
    }
}

impl Processor<FlvData> for ScriptFilterOperator {
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
            FlvData::Header(_) | FlvData::Split(_) | FlvData::EndOfSequence(_) => {
                debug!("{} Resetting script tag filter state", self.context.name);
                self.reset();
                output(input)
            }
            FlvData::Tag(tag) if tag.tag_type() == FlvTagType::ScriptData => {
                if tag.is_filtered() {
                    return output(FlvData::Tag(tag));
                }

                let mut cursor = std::io::Cursor::new(tag.data().clone());
                if let Ok(script) = ScriptData::demux(&mut cursor)
                    && crate::amf::is_non_metadata_script(&script)
                {
                    debug!(
                        "{} Forwarding non-metadata script tag '{}'",
                        self.context.name, script.name
                    );
                    return output(FlvData::Tag(tag));
                }

                if !self.seen_metadata_tag {
                    self.seen_metadata_tag = true;
                    debug!("{} Forwarding first metadata script tag", self.context.name);
                    output(FlvData::Tag(tag))
                } else {
                    self.discarded_metadata_count += 1;
                    debug!(
                        "{} Discarding duplicate metadata script tag #{}",
                        self.context.name, self.discarded_metadata_count
                    );
                    Ok(())
                }
            }
            _ => output(input),
        }
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if self.discarded_metadata_count > 0 {
            info!(
                "{} Filtered out {} excess metadata script tags",
                self.context.name, self.discarded_metadata_count
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
    use pipeline_common::{CancellationToken, StreamerContext, init_test_tracing};

    #[test]
    fn test_multiple_headers_reset_filtering() {
        init_test_tracing!();
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = ScriptFilterOperator::new(context.clone());
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
            .process(&context, create_script_tag(0, true), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(10, true), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_script_tag(20, false), &mut output_fn)
            .unwrap(); // Should be discarded

        // Second segment (new header should reset filtering)
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_script_tag(0, true), &mut output_fn)
            .unwrap(); // Should be kept
        operator
            .process(&context, create_video_tag(10, false), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_script_tag(20, false), &mut output_fn)
            .unwrap(); // Should be discarded

        // Finish processing
        operator.finish(&context, &mut output_fn).unwrap();

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
                FlvData::Tag(tag) if tag.tag_type() == FlvTagType::ScriptData => {
                    if in_first_segment {
                        first_segment_script_count += 1;
                    } else {
                        second_segment_script_count += 1;
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

    #[test]
    fn preserves_non_metadata_script_tags_while_deduplicating_metadata() {
        use std::borrow::Cow;

        use amf0::Amf0Value;
        use bytes::Bytes;
        use flv::tag::FlvTag;

        let make_named_script = |name: &str, ts: u32, extra_str: Option<&'static str>| {
            let mut buf = Vec::new();
            amf0::Amf0Encoder::encode_string(&mut buf, name).unwrap();
            if let Some(s) = extra_str {
                amf0::Amf0Encoder::encode_string(&mut buf, s).unwrap();
            }
            amf0::Amf0Encoder::encode(
                &mut buf,
                &Amf0Value::Object(Cow::Owned(vec![(
                    Cow::Borrowed("k"),
                    Amf0Value::Number(1.0),
                )])),
            )
            .unwrap();
            FlvData::Tag(FlvTag::new(
                ts,
                0,
                FlvTagType::ScriptData,
                false,
                Bytes::from(buf),
            ))
        };

        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = ScriptFilterOperator::new(context.clone());
        let mut out = Vec::new();

        let cue = make_named_script("onCuePoint", 0, None);
        let set_df = make_named_script("@setDataFrame", 0, Some("onMetaData"));
        let dup_meta = create_script_tag(20, false);
        let text = make_named_script("onTextData", 30, None);

        for item in [
            create_test_header(),
            cue.clone(),
            set_df.clone(),
            create_video_tag(10, true),
            dup_meta,
            text.clone(),
        ] {
            operator
                .process(&context, item, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
        }

        assert_eq!(out.len(), 5);
        assert_eq!(out[1], cue);
        assert_eq!(out[2], set_df);
        assert_eq!(out[4], text);
    }
}
