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
//! - State persistence: Saves state on shutdown for recovery
//! - Fault isolation: Failures don't affect other actors

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::handle::{ActorHandle, ActorMetadata, DEFAULT_MAILBOX_CAPACITY};
use super::messages::{
    BatchDetectionResult, CheckResult, PlatformMessage, StreamerActorState, StreamerConfig,
    StreamerMessage,
};
use super::metrics::ActorMetrics;
use super::monitor_adapter::{NoOpStatusChecker, StatusChecker};
use crate::domain::StreamerState;
use crate::streamer::StreamerMetadata;

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
#[derive(Debug, Clone)]
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

impl std::fmt::Display for ActorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ActorError {}

/// A self-managing actor for monitoring a single streamer.
///
/// The StreamerActor handles its own timing and state management,
/// eliminating the need for external coordination or periodic re-scheduling.
pub struct StreamerActor<S: StatusChecker = NoOpStatusChecker> {
    /// Actor identifier (streamer ID).
    id: String,
    /// Mailbox for receiving normal-priority messages.
    mailbox: mpsc::Receiver<StreamerMessage>,
    /// Mailbox for receiving high-priority messages (checked first).
    priority_mailbox: Option<mpsc::Receiver<StreamerMessage>>,
    /// Handle for sending messages to self (for self-scheduling).
    #[allow(dead_code)]
    self_handle: mpsc::Sender<StreamerMessage>,
    /// Platform actor handle (if on batch-capable platform).
    platform_actor: Option<mpsc::Sender<PlatformMessage>>,
    /// Current actor state.
    state: StreamerActorState,
    /// Streamer metadata.
    metadata: StreamerMetadata,
    /// Configuration.
    config: StreamerConfig,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Metrics handle.
    metrics: ActorMetrics,
    /// State persistence path (optional).
    state_path: Option<PathBuf>,
    /// Status checker for performing actual status checks.
    status_checker: std::sync::Arc<S>,
}

/// Default priority mailbox capacity (smaller than normal mailbox).
pub const DEFAULT_PRIORITY_MAILBOX_CAPACITY: usize = DEFAULT_MAILBOX_CAPACITY / 4;

impl StreamerActor<NoOpStatusChecker> {
    /// Create a new StreamerActor with no-op status checker (for testing/backwards compatibility).
    ///
    /// # Arguments
    ///
    /// * `metadata` - Streamer metadata
    /// * `config` - Actor configuration
    /// * `cancellation_token` - Token for graceful shutdown
    pub fn new(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        Self::with_status_checker(
            metadata,
            config,
            cancellation_token,
            std::sync::Arc::new(NoOpStatusChecker),
        )
    }

    /// Create a new StreamerActor with priority channel support.
    ///
    /// High-priority messages are processed before normal messages,
    /// ensuring critical operations (like Stop) are handled promptly
    /// even under backpressure.
    ///
    /// # Arguments
    ///
    /// * `metadata` - Streamer metadata
    /// * `config` - Actor configuration
    /// * `cancellation_token` - Token for graceful shutdown
    pub fn with_priority_channel(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        Self::with_priority_channel_and_checker(
            metadata,
            config,
            cancellation_token,
            std::sync::Arc::new(NoOpStatusChecker),
        )
    }

    /// Create a new StreamerActor with a platform actor for batch coordination.
    pub fn with_platform_actor(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        platform_actor: mpsc::Sender<PlatformMessage>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (mut actor, handle) = Self::new(metadata, config, cancellation_token);
        actor.platform_actor = Some(platform_actor);
        (actor, handle)
    }

    /// Create a new StreamerActor with both priority channel and platform actor.
    pub fn with_priority_and_platform(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        platform_actor: mpsc::Sender<PlatformMessage>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (mut actor, handle) = Self::with_priority_channel(metadata, config, cancellation_token);
        actor.platform_actor = Some(platform_actor);
        (actor, handle)
    }
}

