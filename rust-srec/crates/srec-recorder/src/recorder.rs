//! Single-streamer recording control plane.
//!
//! This module contains the pure synchronous [`StreamerFsm`] state machine,
//! serialized SQLite [`RecorderStore`], [`PlatformExtractorRunner`] /
//! [`RecorderProber`], non-blocking `StreamerActor`, and [`ActorRegistry`].

pub mod actor;
pub mod fsm;
pub mod ports;
pub mod prober;
pub mod registry;
pub mod store;

pub use actor::startup_task::StartupLeaseRegistry;
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
    OBSERVE_RETIREMENT, OutboxEvent, PipelineDrainStatus, PipelineNotification, ProbeFailure,
    ProbePurpose, ProbeRequest, ProbeStatus, RecorderDanmuPort, RecorderDownloadPort,
    RecorderPipelinePort, RecorderProberPort, RecorderRuntimeSink, RecorderStorePort,
    ResolvedActorConfig, RetirementBounds, SessionStartDisposition, StreamOnlineKind,
    StreamerRetirement, StreamerRuntimeView, VideoSegmentRecord, should_spawn_actor,
};
pub use prober::{PlatformExtractorRunner, RecorderProber, RecorderProberDependencies};
pub use registry::{ActorRegistry, ActorRegistryDependencies};
pub use store::{PostPublicationChangeSet, PostPublicationListener, RecorderStore};
