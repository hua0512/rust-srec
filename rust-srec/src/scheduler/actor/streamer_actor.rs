//! StreamerActor implementation.
//!
//! The StreamerActor is a self-managing actor that handles monitoring for a single streamer.
//! It manages its own timing, state transitions, and configuration updates without
//! requiring external coordination.
//!
//! # Responsibilities
//!
//! - Self-scheduling: Determines when to perform the next check based on state
//! - Message handling: Processes CheckStatus, ConfigUpdate, BatchResult, Stop, GetState
//! - Fault isolation: Failures don't affect other actors
//!
//! # State Management
//!
//! The actor fetches streamer metadata on-demand from the shared metadata store
//! rather than storing configuration separately. The database-backed metadata
//! cache and the actor's local scheduling state serve distinct purposes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use super::handle::{ActorHandle, ActorMetadata, DEFAULT_MAILBOX_CAPACITY};
use super::messages::{
    BatchDetectionResult, CheckResult, PlatformMessage, StreamerActorState, StreamerConfig,
    StreamerMessage,
};
use super::metrics::ActorMetrics;
use super::monitor_adapter::StatusChecker;
use crate::domain::{Priority, StreamerState};
use crate::downloader::DownloadStopCause;
use crate::monitor::{LiveStatus, ProcessStatusResult, ProcessStatusSuppression};
use crate::scheduler::actor::DownloadEndPolicy;
use crate::streamer::StreamerMetadata;

mod batch;
mod checks;
mod feedback;
mod mailbox;
mod run;
mod terminal;
mod wake;

/// Result type for actor operations.
pub type ActorResult = Result<ActorOutcome, ActorError>;

/// Outcome of an actor's run loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorOutcome {
    /// Actor stopped gracefully.
    Stopped,
    /// Actor was cancelled.
    Cancelled,
    /// Actor completed its work.
    Completed,
}

/// Error type for actor operations.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ActorError {
    /// Error message.
    pub message: String,
    /// Whether this error is recoverable.
    pub recoverable: bool,
}

impl ActorError {
    /// Create a new recoverable error.
    pub fn recoverable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recoverable: true,
        }
    }

    /// Create a new fatal error.
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            recoverable: false,
        }
    }
}

/// A self-managing actor for monitoring a single streamer.
///
/// The StreamerActor handles its own timing and state management,
/// eliminating the need for external coordination or periodic re-scheduling.
///
/// Instead of storing metadata locally (which can drift), the actor fetches
/// fresh metadata from the shared metadata store on each check.
pub struct StreamerActor {
    feedback_sequence: u64,
    config_revision: u64,
    current_download: Option<(String, String)>,
    /// Actor identifier (streamer ID).
    id: String,
    /// Mailbox for receiving normal-priority messages.
    mailbox: mpsc::Receiver<StreamerMessage>,
    /// Mailbox for receiving high-priority messages (checked first).
    priority_mailbox: Option<mpsc::Receiver<StreamerMessage>>,
    /// Handle for sending messages to self (for self-scheduling).
    #[expect(
        dead_code,
        reason = "retained for optional runtime paths and diagnostics"
    )]
    self_handle: mpsc::Sender<StreamerMessage>,
    /// Platform actor handle (if on batch-capable platform).
    platform_actor: Option<mpsc::Sender<PlatformMessage>>,
    /// Current actor state (runtime scheduling state only).
    state: StreamerActorState,
    /// Hydration resets Live metadata but keeps unfinished database sessions.
    /// Apply one initial live/offline observation before suppressing redundant
    /// offline checks; failed or suppressed writes must leave recovery pending.
    initial_status_pending: bool,
    /// Floor for the next Live watchdog wake after a failed watchdog check.
    ///
    /// `perform_check` sets this when `is_live_watchdog` is true and the check
    /// errors; the Live scheduling branch of `run` clamps its computed wake up
    /// to this instant so a failing check does not busy-retry against the stall
    /// timer. Cleared on the next successful check.
    live_watchdog_backoff_until: Option<Instant>,
    /// Shared metadata store for fetching fresh streamer data.
    metadata_store: Arc<DashMap<String, Arc<StreamerMetadata>>>,
    /// Configuration.
    config: StreamerConfig,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Metrics handle.
    metrics: ActorMetrics,
    /// Status checker for performing actual status checks.
    status_checker: Arc<dyn StatusChecker>,
}

/// Default priority mailbox capacity (smaller than normal mailbox).
pub const DEFAULT_PRIORITY_MAILBOX_CAPACITY: usize = DEFAULT_MAILBOX_CAPACITY / 4;

