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
mod throttle;
mod worker_pool;

pub use crate::database::models::JobStatus;
pub use coordination::{
    PipelineCommand, PipelineCoordinationEvent, PipelineCoordinator, SegmentOutput, SessionOutputs,
    SourceType,
};
pub use dag_scheduler::{DagCreationResult, DagScheduler};
pub use job_queue::{
    Job, JobExecutionInfo, JobLogEntry, JobQueue, JobQueueConfig, JobResult, JobStats, LogLevel,
    QueueDepthStatus,
};
pub(crate) use manager::PipelineRuntimeDependencies;
pub use manager::{
    PipelineCreationResult, PipelineEvent, PipelineManager, PipelineManagerConfig, PipelineStats,
};
pub use processors::{
    AssBurnInConfig, AssBurnInProcessor, AssMatchStrategy, CopyMoveConfig, CopyMoveOperation,
    CopyMoveProcessor, DanmakuFactoryConfig, DanmakuFactoryProcessor, ExecuteCommandProcessor,
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, RcloneProcessor,
    RemuxProcessor, ThumbnailProcessor,
};
pub use progress::{JobProgressSnapshot, ProgressKind, ProgressReporter};
pub use throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
pub use worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
