//! Supervisor for managing actor lifecycle and crash recovery.
//!
//! The `Supervisor` is responsible for:
//! - Detecting actor crashes via JoinSet task completions
//! - Restarting crashed actors with exponential backoff
//! - Coordinating graceful shutdown
//! - Emitting lifecycle events for monitoring

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::handle::ActorHandle;
use super::messages::{PlatformConfig, PlatformMessage, StreamerConfig, StreamerMessage};
use super::platform_actor::PlatformActor;
use super::registry::{ActorRegistry, ActorTaskResult};
use super::restart_tracker::{RestartTracker, RestartTrackerConfig};
use super::streamer_actor::{ActorOutcome, StreamerActor};
use crate::streamer::StreamerMetadata;

/// Configuration for the supervisor.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Restart tracker configuration.
    pub restart_config: RestartTrackerConfig,
    /// Shutdown timeout.
    pub shutdown_timeout: Duration,
    /// State persistence directory (optional).
    pub state_dir: Option<PathBuf>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            restart_config: RestartTrackerConfig::default(),
            shutdown_timeout: Duration::from_secs(10),
            state_dir: None,
        }
    }
}

/// Pending restart information.
#[derive(Debug)]
struct PendingRestart {
    /// Actor ID.
    actor_id: String,
    /// Actor type ("streamer" or "platform").
    actor_type: String,
    /// Metadata for recreating the actor.
    metadata: RestartMetadata,
    /// When the restart should occur.
    restart_at: tokio::time::Instant,
}

/// Metadata needed to restart an actor.
#[derive(Debug, Clone)]
enum RestartMetadata {
    Streamer {
        metadata: StreamerMetadata,
        config: StreamerConfig,
        platform_actor: Option<mpsc::Sender<PlatformMessage>>,
    },
    Platform {
        platform_id: String,
        config: PlatformConfig,
    },
}

/// Supervisor for managing actor lifecycle.
///
/// The supervisor monitors actor tasks via JoinSet and handles:
/// - Crash detection when tasks complete unexpectedly
/// - Restart with exponential backoff
/// - Graceful shutdown coordination
pub struct Supervisor {
    /// Actor registry.
    registry: ActorRegistry,
    /// Restart tracker.
    restart_tracker: RestartTracker,
    /// Configuration.
    config: SupervisorConfig,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Pending restarts (actors waiting for backoff).
    pending_restarts: Vec<PendingRestart>,
    /// Streamer metadata cache for restarts.
    streamer_metadata: HashMap<String, (StreamerMetadata, StreamerConfig)>,
    /// Platform config cache for restarts.
    platform_configs: HashMap<String, PlatformConfig>,
    /// Platform actor senders for streamer-platform association.
    platform_senders: HashMap<String, mpsc::Sender<PlatformMessage>>,
}

impl Supervisor {
    /// Create a new supervisor.
    pub fn new(cancellation_token: CancellationToken) -> Self {
        Self::with_config(cancellation_token, SupervisorConfig::default())
    }

    /// Create a new supervisor with custom configuration.
    pub fn with_config(cancellation_token: CancellationToken, config: SupervisorConfig) -> Self {
        Self {
            registry: ActorRegistry::new(cancellation_token.clone()),
            restart_tracker: RestartTracker::with_config(config.restart_config.clone()),
            config,
            cancellation_token,
            pending_restarts: Vec::new(),
            streamer_metadata: HashMap::new(),
            platform_configs: HashMap::new(),
            platform_senders: HashMap::new(),
        }
    }

    /// Get the actor registry.
    pub fn registry(&self) -> &ActorRegistry {
        &self.registry
    }

    /// Get mutable access to the actor registry.
    pub fn registry_mut(&mut self) -> &mut ActorRegistry {
        &mut self.registry
    }

    /// Get the restart tracker.
    pub fn restart_tracker(&self) -> &RestartTracker {
        &self.restart_tracker
    }

