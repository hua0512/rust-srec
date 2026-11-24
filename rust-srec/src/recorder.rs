//! Single-streamer recording control plane.
//!
//! This module contains the pure synchronous [`StreamerFsm`] state machine and
//! the narrow domain ports that separate recording control decisions from I/O,
//! persistence, download engines, danmu collection, and post-processing.

pub mod fsm;
pub mod ports;
pub mod store;

pub use fsm::{
    AttemptPhase, BlockedKind, DanmuSessionState, ErrorTracker, FilteredKind, FsmEffect, FsmInput,
    InFlightProbe, MonitorPhase, OfflineConfirmation, OpenSession, PendingDbKind, PendingDbOp,
    QuietPeriod, RecoveredMedia, SessionSegmentGuard, SessionSlot, SlidingFailureWindow,
    StepContext, StopKind, StoppingState, StreamerFsm,
};
pub use ports::{
    ActorStopReason, AttemptStartFailure, AttemptStartSpec, DanmuCloseSegmentSpec,
    DanmuSegmentRecord, DanmuStopKind, DanmuStopResult, DbCommitFailure, DbCommitOp,
    DbCommitOutcome, INTERACTIVE_RETIREMENT, InitialStreamerSnapshot, LiveProbePayload,
    OBSERVE_RETIREMENT, OutboxEvent, PipelineNotification, ProbeFailure, ProbePurpose,
    ProbeRequest, ProbeStatus, RecorderDanmuPort, RecorderDownloadPort, RecorderPipelinePort,
    RecorderProberPort, RecorderRuntimeSink, RecorderStorePort, ResolvedActorConfig,
    RetirementBounds, SessionStartDisposition, StreamOnlineKind, StreamerRuntimeView,
    VideoSegmentRecord, should_spawn_actor,
};
pub use store::{PostPublicationChangeSet, PostPublicationListener, RecorderStore};
