//! Pipeline Manager module for post-processing downloaded streams.
//!
//! The Pipeline Manager is responsible for:
//! - Managing a database-backed job queue
//! - Running worker pools for CPU and IO-bound tasks
//! - Processing files through configurable pipelines
//! - Handling backpressure and queue monitoring

mod job_queue;
mod manager;
mod processors;
mod worker_pool;

pub use job_queue::{Job, JobQueue, JobQueueConfig, JobResult, JobStats, JobStatus, QueueDepthStatus};
pub use manager::{PipelineCreationResult, PipelineEvent, PipelineManager, PipelineManagerConfig, PipelineStats};
pub use processors::{
    ExecuteCommandProcessor, Processor, ProcessorInput, ProcessorOutput, ProcessorType,
    RemuxProcessor, ThumbnailProcessor, UploadProcessor,
};
pub use worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