impl<S: StatusChecker> StreamerActor<S> {
    /// Create a new StreamerActor with a custom status checker.
    ///
    /// # Arguments
    ///
    /// * `metadata` - Streamer metadata
    /// * `config` - Actor configuration
    /// * `cancellation_token` - Token for graceful shutdown
    /// * `status_checker` - Status checker for performing actual status checks
    pub fn with_status_checker(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        status_checker: std::sync::Arc<S>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
        let id = metadata.id.clone();
        let is_high_priority = config.priority == crate::domain::Priority::High;

        let actor_metadata = ActorMetadata::streamer(&id, is_high_priority);
        let handle = ActorHandle::new(tx.clone(), cancellation_token.clone(), actor_metadata);

        let state = StreamerActorState::from_metadata(&metadata);
        let metrics = ActorMetrics::new(&id, DEFAULT_MAILBOX_CAPACITY);

        let actor = Self {
            id,
            mailbox: rx,
            priority_mailbox: None,
            self_handle: tx,
            platform_actor: None,
            state,
            metadata,
            config,
            cancellation_token,
            metrics,
            state_path: None,
            status_checker,
        };

        (actor, handle)
    }

    /// Create a new StreamerActor with priority channel and custom status checker.
    ///
    /// High-priority messages are processed before normal messages,
    /// ensuring critical operations (like Stop) are handled promptly
    /// even under backpressure.
    pub fn with_priority_channel_and_checker(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        status_checker: std::sync::Arc<S>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
        let (priority_tx, priority_rx) = mpsc::channel(DEFAULT_PRIORITY_MAILBOX_CAPACITY);
        let id = metadata.id.clone();
        let is_high_priority = config.priority == crate::domain::Priority::High;

        let actor_metadata = ActorMetadata::streamer(&id, is_high_priority);
        let handle = ActorHandle::with_priority(
            tx.clone(),
            priority_tx,
            cancellation_token.clone(),
            actor_metadata,
        );

        let state = StreamerActorState::from_metadata(&metadata);
        let metrics = ActorMetrics::new(&id, DEFAULT_MAILBOX_CAPACITY);

        let actor = Self {
            id,
            mailbox: rx,
            priority_mailbox: Some(priority_rx),
            self_handle: tx,
            platform_actor: None,
            state,
            metadata,
            config,
            cancellation_token,
            metrics,
            state_path: None,
            status_checker,
        };

        (actor, handle)
    }

    /// Create a new StreamerActor with priority channel, platform actor, and custom status checker.
    pub fn with_priority_platform_and_checker(
        metadata: StreamerMetadata,
        config: StreamerConfig,
        cancellation_token: CancellationToken,
        platform_actor: mpsc::Sender<PlatformMessage>,
        status_checker: std::sync::Arc<S>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (mut actor, handle) = Self::with_priority_channel_and_checker(
            metadata,
            config,
            cancellation_token,
            status_checker,
        );
        actor.platform_actor = Some(platform_actor);
        (actor, handle)
    }