    /// Spawn a streamer actor.
    ///
    /// Actors are spawned with priority channel support, ensuring that
    /// high-priority messages (like Stop) are processed before normal messages.
    pub fn spawn_streamer(
        &mut self,
        metadata: StreamerMetadata,
        config: StreamerConfig,
        platform_actor: Option<mpsc::Sender<PlatformMessage>>,
    ) -> Result<ActorHandle<StreamerMessage>, SpawnError> {
        let id = metadata.id.clone();

        if self.registry.has_streamer(&id) {
            return Err(SpawnError::ActorExists(id));
        }

        // Create actor with priority channel support
        let (actor, handle) = if let Some(ref platform_sender) = platform_actor {
            StreamerActor::with_priority_and_platform(
                metadata.clone(),
                config.clone(),
                self.registry.child_token(),
                platform_sender.clone(),
            )
        } else {
            StreamerActor::with_priority_channel(
                metadata.clone(),
                config.clone(),
                self.registry.child_token(),
            )
        };

        // Cache metadata for potential restart
        self.streamer_metadata
            .insert(id.clone(), (metadata, config));
        if let Some(sender) = platform_actor {
            self.platform_senders.insert(id.clone(), sender);
        }

        // Spawn in registry
        self.registry
            .spawn_streamer(actor, handle.clone())
            .map_err(|e| SpawnError::RegistryError(e.to_string()))?;

        info!("Spawned streamer actor: {}", id);
        Ok(handle)
    }

    /// Spawn a platform actor.
    ///
    /// Actors are spawned with priority channel support, ensuring that
    /// high-priority messages (like Stop) are processed before normal messages.
    pub fn spawn_platform(
        &mut self,
        platform_id: impl Into<String>,
        config: PlatformConfig,
    ) -> Result<ActorHandle<PlatformMessage>, SpawnError> {
        let platform_id = platform_id.into();

        if self.registry.has_platform(&platform_id) {
            return Err(SpawnError::ActorExists(platform_id));
        }

        // Create actor with priority channel support
        let (actor, handle) = PlatformActor::with_priority_channel(
            platform_id.clone(),
            config.clone(),
            self.registry.child_token(),
        );

        // Cache config for potential restart
        self.platform_configs.insert(platform_id.clone(), config);

        // Spawn in registry
        self.registry
            .spawn_platform(actor, handle.clone())
            .map_err(|e| SpawnError::RegistryError(e.to_string()))?;

        info!("Spawned platform actor: {}", platform_id);
        Ok(handle)
    }

    /// Remove a streamer actor.
    pub fn remove_streamer(&mut self, id: &str) -> bool {
        self.streamer_metadata.remove(id);
        self.platform_senders.remove(id);
        self.restart_tracker.remove(id);
        self.registry.remove_streamer(id).is_some()
    }

    /// Remove a platform actor.
    pub fn remove_platform(&mut self, platform_id: &str) -> bool {
        self.platform_configs.remove(platform_id);
        self.restart_tracker.remove(platform_id);
        self.registry.remove_platform(platform_id).is_some()
    }

    /// Handle a completed actor task.
    ///
    /// This is the core crash detection logic. When an actor task completes,
    /// we determine if it was a crash and schedule a restart if appropriate.
    pub fn handle_task_completion(&mut self, result: ActorTaskResult) -> TaskCompletionAction {
        let actor_id = result.actor_id.clone();
        let actor_type = result.actor_type.clone();

        // Remove from registry
        let result = self.registry.handle_task_completion(result);

        if result.is_crash() {
            let error_msg = result.error_message().unwrap_or("unknown error");
            error!("Actor {} ({}) crashed: {}", actor_id, actor_type, error_msg);

            // Record failure and get backoff
            let backoff = self.restart_tracker.record_failure(&actor_id);

            // Check if we should restart
            if self.restart_tracker.should_restart(&actor_id) {
                info!(
                    "Scheduling restart for {} ({}) with backoff {:?}",
                    actor_id, actor_type, backoff
                );

                // Schedule restart
                if let Some(restart_metadata) = self.get_restart_metadata(&actor_id, &actor_type) {
                    let restart_at = tokio::time::Instant::now() + backoff;
                    self.pending_restarts.push(PendingRestart {
                        actor_id: actor_id.clone(),
                        actor_type,
                        metadata: restart_metadata,
                        restart_at,
                    });

                    return TaskCompletionAction::RestartScheduled { actor_id, backoff };
                } else {
                    warn!("No metadata found for restarting actor {}", actor_id);
                    return TaskCompletionAction::RestartFailed {
                        actor_id,
                        reason: "No metadata available".to_string(),
                    };
                }
            } else {
                warn!("Actor {} exceeded restart limit, not restarting", actor_id);
                return TaskCompletionAction::RestartLimitExceeded { actor_id };
            }
        }

        // Normal completion
        match result.outcome {
            Ok(ActorOutcome::Stopped) => {
                debug!("Actor {} stopped gracefully", actor_id);
                TaskCompletionAction::Stopped { actor_id }
            }
            Ok(ActorOutcome::Cancelled) => {
                debug!("Actor {} was cancelled", actor_id);
                TaskCompletionAction::Cancelled { actor_id }
            }
            Ok(ActorOutcome::Completed) => {
                debug!("Actor {} completed", actor_id);
                TaskCompletionAction::Completed { actor_id }
            }
            Err(_) => {
                // Already handled above
                TaskCompletionAction::Crashed { actor_id }
            }
        }
    }

