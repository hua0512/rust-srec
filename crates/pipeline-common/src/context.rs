//! Stream processing context and configuration
//!
//! This module provides the context and configuration structures needed for
//! FLV stream processing. It includes statistics tracking, processing configuration
//! options, and shared context for operators in the processing pipeline.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Statistics collected during stream processing
///
/// Tracks various metrics about the processed stream including tag counts,
/// fragmentation information, file characteristics, and codec details.
#[derive(Debug, Default)]
pub struct Statistics {
    /// Total number of FLV tags processed
    pub processed_tags: usize,
    /// Count of detected fragmented segments
    pub fragmented_segments: usize,
    /// Total size of the processed data in bytes
    pub file_size: usize,
    /// Total duration of the stream
    pub duration: Duration,
    /// Count of keyframes in the stream
    pub keyframes: usize,
    /// Whether the stream contains video data
    pub has_video: bool,
    /// Whether the stream contains audio data
    pub has_audio: bool,
    /// Identified video codec (if present)
    pub video_codec: Option<String>,
    /// Identified audio codec (if present)
    pub audio_codec: Option<String>,
}

/// Shared context for FLV stream processing operations
///
/// Provides a common context shared across the processing pipeline including
/// the stream name, statistics, configuration, and metadata. This context is used
/// by operators to coordinate their actions and share information.
#[derive(Debug, Clone)]
pub struct StreamerContext {
    /// Name of the stream/file being processed
    pub name: String,
    /// Runtime statistics about the processing operation
    pub statistics: Arc<Mutex<Statistics>>,
    /// Additional metadata properties
    pub metadata: Arc<Mutex<HashMap<String, String>>>,
}

impl StreamerContext {
    /// Create a new StreamerContext with the specified configuration
    ///
    /// # Arguments
    ///
    /// # Returns
    /// A new StreamerContext with the given configuration and default values for other fields
    pub fn new() -> Self {
        Self {
            name: "DefaultStreamer".to_string(),
            statistics: Arc::new(Mutex::new(Statistics::default())),
            metadata: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn arc_new() -> Arc<Self> {
        Arc::new(Self::new())
    }

    pub fn with_name(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::new()
        }
    }
}

impl Default for StreamerContext {
    fn default() -> Self {
        Self::new()
    }
}
