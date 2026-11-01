//! FLV stream processing and fixing library
//!
//! This crate provides tools and components for processing, analyzing, and fixing FLV (Flash Video)
//! streams. It includes functionality for handling common FLV issues like timing inconsistencies,
//! fragmentation, and metadata problems.
//!
//! ## Features
//!
//! - FLV stream analysis and validation
//! - Stream repair: defragmentation, header checks, GOP sorting, duplicate
//!   filtering, timestamp repair, size/duration splitting
//! - Pipeline-based processing architecture with configurable operators
//! - `onMetaData` patching (duration, keyframe index) when an output file closes
//!
//! ## Component Overview
//!
//! - `amf`: `onMetaData` model and builder used to patch closed files
//! - `analyzer`: Tools for analyzing FLV stream structure and content
//! - `constants`: String constants to avoid repeated allocations
//! - `operators`: Modular pipeline operators for stream transformations
//! - `pipeline`: `FlvPipeline`, which fixes the operator order
//! - `script_modifier`: Post-hoc patcher for FLV script tags in finished files
//! - `utils`: Helper functions and utilities
//! - `writer`: `FlvWriter`, the protocol writer for FLV output
//! - `writer_task`: FLV format strategy for the shared writer task (file
//!   rotation, metadata patching)

pub mod amf;
mod analyzer;
mod constants;
mod operators;
mod pipeline;
mod script_modifier;
mod utils;
pub mod writer;
mod writer_task;

#[cfg(test)]
pub mod test_utils;

pub use analyzer::{AnalyzerError, FlvAnalyzer};
pub use constants::*;
pub use operators::*;
pub use pipeline::*;
pub use script_modifier::*;
pub use utils::*;

pub use crate::writer::FlvWriter;
pub use crate::writer_task::{FlvFormatStrategy, FlvStrategyError, FlvWriterConfig};
