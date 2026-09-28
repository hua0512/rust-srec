//! Stream processing context and configuration
//!
//! This module provides the context and configuration structures needed for
//! stream processing. It includes the shared context for operators in the processing pipeline.

use crate::cancellation::CancellationToken;

/// Shared context for stream processing operations
///
/// Provides a common context shared across the processing pipeline including
/// the stream name and cancellation token. This context is used
/// by operators to coordinate their actions and share information.
#[derive(Debug, Clone)]
pub struct StreamerContext {
    /// Name of the stream/file being processed
    pub name: String,
    /// The cancellation token
    pub token: CancellationToken,
    /// Optional live-recording control; offline repair pipelines leave it unset.
    pub manual_split: Option<std::sync::Arc<crate::ManualSplitControl>>,
}

impl StreamerContext {
    /// Create a new StreamerContext with the specified configuration
    pub fn new(token: CancellationToken) -> Self {
        Self {
            name: "DefaultStreamer".to_string(),
            token,
            manual_split: None,
        }
    }

    pub fn arc_new(token: CancellationToken) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::new(token))
    }

    pub fn with_manual_split(mut self, control: std::sync::Arc<crate::ManualSplitControl>) -> Self {
        self.manual_split = Some(control);
        self
    }

    pub fn with_name(name: impl Into<String>, token: CancellationToken) -> Self {
        Self {
            name: name.into(),
            ..Self::new(token)
        }
    }
}
