//! Scheduler module for orchestrating monitoring tasks.
//!
//! The Scheduler is responsible for:
//! - Spawning monitoring actors for all active streamers
//! - Reacting to configuration update events dynamically
//! - Coordinating batch detection for batch-capable platforms
//! - Managing actor lifecycle with crash recovery
//! - Implementing graceful shutdown
//! - Monitoring system resources (disk space)
//!
//! # Actor Model
//!
//! The scheduler uses an actor-based architecture where:
//! - Each streamer is managed by a `StreamerActor` that handles its own timing
//! - Batch-capable platforms have a `PlatformActor` for coordinating batch detection
//! - The `Scheduler` acts as a supervisor, spawning and monitoring actors
//! - Actors manage their own scheduling internally, eliminating periodic re-scheduling

pub mod actor;
mod batch;
mod resource;
mod service;
mod task;

// Actor model exports (primary API)
pub use actor::{
    ActorError, ActorHandle, ActorMetadata, ActorMetrics, ActorOutcome, ActorRegistry,
    ActorResult, ActorTaskResult, BatchDetectionResult, CheckResult, ConfigRouter, ConfigScope,
    PersistedActorState, PersistedConfig, PlatformActorState, PlatformConfig, PlatformMapping,
    PlatformMessage, RegistryError, RestartTracker, RestartTrackerConfig, RestartTrackerStats,
    RoutingResult, SendError, ShutdownReport, SpawnError, StreamerActor, StreamerActorState,
    StreamerConfig, StreamerMessage, Supervisor, SupervisorConfig, SupervisorMessage,
    SupervisorStats, TaskCompletionAction,
};

// Batch coordination
pub use batch::BatchGroup;

// Resource monitoring
pub use resource::{DiskInfo, DiskSpaceStatus, ResourceMonitor};

// Scheduler service
pub use service::{Scheduler, SchedulerConfig};

// Legacy task types (kept for compatibility, may be deprecated)
pub use task::{MonitoringTask, TaskHandle, TaskStatus};
