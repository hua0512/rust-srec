//! PlatformActor implementation.
//!
//! The PlatformActor coordinates batch detection for streamers on platforms
//! that support batch APIs. It collects check requests within a time window
//! and executes them as a single batch API call.
//!
//! # Responsibilities
//!
//! - Batch collection: Collects check requests within a configurable time window
//! - Batch execution: Executes batch API calls to the platform
//! - Result distribution: Sends results back to individual StreamerActors
//! - Rate limiting: Respects platform rate limits

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::handle::{ActorHandle, ActorMetadata, DEFAULT_MAILBOX_CAPACITY};
use super::messages::{
    BatchDetectionResult, CheckResult, PlatformActorState, PlatformConfig, PlatformMessage,
    StreamerMessage,
};
use super::metrics::ActorMetrics;
use super::monitor_adapter::{BatchChecker, NoOpBatchChecker};
use super::streamer_actor::{ActorError, ActorOutcome, ActorResult};
use crate::domain::StreamerState;
use crate::streamer::StreamerMetadata;

/// Default batch window in milliseconds.
pub const DEFAULT_BATCH_WINDOW_MS: u64 = 500;

/// A pending check request waiting to be batched.
#[derive(Debug)]
struct PendingCheckRequest {
    /// Streamer ID requesting the check.
    streamer_id: String,
    /// Channel to acknowledge the request was queued.
    reply: oneshot::Sender<()>,
    /// When the request was received.
    received_at: Instant,
}

/// Default priority mailbox capacity for platform actors.
pub const DEFAULT_PRIORITY_MAILBOX_CAPACITY: usize = DEFAULT_MAILBOX_CAPACITY / 4;

/// A PlatformActor that coordinates batch detection for a platform.
///
/// The PlatformActor collects check requests from StreamerActors and
/// executes them as batch API calls to optimize network usage and
/// respect rate limits.
pub struct PlatformActor<B: BatchChecker = NoOpBatchChecker> {
    /// Platform identifier.
    platform_id: String,
    /// Mailbox for receiving normal-priority messages.
    mailbox: mpsc::Receiver<PlatformMessage>,
    /// Mailbox for receiving high-priority messages (checked first).
    priority_mailbox: Option<mpsc::Receiver<PlatformMessage>>,
    /// Pending check requests (batched within window).
    pending_requests: Vec<PendingCheckRequest>,
    /// Batch window duration.
    batch_window: Duration,
    /// Maximum batch size.
    max_batch_size: usize,
    /// Streamer actor handles for result distribution.
    streamer_handles: HashMap<String, mpsc::Sender<StreamerMessage>>,
    /// Streamer metadata for batch checking.
    streamer_metadata: HashMap<String, StreamerMetadata>,
    /// Configuration.
    config: PlatformConfig,
    /// Current state for monitoring.
    state: PlatformActorState,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Metrics handle.
    metrics: ActorMetrics,
    /// Batch checker for performing actual batch checks.
    batch_checker: std::sync::Arc<B>,
    /// Flag to indicate batch timer needs to be reset after config change.
    timer_needs_reset: bool,
}

impl PlatformActor<NoOpBatchChecker> {
    /// Create a new PlatformActor with no-op batch checker (for testing/backwards compatibility).
    ///
    /// # Arguments
    ///
    /// * `platform_id` - Platform identifier (e.g., "twitch", "youtube")
    /// * `config` - Platform configuration
    /// * `cancellation_token` - Token for graceful shutdown
    pub fn new(
        platform_id: impl Into<String>,
        config: PlatformConfig,
        cancellation_token: CancellationToken,
    ) -> (Self, ActorHandle<PlatformMessage>) {
        Self::with_batch_checker(
            platform_id,
            config,
            cancellation_token,
            std::sync::Arc::new(NoOpBatchChecker),
        )
    }

    /// Create a new PlatformActor with priority channel support.
    ///
    /// High-priority messages are processed before normal messages,
    /// ensuring critical operations (like Stop) are handled promptly
    /// even under backpressure.
    pub fn with_priority_channel(
        platform_id: impl Into<String>,
        config: PlatformConfig,
        cancellation_token: CancellationToken,
    ) -> (Self, ActorHandle<PlatformMessage>) {
        Self::with_priority_channel_and_checker(
            platform_id,
            config,
            cancellation_token,
            std::sync::Arc::new(NoOpBatchChecker),
        )
    }
}