    /// Get metadata needed to restart an actor.
    fn get_restart_metadata(&self, actor_id: &str, actor_type: &str) -> Option<RestartMetadata> {
        match actor_type {
            "streamer" => {
                let (metadata, config) = self.streamer_metadata.get(actor_id)?;
                let platform_actor = self.platform_senders.get(actor_id).cloned();
                Some(RestartMetadata::Streamer {
                    metadata: metadata.clone(),
                    config: config.clone(),
                    platform_actor,
                })
            }
            "platform" => {
                let config = self.platform_configs.get(actor_id)?;
                Some(RestartMetadata::Platform {
                    platform_id: actor_id.to_string(),
                    config: config.clone(),
                })
            }
            _ => None,
        }
    }

    /// Process pending restarts that are due.
    ///
    /// Returns the number of actors restarted.
    pub fn process_pending_restarts(&mut self) -> usize {
        let now = tokio::time::Instant::now();
        let mut restarted = 0;

        // Collect restarts that are due
        let due_restarts: Vec<_> = self
            .pending_restarts
            .drain(..)
            .filter(|r| r.restart_at <= now)
            .collect();

        // Re-add restarts that aren't due yet
        let not_due: Vec<_> = self
            .pending_restarts
            .drain(..)
            .filter(|r| r.restart_at > now)
            .collect();
        self.pending_restarts = not_due;

        // Process due restarts
        for restart in due_restarts {
            if let Err(e) = self.execute_restart(restart) {
                error!("Failed to restart actor: {}", e);
            } else {
                restarted += 1;
            }
        }

        restarted
    }

    /// Execute a pending restart.
    fn execute_restart(&mut self, restart: PendingRestart) -> Result<(), SpawnError> {
        info!(
            "Restarting actor {} ({})",
            restart.actor_id, restart.actor_type
        );

        match restart.metadata {
            RestartMetadata::Streamer {
                metadata,
                config,
                platform_actor,
            } => {
                // Remove old metadata (will be re-added by spawn_streamer)
                self.streamer_metadata.remove(&restart.actor_id);
                self.platform_senders.remove(&restart.actor_id);

                self.spawn_streamer(metadata, config, platform_actor)?;
            }
            RestartMetadata::Platform {
                platform_id,
                config,
            } => {
                // Remove old config (will be re-added by spawn_platform)
                self.platform_configs.remove(&platform_id);

                self.spawn_platform(platform_id, config)?;
            }
        }

        Ok(())
    }

    /// Get the next restart time, if any.
    pub fn next_restart_time(&self) -> Option<tokio::time::Instant> {
        self.pending_restarts.iter().map(|r| r.restart_at).min()
    }

    /// Get the number of pending restarts.
    pub fn pending_restart_count(&self) -> usize {
        self.pending_restarts.len()
    }

