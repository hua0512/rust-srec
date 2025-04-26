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
pub mod operators;
pub mod pipeline;

// Re-export commonly used types from the enhanced pipeline
pub use pipeline::{HlsPipeline, PipelineConfig};