impl StreamerActor {
    /// Create a new StreamerActor with a status checker.
    ///
    /// # Arguments
    ///
    /// * `streamer_id` - The streamer ID
    /// * `metadata_store` - Shared metadata store for fetching fresh streamer data
    /// * `config` - Actor configuration
    /// * `cancellation_token` - Token for graceful shutdown
    /// * `status_checker` - Status checker for performing actual status checks
    pub fn new(
        streamer_id: String,
        metadata_store: Arc<DashMap<String, Arc<StreamerMetadata>>>,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        status_checker: Arc<dyn StatusChecker>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
        let is_high_priority = config.priority == Priority::High;

        let actor_metadata = ActorMetadata::streamer(&streamer_id, is_high_priority);
        let handle = ActorHandle::new(tx.clone(), cancellation_token.clone(), actor_metadata);

        // Get initial state from metadata store
        let state = metadata_store
            .get(&streamer_id)
            .map(|m| StreamerActorState::from_metadata(&m))
            .unwrap_or_default();
        let metrics = ActorMetrics::new(&streamer_id, DEFAULT_MAILBOX_CAPACITY);

        let actor = Self {
            feedback_sequence: 0,
            config_revision: 0,
            current_download: None,
            id: streamer_id,
            mailbox: rx,
            priority_mailbox: None,
            self_handle: tx,
            platform_actor: None,
            state,
            initial_status_pending: true,
            live_watchdog_backoff_until: None,
            metadata_store,
            config,
            cancellation_token,
            metrics,
            status_checker,
        };

        (actor, handle)
    }

    /// Create a new StreamerActor with priority channel support.
    ///
    /// High-priority messages are processed before normal messages,
    /// ensuring critical operations (like Stop) are handled promptly
    /// even under backpressure.
    pub fn with_priority_channel(
        streamer_id: String,
        metadata_store: Arc<DashMap<String, Arc<StreamerMetadata>>>,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        status_checker: Arc<dyn StatusChecker>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
        let (priority_tx, priority_rx) = mpsc::channel(DEFAULT_PRIORITY_MAILBOX_CAPACITY);
        let is_high_priority = config.priority == Priority::High;

        let actor_metadata = ActorMetadata::streamer(&streamer_id, is_high_priority);
        let handle = ActorHandle::with_priority(
            tx.clone(),
            priority_tx,
            cancellation_token.clone(),
            actor_metadata,
        );

        // Get initial state from metadata store
        let state = metadata_store
            .get(&streamer_id)
            .map(|m| StreamerActorState::from_metadata(&m))
            .unwrap_or_default();
        let metrics = ActorMetrics::new(&streamer_id, DEFAULT_MAILBOX_CAPACITY);

        let actor = Self {
            feedback_sequence: 0,
            config_revision: 0,
            current_download: None,
            id: streamer_id,
            mailbox: rx,
            priority_mailbox: Some(priority_rx),
            self_handle: tx,
            platform_actor: None,
            state,
            initial_status_pending: true,
            live_watchdog_backoff_until: None,
            metadata_store,
            config,
            cancellation_token,
            metrics,
            status_checker,
        };

        (actor, handle)
    }

    /// Create a new StreamerActor with priority channel and platform actor.
    pub fn with_priority_and_platform(
        streamer_id: String,
        metadata_store: Arc<DashMap<String, Arc<StreamerMetadata>>>,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        platform_actor: mpsc::Sender<PlatformMessage>,
        status_checker: Arc<dyn StatusChecker>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (mut actor, handle) = Self::with_priority_channel(
            streamer_id,
            metadata_store,
            config,
            cancellation_token,
            status_checker,
        );
        actor.platform_actor = Some(platform_actor);
        (actor, handle)
    }

    /// Get the actor's ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the current state.
    pub fn state(&self) -> &StreamerActorState {
        &self.state
    }

    /// Get the configuration.
    pub fn config(&self) -> &StreamerConfig {
        &self.config
    }

    /// Check if this actor uses batch detection.
    pub fn uses_batch_detection(&self) -> bool {
        self.platform_actor.is_some() && self.config.batch_capable
    }

    /// Get the current streamer metadata from the shared store.
    ///
    /// Returns None if the streamer has been removed from the store.
    fn get_metadata(&self) -> Option<Arc<StreamerMetadata>> {
        self.metadata_store
            .get(&self.id)
            .map(|r| Arc::clone(r.value()))
    }

    /// Get the current error count from metadata, defaulting to 0 if not found.
    fn get_error_count(&self) -> u32 {
        self.metadata_store
            .get(&self.id)
            .map(|m| m.consecutive_error_count as u32)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests;