    /// Initiate graceful shutdown.
    ///
    /// This method implements the graceful shutdown sequence:
    /// 1. Send Stop messages to all actors
    /// 2. Wait for acknowledgments with configurable timeout (default 10s)
    /// 3. Forcefully terminate non-responsive actors
    /// 4. Generate shutdown report with statistics
    ///
    /// Returns a shutdown report with statistics.
    pub async fn shutdown(&mut self) -> ShutdownReport {
        info!("Initiating supervisor shutdown");

        let total_actors = self.registry.total_count();
        let mut graceful_stops = 0;
        let mut forced_terminations = 0;
        let mut stop_message_failures = 0;

        // Phase 1: Send Stop messages to all actors via priority channel
        // Using priority channel ensures Stop messages are processed promptly
        // even when actors are under backpressure
        info!(
            "Phase 1: Sending Stop messages to {} actors via priority channel",
            total_actors
        );

        // Send Stop to all streamer actors via priority channel
        for (id, handle) in self.registry.streamer_handles() {
            debug!(
                "Sending Stop to streamer actor via priority channel: {}",
                id
            );
            // Use send_priority to ensure Stop is processed before normal messages
            match handle.send_priority(StreamerMessage::Stop).await {
                Ok(()) => {}
                Err(e) => {
                    warn!("Failed to send Stop to streamer {}: {:?}", id, e);
                    stop_message_failures += 1;
                }
            }
        }

        // Send Stop to all platform actors via priority channel
        for (id, handle) in self.registry.platform_handles() {
            debug!(
                "Sending Stop to platform actor via priority channel: {}",
                id
            );
            // Use send_priority to ensure Stop is processed before normal messages
            match handle.send_priority(PlatformMessage::Stop).await {
                Ok(()) => {}
                Err(e) => {
                    warn!("Failed to send Stop to platform {}: {:?}", id, e);
                    stop_message_failures += 1;
                }
            }
        }

        info!(
            "Stop messages sent: {} successful, {} failed",
            total_actors - stop_message_failures,
            stop_message_failures
        );

        // Phase 2: Wait for actors to complete with timeout
        info!(
            "Phase 2: Waiting for actors to stop gracefully (timeout: {:?})",
            self.config.shutdown_timeout
        );
        let deadline = tokio::time::Instant::now() + self.config.shutdown_timeout;

        while self.registry.has_pending_tasks() {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    let remaining = self.registry.pending_task_count();
                    warn!(
                        "Shutdown timeout reached, {} tasks still running",
                        remaining
                    );

                    // Phase 3: Forcefully terminate non-responsive actors
                    info!("Phase 3: Forcefully terminating {} non-responsive actors", remaining);

                    // First try cancellation tokens
                    self.registry.cancel_all();

                    // Give a brief grace period for cancellation to take effect
                    tokio::time::sleep(Duration::from_millis(100)).await;

                    // If still running, abort tasks
                    if self.registry.has_pending_tasks() {
                        forced_terminations = self.registry.pending_task_count();
                        self.registry.abort_all();
                    }
                    break;
                }
                result = self.registry.join_next() => {
                    match result {
                        Some(Ok(task_result)) => {
                            if task_result.is_crash() {
                                // Don't count crashes during shutdown as graceful
                                debug!("Actor {} crashed during shutdown", task_result.actor_id);
                                // Crashes during shutdown are counted as forced terminations
                                forced_terminations += 1;
                            } else {
                                debug!("Actor {} stopped gracefully", task_result.actor_id);
                                graceful_stops += 1;
                            }
                        }
                        Some(Err(e)) => {
                            warn!("Task join error during shutdown: {}", e);
                            forced_terminations += 1;
                        }
                        None => break,
                    }
                }
            }
        }

        // Clear registry and pending restarts
        self.registry.clear();
        self.pending_restarts.clear();

        let report = ShutdownReport {
            total_actors,
            graceful_stops,
            forced_terminations,
            stop_message_failures,
        };

        info!(
            "Shutdown complete: {} total, {} graceful, {} forced, {} message failures",
            report.total_actors,
            report.graceful_stops,
            report.forced_terminations,
            report.stop_message_failures
        );

        report
    }

    /// Check if the supervisor is cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Get statistics about the supervisor.
    pub fn stats(&self) -> SupervisorStats {
        SupervisorStats {
            streamer_count: self.registry.streamer_count(),
            platform_count: self.registry.platform_count(),
            pending_restarts: self.pending_restarts.len(),
            restart_stats: self.restart_tracker.stats(),
        }
    }
}

/// Action to take after a task completion.
#[derive(Debug, Clone)]
pub enum TaskCompletionAction {
    /// Actor stopped gracefully.
    Stopped { actor_id: String },
    /// Actor was cancelled.
    Cancelled { actor_id: String },
    /// Actor completed its work.
    Completed { actor_id: String },
    /// Actor crashed.
    Crashed { actor_id: String },
    /// Restart has been scheduled.
    RestartScheduled { actor_id: String, backoff: Duration },
    /// Restart failed.
    RestartFailed { actor_id: String, reason: String },
    /// Restart limit exceeded.
    RestartLimitExceeded { actor_id: String },
}

