//! Scheduler module for orchestrating monitoring tasks.
//!
//! The Scheduler is responsible for:
//! - Spawning monitoring actors for all active streamers
//! - Reacting to configuration update events dynamically
//! - Managing actor lifecycle with crash recovery
//! - Implementing graceful shutdown
//!
//! # Actor Model
//!
//! The scheduler uses an actor-based architecture where:
//! - Each streamer is managed by a `StreamerActor` that handles its own timing
//!   and checks its streamer's status individually
//! - The `Scheduler` acts as a supervisor, spawning and monitoring actors
//! - Actors manage their own scheduling internally, eliminating periodic re-scheduling

pub mod actor;
mod service;

// Actor model exports (primary API)
pub use actor::{
    ActorError, ActorHandle, ActorMetadata, ActorOutcome, ActorRegistry, ActorResult,
    ActorTaskResult, CheckResult, RegistryError, RestartTracker, RestartTrackerConfig,
    RestartTrackerStats, SendError, ShutdownReport, SpawnError, StreamerActor, StreamerActorState,
    StreamerConfig, StreamerMessage, Supervisor, SupervisorConfig, SupervisorStats,
    TaskCompletionAction,
};

// Scheduler service
pub use service::{Scheduler, SchedulerConfig, SchedulerHandle};

pub(crate) mod feedback;