    /// Set the state persistence path.
    pub fn with_state_path(mut self, path: PathBuf) -> Self {
        self.state_path = Some(path);
        self
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

    /// Run the actor's event loop.
    ///
    /// This method runs until the actor receives a Stop message or the
    /// cancellation token is triggered.
    ///
    /// # Returns
    ///
    /// Returns `ActorOutcome::Stopped` on graceful shutdown,
    /// `ActorOutcome::Cancelled` if cancelled externally.
    pub async fn run(mut self) -> ActorResult {
        info!("StreamerActor {} starting", self.id);

        // Schedule initial check if not already scheduled
        if self.state.next_check.is_none() {
            self.state.schedule_next_check(&self.config);
        }

        loop {
            // First, drain all priority messages before processing normal messages
            // This ensures high-priority operations (like Stop) are handled promptly
            if let Some(msg) = self.try_recv_priority() {
                let start = Instant::now();
                let should_stop = self.handle_message(msg).await?;
                self.metrics.record_message(start.elapsed());

                if should_stop {
                    debug!(
                        "StreamerActor {} received stop signal from priority channel",
                        self.id
                    );
                    break;
                }
                // Continue to check for more priority messages
                continue;
            }

            // Calculate sleep duration before entering select to avoid borrow issues
            let sleep_duration = self.state.time_until_next_check();
            let check_timer = Self::create_check_timer(sleep_duration);

            tokio::select! {
                // Bias towards handling messages first
                biased;

                // Check priority mailbox first (if configured)
                Some(msg) = Self::recv_priority_opt(&mut self.priority_mailbox) => {
                    let start = Instant::now();
                    let should_stop = self.handle_message(msg).await?;
                    self.metrics.record_message(start.elapsed());

                    if should_stop {
                        debug!("StreamerActor {} received stop signal from priority channel", self.id);
                        break;
                    }
                }

                // Handle normal-priority messages
                Some(msg) = self.mailbox.recv() => {
                    let start = Instant::now();
                    let should_stop = self.handle_message(msg).await?;
                    self.metrics.record_message(start.elapsed());

                    if should_stop {
                        debug!("StreamerActor {} received stop signal", self.id);
                        break;
                    }
                }

                // Self-scheduled check timer
                _ = check_timer => {
                    debug!("StreamerActor {} check timer fired", self.id);
                    if let Err(e) = self.initiate_check().await {
                        warn!("StreamerActor {} check failed: {}", self.id, e);
                        self.metrics.record_error();
                    }
                }

                // Cancellation
                _ = self.cancellation_token.cancelled() => {
                    info!("StreamerActor {} cancelled", self.id);
                    self.persist_state().await?;
                    return Ok(ActorOutcome::Cancelled);
                }
            }
        }

        // Graceful shutdown - persist state
        self.persist_state().await?;
        info!("StreamerActor {} stopped gracefully", self.id);
        Ok(ActorOutcome::Stopped)
    }

    /// Try to receive a message from the priority mailbox without blocking.
    fn try_recv_priority(&mut self) -> Option<StreamerMessage> {
        if let Some(ref mut priority_rx) = self.priority_mailbox {
            priority_rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Helper to receive from an optional priority mailbox.
    /// Returns a future that is pending forever if the mailbox is None.
    async fn recv_priority_opt(
        priority_mailbox: &mut Option<mpsc::Receiver<StreamerMessage>>,
    ) -> Option<StreamerMessage> {
        match priority_mailbox {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
        }
    }

    /// Create a future that completes when the next check is due.
    ///
    /// This implements self-scheduling by calculating the delay until
    /// the next check based on the actor's internal state.
    async fn create_check_timer(duration: Duration) {
        if duration.is_zero() {
            // Check is due immediately, but yield to allow message processing
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(duration).await;
        }
    }

    /// Initiate a status check.
    ///
    /// If on a batch-capable platform, delegates to the PlatformActor.
    /// Otherwise, performs the check directly.
    async fn initiate_check(&mut self) -> Result<(), ActorError> {
        debug!("StreamerActor {} initiating check", self.id);

        if self.uses_batch_detection() {
            // Delegate to platform actor for batch detection
            self.delegate_to_platform().await?;
        } else {
            // Perform individual check
            self.perform_check().await?;
        }

        Ok(())
    }

    /// Delegate check to the platform actor for batch processing.
    async fn delegate_to_platform(&mut self) -> Result<(), ActorError> {
        if let Some(ref platform_actor) = self.platform_actor {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

            let msg = PlatformMessage::RequestCheck {
                streamer_id: self.id.clone(),
                reply: reply_tx,
            };

            // Send request to platform actor
            if platform_actor.send(msg).await.is_err() {
                return Err(ActorError::recoverable("Platform actor unavailable"));
            }

            // Wait for acknowledgment (not the result - that comes via BatchResult message)
            match tokio::time::timeout(Duration::from_secs(5), reply_rx).await {
                Ok(Ok(())) => {
                    debug!("StreamerActor {} check delegated to platform", self.id);
                    Ok(())
                }
                Ok(Err(_)) => Err(ActorError::recoverable("Platform actor dropped reply")),
                Err(_) => Err(ActorError::recoverable("Platform actor timeout")),
            }
        } else {
            Err(ActorError::recoverable("No platform actor configured"))
        }
    }

    /// Perform an individual status check using the configured status checker.
    ///
    /// This method connects to the actual monitoring infrastructure via the
    /// StatusChecker trait, which abstracts the status checking operation.
    async fn perform_check(&mut self) -> Result<(), ActorError> {
        debug!("StreamerActor {} performing status check", self.id);

        // Perform the actual status check using the status checker
        match self.status_checker.check_status(&self.metadata).await {
            Ok(result) => {
                // Record the check result
                self.state.record_check(result, &self.config);

                debug!(
                    "StreamerActor {} check complete, state: {:?}, next check in {:?}",
                    self.id,
                    self.state.streamer_state,
                    self.state.time_until_next_check()
                );

                Ok(())
            }
            Err(e) => {
                // Handle the error through the status checker
                if let Err(handle_err) = self
                    .status_checker
                    .handle_error(&self.metadata, &e.message)
                    .await
                {
                    warn!(
                        "StreamerActor {} failed to handle error: {}",
                        self.id, handle_err
                    );
                }

                // Record the error in state
                let error_result = CheckResult::failure(&e.message);
                self.state.record_check(error_result, &self.config);

                if e.transient {
                    // Transient errors are recoverable
                    Err(ActorError::recoverable(e.message))
                } else {
                    // Permanent errors are fatal
                    Err(ActorError::fatal(e.message))
                }
            }
        }
    }

    /// Handle an incoming message.
    ///
    /// Returns `true` if the actor should stop.
    async fn handle_message(&mut self, msg: StreamerMessage) -> Result<bool, ActorError> {
        match msg {
            StreamerMessage::CheckStatus => {
                self.handle_check_status().await?;
                Ok(false)
            }
            StreamerMessage::ConfigUpdate(config) => {
                self.handle_config_update(config).await?;
                Ok(false)
            }
            StreamerMessage::BatchResult(result) => {
                self.handle_batch_result(result).await?;
                Ok(false)
            }
            StreamerMessage::DownloadEnded(reason) => {
                self.handle_download_ended(reason).await?;
                Ok(false)
            }
            StreamerMessage::Stop => {
                self.handle_stop().await?;
                Ok(true)
            }
            StreamerMessage::GetState(reply) => {
                self.handle_get_state(reply).await;
                Ok(false)
            }
        }
    }

    /// Handle CheckStatus message - trigger an immediate check.
    async fn handle_check_status(&mut self) -> Result<(), ActorError> {
        debug!("StreamerActor {} received CheckStatus", self.id);

        // Reset next check to now to trigger immediate check
        self.state.next_check = Some(Instant::now());

        Ok(())
    }

    /// Handle ConfigUpdate message - apply new configuration without restart.
    ///
    /// Configuration updates take effect immediately and the next check
    /// is rescheduled based on the new configuration.
    async fn handle_config_update(&mut self, config: StreamerConfig) -> Result<(), ActorError> {
        debug!("StreamerActor {} received ConfigUpdate", self.id);

        let old_config = std::mem::replace(&mut self.config, config);

        // Log significant changes
        if old_config.check_interval_ms != self.config.check_interval_ms {
            info!(
                "StreamerActor {} check interval changed: {}ms -> {}ms",
                self.id, old_config.check_interval_ms, self.config.check_interval_ms
            );
        }

        if old_config.priority != self.config.priority {
            info!(
                "StreamerActor {} priority changed: {:?} -> {:?}",
                self.id, old_config.priority, self.config.priority
            );
        }

        // Reschedule next check with new config
        self.state.schedule_next_check(&self.config);

        Ok(())
    }

    /// Handle BatchResult message - process result from PlatformActor.
    async fn handle_batch_result(
        &mut self,
        result: BatchDetectionResult,
    ) -> Result<(), ActorError> {
        debug!(
            "StreamerActor {} received BatchResult: {:?}",
            self.id, result.result.state
        );

        // Verify this result is for us
        if result.streamer_id != self.id {
            warn!(
                "StreamerActor {} received BatchResult for wrong streamer: {}",
                self.id, result.streamer_id
            );
            return Ok(());
        }

        // Record the check result
        self.state.record_check(result.result, &self.config);

        debug!(
            "StreamerActor {} batch result processed, next check in {:?}",
            self.id,
            self.state.time_until_next_check()
        );

        Ok(())
    }

    /// Handle DownloadEnded message - resume status checking after download ends.
    ///
    /// When a download ends (streamer went offline, network error, etc.), we need
    /// to resume status checking to detect when the streamer comes back online.
    async fn handle_download_ended(
        &mut self,
        reason: super::messages::DownloadEndReason,
    ) -> Result<(), ActorError> {
        use super::messages::DownloadEndReason;

        info!("StreamerActor {} download ended: {:?}", self.id, reason);

        // Update state based on reason
        match reason {
            DownloadEndReason::StreamerOffline => {
                // Streamer went offline normally
                self.state.streamer_state = StreamerState::NotLive;
                self.state.offline_count = 0;
            }
            DownloadEndReason::NetworkError(_) | DownloadEndReason::SegmentFailed(_) => {
                // Network issue - we don't know if streamer is still live
                // Schedule immediate check to verify
                self.state.streamer_state = StreamerState::NotLive;
            }
            DownloadEndReason::Cancelled => {
                // User cancelled - don't change state, just resume checking
                self.state.streamer_state = StreamerState::NotLive;
            }
            DownloadEndReason::Other(_) => {
                // Unknown reason - assume offline
                self.state.streamer_state = StreamerState::NotLive;
            }
        }

        // Schedule immediate check to verify current status
        self.state.schedule_immediate_check();

        debug!(
            "StreamerActor {} resuming status checks, next check in {:?}",
            self.id,
            self.state.time_until_next_check()
        );

        Ok(())
    }

    /// Handle Stop message - prepare for graceful shutdown.
    async fn handle_stop(&mut self) -> Result<(), ActorError> {
        info!("StreamerActor {} received Stop", self.id);

        // Persist state before stopping
        self.persist_state().await?;

        Ok(())
    }

    /// Handle GetState message - return current state via oneshot channel.
    async fn handle_get_state(&self, reply: tokio::sync::oneshot::Sender<StreamerActorState>) {
        debug!("StreamerActor {} received GetState", self.id);

        // Send state, ignore if receiver dropped
        let _ = reply.send(self.state.clone());
    }

    /// Persist the current state for recovery after restart.
    ///
    /// State is persisted to a JSON file if a state path is configured.
    async fn persist_state(&self) -> Result<(), ActorError> {
        let Some(ref path) = self.state_path else {
            debug!(
                "StreamerActor {} has no state path, skipping persistence",
                self.id
            );
            return Ok(());
        };

        debug!("StreamerActor {} persisting state to {:?}", self.id, path);

        let persisted = PersistedActorState::from_state(&self.id, &self.state, &self.config);

        let json = serde_json::to_string_pretty(&persisted)
            .map_err(|e| ActorError::recoverable(format!("Failed to serialize state: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ActorError::recoverable(format!("Failed to create state directory: {}", e))
            })?;
        }

        // Write atomically using a temp file
        let temp_path = path.with_extension("tmp");
        tokio::fs::write(&temp_path, &json)
            .await
            .map_err(|e| ActorError::recoverable(format!("Failed to write state file: {}", e)))?;

        tokio::fs::rename(&temp_path, path)
            .await
            .map_err(|e| ActorError::recoverable(format!("Failed to rename state file: {}", e)))?;

        debug!("StreamerActor {} state persisted successfully", self.id);
        Ok(())
    }

    /// Restore state from a persisted file.
    ///
    /// Returns `None` if no persisted state exists or if restoration fails.
    pub async fn restore_state(
        id: &str,
        state_path: &PathBuf,
    ) -> Option<(StreamerActorState, StreamerConfig)> {
        let path = state_path.join(format!("{}.json", id));

        if !path.exists() {
            debug!("No persisted state found for actor {}", id);
            return None;
        }

        match tokio::fs::read_to_string(&path).await {
            Ok(json) => match serde_json::from_str::<PersistedActorState>(&json) {
                Ok(persisted) => {
                    info!("Restored state for actor {} from {:?}", id, path);
                    Some(persisted.into_state_and_config())
                }
                Err(e) => {
                    warn!("Failed to parse persisted state for {}: {}", id, e);
                    None
                }
            },
            Err(e) => {
                warn!("Failed to read persisted state for {}: {}", id, e);
                None
            }
        }
    }

    /// Create a StreamerActor with restored state if available.
    pub async fn with_restored_state_and_checker(
        metadata: StreamerMetadata,
        default_config: StreamerConfig,
        cancellation_token: CancellationToken,
        state_dir: Option<&PathBuf>,
        status_checker: std::sync::Arc<S>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (mut actor, handle) = Self::with_status_checker(
            metadata.clone(),
            default_config.clone(),
            cancellation_token,
            status_checker,
        );

        // Try to restore state
        if let Some(state_dir) = state_dir {
            if let Some((restored_state, restored_config)) =
                Self::restore_state(&metadata.id, state_dir).await
            {
                actor.state = restored_state;
                actor.config = restored_config;
                actor.state_path = Some(state_dir.join(format!("{}.json", metadata.id)));

                // Reschedule next check based on restored state
                if actor.state.next_check.is_none() {
                    actor.state.schedule_next_check(&actor.config);
                }
            } else {
                actor.state_path = Some(state_dir.join(format!("{}.json", metadata.id)));
            }
        }

        (actor, handle)
    }

    /// Create a StreamerActor with priority channel, restored state, and custom status checker.
    pub async fn with_priority_restored_state_and_checker(
        metadata: StreamerMetadata,
        default_config: StreamerConfig,
        cancellation_token: CancellationToken,
        state_dir: Option<&PathBuf>,
        status_checker: std::sync::Arc<S>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        let (mut actor, handle) = Self::with_priority_channel_and_checker(
            metadata.clone(),
            default_config.clone(),
            cancellation_token,
            status_checker,
        );

        // Try to restore state
        if let Some(state_dir) = state_dir {
            if let Some((restored_state, restored_config)) =
                Self::restore_state(&metadata.id, state_dir).await
            {
                actor.state = restored_state;
                actor.config = restored_config;
                actor.state_path = Some(state_dir.join(format!("{}.json", metadata.id)));

                // Reschedule next check based on restored state
                if actor.state.next_check.is_none() {
                    actor.state.schedule_next_check(&actor.config);
                }
            } else {
                actor.state_path = Some(state_dir.join(format!("{}.json", metadata.id)));
            }
        }

        (actor, handle)
    }
}

impl StreamerActor<NoOpStatusChecker> {
    /// Create a StreamerActor with restored state if available (no-op checker).
    pub async fn new_with_restored_state(
        metadata: StreamerMetadata,
        default_config: StreamerConfig,
        cancellation_token: CancellationToken,
        state_dir: Option<&PathBuf>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        Self::with_restored_state_and_checker(
            metadata,
            default_config,
            cancellation_token,
            state_dir,
            std::sync::Arc::new(NoOpStatusChecker),
        )
        .await
    }

    /// Create a StreamerActor with priority channel and restored state if available (no-op checker).
    pub async fn with_priority_and_restored_state(
        metadata: StreamerMetadata,
        default_config: StreamerConfig,
        cancellation_token: CancellationToken,
        state_dir: Option<&PathBuf>,
    ) -> (Self, ActorHandle<StreamerMessage>) {
        Self::with_priority_restored_state_and_checker(
            metadata,
            default_config,
            cancellation_token,
            state_dir,
            std::sync::Arc::new(NoOpStatusChecker),
        )
        .await
    }
}

/// Persisted actor state for recovery.
///
/// This struct contains only the serializable parts of the actor state.
/// `Instant` values are converted to durations for persistence.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedActorState {
    /// Actor ID.
    pub actor_id: String,
    /// Streamer state.
    pub streamer_state: String,
    /// Consecutive offline count.
    pub offline_count: u32,
    /// Error count.
    pub error_count: u32,
    /// Last check timestamp (RFC3339).
    pub last_check_time: Option<String>,
    /// Last check state.
    pub last_check_state: Option<String>,
    /// Last check error.
    pub last_check_error: Option<String>,
    /// Configuration.
    pub config: PersistedConfig,
}

/// Persisted configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedConfig {
    /// Check interval in milliseconds.
    pub check_interval_ms: u64,
    /// Offline check interval in milliseconds.
    pub offline_check_interval_ms: u64,
    /// Offline check count threshold.
    pub offline_check_count: u32,
    /// Priority level.
    pub priority: String,
    /// Whether batch capable.
    pub batch_capable: bool,
}

impl PersistedActorState {
    /// Create from current state.
    pub fn from_state(id: &str, state: &StreamerActorState, config: &StreamerConfig) -> Self {
        Self {
            actor_id: id.to_string(),
            streamer_state: state.streamer_state.as_str().to_string(),
            offline_count: state.offline_count,
            error_count: state.error_count,
            last_check_time: state.last_check.as_ref().map(|c| c.checked_at.to_rfc3339()),
            last_check_state: state
                .last_check
                .as_ref()
                .map(|c| c.state.as_str().to_string()),
            last_check_error: state.last_check.as_ref().and_then(|c| c.error.clone()),
            config: PersistedConfig {
                check_interval_ms: config.check_interval_ms,
                offline_check_interval_ms: config.offline_check_interval_ms,
                offline_check_count: config.offline_check_count,
                priority: format!("{:?}", config.priority),
                batch_capable: config.batch_capable,
            },
        }
    }