/// Error type for spawn operations.
#[derive(Debug, Clone)]
pub enum SpawnError {
    /// An actor with the given ID already exists.
    ActorExists(String),
    /// Registry error.
    RegistryError(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::ActorExists(id) => write!(f, "Actor already exists: {}", id),
            SpawnError::RegistryError(e) => write!(f, "Registry error: {}", e),
        }
    }
}

impl std::error::Error for SpawnError {}

/// Report from a shutdown operation.
#[derive(Debug, Clone)]
pub struct ShutdownReport {
    /// Total number of actors at shutdown start.
    pub total_actors: usize,
    /// Number of actors that stopped gracefully.
    pub graceful_stops: usize,
    /// Number of actors that were forcefully terminated.
    pub forced_terminations: usize,
    /// Number of Stop messages that failed to send.
    pub stop_message_failures: usize,
}

impl ShutdownReport {
    /// Check if all actors stopped gracefully.
    pub fn all_graceful(&self) -> bool {
        self.forced_terminations == 0 && self.stop_message_failures == 0
    }

    /// Get the percentage of actors that stopped gracefully.
    pub fn graceful_percentage(&self) -> f64 {
        if self.total_actors == 0 {
            100.0
        } else {
            (self.graceful_stops as f64 / self.total_actors as f64) * 100.0
        }
    }
}

