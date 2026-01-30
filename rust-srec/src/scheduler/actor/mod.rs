//! Actor-based scheduler infrastructure.
//!
//! This module provides the actor model implementation for the scheduler,
//! enabling self-managing streamer actors with fault isolation and
//! targeted configuration updates.
//!
//! # Architecture
//!
//! - `StreamerActor`: Self-managing actor for individual streamer monitoring
//! - `PlatformActor`: Coordinates batch detection for batch-capable platforms
//! - `ActorHandle`: Type-safe handle for sending messages to actors
//! - `ActorMetrics`: Per-actor metrics collection
//! - `ActorRegistry`: Centralized actor tracking and task management
//! - `ConfigRouter`: Routes configuration updates to appropriate actors

mod config_router;
mod handle;
mod messages;
mod metrics;
mod monitor_adapter;
mod platform_actor;
mod registry;
mod restart_tracker;
mod streamer_actor;
mod supervisor;

pub use config_router::{ConfigRouter, ConfigScope, PlatformMapping, RoutingPlan, RoutingResult};
pub use handle::{ActorHandle, ActorMetadata, SendError};
pub use messages::{
    BatchDetectionResult, CheckResult, DownloadEndPolicy, PlatformActorState, PlatformConfig,
    PlatformMessage, StreamerActorState, StreamerConfig, StreamerMessage, SupervisorMessage,
};
pub use metrics::{
    ActorMetrics, ActorType, LifecycleEvent, MetricsSnapshot, SchedulerMetrics,
    SchedulerMetricsSnapshot, SharedActorMetrics, SharedSchedulerMetrics, create_metrics,
    create_scheduler_metrics,
};
pub use monitor_adapter::{
    BatchChecker, CheckError, MonitorBatchChecker, MonitorStatusChecker, NoOpBatchChecker,
    NoOpStatusChecker, StatusChecker,
};
pub use platform_actor::PlatformActor;
pub use registry::{ActorRegistry, ActorTaskResult, RegistryError};
pub use restart_tracker::{RestartTracker, RestartTrackerConfig, RestartTrackerStats};
pub use streamer_actor::{
    ActorError, ActorOutcome, ActorResult, PersistedActorState, PersistedConfig, StreamerActor,
};
pub use supervisor::{
    ShutdownReport, SpawnError, Supervisor, SupervisorConfig, SupervisorStats, TaskCompletionAction,
};
