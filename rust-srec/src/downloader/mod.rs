//! Download Manager module for stream recording.
//!
//! The Download Manager is responsible for:
//! - Managing concurrent downloads with configurable limits
//! - Abstracting download engines (ffmpeg, streamlink, mesio)
//! - Handling segment completion events
//! - Implementing retry logic with circuit breaker pattern
//! - Supporting priority-based download scheduling
//! - Stream selection based on quality, format, and CDN preferences

pub mod engine;

mod manager;
pub mod output_root_gate;
pub mod queue;
mod resilience;
mod stream_selector;

pub use engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadInfo,
    IoErrorKindSer, SegmentEvent, SegmentInfo,
};

pub use output_root_gate::{
    DEFAULT_GATE_COOLDOWN_SECS, GateBlocked, LAST_ERROR_GATE_PREFIX, OutputRootGate, RecoveryHook,
    RootHealth, RootHealthState,
};

pub use manager::{
    ConfigUpdateType, DownloadManager, DownloadManagerConfig, DownloadManagerEvent,
    DownloadProgressEvent, DownloadRejectedKind, DownloadStopCause, DownloadTerminalEvent,
    EngineEndSignal, EngineHandle, PreflightRequest,
};
pub use queue::{
    AcquireError, AcquireRequest, ActiveSlot, DownloadQueue, PendingEntry, Priority, SlotGuard,
};
pub use resilience::{CircuitBreaker, EngineKey, RetryConfig};
pub use stream_selector::{StreamSelectionConfig, StreamSelector};
