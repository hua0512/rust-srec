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
//! ## Example
//!
//! ```no_run
//! use std::sync::Arc;
//! use kanal::{bounded_async};
//! use crate::context::StreamerContext;
//! use crate::operators::script_filter::ScriptFilterOperator;
//!
//! async fn example() {
//!     let context = Arc::new(StreamerContext::default());
//!     let mut operator = ScriptFilterOperator::new(context);
//!     
//!     // Create channels for the pipeline
//!     let (input_tx, input_rx) = bounded_async(32);
//!     let (output_tx, output_rx) = bounded_async(32);
//!     
//!     // Process stream in background task
//!     tokio::spawn(async move {
//!         operator.process(input_rx, output_tx).await;
//!     });
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
use flv::tag::FlvTagType;
use kanal::{AsyncReceiver, AsyncSender};
use std::sync::Arc;
use tracing::{debug, info};

/// Operator that filters out script data tags except for the first one
pub struct ScriptFilterOperator {
    context: Arc<StreamerContext>,
}

impl ScriptFilterOperator {
    /// Create a new ScriptFilterOperator
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self { context }
    }
}

impl FlvOperator for ScriptFilterOperator {
    fn context(&self) -> &Arc<StreamerContext> {
        &self.context
    }

    async fn process(
        &mut self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) {
        let mut seen_script_tag = false;
        let mut script_tag_count = 0;

        while let Ok(item) = input.recv().await {
            match item {
                Ok(data) => {
                    match &data {
                        FlvData::Header(_) => {
                            debug!("{} Resetting script tag filter state", self.context.name);
                            // Reset state on header
                            seen_script_tag = false;
                            script_tag_count = 0;

                            // Forward the header
                            if output.send(Ok(data)).await.is_err() {
                                return;
                            }
                        }
                        FlvData::Tag(tag) => {
                            // Check if this is a script tag
                            if tag.tag_type == FlvTagType::ScriptData {
                                script_tag_count += 1;

                                if !seen_script_tag {
                                    // First script tag, keep it and mark as seen
                                    seen_script_tag = true;
                                    debug!("{} Forwarding first script tag", self.context.name);
                                    if output.send(Ok(data)).await.is_err() {
                                        return;
                                    }
                                } else {
                                    // Subsequent script tag, discard it
                                    debug!(
                                        "{} Discarding subsequent script tag #{}",
                                        self.context.name, script_tag_count
                                    );
                                    // Skip sending this to output
                                    continue;
                                }
                            } else {
                                // Not a script tag, forward it
                                if output.send(Ok(data)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        // Forward other data types unmodified
                        _ => {
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

        if script_tag_count > 1 {
            info!(
                "{} Filtered out {} excess script tags",
                self.context.name,
                script_tag_count - 1
            );
        }
        debug!("{} Script filter operator completed", self.context.name);
    }

    fn name(&self) -> &'static str {
        "ScriptFilterOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::header::FlvHeader;
    use flv::tag::FlvTag;

    // Helper functions for testing
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    fn create_header() -> FlvData {
        FlvData::Header(FlvHeader::new(true, true))
    }

    fn create_script_tag(timestamp: u32, data: Vec<u8>) -> FlvData {
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::ScriptData,
            data: Bytes::from(data),
        })
    }

    fn create_video_tag(timestamp: u32) -> FlvData {
        let data = vec![0x17, 0x01, 0x00, 0x00, 0x00];
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type: FlvTagType::Video,
            data: Bytes::from(data),
        })
    }

    #[tokio::test]
    async fn test_filter_script_tags() {
        let context = create_test_context();
        let mut operator = ScriptFilterOperator::new(context);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, output_rx) = kanal::bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send header
        input_tx.send(Ok(create_header())).await.unwrap();

        // Send first script tag - should be kept
        input_tx
            .send(Ok(create_script_tag(0, vec![0x01, 0x02])))
            .await
            .unwrap();

        // Send some video tags
        input_tx.send(Ok(create_video_tag(10))).await.unwrap();
        input_tx.send(Ok(create_video_tag(20))).await.unwrap();

        // Send another script tag - should be discarded
        input_tx
            .send(Ok(create_script_tag(30, vec![0x03, 0x04])))
            .await
            .unwrap();

        // Send more video tags
        input_tx.send(Ok(create_video_tag(40))).await.unwrap();

        // Send a third script tag - should be discarded
        input_tx
            .send(Ok(create_script_tag(50, vec![0x05, 0x06])))
            .await
            .unwrap();

        // Close input
        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        // Check we have the correct number of items (header + 1 script tag + 3 video tags = 5)
        assert_eq!(results.len(), 5, "Expected 5 items, got {}", results.len());

        // Verify the order and types of tags
        let mut tag_types = Vec::new();
        for item in &results {
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

    #[tokio::test]
    async fn test_multiple_headers_reset_filtering() {
        let context: Arc<StreamerContext> = create_test_context();
        let mut operator = ScriptFilterOperator::new(context);

        let (input_tx, input_rx) = kanal::bounded_async(32);
        let (output_tx, output_rx) = kanal::bounded_async(32);

        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // First segment
        input_tx.send(Ok(create_header())).await.unwrap();
        input_tx
            .send(Ok(create_script_tag(0, vec![0x01])))
            .await
            .unwrap();
        input_tx.send(Ok(create_video_tag(10))).await.unwrap();
        input_tx
            .send(Ok(create_script_tag(20, vec![0x02])))
            .await
            .unwrap(); // Should be discarded

        // Second segment (new header should reset filtering)
        input_tx.send(Ok(create_header())).await.unwrap();
        input_tx
            .send(Ok(create_script_tag(0, vec![0x03])))
            .await
            .unwrap(); // Should be kept
        input_tx.send(Ok(create_video_tag(10))).await.unwrap();
        input_tx
            .send(Ok(create_script_tag(20, vec![0x04])))
            .await
            .unwrap(); // Should be discarded

        // Close input
        drop(input_tx);

        // Collect results
        let mut results = Vec::new();
        while let Ok(result) = output_rx.recv().await {
            results.push(result.unwrap());
        }

        // Check we have the correct number of items (2 headers + 2 script tags + 2 video tags = 6)
        assert_eq!(results.len(), 6, "Expected 6 items, got {}", results.len());

        // Verify each segment has exactly one script tag
        let mut first_segment_script_count = 0;
        let mut second_segment_script_count = 0;
        // first element should be the header

        matches!(results[0], FlvData::Header(_));
        // The first segment is everything after the first header and before the second header
        let mut in_first_segment = true;

        // Iterate over the results starting from the second item
        for item in &results[1..] {
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
