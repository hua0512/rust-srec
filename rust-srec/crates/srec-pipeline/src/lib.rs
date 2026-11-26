//! Pipeline Manager crate for post-processing downloaded streams.
//!
//! The Pipeline Manager is responsible for:
//! - Managing a database-backed job queue
//! - Running worker pools for CPU, IO, and upload tasks
//! - Processing files through configurable DAG pipelines
//! - Handling backpressure and queue monitoring
//! - Automatic purging of old completed/failed jobs
//! - Download throttling based on queue depth
//! - Persisting DAG, job, staged-output, and upload-record state

mod coordination;
mod dag_scheduler;
pub mod error;
mod job_queue;
mod manager;
mod manifest;
mod ports;
pub use srec_processors::processors;
mod progress;
pub mod repositories;
mod staged_outputs;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod throttle;
mod upload_events;
mod worker_pool;

pub use coordination::{
    PipelineCommand, PipelineCoordinationEvent, PipelineCoordinator, SegmentOutput,
    SessionCoordinationOutstanding, SessionOutputs, SourceType,
};
pub use dag_scheduler::{DagCreationResult, DagScheduler};
pub use error::{PipelineError, PipelineResult};
pub use job_queue::db_model_to_job;
pub use job_queue::{
    ActiveJobInfo, Job, JobExecutionInfo, JobFailureOutcome, JobLogEntry, JobQueue, JobQueueConfig,
    JobResult, JobStats, LogLevel, QueueDepthStatus,
};
#[doc(hidden)]
pub use manager::override_retry_sweep_interval;
pub use manager::{
    DagAnalysis, DagRetryResult, PipelineCreationResult, PipelineEvent, PipelineManager,
    PipelineManagerConfig, PipelineRuntimeDependencies, PipelineStats, RecoveryReport,
    SessionDrain, SessionPipelineOutstanding, StreamerDrain,
};
pub use manifest::{ManifestScope, ManifestSegment, PipelineInputManifest};
pub use ports::{
    PipelineNotification, PipelineSessionMeta, PipelineSessionPort, PipelineSessionSegment,
    PipelineSessionSummary, PipelineStreamerMeta, SessionPipelineConfig, SessionPipelineConfigPort,
};
pub use processors::{
    AssBurnInConfig, AssBurnInProcessor, AssMatchStrategy, BaiduPcsConfig, BaiduPcsPolicy,
    BaiduPcsProcessor, ConfirmedUpload, CopyMoveConfig, CopyMoveOperation, CopyMoveProcessor,
    DanmakuFactoryConfig, DanmakuFactoryProcessor, ExecuteCommandProcessor, LocalFileFingerprint,
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, RcloneProcessor,
    RemuxProcessor, ThumbnailProcessor, UploadItemStatus, UploadResultItem, ZipArchiveEntry,
    ZipArchiveError, write_streaming_zip,
};
pub use progress::{JobPhase, JobProgressSnapshot, ProgressKind, ProgressReporter};
pub use srec_sqlite::models::JobStatus;
pub(crate) use staged_outputs::StagedOutputRecorder;
pub use staged_outputs::{is_temp_path_for, remove_leftover};
pub use throttle::{DownloadLimitAdjuster, ThrottleConfig, ThrottleController, ThrottleEvent};
pub use upload_events::{
    UploadBroadcastEnvelope, UploadStatusBroadcaster, UploadStatusEvent, UploadTerminalStatus,
    UploadWsEncoder,
};
#[cfg(any(test, feature = "test-support"))]
pub use worker_pool::override_shutdown_drain_grace;
pub use worker_pool::{StalledOperation, WorkerPool, WorkerPoolConfig, WorkerType};
