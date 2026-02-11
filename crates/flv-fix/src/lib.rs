//! FLV stream processing and fixing library
//!
//! This crate provides tools and components for processing, analyzing, and fixing FLV (Flash Video)
//! streams. It includes functionality for handling common FLV issues like timing inconsistencies,
//! fragmentation, and metadata problems.
//!
//! ## Features
//!
//! - FLV stream analysis and validation
//! - Stream repair and fixing capabilities
//! - Pipeline-based processing architecture
//! - Configurable processing operators
//! - Async and sync processing modes
//! - Metadata manipulation and script tag handling
//!
//! ## Component Overview
//!
//! - `analyzer`: Tools for analyzing FLV stream structure and content
//! - `constants`: String constants to avoid repeated allocations
//! - `operators`: Modular pipeline operators for stream transformations
//! - `pipeline`: Stream processing pipeline implementation
//! - `script_modifier`: Utilities for manipulating FLV script tags
//! - `utils`: Helper functions and utilities
//! - `writer`: Asynchronous FLV writing functionality

pub mod amf;
mod analyzer;
mod constants;
mod crc32;
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
