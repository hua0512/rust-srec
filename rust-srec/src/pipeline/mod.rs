//! Pipeline Manager module for post-processing downloaded streams.
//!
//! The Pipeline Manager is responsible for:
//! - Managing a database-backed job queue
//! - Running worker pools for CPU and IO-bound tasks
//! - Processing files through configurable pipelines
//! - Handling backpressure and queue monitoring
//! - Automatic purging of old completed/failed jobs
//! - Download throttling based on queue depth
//! - DAG pipeline support with fan-in/fan-out

mod coordination;
mod dag_scheduler;
mod job_queue;
mod manager;
mod processors;
mod progress;
mod purge;
mod throttle;
mod worker_pool;

pub use coordination::{SegmentOutput, SessionCompleteCoordinator, SessionOutputs, SourceType};
pub use dag_scheduler::{DagCreationResult, DagScheduler};
pub use job_queue::{
    Job, JobExecutionInfo, JobLogEntry, JobQueue, JobQueueConfig, JobResult, JobStats, JobStatus,
    LogLevel, QueueDepthStatus,
};
pub use manager::{
    PipelineCreationResult, PipelineEvent, PipelineManager, PipelineManagerConfig, PipelineStats,
};
pub use processors::{
    CopyMoveConfig, CopyMoveOperation, CopyMoveProcessor, ExecuteCommandProcessor, Processor,
    ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, RcloneProcessor,
    RemuxProcessor, ThumbnailProcessor,
};
pub use progress::{JobProgressSnapshot, ProgressKind, ProgressReporter};
pub use purge::{JobPurgeService, PurgeConfig};
pub use throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
pub use worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