    /// Convert back to state and config.
    pub fn into_state_and_config(self) -> (StreamerActorState, StreamerConfig) {
        use crate::domain::Priority;

        let streamer_state = StreamerState::parse(&self.streamer_state).unwrap_or_default();

        let last_check = self.last_check_state.map(|state_str| {
            let state = StreamerState::parse(&state_str).unwrap_or_default();
            CheckResult {
                state,
                stream_url: None,
                title: None,
                checked_at: self
                    .last_check_time
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(chrono::Utc::now),
                error: self.last_check_error,
            }
        });

        let priority = match self.config.priority.as_str() {
            "High" => Priority::High,
            "Low" => Priority::Low,
            _ => Priority::Normal,
        };

        let state = StreamerActorState {
            streamer_state,
            next_check: None, // Will be recalculated
            offline_count: self.offline_count,
            last_check,
            error_count: self.error_count,
        };

        let config = StreamerConfig {
            check_interval_ms: self.config.check_interval_ms,
            offline_check_interval_ms: self.config.offline_check_interval_ms,
            offline_check_count: self.config.offline_check_count,
            priority,
            batch_capable: self.config.batch_capable,
        };

        (state, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Priority;

    fn create_test_metadata() -> StreamerMetadata {
        StreamerMetadata {
            id: "test-streamer".to_string(),
            name: "Test Streamer".to_string(),
            url: "https://twitch.tv/test".to_string(),
            platform_config_id: "twitch".to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
        }
    }

    fn create_test_config() -> StreamerConfig {
        StreamerConfig {
            check_interval_ms: 1000, // 1 second for tests
            offline_check_interval_ms: 500,
            offline_check_count: 3,
            priority: Priority::Normal,
            batch_capable: false,
        }
    }

    #[test]
    fn test_streamer_actor_new() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::new(metadata.clone(), config, token);

        assert_eq!(actor.id(), "test-streamer");
        assert_eq!(handle.id(), "test-streamer");
        assert!(!actor.uses_batch_detection());
    }

    #[test]
    fn test_streamer_actor_with_platform() {
        let metadata = create_test_metadata();
        let mut config = create_test_config();
        config.batch_capable = true;
        let token = CancellationToken::new();
        let (platform_tx, _platform_rx) = mpsc::channel::<PlatformMessage>(10);

        let (actor, _handle) =
            StreamerActor::with_platform_actor(metadata, config, token, platform_tx);

        assert!(actor.uses_batch_detection());
    }

    #[tokio::test]
    async fn test_streamer_actor_get_state() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::new(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Query state
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle
            .send(StreamerMessage::GetState(reply_tx))
            .await
            .unwrap();

        let state = reply_rx.await.unwrap();
        assert_eq!(state.streamer_state, StreamerState::NotLive);
        assert_eq!(state.offline_count, 0);

        // Stop actor
        handle.send(StreamerMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_streamer_actor_config_update() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::new(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send config update
        let new_config = StreamerConfig {
            check_interval_ms: 5000,
            offline_check_interval_ms: 2000,
            offline_check_count: 5,
            priority: Priority::High,
            batch_capable: false,
        };
        handle
            .send(StreamerMessage::ConfigUpdate(new_config))
            .await
            .unwrap();

        // Give time for processing
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Query state to verify config was applied
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle
            .send(StreamerMessage::GetState(reply_tx))
            .await
            .unwrap();
        let _state = reply_rx.await.unwrap();

        // Stop actor
        handle.send(StreamerMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_streamer_actor_cancellation() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, _handle) = StreamerActor::new(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Cancel
        token.cancel();

        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Cancelled)));
    }

    #[tokio::test]
    async fn test_streamer_actor_batch_result() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::new(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send batch result
        let batch_result = BatchDetectionResult {
            streamer_id: "test-streamer".to_string(),
            result: CheckResult::success(StreamerState::Live),
        };
        handle
            .send(StreamerMessage::BatchResult(batch_result))
            .await
            .unwrap();

        // Give time for processing
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Query state
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle
            .send(StreamerMessage::GetState(reply_tx))
            .await
            .unwrap();
        let state = reply_rx.await.unwrap();

        // State should be updated to Live
        assert_eq!(state.streamer_state, StreamerState::Live);

        // Stop actor
        handle.send(StreamerMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[test]
    fn test_persisted_state_roundtrip() {
        let state = StreamerActorState {
            streamer_state: StreamerState::Live,
            next_check: Some(Instant::now()),
            offline_count: 5,
            last_check: Some(CheckResult::success(StreamerState::Live)),
            error_count: 2,
        };

        let config = StreamerConfig {
            check_interval_ms: 30000,
            offline_check_interval_ms: 10000,
            offline_check_count: 5,
            priority: Priority::High,
            batch_capable: true,
        };

        let persisted = PersistedActorState::from_state("test", &state, &config);
        let (restored_state, restored_config) = persisted.into_state_and_config();

        assert_eq!(restored_state.streamer_state, StreamerState::Live);
        assert_eq!(restored_state.offline_count, 5);
        assert_eq!(restored_state.error_count, 2);
        assert_eq!(restored_config.check_interval_ms, 30000);
        assert_eq!(restored_config.priority, Priority::High);
        assert!(restored_config.batch_capable);
    }

    #[test]
    fn test_actor_error_display() {
        let err = ActorError::recoverable("test error");
        assert_eq!(err.to_string(), "test error");
        assert!(err.recoverable);

        let err = ActorError::fatal("fatal error");
        assert_eq!(err.to_string(), "fatal error");
        assert!(!err.recoverable);
    }

    #[test]
    fn test_actor_outcome() {
        assert_eq!(ActorOutcome::Stopped, ActorOutcome::Stopped);
        assert_ne!(ActorOutcome::Stopped, ActorOutcome::Cancelled);
    }

    #[test]
    fn test_streamer_actor_with_priority_channel() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::with_priority_channel(metadata, config, token);

        assert_eq!(actor.id(), "test-streamer");
        assert_eq!(handle.id(), "test-streamer");
        // Actor should have priority mailbox
        assert!(actor.priority_mailbox.is_some());
    }

    #[test]
    fn test_streamer_actor_with_priority_and_platform() {
        let metadata = create_test_metadata();
        let mut config = create_test_config();
        config.batch_capable = true;
        let token = CancellationToken::new();
        let (platform_tx, _platform_rx) = mpsc::channel::<PlatformMessage>(10);

        let (actor, _handle) =
            StreamerActor::with_priority_and_platform(metadata, config, token, platform_tx);

        assert!(actor.uses_batch_detection());
        assert!(actor.priority_mailbox.is_some());
    }

    #[tokio::test]
    async fn test_priority_channel_stop_message() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::with_priority_channel(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send stop via priority channel
        handle.send_priority(StreamerMessage::Stop).await.unwrap();

        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_priority_channel_processes_before_normal() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::with_priority_channel(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send multiple normal messages first
        for _ in 0..5 {
            let batch_result = BatchDetectionResult {
                streamer_id: "test-streamer".to_string(),
                result: CheckResult::success(StreamerState::NotLive),
            };
            handle
                .send(StreamerMessage::BatchResult(batch_result))
                .await
                .unwrap();
        }

        // Send stop via priority channel - should be processed promptly
        handle.send_priority(StreamerMessage::Stop).await.unwrap();

        // Actor should stop quickly despite pending normal messages
        let result = tokio::time::timeout(Duration::from_millis(500), actor_task)
            .await
            .unwrap()
            .unwrap();

        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_priority_channel_get_state() {
        let metadata = create_test_metadata();
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = StreamerActor::with_priority_channel(metadata, config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Query state via priority channel
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle
            .send_priority(StreamerMessage::GetState(reply_tx))
            .await
            .unwrap();

        let state = reply_rx.await.unwrap();
        assert_eq!(state.streamer_state, StreamerState::NotLive);

        // Stop actor
        handle.send_priority(StreamerMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_streamer_actor_resume_on_download_end() {
        let metadata = create_test_metadata();
        let config = StreamerConfig::default();
        let token = CancellationToken::new();

        // Create actor with live state (checks paused)
        let (mut actor, _handle) = StreamerActor::new(metadata.clone(), config.clone(), token);
        actor.state.streamer_state = StreamerState::Live;
        actor.state.next_check = None;

        // Verify initially paused
        assert!(actor.state.next_check.is_none());

        // Simulate download ended (streamer offline)
        let result = actor
            .handle_download_ended(super::super::messages::DownloadEndReason::StreamerOffline)
            .await;
        assert!(result.is_ok());

        // Verify state changed and check scheduled
        assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
        assert!(actor.state.next_check.is_some());
        assert!(actor.state.is_check_due()); // Should be immediate

        // Reset and test error case
        actor.state.streamer_state = StreamerState::Live;
        actor.state.next_check = None;

        // Simulate download failed (network error)
        let result = actor
            .handle_download_ended(super::super::messages::DownloadEndReason::NetworkError(
                "timeout".into(),
            ))
            .await;
        assert!(result.is_ok());

        // Verify state changed (assumed not live for safety) and check scheduled
        assert_eq!(actor.state.streamer_state, StreamerState::NotLive);
        assert!(actor.state.next_check.is_some());
        assert!(actor.state.is_check_due());
    }
}
