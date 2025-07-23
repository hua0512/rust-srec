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

pub mod analyzer;
pub mod operators;
pub mod pipeline;
mod writer_task;

pub use pipeline::{HlsPipeline, HlsPipelineConfig};
pub use writer_task::HlsWriter;