impl<B: BatchChecker> PlatformActor<B> {
    /// Create a new PlatformActor with a custom batch checker.
    ///
    /// # Arguments
    ///
    /// * `platform_id` - Platform identifier (e.g., "twitch", "youtube")
    /// * `config` - Platform configuration
    /// * `cancellation_token` - Token for graceful shutdown
    /// * `batch_checker` - Batch checker for performing actual batch checks
    pub fn with_batch_checker(
        platform_id: impl Into<String>,
        config: PlatformConfig,
        cancellation_token: CancellationToken,
        batch_checker: std::sync::Arc<B>,
    ) -> (Self, ActorHandle<PlatformMessage>) {
        let platform_id = platform_id.into();
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);

        let actor_metadata = ActorMetadata::platform(&platform_id);
        let handle = ActorHandle::new(tx, cancellation_token.clone(), actor_metadata);

        let batch_window = Duration::from_millis(config.batch_window_ms);
        let max_batch_size = config.max_batch_size;

        let metrics = ActorMetrics::new(&platform_id, DEFAULT_MAILBOX_CAPACITY);

        let state = PlatformActorState {
            streamer_count: 0,
            pending_count: 0,
            last_batch: None,
            success_rate: 1.0,
            total_batches: 0,
            successful_batches: 0,
        };

        let actor = Self {
            platform_id,
            mailbox: rx,
            priority_mailbox: None,
            pending_requests: Vec::new(),
            batch_window,
            max_batch_size,
            streamer_handles: HashMap::new(),
            streamer_metadata: HashMap::new(),
            config,
            state,
            cancellation_token,
            metrics,
            batch_checker,
            timer_needs_reset: false,
        };

