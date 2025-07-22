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
mod new_writer_task;
pub mod operators;
pub mod pipeline;
mod writer_task;

pub use adapter::detect_and_create_hls_data;
pub use new_writer_task::HlsWriter;
pub use pipeline::{HlsPipeline, HlsPipelineConfig};
