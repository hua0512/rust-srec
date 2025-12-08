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

use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

pub mod cancellation;
pub mod channel_pipeline;
pub mod config;
mod context;
pub mod pipeline;
pub mod processor;
pub mod progress;
mod utils;
mod writer_task;

pub use channel_pipeline::ChannelPipeline;
/// Re-export key traits and types
pub use context::{Statistics, StreamerContext};
pub use pipeline::Pipeline;
pub use processor::Processor;
pub use progress::{Progress, ProgressEvent};
pub use utils::{expand_filename_template, sanitize_filename};

pub use writer_task::{
    FormatStrategy, PostWriteAction, ProgressCallback, ProgressConfig, TaskError, WriterConfig,
    WriterError, WriterProgress, WriterState, WriterTask,
};

use crate::config::PipelineConfig;
pub use cancellation::CancellationToken;

/// Common error type for pipeline operations
#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Processing error: {0}")]
    Processing(String),

    #[error("Invalid data: {0}")]
    InvalidData(String),

    #[error("Operation was cancelled")]
    Cancelled,
}

pub trait ProtocolWriter: Send + 'static {
    type Item: Send + 'static;
    type Stats: Send + 'static;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Creates a new writer instance.
    /// Progress tracking is now handled via tracing spans instead of callbacks.
    fn new(
        output_dir: PathBuf,
        base_name: String,
        extension: String,
        extras: Option<HashMap<String, String>>,
    ) -> Self;

    fn get_state(&self) -> &WriterState;

    fn run(
        &mut self,
        input_stream: tokio::sync::mpsc::Receiver<Result<Self::Item, PipelineError>>,
    ) -> Result<Self::Stats, Self::Error>;
}

pub trait PipelineProvider: Send + 'static {
    type Item: Send + 'static;
    type Config: Send + 'static;

    fn with_config(
        context: StreamerContext,
        common_config: &PipelineConfig,
        config: Self::Config,
    ) -> Self;

    fn build_pipeline(&self) -> ChannelPipeline<Self::Item>;
}