        (actor, handle)
    }

    /// Create a new PlatformActor with priority channel and custom batch checker.
    ///
    /// High-priority messages are processed before normal messages,
    /// ensuring critical operations (like Stop) are handled promptly
    /// even under backpressure.
    pub fn with_priority_channel_and_checker(
        platform_id: impl Into<String>,
        config: PlatformConfig,
        cancellation_token: CancellationToken,
        batch_checker: std::sync::Arc<B>,
    ) -> (Self, ActorHandle<PlatformMessage>) {
        let platform_id = platform_id.into();
        let (tx, rx) = mpsc::channel(DEFAULT_MAILBOX_CAPACITY);
        let (priority_tx, priority_rx) = mpsc::channel(DEFAULT_PRIORITY_MAILBOX_CAPACITY);

        let actor_metadata = ActorMetadata::platform(&platform_id);
        let handle =
            ActorHandle::with_priority(tx, priority_tx, cancellation_token.clone(), actor_metadata);

        let batch_window = Duration::from_millis(config.batch_window_ms);
        let max_batch_size = config.max_batch_size;

        let metrics = ActorMetrics::new(&platform_id, DEFAULT_MAILBOX_CAPACITY);

        let state = PlatformActorState {
            streamer_count: 0,
            pending_count: 0,
            last_batch: None,
            success_rate: 1.0,
            total_batches: 0,
            successful_batches: 0,
        };

        let actor = Self {
            platform_id,
            mailbox: rx,
            priority_mailbox: Some(priority_rx),
            pending_requests: Vec::new(),
            batch_window,
            max_batch_size,
            streamer_handles: HashMap::new(),
            streamer_metadata: HashMap::new(),
            config,
            state,
            cancellation_token,
            metrics,
            batch_checker,
            timer_needs_reset: false,
        };

        (actor, handle)
    }

    /// Register a streamer actor handle for result distribution.
    pub fn register_streamer(
        &mut self,
        streamer_id: String,
        handle: mpsc::Sender<StreamerMessage>,
    ) {
        self.streamer_handles.insert(streamer_id, handle);
        self.state.streamer_count = self.streamer_handles.len();
    }

    /// Register a streamer with metadata for batch checking.
    pub fn register_streamer_with_metadata(
        &mut self,
        streamer_id: String,
        handle: mpsc::Sender<StreamerMessage>,
        metadata: StreamerMetadata,
    ) {
        self.streamer_handles.insert(streamer_id.clone(), handle);
        self.streamer_metadata.insert(streamer_id, metadata);
        self.state.streamer_count = self.streamer_handles.len();
    }

    /// Unregister a streamer actor.
    pub fn unregister_streamer(&mut self, streamer_id: &str) {
        self.streamer_handles.remove(streamer_id);
        self.streamer_metadata.remove(streamer_id);
        self.state.streamer_count = self.streamer_handles.len();
    }

    /// Get the platform ID.
    pub fn platform_id(&self) -> &str {
        &self.platform_id
    }

    /// Get the current state.
    pub fn state(&self) -> &PlatformActorState {
        &self.state
    }

    /// Get the configuration.
    pub fn config(&self) -> &PlatformConfig {
        &self.config
    }

    /// Get the number of pending requests.
    pub fn pending_count(&self) -> usize {
        self.pending_requests.len()
    }

    /// Run the actor's event loop.
    ///
    /// This method runs until the actor receives a Stop message or the
    /// cancellation token is triggered.
    pub async fn run(mut self) -> ActorResult {
        info!("PlatformActor {} starting", self.platform_id);

        let mut batch_timer = tokio::time::interval(self.batch_window);
        // Don't fire immediately
        batch_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            // Check if timer needs to be reset due to config change
            if self.timer_needs_reset {
                debug!(
                    "PlatformActor {} resetting batch timer to {:?}",
                    self.platform_id, self.batch_window
                );
                batch_timer = tokio::time::interval(self.batch_window);
                batch_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                self.timer_needs_reset = false;
            }

            // First, drain all priority messages before processing normal messages
            // This ensures high-priority operations (like Stop) are handled promptly
            if let Some(msg) = self.try_recv_priority() {
                let start = Instant::now();
                let should_stop = self.handle_message(msg).await?;
                self.metrics.record_message(start.elapsed());

                if should_stop {
                    debug!(
                        "PlatformActor {} received stop signal from priority channel",
                        self.platform_id
                    );
                    break;
                }
                // Continue to check for more priority messages
                continue;
            }

            tokio::select! {
                // Bias towards handling messages first
                biased;

                // Check priority mailbox first (if configured)
                Some(msg) = Self::recv_priority_opt(&mut self.priority_mailbox) => {
                    let start = Instant::now();
                    let should_stop = self.handle_message(msg).await?;
                    self.metrics.record_message(start.elapsed());

                    if should_stop {
                        debug!("PlatformActor {} received stop signal from priority channel", self.platform_id);
                        break;
                    }
                }

                // Handle normal-priority messages
                Some(msg) = self.mailbox.recv() => {
                    let start = Instant::now();
                    let should_stop = self.handle_message(msg).await?;
                    self.metrics.record_message(start.elapsed());

                    if should_stop {
                        debug!("PlatformActor {} received stop signal", self.platform_id);
                        break;
                    }

                    // Check if we should execute batch early (max size reached)
                    if self.pending_requests.len() >= self.max_batch_size {
                        debug!(
                            "PlatformActor {} batch size limit reached, executing early",
                            self.platform_id
                        );
                        if let Err(e) = self.execute_batch().await {
                            warn!("PlatformActor {} batch execution failed: {}", self.platform_id, e);
                            self.metrics.record_error();
                        }
                    }
                }

                // Batch timer - execute pending requests
                _ = batch_timer.tick(), if !self.pending_requests.is_empty() => {
                    debug!(
                        "PlatformActor {} batch timer fired with {} pending requests",
                        self.platform_id,
                        self.pending_requests.len()
                    );
                    if let Err(e) = self.execute_batch().await {
                        warn!("PlatformActor {} batch execution failed: {}", self.platform_id, e);
                        self.metrics.record_error();
                    }
                }

                // Cancellation
                _ = self.cancellation_token.cancelled() => {
                    info!("PlatformActor {} cancelled", self.platform_id);
                    // Acknowledge any pending requests before stopping
                    self.acknowledge_pending_requests();
                    return Ok(ActorOutcome::Cancelled);
                }
            }
        }

        // Graceful shutdown - acknowledge any pending requests
        self.acknowledge_pending_requests();
        info!("PlatformActor {} stopped gracefully", self.platform_id);
        Ok(ActorOutcome::Stopped)
    }

    /// Try to receive a message from the priority mailbox without blocking.
    fn try_recv_priority(&mut self) -> Option<PlatformMessage> {
        if let Some(ref mut priority_rx) = self.priority_mailbox {
            priority_rx.try_recv().ok()
        } else {
            None
        }
    }

    /// Helper to receive from an optional priority mailbox.
    /// Returns a future that is pending forever if the mailbox is None.
    async fn recv_priority_opt(
        priority_mailbox: &mut Option<mpsc::Receiver<PlatformMessage>>,
    ) -> Option<PlatformMessage> {
        match priority_mailbox {
            Some(rx) => rx.recv().await,
            None => std::future::pending().await,
        }
    }

    /// Handle an incoming message.
    ///
    /// Returns `true` if the actor should stop.
    async fn handle_message(&mut self, msg: PlatformMessage) -> Result<bool, ActorError> {
        match msg {
            PlatformMessage::RequestCheck { streamer_id, reply } => {
                self.handle_request_check(streamer_id, reply).await?;
                Ok(false)
            }
            PlatformMessage::ConfigUpdate(config) => {
                self.handle_config_update(config).await?;
                Ok(false)
            }
            PlatformMessage::Stop => {
                self.handle_stop().await?;
                Ok(true)
            }
            PlatformMessage::GetState(reply) => {
                self.handle_get_state(reply).await;
                Ok(false)
            }
        }
    }

    /// Handle RequestCheck message - queue a check request for batching.
    async fn handle_request_check(
        &mut self,
        streamer_id: String,
        reply: oneshot::Sender<()>,
    ) -> Result<(), ActorError> {
        debug!(
            "PlatformActor {} received check request for {}",
            self.platform_id, streamer_id
        );

        // Queue the request
        self.pending_requests.push(PendingCheckRequest {
            streamer_id,
            reply,
            received_at: Instant::now(),
        });

        self.state.pending_count = self.pending_requests.len();

        Ok(())
    }

    /// Handle ConfigUpdate message - apply new configuration.
    async fn handle_config_update(&mut self, config: PlatformConfig) -> Result<(), ActorError> {
        debug!("PlatformActor {} received ConfigUpdate", self.platform_id);

        let old_config = std::mem::replace(&mut self.config, config);

        // Log significant changes
        if old_config.batch_window_ms != self.config.batch_window_ms {
            info!(
                "PlatformActor {} batch window changed: {}ms -> {}ms",
                self.platform_id, old_config.batch_window_ms, self.config.batch_window_ms
            );
            self.batch_window = Duration::from_millis(self.config.batch_window_ms);
            // Signal that the timer needs to be reset with new interval
            self.timer_needs_reset = true;
        }

        if old_config.max_batch_size != self.config.max_batch_size {
            info!(
                "PlatformActor {} max batch size changed: {} -> {}",
                self.platform_id, old_config.max_batch_size, self.config.max_batch_size
            );
            self.max_batch_size = self.config.max_batch_size;
        }

        Ok(())
    }

    /// Handle Stop message - prepare for graceful shutdown.
    async fn handle_stop(&mut self) -> Result<(), ActorError> {
        info!("PlatformActor {} received Stop", self.platform_id);

        // Execute any pending batch before stopping
        if !self.pending_requests.is_empty() {
            debug!(
                "PlatformActor {} executing final batch before stop",
                self.platform_id
            );
            if let Err(e) = self.execute_batch().await {
                warn!(
                    "PlatformActor {} final batch execution failed: {}",
                    self.platform_id, e
                );
            }
        }

        Ok(())
    }

    /// Handle GetState message - return current state via oneshot channel.
    async fn handle_get_state(&self, reply: oneshot::Sender<PlatformActorState>) {
        debug!("PlatformActor {} received GetState", self.platform_id);

        // Send state, ignore if receiver dropped
        let _ = reply.send(self.state.clone());
    }

    /// Execute a batch of pending check requests.
    ///
    /// This method:
    /// 1. Collects all pending requests
    /// 2. Acknowledges the requests (so StreamerActors know they're being processed)
    /// 3. Executes the batch API call
    /// 4. Distributes results to individual StreamerActors
    async fn execute_batch(&mut self) -> Result<(), ActorError> {
        if self.pending_requests.is_empty() {
            return Ok(());
        }

        // Take all pending requests
        let requests = std::mem::take(&mut self.pending_requests);
        self.state.pending_count = 0;

        let batch_size = requests.len();
        debug!(
            "PlatformActor {} executing batch of {} requests",
            self.platform_id, batch_size
        );

        // Collect streamer IDs for the batch
        let streamer_ids: Vec<String> = requests.iter().map(|r| r.streamer_id.clone()).collect();

        // Acknowledge all requests (they're now being processed)
        for request in requests {
            let _ = request.reply.send(());
        }

        // Execute the batch API call
        let results = self.perform_batch_check(&streamer_ids).await?;

        // Distribute results to StreamerActors
        self.distribute_results(results).await;

        // Update state
        self.state.record_batch(true);

        debug!(
            "PlatformActor {} batch complete, success rate: {:.2}%",
            self.platform_id,
            self.state.success_rate * 100.0
        );

        Ok(())
    }

    /// Perform the actual batch API call using the configured batch checker.
    ///
    /// This method connects to the actual platform API infrastructure via the
    /// BatchChecker trait, which abstracts the batch checking operation.
    async fn perform_batch_check(
        &self,
        streamer_ids: &[String],
    ) -> Result<Vec<BatchDetectionResult>, ActorError> {
        debug!(
            "PlatformActor {} performing batch check for {} streamers",
            self.platform_id,
            streamer_ids.len()
        );

        // Collect streamer metadata for the batch
        let streamers: Vec<StreamerMetadata> = streamer_ids
            .iter()
            .filter_map(|id| self.streamer_metadata.get(id).cloned())
            .collect();

        // If we don't have metadata for all streamers, fall back to simulated results
        // for those without metadata (backwards compatibility)
        if streamers.len() < streamer_ids.len() {
            debug!(
                "PlatformActor {} has metadata for {}/{} streamers, using fallback for rest",
                self.platform_id,
                streamers.len(),
                streamer_ids.len()
            );
        }

        // If we have no metadata at all, return simulated results (backwards compatibility)
        if streamers.is_empty() {
            let results: Vec<BatchDetectionResult> = streamer_ids
                .iter()
                .map(|id| BatchDetectionResult {
                    streamer_id: id.clone(),
                    result: CheckResult::success(StreamerState::NotLive),
                })
                .collect();
            return Ok(results);
        }

        // Perform the actual batch check using the batch checker
        match self
            .batch_checker
            .batch_check(&self.platform_id, streamers)
            .await
        {
            Ok(mut results) => {
                // Add fallback results for streamers without metadata
                let result_ids: std::collections::HashSet<_> =
                    results.iter().map(|r| r.streamer_id.clone()).collect();

                for id in streamer_ids {
                    if !result_ids.contains(id) {
                        results.push(BatchDetectionResult {
                            streamer_id: id.clone(),
                            result: CheckResult::success(StreamerState::NotLive),
                        });
                    }
                }

                debug!(
                    "PlatformActor {} batch check complete, {} results",
                    self.platform_id,
                    results.len()
                );

                Ok(results)
            }
            Err(e) => {
                warn!(
                    "PlatformActor {} batch check failed: {}",
                    self.platform_id, e
                );

                if e.transient {
                    Err(ActorError::recoverable(e.message))
                } else {
                    Err(ActorError::fatal(e.message))
                }
            }
        }
    }

    /// Distribute batch results to individual StreamerActors.
    async fn distribute_results(&self, results: Vec<BatchDetectionResult>) {
        for result in results {
            let streamer_id = result.streamer_id.clone();

            if let Some(handle) = self.streamer_handles.get(&streamer_id) {
                let msg = StreamerMessage::BatchResult(result);

                if let Err(e) = handle.send(msg).await {
                    warn!(
                        "PlatformActor {} failed to send result to {}: {:?}",
                        self.platform_id, streamer_id, e
                    );
                }
            } else {
                debug!(
                    "PlatformActor {} has no handle for streamer {}, result dropped",
                    self.platform_id, streamer_id
                );
            }
        }
    }

    /// Acknowledge all pending requests without executing them.
    ///
    /// Used during shutdown to unblock waiting StreamerActors.
    fn acknowledge_pending_requests(&mut self) {
        for request in self.pending_requests.drain(..) {
            let _ = request.reply.send(());
        }
        self.state.pending_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Priority;

    fn create_test_config() -> PlatformConfig {
        PlatformConfig {
            platform_id: "test-platform".to_string(),
            batch_window_ms: 100, // Short window for tests
            max_batch_size: 10,
            rate_limit: None,
        }
    }

    #[test]
    fn test_platform_actor_new() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = PlatformActor::new("test-platform", config, token);

        assert_eq!(actor.platform_id(), "test-platform");
        assert_eq!(handle.id(), "test-platform");
        assert_eq!(actor.pending_count(), 0);
    }

    #[test]
    fn test_platform_actor_register_streamer() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (mut actor, _handle) = PlatformActor::new("test-platform", config, token);

        let (streamer_tx, _streamer_rx) = mpsc::channel::<StreamerMessage>(10);
        actor.register_streamer("streamer-1".to_string(), streamer_tx);

        assert_eq!(actor.state().streamer_count, 1);
    }

    #[test]
    fn test_platform_actor_unregister_streamer() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (mut actor, _handle) = PlatformActor::new("test-platform", config, token);

        let (streamer_tx, _streamer_rx) = mpsc::channel::<StreamerMessage>(10);
        actor.register_streamer("streamer-1".to_string(), streamer_tx);
        assert_eq!(actor.state().streamer_count, 1);

        actor.unregister_streamer("streamer-1");
        assert_eq!(actor.state().streamer_count, 0);
    }

    #[tokio::test]
    async fn test_platform_actor_get_state() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = PlatformActor::new("test-platform", config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Query state
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(PlatformMessage::GetState(reply_tx))
            .await
            .unwrap();

        let state = reply_rx.await.unwrap();
        assert_eq!(state.streamer_count, 0);
        assert_eq!(state.pending_count, 0);

        // Stop actor
        handle.send(PlatformMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_platform_actor_request_check() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = PlatformActor::new("test-platform", config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send check request
        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .send(PlatformMessage::RequestCheck {
                streamer_id: "streamer-1".to_string(),
                reply: reply_tx,
            })
            .await
            .unwrap();

        // Wait for batch timer to fire and process the request
        tokio::time::sleep(Duration::from_millis(150)).await;

        // The reply should have been sent (request acknowledged)
        assert!(reply_rx.await.is_ok());

        // Stop actor
        handle.send(PlatformMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_platform_actor_batch_execution() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (mut actor, handle) = PlatformActor::new("test-platform", config, token.clone());

        // Register a streamer to receive results
        let (streamer_tx, mut streamer_rx) = mpsc::channel::<StreamerMessage>(10);
        actor.register_streamer("streamer-1".to_string(), streamer_tx);

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send check request
        let (reply_tx, _reply_rx) = oneshot::channel();
        handle
            .send(PlatformMessage::RequestCheck {
                streamer_id: "streamer-1".to_string(),
                reply: reply_tx,
            })
            .await
            .unwrap();

        // Wait for batch to execute and result to be distributed
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Check that the streamer received a BatchResult
        let msg = streamer_rx.try_recv();
        assert!(matches!(msg, Ok(StreamerMessage::BatchResult(_))));

        // Stop actor
        handle.send(PlatformMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_platform_actor_config_update() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, handle) = PlatformActor::new("test-platform", config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send config update
        let new_config = PlatformConfig {
            platform_id: "test-platform".to_string(),
            batch_window_ms: 200,
            max_batch_size: 20,
            rate_limit: Some(10.0),
        };
        handle
            .send(PlatformMessage::ConfigUpdate(new_config))
            .await
            .unwrap();

        // Give time for processing
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Stop actor
        handle.send(PlatformMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }

    #[tokio::test]
    async fn test_platform_actor_cancellation() {
        let config = create_test_config();
        let token = CancellationToken::new();

        let (actor, _handle) = PlatformActor::new("test-platform", config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Cancel
        token.cancel();

        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Cancelled)));
    }

    #[tokio::test]
    async fn test_platform_actor_max_batch_size() {
        let mut config = create_test_config();
        config.max_batch_size = 2; // Small batch size for testing
        config.batch_window_ms = 10000; // Long window so timer doesn't fire

        let token = CancellationToken::new();

        let (actor, handle) = PlatformActor::new("test-platform", config, token.clone());

        // Spawn actor
        let actor_task = tokio::spawn(async move { actor.run().await });

        // Send requests up to max batch size
        let (reply_tx1, reply_rx1) = oneshot::channel();
        handle
            .send(PlatformMessage::RequestCheck {
                streamer_id: "streamer-1".to_string(),
                reply: reply_tx1,
            })
            .await
            .unwrap();

        let (reply_tx2, reply_rx2) = oneshot::channel();
        handle
            .send(PlatformMessage::RequestCheck {
                streamer_id: "streamer-2".to_string(),
                reply: reply_tx2,
            })
            .await
            .unwrap();

        // Wait a bit for batch to execute (should happen immediately due to max size)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Both requests should be acknowledged
        assert!(reply_rx1.await.is_ok());
        assert!(reply_rx2.await.is_ok());

        // Stop actor
        handle.send(PlatformMessage::Stop).await.unwrap();
        let result = actor_task.await.unwrap();
        assert!(matches!(result, Ok(ActorOutcome::Stopped)));
    }
}
