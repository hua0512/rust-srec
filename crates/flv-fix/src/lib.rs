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
//! - `adapter`: Adapters for integrating with the generic pipeline infrastructure
//! - `analyzer`: Tools for analyzing FLV stream structure and content
//! - `operators`: Modular pipeline operators for stream transformations
//! - `pipeline`: Stream processing pipeline implementation
//! - `script_modifier`: Utilities for manipulating FLV script tags
//! - `utils`: Helper functions and utilities
//! - `writer_task`: Asynchronous FLV writing functionality

mod adapter;
mod analyzer;
mod operators;
mod pipeline;
mod script_modifier;
mod utils;
mod writer_task;

#[cfg(test)]
pub mod test_utils;

pub use adapter::flv_error_to_pipeline_error;
pub use analyzer::*;
pub use operators::*;
pub use pipeline::*;
pub use script_modifier::*;
pub use utils::*;
pub use writer_task::*;
