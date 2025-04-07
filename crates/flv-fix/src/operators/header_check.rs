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

use crate::context::StreamerContext;
use crate::operators::FlvOperator;
use flv::data::FlvData;
use flv::error::FlvError;
use flv::header::FlvHeader;
use kanal;
use kanal::{AsyncReceiver, AsyncSender};
use std::sync::Arc;
use tracing::warn;

/// An operator that validates and ensures FLV streams have a proper header.
///
/// The HeaderCheckOperator inspects the beginning of a stream and inserts
/// a standard FLV header if one is not present. This helps downstream
/// processors by ensuring they always receive well-formed FLV data.
pub struct HeaderCheckOperator {
    context: Arc<StreamerContext>,
}

impl HeaderCheckOperator {
    /// Creates a new HeaderCheckOperator with the given context.
    ///
    /// # Arguments
    ///
    /// * `context` - The shared StreamerContext containing configuration and state
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self { context }
    }
}

impl FlvOperator for HeaderCheckOperator {
    fn context(&self) -> &Arc<StreamerContext> {
        &self.context
    }

    async fn process(
        &mut self,
        input: AsyncReceiver<Result<FlvData, FlvError>>,
        output: AsyncSender<Result<FlvData, FlvError>>,
    ) {
        let mut first_item = true;

        while let Ok(item) = input.recv().await {
            match item {
                Ok(data) => {
                    if first_item {
                        first_item = false;

                        // If the first item is not a header, insert a default one
                        if !data.is_header() {
                            warn!(
                                "{} FLV header is missing, inserted a default header",
                                self.context.name
                            );
                            // Send a default header
                            let default_header = FlvHeader::new(true, true);
                            if output
                                .send(Ok(FlvData::Header(default_header)))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }

                    // Forward the data
                    if output.send(Ok(data)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    // Error handling - just forward the error
                    first_item = false;
                    if output.send(Err(e)).await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        "HeaderCheckOperator"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use flv::tag::{FlvTag, FlvTagType};

    // Helper function to create a test context
    fn create_test_context() -> Arc<StreamerContext> {
        Arc::new(StreamerContext::default())
    }

    // Helper function to create a FlvHeader for testing
    fn create_test_header() -> FlvData {
        FlvData::Header(FlvHeader::new(true, true))
    }

    // Helper function to create a FlvTag for testing
    fn create_test_tag(tag_type: FlvTagType, timestamp: u32) -> FlvData {
        let data = vec![0u8; 10]; // Sample tag data
        FlvData::Tag(FlvTag {
            timestamp_ms: timestamp,
            stream_id: 0,
            tag_type,
            data: Bytes::from(data),
        })
    }

    #[tokio::test]
    async fn test_with_header_present() {
        let context = create_test_context();
        let mut operator = HeaderCheckOperator::new(context);

        let (input_tx, input_rx) = kanal::unbounded_async();
        let (output_tx, output_rx) = kanal::unbounded_async();

        // Start the process in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send a header followed by some tags
        input_tx.send(Ok(create_test_header())).await.unwrap();
        for i in 0..5 {
            input_tx
                .send(Ok(create_test_tag(FlvTagType::Video, i)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Should receive all 6 items (header + 5 tags)
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
            if received_items.len() == 6 {
                // Got all expected items
                break;
            }
        }

        assert_eq!(received_items.len(), 6);

        // First item should be a header
        assert!(matches!(received_items[0], FlvData::Header(_)));
    }

    #[tokio::test]
    async fn test_without_header() {
        let context = create_test_context();
        let mut operator = HeaderCheckOperator::new(context);

        let (input_tx, input_rx) = kanal::unbounded_async();
        let (output_tx, output_rx) = kanal::unbounded_async();

        // Start the process in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send tags without a header
        for i in 0..5 {
            input_tx
                .send(Ok(create_test_tag(FlvTagType::Video, i)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Should receive 6 items (default header + 5 tags)
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item.unwrap());
            if received_items.len() == 6 {
                // Got all expected items
                break;
            }
        }

        assert_eq!(received_items.len(), 6);

        // First item should be a header
        assert!(matches!(received_items[0], FlvData::Header(_)));
    }

    #[tokio::test]
    async fn test_with_error() {
        let context = create_test_context();
        let mut operator = HeaderCheckOperator::new(context);

        let (input_tx, input_rx) = kanal::unbounded_async();
        let (output_tx, output_rx) = kanal::unbounded_async();

        // Start the process in a separate task
        tokio::spawn(async move {
            operator.process(input_rx, output_tx).await;
        });

        // Send an error as the first item
        input_tx
            .send(Err(FlvError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Test error",
            ))))
            .await
            .unwrap();

        // Send some valid data after the error
        for i in 0..3 {
            input_tx
                .send(Ok(create_test_tag(FlvTagType::Video, i)))
                .await
                .unwrap();
        }

        // Close the input
        drop(input_tx);

        // Should receive 4 items (1 error + 3 tags)
        let mut received_items = Vec::new();
        while let Ok(item) = output_rx.recv().await {
            received_items.push(item);
            if received_items.len() == 4 {
                // Got all expected items
                break;
            }
        }

        assert_eq!(received_items.len(), 4);

        // First item should be an error
        assert!(received_items[0].is_err());

        // No header should be inserted after an error
        assert!(matches!(received_items[1], Ok(FlvData::Tag(_))));
    }
}
