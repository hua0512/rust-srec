//! HLS stream processing library
//!
//! This crate provides tools and components for processing and analyzing HLS (HTTP Live Streaming)
//! streams.
//!
//! ## Features
//!
//! - Pipeline-based processing architecture
//! - Configurable processing operators
//!
//! ## Component Overview
//!
//! - `pipeline`: HLS processing pipeline implementation

mod adapter;
pub mod analyzer;
pub mod operators;
pub mod pipeline;
pub mod writer_task;

// Re-export commonly used types from the enhanced pipeline
pub use adapter::detect_and_create_hls_data;
pub use operators::{SegmentLimiterOperator, SegmentSplitOperator};
pub use pipeline::{HlsPipeline, HlsPipelineConfig};
