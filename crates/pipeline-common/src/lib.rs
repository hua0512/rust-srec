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

use std::sync::Arc;

use thiserror::Error;

pub mod cancellation;
pub mod channel_pipeline;
pub mod config;
mod context;
pub mod pipeline;
pub mod processor;
pub mod progress;
mod run_completion;
mod utils;
mod writer_task;

/// Re-export key traits and types
pub use channel_pipeline::ChannelPipeline;
pub use context::StreamerContext;
pub use pipeline::Pipeline;
pub use processor::Processor;
pub use progress::{Progress, ProgressEvent};
pub use run_completion::{RunCompletionError, settle_run};
pub use utils::{
    expand_filename_template, expand_path_template, expand_path_template_at, sanitize_filename,
};

pub use writer_task::{
    FormatStrategy, PostWriteAction, ProgressCallback, ProgressConfig, WriterConfig, WriterError,
    WriterProgress, WriterState, WriterStats, WriterTask,
};

use crate::config::PipelineConfig;
pub use cancellation::CancellationToken;

/// Common error type for pipeline operations
#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Operation was cancelled")]
    Cancelled,

    #[error("Channel closed: {0}")]
    ChannelClosed(&'static str),

    #[error("{0}")]
    Strategy(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Stage process failed ({stage}): {source}")]
    StageProcess {
        stage: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Stage finish failed ({stage}): {source}")]
    StageFinish {
        stage: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub trait ProtocolWriter: Send + 'static {
    type Item: Send + 'static;

    fn get_state(&self) -> &WriterState;

    fn run(
        &mut self,
        input: tokio::sync::mpsc::Receiver<Result<Self::Item, PipelineError>>,
    ) -> Result<WriterStats, WriterError>;
}

pub trait PipelineProvider: Send + 'static {
    type Item: Send + 'static;
    type Config: Send + 'static;

    fn with_config(
        context: Arc<StreamerContext>,
        common_config: &PipelineConfig,
        config: Self::Config,
    ) -> Self;

    fn build_pipeline(&self) -> ChannelPipeline<Self::Item>;
}
