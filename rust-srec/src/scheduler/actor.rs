//! Actor-based scheduler infrastructure.
//!
//! This module provides the actor model implementation for the scheduler,
//! enabling self-managing streamer actors with fault isolation and
//! targeted configuration updates.
//!
//! # Architecture
//!
//! - `StreamerActor`: Self-managing actor for individual streamer monitoring
//! - `ActorHandle`: Type-safe handle for sending messages to actors
//! - `ActorRegistry`: Centralized actor tracking and task management

mod handle;
mod messages;
mod monitor_adapter;
mod registry;
mod restart_tracker;
mod streamer_actor;
mod supervisor;

pub use handle::{ActorHandle, ActorMetadata, SendError};
pub use messages::{
    CheckResult, DownloadEndPolicy, StreamerActorState, StreamerConfig, StreamerMessage,
};
pub use monitor_adapter::{CheckError, MonitorStatusChecker, NoOpStatusChecker, StatusChecker};
pub use registry::{
    ActorRegistry, ActorRemoval, ActorRemovalOutcome, ActorTaskResult, RegistryError,
};
pub use restart_tracker::{RestartTracker, RestartTrackerConfig, RestartTrackerStats};
pub use streamer_actor::{ActorError, ActorOutcome, ActorResult, StreamerActor};
pub use supervisor::{
    ShutdownReport, SpawnError, Supervisor, SupervisorConfig, SupervisorStats, TaskCompletionAction,
};
