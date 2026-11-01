//! HLS stream processing library
//!
//! This crate provides tools and components for processing and analyzing HLS (HTTP Live Streaming)
//! streams.
//!
//! ## Features
//!
//! - Pipeline-based processing architecture
//! - Configurable processing operators
//! - TS and fMP4 output writing
//!
//! ## Component Overview
//!
//! - `analyzer`: Segment analysis and statistics
//! - `operators`: Defragmentation, segment splitting and segment limits
//! - `pipeline`: HLS processing pipeline implementation
//! - `HlsWriter`: TS/fMP4 writer task with file rotation

pub mod analyzer;
pub mod operators;
mod output_state;
pub mod pipeline;
mod writer_task;

#[cfg(test)]
mod test_support;

pub use pipeline::{HlsPipeline, HlsPipelineConfig};
pub use writer_task::{HlsWriter, HlsWriterConfig};