/// Statistics about the supervisor.
#[derive(Debug, Clone)]
pub struct SupervisorStats {
    /// Number of streamer actors.
    pub streamer_count: usize,
    /// Number of platform actors.
    pub platform_count: usize,
    /// Number of pending restarts.
    pub pending_restarts: usize,
    /// Restart tracker statistics.
    pub restart_stats: super::restart_tracker::RestartTrackerStats,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Priority, StreamerState};
    use crate::scheduler::actor::messages::StreamerConfig;
    use crate::scheduler::actor::registry::ActorTaskResult;
    use crate::scheduler::actor::streamer_actor::ActorOutcome;

    fn create_test_metadata(id: &str) -> StreamerMetadata {
        StreamerMetadata {
            id: id.to_string(),
            name: format!("Test Streamer {}", id),
            url: format!("https://twitch.tv/{}", id),
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
            check_interval_ms: 1000,
            offline_check_interval_ms: 500,
            offline_check_count: 3,
            priority: Priority::Normal,
            batch_capable: false,
        }
    }

    fn create_test_platform_config(platform_id: &str) -> PlatformConfig {
        PlatformConfig {
            platform_id: platform_id.to_string(),
            batch_window_ms: 100,
            max_batch_size: 10,
            rate_limit: None,
        }
    }

    #[test]
    fn test_supervisor_new() {
        let token = CancellationToken::new();
        let supervisor = Supervisor::new(token);

        assert_eq!(supervisor.registry().streamer_count(), 0);
        assert_eq!(supervisor.registry().platform_count(), 0);
        assert_eq!(supervisor.pending_restart_count(), 0);
    }

    #[tokio::test]
    async fn test_supervisor_spawn_streamer() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        let metadata = create_test_metadata("test-1");
        let config = create_test_config();

        let result = supervisor.spawn_streamer(metadata, config, None);
        assert!(result.is_ok());
        assert_eq!(supervisor.registry().streamer_count(), 1);
        assert!(supervisor.registry().has_streamer("test-1"));

        // Cancel to clean up
        token.cancel();
    }

    #[tokio::test]
    async fn test_supervisor_spawn_duplicate_streamer() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        let metadata = create_test_metadata("test-1");
        let config = create_test_config();

        supervisor
            .spawn_streamer(metadata.clone(), config.clone(), None)
            .unwrap();

        // Try to spawn duplicate
        let result = supervisor.spawn_streamer(metadata, config, None);
        assert!(matches!(result, Err(SpawnError::ActorExists(_))));

        token.cancel();
    }

    #[tokio::test]
    async fn test_supervisor_spawn_platform() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        let config = create_test_platform_config("twitch");

        let result = supervisor.spawn_platform("twitch", config);
        assert!(result.is_ok());
        assert_eq!(supervisor.registry().platform_count(), 1);
        assert!(supervisor.registry().has_platform("twitch"));

        token.cancel();
    }

    #[tokio::test]
    async fn test_supervisor_remove_streamer() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        let metadata = create_test_metadata("test-1");
        let config = create_test_config();

        supervisor.spawn_streamer(metadata, config, None).unwrap();
        assert!(supervisor.registry().has_streamer("test-1"));

        let removed = supervisor.remove_streamer("test-1");
        assert!(removed);
        assert!(!supervisor.registry().has_streamer("test-1"));

        token.cancel();
    }

    #[test]
    fn test_supervisor_handle_graceful_stop() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token);

        let result = ActorTaskResult::streamer("test-1", Ok(ActorOutcome::Stopped));
        let action = supervisor.handle_task_completion(result);

        assert!(matches!(action, TaskCompletionAction::Stopped { .. }));
    }

    #[test]
    fn test_supervisor_handle_cancellation() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token);

        let result = ActorTaskResult::streamer("test-1", Ok(ActorOutcome::Cancelled));
        let action = supervisor.handle_task_completion(result);

        assert!(matches!(action, TaskCompletionAction::Cancelled { .. }));
    }

    #[tokio::test]
    async fn test_supervisor_handle_crash_schedules_restart() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        // Spawn an actor first so we have metadata for restart
        let metadata = create_test_metadata("test-1");
        let config = create_test_config();
        supervisor.spawn_streamer(metadata, config, None).unwrap();

        // Simulate crash
        let result = ActorTaskResult::streamer(
            "test-1",
            Err(crate::scheduler::actor::streamer_actor::ActorError::fatal(
                "test crash",
            )),
        );
        let action = supervisor.handle_task_completion(result);

        // First crash should schedule immediate restart (no backoff)
        assert!(matches!(
            action,
            TaskCompletionAction::RestartScheduled { .. }
        ));
        assert_eq!(supervisor.pending_restart_count(), 1);

        token.cancel();
    }

    #[tokio::test]
    async fn test_supervisor_process_pending_restarts() {
        let token = CancellationToken::new();
        let config = SupervisorConfig {
            restart_config: RestartTrackerConfig {
                base_backoff: Duration::from_millis(1), // Very short for testing
                ..Default::default()
            },
            ..Default::default()
        };
        let mut supervisor = Supervisor::with_config(token.clone(), config);

        // Spawn an actor
        let metadata = create_test_metadata("test-1");
        let config = create_test_config();
        supervisor.spawn_streamer(metadata, config, None).unwrap();

        // Simulate crash
        let result = ActorTaskResult::streamer(
            "test-1",
            Err(crate::scheduler::actor::streamer_actor::ActorError::fatal(
                "test crash",
            )),
        );
        supervisor.handle_task_completion(result);

        // Wait a bit for backoff
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Process restarts
        let restarted = supervisor.process_pending_restarts();
        assert_eq!(restarted, 1);
        assert_eq!(supervisor.pending_restart_count(), 0);

        // Actor should be back
        assert!(supervisor.registry().has_streamer("test-1"));

        token.cancel();
    }

    #[tokio::test]
    async fn test_supervisor_shutdown() {
        let token = CancellationToken::new();
        let config = SupervisorConfig {
            shutdown_timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let mut supervisor = Supervisor::with_config(token.clone(), config);

        // Spawn some actors
        for i in 0..3 {
            let metadata = create_test_metadata(&format!("test-{}", i));
            let config = create_test_config();
            supervisor.spawn_streamer(metadata, config, None).unwrap();
        }

        assert_eq!(supervisor.registry().streamer_count(), 3);

        // Shutdown
        let report = supervisor.shutdown().await;

        assert_eq!(report.total_actors, 3);
        assert_eq!(supervisor.registry().streamer_count(), 0);
    }

    #[test]
    fn test_supervisor_stats() {
        let token = CancellationToken::new();
        let supervisor = Supervisor::new(token);

        let stats = supervisor.stats();
        assert_eq!(stats.streamer_count, 0);
        assert_eq!(stats.platform_count, 0);
        assert_eq!(stats.pending_restarts, 0);
    }

    #[test]
    fn test_spawn_error_display() {
        let exists = SpawnError::ActorExists("test".to_string());
        assert_eq!(exists.to_string(), "Actor already exists: test");

        let registry = SpawnError::RegistryError("error".to_string());
        assert_eq!(registry.to_string(), "Registry error: error");
    }

    #[tokio::test]
    async fn test_supervisor_shutdown_graceful_with_stop_messages() {
        let token = CancellationToken::new();
        let config = SupervisorConfig {
            shutdown_timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let mut supervisor = Supervisor::with_config(token.clone(), config);

        // Spawn streamer and platform actors
        for i in 0..2 {
            let metadata = create_test_metadata(&format!("streamer-{}", i));
            let config = create_test_config();
            supervisor.spawn_streamer(metadata, config, None).unwrap();
        }

        let platform_config = create_test_platform_config("twitch");
        supervisor
            .spawn_platform("twitch", platform_config)
            .unwrap();

        assert_eq!(supervisor.registry().streamer_count(), 2);
        assert_eq!(supervisor.registry().platform_count(), 1);

        // Shutdown - actors should receive Stop messages and stop gracefully
        let report = supervisor.shutdown().await;

        // Verify shutdown report
        assert_eq!(report.total_actors, 3);
        // All actors should have stopped gracefully (received Stop message)
        assert_eq!(report.stop_message_failures, 0);
        assert_eq!(supervisor.registry().streamer_count(), 0);
        assert_eq!(supervisor.registry().platform_count(), 0);
    }

    #[tokio::test]
    async fn test_supervisor_shutdown_empty() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token);

        // Shutdown with no actors
        let report = supervisor.shutdown().await;

        assert_eq!(report.total_actors, 0);
        assert_eq!(report.graceful_stops, 0);
        assert_eq!(report.forced_terminations, 0);
        assert_eq!(report.stop_message_failures, 0);
        assert!(report.all_graceful());
        assert_eq!(report.graceful_percentage(), 100.0);
    }

    #[test]
    fn test_shutdown_report_all_graceful() {
        let report = ShutdownReport {
            total_actors: 5,
            graceful_stops: 5,
            forced_terminations: 0,
            stop_message_failures: 0,
        };
        assert!(report.all_graceful());
        assert_eq!(report.graceful_percentage(), 100.0);
    }

    #[test]
    fn test_shutdown_report_with_forced() {
        let report = ShutdownReport {
            total_actors: 5,
            graceful_stops: 3,
            forced_terminations: 2,
            stop_message_failures: 0,
        };
        assert!(!report.all_graceful());
        assert_eq!(report.graceful_percentage(), 60.0);
    }

    #[test]
    fn test_shutdown_report_with_message_failures() {
        let report = ShutdownReport {
            total_actors: 5,
            graceful_stops: 4,
            forced_terminations: 0,
            stop_message_failures: 1,
        };
        assert!(!report.all_graceful());
        assert_eq!(report.graceful_percentage(), 80.0);
    }

    #[tokio::test]
    async fn test_supervisor_spawns_actors_with_priority_channel() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        // Spawn a streamer actor
        let metadata = create_test_metadata("test-priority");
        let config = create_test_config();
        let handle = supervisor.spawn_streamer(metadata, config, None).unwrap();

        // Verify the handle supports priority sending by sending a priority message
        // If priority channel wasn't configured, send_priority would fall back to normal send
        // but we can verify it works without error
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle
            .send_priority(StreamerMessage::GetState(reply_tx))
            .await
            .unwrap();

        // Should receive state response
        let state = reply_rx.await.unwrap();
        assert_eq!(state.streamer_state, StreamerState::NotLive);

        // Stop via priority channel
        handle.send_priority(StreamerMessage::Stop).await.unwrap();

        // Give time for actor to stop
        tokio::time::sleep(Duration::from_millis(50)).await;

        token.cancel();
    }

    #[tokio::test]
    async fn test_supervisor_spawns_platform_with_priority_channel() {
        let token = CancellationToken::new();
        let mut supervisor = Supervisor::new(token.clone());

        // Spawn a platform actor
        let config = create_test_platform_config("test-platform");
        let handle = supervisor.spawn_platform("test-platform", config).unwrap();

        // Verify the handle supports priority sending
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        handle
            .send_priority(PlatformMessage::GetState(reply_tx))
            .await
            .unwrap();

        // Should receive state response
        let state = reply_rx.await.unwrap();
        assert_eq!(state.streamer_count, 0);

        // Stop via priority channel
        handle.send_priority(PlatformMessage::Stop).await.unwrap();

        // Give time for actor to stop
        tokio::time::sleep(Duration::from_millis(50)).await;

        token.cancel();
    }
}
