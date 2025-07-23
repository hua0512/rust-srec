//! # Pipeline Common
//!
//! This crate provides common abstractions for building media processing pipelines.
//! It defines generic traits and implementations that can be used across different
//! types of media processors, including FLV and HLS streams.
//!
//! ## Features
//!
//! - Generic `Processor<T>` trait for processing any type of data
//! - Generic `Pipeline<T>` implementation for chaining processors
//! - Common error types and context sharing utilities
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use thiserror::Error;

pub mod context;
pub mod pipeline;
pub mod processor;
pub mod progress;
mod template;
pub mod test_utils;
mod writer_task_common;

/// Re-export key traits and types
pub use context::StreamerContext;
pub use pipeline::Pipeline;
pub use processor::Processor;
pub use progress::{OnProgress, Progress, ProgressEvent};
pub use template::{expand_filename_template, sanitize_filename};
pub use writer_task_common::{
    FormatStrategy, PostWriteAction, TaskError, WriterConfig, WriterState, WriterTask,
};

pub use test_utils::create_test_context;

/// Common error type for pipeline operations
#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Processing error: {0}")]
    Processing(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),
}
