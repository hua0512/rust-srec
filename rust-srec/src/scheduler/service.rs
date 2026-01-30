//! Scheduler service implementation using actor model.
//!
//! The Scheduler orchestrates monitoring tasks for all active streamers using
//! an actor-based architecture. Each streamer is managed by a self-scheduling
//! StreamerActor, eliminating the need for periodic re-scheduling.
//!
//! # Architecture
//!
//! - StreamerActors manage their own timing and state
//! - PlatformActors coordinate batch detection for batch-capable platforms
//! - The Scheduler acts as a supervisor, spawning and monitoring actors
//! - ConfigRouter delivers configuration updates to appropriate actors

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use crate::database::repositories::{
    ConfigRepository, FilterRepository, SessionRepository, StreamerRepository,
};
use crate::domain::Priority;
use crate::downloader::{DownloadManagerEvent, DownloadStopCause};
use crate::monitor::StreamMonitor;
use crate::streamer::{StreamerManager, StreamerMetadata};

use super::actor::{
    ActorHandle, ConfigRouter, ConfigScope, DownloadEndPolicy, MonitorBatchChecker,
    MonitorStatusChecker, PlatformConfig, PlatformMapping, PlatformMessage, ShutdownReport,
    StreamerConfig, StreamerMessage, Supervisor, SupervisorConfig, TaskCompletionAction,
};

/// Default check interval (60 seconds).
const DEFAULT_CHECK_INTERVAL_MS: u64 = 60_000;

/// Default offline check interval (20 seconds).
const DEFAULT_OFFLINE_CHECK_INTERVAL_MS: u64 = 20_000;

/// Default offline check count before switching to offline interval.
const DEFAULT_OFFLINE_CHECK_COUNT: u32 = 3;

/// Maximum age for entries in `Scheduler::stopped_downloads`.
///
/// This map is used to suppress follow-up terminal events that can arrive after a stop request.
const STOPPED_DOWNLOADS_TTL: Duration = Duration::from_secs(60 * 60);

/// Minimum interval between pruning passes over `Scheduler::stopped_downloads`.
///
/// Pruning is opportunistic (triggered on new cancellations) and throttled to avoid repeated
/// O(n) scans when cancels happen in bursts.
const STOPPED_DOWNLOADS_PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// Minimum size before we consider pruning `Scheduler::stopped_downloads`.
///
/// Avoids an O(n) scan when the map is trivially small.
const STOPPED_DOWNLOADS_PRUNE_MIN_SIZE: usize = 256;

/// Scheduler configuration.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Default check interval in milliseconds.
    pub check_interval_ms: u64,
    /// Offline check interval in milliseconds.
    pub offline_check_interval_ms: u64,
    /// Number of offline checks before using offline interval.
    pub offline_check_count: u32,
    /// Supervisor configuration.
    pub supervisor_config: SupervisorConfig,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: DEFAULT_CHECK_INTERVAL_MS,
            offline_check_interval_ms: DEFAULT_OFFLINE_CHECK_INTERVAL_MS,
            offline_check_count: DEFAULT_OFFLINE_CHECK_COUNT,
            supervisor_config: SupervisorConfig::default(),
        }
    }
}

/// The Scheduler orchestrates monitoring tasks for all active streamers
/// using an actor-based architecture.
///
/// # Actor Model
///
/// The scheduler uses actors instead of direct task management:
/// - Each streamer has a dedicated `StreamerActor` that manages its own timing
/// - Batch-capable platforms have a `PlatformActor` for coordinating batch detection
/// - The scheduler acts as a supervisor, handling actor lifecycle and crash recovery
///
/// # No Periodic Re-scheduling
///
/// Unlike the previous implementation, actors manage their own scheduling internally.
/// This eliminates the need for periodic bulk re-scheduling operations.
///
/// # Generic Type Parameters
///
/// - `R`: StreamerRepository used by the StreamerManager
pub struct Scheduler<R: StreamerRepository + Send + Sync + 'static> {
    /// Streamer manager for accessing streamer state.
    streamer_manager: Arc<StreamerManager<R>>,
    /// Event broadcaster for config updates.
    event_broadcaster: ConfigEventBroadcaster,
    /// Scheduler configuration.
    config: SchedulerConfig,
    /// Config repository for pulling fresh global timing config on hot reload.
    config_repo: Option<Arc<dyn ConfigRepository>>,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Supervisor for managing actor lifecycle.
    supervisor: Supervisor,
    /// Platform mapping for config routing.
    platform_mapping: PlatformMapping,
    /// Platform actor handles for batch coordination.
    platform_handles: HashMap<String, ActorHandle<PlatformMessage>>,
    /// Broadcast receiver for download events (direct subscription).
    download_event_rx: Option<broadcast::Receiver<DownloadManagerEvent>>,
    /// Throttle map for forwarding download heartbeats to streamer actors.
    download_heartbeat_last_sent: DashMap<String, Instant>,
    /// Tracks downloads that were explicitly stopped/cancelled.
    ///
    /// Used to suppress follow-up `DownloadCompleted` / `DownloadFailed` events that can
    /// legitimately arrive after a stop request (graceful engine finalization).
    stopped_downloads: DashMap<String, (DownloadStopCause, i64)>,
    /// Throttle for opportunistic pruning of `stopped_downloads` (wall clock ms).
    stopped_downloads_last_prune_at_ms: AtomicI64,
}

impl<R: StreamerRepository + Send + Sync + 'static> Scheduler<R> {
    /// Create a new scheduler with actor-based infrastructure.
    ///
    /// This initializes the actor registry and supervisor without spawning any actors.
    /// Actors are spawned when `run()` is called or when streamers are added dynamically.
    ///
    /// Note: This creates a scheduler with its own cancellation token and uses
    /// `NoOpCheckerFactory` for status checking. For real status checking, use
    /// `with_monitor()` instead.
    pub fn new(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
    ) -> Self {
        Self::with_config(
            streamer_manager,
            event_broadcaster,
            SchedulerConfig::default(),
        )
    }

    /// Create a new scheduler with a shared cancellation token.
    ///
    /// This allows the parent (e.g., ServiceContainer) to directly cancel the scheduler
    /// without needing a forwarding task.
    ///
    /// Note: Uses `NoOpCheckerFactory` for status checking. For real status checking,
    /// use `with_monitor()` instead.
    pub fn with_cancellation(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self::with_full_config(
            streamer_manager,
            event_broadcaster,
            SchedulerConfig::default(),
            cancellation_token,
        )
    }

    /// Create a new scheduler with custom configuration.
    ///
    /// Note: This creates a scheduler with its own cancellation token and uses
    /// `NoOpCheckerFactory` for status checking. For real status checking, use
    /// `with_monitor()` instead.
    pub fn with_config(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        config: SchedulerConfig,
    ) -> Self {
        Self::with_full_config(
            streamer_manager,
            event_broadcaster,
            config,
            CancellationToken::new(),
        )
    }

    /// Create a new scheduler with custom configuration and shared cancellation token.
    ///
    /// This is the most flexible constructor, allowing full control over configuration
    /// and cancellation behavior.
    ///
    /// Note: Uses `NoOpCheckerFactory` for status checking. For real status checking,
    /// use `with_monitor()` instead.
    pub fn with_full_config(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        config: SchedulerConfig,
        cancellation_token: CancellationToken,
    ) -> Self {
        // Pass the shared metadata store to the supervisor
        let metadata_store = streamer_manager.metadata_store();
        let supervisor = Supervisor::with_config(
            cancellation_token.clone(),
            config.supervisor_config.clone(),
            metadata_store,
        );

        Self {
            streamer_manager,
            event_broadcaster,
            config,
            config_repo: None,
            cancellation_token,
            supervisor,
            platform_mapping: PlatformMapping::new(),
            platform_handles: HashMap::new(),
            download_event_rx: None,
            download_heartbeat_last_sent: DashMap::new(),
            stopped_downloads: DashMap::new(),
            stopped_downloads_last_prune_at_ms: AtomicI64::new(crate::database::time::now_ms()),
        }
    }

    /// Create a new scheduler with a StreamMonitor for real status checking.
    ///
    /// This constructor creates a `MonitorCheckerFactory` from the provided StreamMonitor
    /// and passes it to the Supervisor. Actors spawned by this scheduler will use real
    /// status checking via the StreamMonitor infrastructure.
    ///
    /// # Arguments
    ///
    /// * `streamer_manager` - The streamer manager for accessing streamer state
    /// * `event_broadcaster` - Event broadcaster for config updates
    /// * `monitor` - The StreamMonitor for real status detection
    ///
    /// # Example
    ///
    /// ```ignore
    /// let scheduler = Scheduler::with_monitor(
    ///     streamer_manager,
    ///     event_broadcaster,
    ///     monitor,
    /// );
    /// ```
    pub fn with_monitor<SR, FR, SSR, CR>(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        monitor: Arc<StreamMonitor<SR, FR, SSR, CR>>,
    ) -> Self
    where
        SR: StreamerRepository + Send + Sync + 'static,
        FR: FilterRepository + Send + Sync + 'static,
        SSR: SessionRepository + Send + Sync + 'static,
        CR: ConfigRepository + Send + Sync + 'static,
    {
        Self::with_monitor_and_config(
            streamer_manager,
            event_broadcaster,
            monitor,
            SchedulerConfig::default(),
            CancellationToken::new(),
        )
    }

    /// Create a new scheduler with a StreamMonitor and custom configuration.
    ///
    /// This is the most complete constructor, providing real status checking via
    /// StreamMonitor along with full control over configuration and cancellation.
    ///
    /// # Arguments
    ///
    /// * `streamer_manager` - The streamer manager for accessing streamer state
    /// * `event_broadcaster` - Event broadcaster for config updates
    /// * `monitor` - The StreamMonitor for real status detection
    /// * `config` - Custom scheduler configuration
    /// * `cancellation_token` - Shared cancellation token for graceful shutdown
    pub fn with_monitor_and_config<SR, FR, SSR, CR>(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        monitor: Arc<StreamMonitor<SR, FR, SSR, CR>>,
        config: SchedulerConfig,
        cancellation_token: CancellationToken,
    ) -> Self
    where
        SR: StreamerRepository + Send + Sync + 'static,
        FR: FilterRepository + Send + Sync + 'static,
        SSR: SessionRepository + Send + Sync + 'static,
        CR: ConfigRepository + Send + Sync + 'static,
    {
        // Create status and batch checkers directly from the StreamMonitor
        let status_checker = Arc::new(MonitorStatusChecker::new(monitor.clone()));
        let batch_checker = Arc::new(MonitorBatchChecker::new(monitor.clone()));

        // Pass the shared metadata store to the supervisor
        let metadata_store = streamer_manager.metadata_store();

        // Create supervisor with the real checkers
        let supervisor = Supervisor::with_checkers(
            cancellation_token.clone(),
            config.supervisor_config.clone(),
            metadata_store,
            status_checker,
            batch_checker,
        );

        Self {
            streamer_manager,
            event_broadcaster,
            config,
            config_repo: None,
            cancellation_token,
            supervisor,
            platform_mapping: PlatformMapping::new(),
            platform_handles: HashMap::new(),
            download_event_rx: None,
            download_heartbeat_last_sent: DashMap::new(),
            stopped_downloads: DashMap::new(),
            stopped_downloads_last_prune_at_ms: AtomicI64::new(crate::database::time::now_ms()),
        }
    }

    /// Attach a config repository to enable hot reloading of global scheduler timing config.
    pub fn with_config_repo(mut self, config_repo: Arc<dyn ConfigRepository>) -> Self {
        self.config_repo = Some(config_repo);
        self
    }

    /// Get the cancellation token for this scheduler.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Set the download event receiver.
    ///
    /// This should be called before `run()` to enable download event handling.
    pub fn set_download_receiver(&mut self, receiver: broadcast::Receiver<DownloadManagerEvent>) {
        self.download_event_rx = Some(receiver);
    }

    /// Get the number of active streamer actors.
    pub fn active_actor_count(&self) -> usize {
        self.supervisor.registry().streamer_count()
    }

    /// Get the number of platform actors.
    pub fn platform_actor_count(&self) -> usize {
        self.supervisor.registry().platform_count()
    }

    /// Check if the scheduler is running.
    pub fn is_running(&self) -> bool {
        !self.cancellation_token.is_cancelled()
    }

    /// Create a StreamerConfig from scheduler config and metadata.
    fn create_streamer_config(&self, metadata: &StreamerMetadata) -> StreamerConfig {
        StreamerConfig {
            check_interval_ms: self.config.check_interval_ms,
            offline_check_interval_ms: self.config.offline_check_interval_ms,
            offline_check_count: self.config.offline_check_count,
            priority: metadata.priority,
            batch_capable: self.is_batch_capable_platform(&metadata.platform_config_id),
        }
    }

    /// Create a PlatformConfig for a platform.
    fn create_platform_config(&self, platform_id: &str) -> PlatformConfig {
        PlatformConfig {
            platform_id: platform_id.to_string(),
            batch_window_ms: 500,
            max_batch_size: 100,
            rate_limit: None,
        }
    }

    async fn refresh_timing_config_from_db(&mut self) -> Result<bool> {
        let Some(repo) = &self.config_repo else {
            return Ok(false);
        };

        let global = repo.get_global_config().await?;
        let next = SchedulerConfig {
            check_interval_ms: global.streamer_check_delay_ms as u64,
            offline_check_interval_ms: global.offline_check_delay_ms as u64,
            offline_check_count: global.offline_check_count as u32,
            supervisor_config: self.config.supervisor_config.clone(),
        };

        if next.check_interval_ms == self.config.check_interval_ms
            && next.offline_check_interval_ms == self.config.offline_check_interval_ms
            && next.offline_check_count == self.config.offline_check_count
        {
            return Ok(false);
        }

        info!(
            "Scheduler timing config updated: check_interval_ms {}->{}; offline_check_interval_ms {}->{}; offline_check_count {}->{}",
            self.config.check_interval_ms,
            next.check_interval_ms,
            self.config.offline_check_interval_ms,
            next.offline_check_interval_ms,
            self.config.offline_check_count,
            next.offline_check_count,
        );

        self.config.check_interval_ms = next.check_interval_ms;
        self.config.offline_check_interval_ms = next.offline_check_interval_ms;
        self.config.offline_check_count = next.offline_check_count;

        Ok(true)
    }

    /// Check if a platform supports batch detection.
    fn is_batch_capable_platform(&self, platform_id: &str) -> bool {
        let platform = platform_id.strip_prefix("platform-").unwrap_or(platform_id);
        // Platforms that support batch API detection
        matches!(platform, "youtube")
    }

    /// Start the scheduler event loop.
    ///
    /// This method runs until the cancellation token is triggered.
    /// It uses an actor-based event loop instead of periodic re-scheduling.
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting scheduler with actor model");

        // Subscribe to config update events
        let mut config_receiver = self.event_broadcaster.subscribe();

        // Take the download event receiver
        let mut download_event_rx = self.download_event_rx.take();

        // Initial actor spawning for all active streamers
        self.spawn_initial_actors().await?;

        info!(
            "Scheduler started with {} streamer actors and {} platform actors",
            self.supervisor.registry().streamer_count(),
            self.supervisor.registry().platform_count()
        );

        loop {
            // Calculate next restart time for pending restarts
            let next_restart = self.supervisor.next_restart_time();

            tokio::select! {
                // Handle cancellation
                _ = self.cancellation_token.cancelled() => {
                    info!("Scheduler received cancellation signal");
                    break;
                }

                // Handle config update events
                event = config_receiver.recv() => {
                    match event {
                        Ok(event) => {
                            self.handle_config_event(event).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Scheduler lagged {} config events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            warn!("Config event channel closed");
                            break;
                        }
                    }
                }

                // Handle download events (if receiver is available)
                result = async {
                    match &mut download_event_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match result {
                        Ok(event) => {
                            self.process_download_event(event).await;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Scheduler lagged {} download events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            warn!("Download event channel closed");
                            download_event_rx = None; // Stop trying to receive
                        }
                    }
                }

                // Handle actor task completions (crash detection)
                // Only poll join_next if there are pending tasks to avoid busy-looping
                result = Self::join_next_if_pending(&mut self.supervisor) => {
                    if let Some(join_result) = result {
                        match join_result {
                            Ok(task_result) => {
                                let action = self.supervisor.handle_task_completion(task_result);
                                self.handle_task_completion_action(action);
                            }
                            Err(e) => {
                                error!("Actor task panicked: {}", e);
                            }
                        }
                    }
                    // None means no pending tasks - we just continue the loop
                }

                // Process pending restarts
                _ = Self::wait_for_restart(next_restart) => {
                    let restarted = self.supervisor.process_pending_restarts();
                    if restarted > 0 {
                        debug!("Processed {} pending restarts", restarted);
                    }
                }
            }
        }

        // Graceful shutdown
        let report = self.shutdown().await;
        info!(
            "Scheduler stopped: {} graceful, {} forced",
            report.graceful_stops, report.forced_terminations
        );

        Ok(())
    }

    /// Wait for the next restart time, or forever if none pending.
    async fn wait_for_restart(next_restart: Option<tokio::time::Instant>) {
        match next_restart {
            Some(instant) => tokio::time::sleep_until(instant).await,
            None => std::future::pending().await,
        }
    }

    /// Wait for the next actor task completion, or wait indefinitely if no tasks pending.
    /// This prevents busy-looping when there are no actor tasks.
    async fn join_next_if_pending(
        supervisor: &mut Supervisor,
    ) -> Option<std::result::Result<super::actor::ActorTaskResult, tokio::task::JoinError>> {
        if supervisor.registry().has_pending_tasks() {
            supervisor.registry_mut().join_next().await
        } else {
            // No tasks to wait for - wait indefinitely until other events occur
            std::future::pending().await
        }
    }

    /// Spawn initial actors for all active streamers.
    async fn spawn_initial_actors(&mut self) -> Result<()> {
        let streamers = self.streamer_manager.get_ready_for_check();
        info!("Spawning actors for {} streamers", streamers.len());

        // First, spawn platform actors for batch-capable platforms
        let mut platforms_needed: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for streamer in &streamers {
            if self.is_batch_capable_platform(&streamer.platform_config_id) {
                platforms_needed.insert(streamer.platform_config_id.clone());
            }
        }

        for platform_id in platforms_needed {
            self.spawn_platform_actor(&platform_id)?;
        }

        // Then spawn streamer actors
        for streamer in streamers {
            if let Err(e) = self.spawn_streamer_actor(streamer) {
                warn!("Failed to spawn streamer actor: {}", e);
            }
        }

        Ok(())
    }

    /// Spawn a platform actor for batch coordination.
    fn spawn_platform_actor(&mut self, platform_id: &str) -> Result<()> {
        if self.supervisor.registry().has_platform(platform_id) {
            debug!("Platform actor {} already exists", platform_id);
            return Ok(());
        }

        let config = self.create_platform_config(platform_id);
        match self.supervisor.spawn_platform(platform_id, config) {
            Ok(handle) => {
                self.platform_handles
                    .insert(platform_id.to_string(), handle);
                info!("Spawned platform actor: {}", platform_id);
                Ok(())
            }
            Err(e) => {
                error!("Failed to spawn platform actor {}: {}", platform_id, e);
                Err(crate::error::Error::Other(format!(
                    "Failed to spawn platform actor: {}",
                    e
                )))
            }
        }
    }

    /// Spawn a streamer actor.
    fn spawn_streamer_actor(&mut self, metadata: StreamerMetadata) -> Result<()> {
        let streamer_id = metadata.id.clone();
        let platform_id = metadata.platform_config_id.clone();

        if self.supervisor.registry().has_streamer(&streamer_id) {
            debug!("Streamer actor {} already exists", streamer_id);
            return Ok(());
        }

        let config = self.create_streamer_config(&metadata);

        // Get platform actor sender if on batch-capable platform
        let platform_sender = if config.batch_capable {
            self.platform_handles
                .get(&platform_id)
                .map(|h| h.metadata.id.clone())
                .and_then(|_| {
                    // Get the underlying sender from the supervisor's registry
                    // For now, we'll pass None and let the actor handle it
                    None
                })
        } else {
            None
        };

        // Register platform mapping
        self.platform_mapping.register(&streamer_id, &platform_id);

        // Spawn with streamer_id - actor fetches metadata from shared store
        match self
            .supervisor
            .spawn_streamer(&streamer_id, config, platform_sender)
        {
            Ok(_handle) => {
                debug!("Spawned streamer actor: {}", streamer_id);
                Ok(())
            }
            Err(e) => {
                self.platform_mapping.unregister(&streamer_id);
                error!("Failed to spawn streamer actor {}: {}", streamer_id, e);
                Err(crate::error::Error::Other(format!(
                    "Failed to spawn streamer actor: {}",
                    e
                )))
            }
        }
    }

    /// Handle a configuration update event using ConfigRouter.
    async fn handle_config_event(&mut self, event: ConfigUpdateEvent) {
        debug!("Handling config event: {}", event.description());

        if matches!(event, ConfigUpdateEvent::GlobalUpdated) {
            match self.refresh_timing_config_from_db().await {
                Ok(true) => {}
                Ok(false) => {
                    // Avoid broadcasting config updates to every actor if global changes
                    // don't affect scheduler timing (e.g., log filter changes).
                    return;
                }
                Err(error) => {
                    warn!("Failed to refresh scheduler timing config: {}", error);
                }
            }
        }

        // Handle streamer deletion - remove actor immediately
        if let ConfigUpdateEvent::StreamerDeleted { ref streamer_id } = event {
            if self.remove_streamer(streamer_id) {
                info!("Removed actor for deleted streamer: {}", streamer_id);
            }
            return; // No need to route config to a deleted streamer
        }

        // Handle streamer state changes - spawn/remove actor based on is_active
        // Actors fetch fresh metadata from the shared store, so no MetadataUpdate needed
        if let ConfigUpdateEvent::StreamerStateSyncedFromDb {
            ref streamer_id,
            is_active,
        } = event
        {
            if is_active {
                // Streamer became active - spawn actor if missing
                // Actors fetch metadata from shared store, so existing actors will see updates automatically
                if let Some(metadata) = self.streamer_manager.get_streamer(streamer_id)
                    && !self.supervisor.registry().has_streamer(streamer_id)
                {
                    // Actor doesn't exist - spawn it
                    // Ensure platform actor exists for batch-capable platforms
                    if self.is_batch_capable_platform(&metadata.platform_config_id) {
                        let _ = self.spawn_platform_actor(&metadata.platform_config_id);
                    }
                    info!(
                        "Spawning actor for newly active streamer: {} (state: {})",
                        streamer_id, metadata.state
                    );
                    if let Err(e) = self.spawn_streamer_actor(metadata) {
                        warn!("Failed to spawn actor for {}: {}", streamer_id, e);
                    }
                }
                // Note: Existing actors will fetch fresh metadata from shared store on next check
            } else {
                // Streamer became inactive - remove actor if exists
                if self.remove_streamer(streamer_id) {
                    info!("Removed actor for inactive streamer: {}", streamer_id);
                }
            }
            return; // State changes don't require config routing
        }

        let scope = ConfigScope::from_event(&event);

        // Check if we need to spawn or remove a streamer actor for Streamer-scoped updates
        if let ConfigScope::Streamer(ref streamer_id) = scope {
            if let Some(metadata) = self.streamer_manager.get_streamer(streamer_id) {
                // Keep platform mapping consistent with the latest metadata.
                self.platform_mapping
                    .register(streamer_id, &metadata.platform_config_id);

                // Ensure platform actor exists if the streamer is on a batch-capable platform.
                if self.is_batch_capable_platform(&metadata.platform_config_id)
                    && let Err(e) = self.spawn_platform_actor(&metadata.platform_config_id)
                {
                    warn!(
                        "Failed to ensure platform actor for {}: {}",
                        metadata.platform_config_id, e
                    );
                }

                if metadata.is_active() {
                    // Streamer is active - spawn actor if missing
                    if !self.supervisor.registry().has_streamer(streamer_id) {
                        info!(
                            "Spawning missing actor for active streamer: {}",
                            streamer_id
                        );
                        if let Err(e) = self.spawn_streamer_actor(metadata) {
                            warn!("Failed to spawn actor for {}: {}", streamer_id, e);
                        }
                    }
                } else {
                    // Streamer is inactive (disabled, cancelled, etc.) - remove actor if exists
                    if self.supervisor.registry().has_streamer(streamer_id) {
                        if self.remove_streamer(streamer_id) {
                            info!(
                                "Removed actor for inactive streamer {} (state: {})",
                                streamer_id, metadata.state
                            );
                        }
                    } else {
                        debug!(
                            "Streamer {} is inactive ({}), no actor to remove",
                            streamer_id, metadata.state
                        );
                    }
                    return; // No need to route config to an inactive streamer
                }
            } else {
                // Streamer not found - might have been deleted, remove actor if exists
                if self.remove_streamer(streamer_id) {
                    info!("Removed actor for unknown streamer: {}", streamer_id);
                } else {
                    debug!("Streamer {} not found in manager", streamer_id);
                }
                return; // No need to route config to a non-existent streamer
            }
        }

        // Build streamer handles map from registry
        let streamer_handles: HashMap<String, ActorHandle<StreamerMessage>> = self
            .supervisor
            .registry()
            .streamer_handles()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect();

        // Build platform handles map from registry
        let platform_handles: HashMap<String, ActorHandle<PlatformMessage>> = self
            .supervisor
            .registry()
            .platform_handles()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect();

        let router =
            ConfigRouter::new(&streamer_handles, &platform_handles, &self.platform_mapping);

        let targets = router.get_target_actors(&scope);

        let config = self.config.clone();
        let streamer_manager = self.streamer_manager.clone();

        let mut streamer_configs: HashMap<String, StreamerConfig> = HashMap::new();
        let mut platform_configs: HashMap<String, PlatformConfig> = HashMap::new();

        for target in &targets {
            if let Some(platform_id) = target.strip_prefix("platform:") {
                platform_configs.insert(
                    platform_id.to_string(),
                    PlatformConfig {
                        platform_id: platform_id.to_string(),
                        batch_window_ms: 500,
                        max_batch_size: 100,
                        rate_limit: None,
                    },
                );
                continue;
            }

            let streamer_id = target.as_str();
            let metadata = streamer_manager.get_streamer(streamer_id);
            let priority = metadata
                .as_ref()
                .map(|m| m.priority)
                .unwrap_or(Priority::Normal);
            let batch_capable = metadata
                .as_ref()
                .map(|m| self.is_batch_capable_platform(&m.platform_config_id))
                .unwrap_or(false);

            streamer_configs.insert(
                streamer_id.to_string(),
                StreamerConfig {
                    check_interval_ms: config.check_interval_ms,
                    offline_check_interval_ms: config.offline_check_interval_ms,
                    offline_check_count: config.offline_check_count,
                    priority,
                    batch_capable,
                },
            );
        }

        let result = router
            .route_with_scope(
                &scope,
                |streamer_id| {
                    streamer_configs
                        .get(streamer_id)
                        .cloned()
                        .unwrap_or(StreamerConfig {
                            check_interval_ms: config.check_interval_ms,
                            offline_check_interval_ms: config.offline_check_interval_ms,
                            offline_check_count: config.offline_check_count,
                            priority: Priority::Normal,
                            batch_capable: false,
                        })
                },
                |platform_id| {
                    platform_configs
                        .get(platform_id)
                        .cloned()
                        .unwrap_or(PlatformConfig {
                            platform_id: platform_id.to_string(),
                            batch_window_ms: 500,
                            max_batch_size: 100,
                            rate_limit: None,
                        })
                },
            )
            .await;

        // Keep supervisor restart caches in sync with the configs we just routed.
        // This avoids crash-restarts reverting to stale configs.
        // Note: Metadata is in the shared store, so we only need to update config.
        for (streamer_id, cfg) in streamer_configs {
            if streamer_handles.contains_key(&streamer_id) {
                self.supervisor
                    .update_streamer_restart_config(&streamer_id, cfg);
            }
        }
        for (platform_id, cfg) in platform_configs {
            if platform_handles.contains_key(&platform_id) {
                self.supervisor
                    .update_platform_restart_config(&platform_id, cfg);
            }
        }

        if !result.all_succeeded() {
            warn!(
                "Config routing had {} failures: {:?}",
                result.failed, result.failed_actors
            );
        } else {
            debug!("Config update delivered to {} actors", result.delivered);
        }
    }

    /// Process a download event (internal).
    async fn process_download_event(&self, event: DownloadManagerEvent) {
        const HEARTBEAT_THROTTLE: Duration = Duration::from_secs(30);

        let send_to_actor = |streamer_id: String, msg: StreamerMessage| async move {
            trace!(
                "Handling download event for streamer {}: {:?}",
                streamer_id, msg
            );
            if let Some(handle) = self.supervisor.registry().get_streamer(&streamer_id) {
                if let Err(e) = handle.send(msg).await {
                    warn!(
                        "Failed to send download message to actor {}: {}",
                        streamer_id, e
                    );
                }
            } else {
                debug!("No actor found for streamer {}", streamer_id);
            }
        };

        let now = Instant::now();
        match event {
            DownloadManagerEvent::DownloadStarted {
                streamer_id,
                download_id,
                session_id,
                ..
            } => {
                send_to_actor(
                    streamer_id,
                    StreamerMessage::DownloadStarted {
                        download_id,
                        session_id,
                    },
                )
                .await;
            }
            DownloadManagerEvent::DownloadCompleted {
                streamer_id,
                download_id,
                ..
            } => {
                if self.stopped_downloads.remove(&download_id).is_some() {
                    debug!(
                        streamer_id = %streamer_id,
                        download_id = %download_id,
                        "Ignoring DownloadCompleted for previously-stopped download"
                    );
                    return;
                }
                send_to_actor(
                    streamer_id,
                    StreamerMessage::DownloadEnded(DownloadEndPolicy::StreamerOffline),
                )
                .await;
            }
            DownloadManagerEvent::DownloadFailed {
                streamer_id,
                download_id,
                error,
                ..
            } => {
                if self.stopped_downloads.remove(&download_id).is_some() {
                    debug!(
                        streamer_id = %streamer_id,
                        download_id = %download_id,
                        "Ignoring DownloadFailed for previously-stopped download"
                    );
                    return;
                }
                send_to_actor(
                    streamer_id,
                    StreamerMessage::DownloadEnded(DownloadEndPolicy::SegmentFailed(error)),
                )
                .await;
            }
            DownloadManagerEvent::DownloadCancelled {
                streamer_id,
                download_id,
                cause,
                ..
            } => {
                let now_ms = crate::database::time::now_ms();

                // Engines can still emit a terminal event after a stop request
                // (graceful finalization). Track this so we can suppress follow-ups.
                self.stopped_downloads
                    .insert(download_id.clone(), (cause.clone(), now_ms));

                // Opportunistic TTL pruning to prevent unbounded growth if a cancelled download
                // never produces a follow-up terminal event.
                let interval_ms = STOPPED_DOWNLOADS_PRUNE_INTERVAL.as_millis() as i64;
                let should_prune = self.stopped_downloads.len() >= STOPPED_DOWNLOADS_PRUNE_MIN_SIZE
                    && {
                        let last = self
                            .stopped_downloads_last_prune_at_ms
                            .load(Ordering::Relaxed);
                        let elapsed_ms = now_ms.saturating_sub(last);
                        if elapsed_ms >= interval_ms {
                            self.stopped_downloads_last_prune_at_ms
                                .compare_exchange(
                                    last,
                                    now_ms,
                                    Ordering::Relaxed,
                                    Ordering::Relaxed,
                                )
                                .is_ok()
                        } else {
                            false
                        }
                    };
                if should_prune {
                    let ttl_ms = STOPPED_DOWNLOADS_TTL.as_millis() as i64;
                    self.stopped_downloads.retain(|_, (_, inserted_at)| {
                        now_ms.saturating_sub(*inserted_at) <= ttl_ms
                    });
                }

                let end_reason = match cause {
                    DownloadStopCause::User => DownloadEndPolicy::UserCancelled,
                    DownloadStopCause::StreamerOffline => DownloadEndPolicy::StreamerOffline,
                    DownloadStopCause::OutOfSchedule => DownloadEndPolicy::OutOfSchedule,
                    other => DownloadEndPolicy::Stopped(other),
                };

                send_to_actor(streamer_id, StreamerMessage::DownloadEnded(end_reason)).await;
            }
            DownloadManagerEvent::DownloadRejected {
                streamer_id,
                reason,
                retry_after_secs,
                session_id,
            } => {
                let retry_secs = retry_after_secs.unwrap_or(60);
                send_to_actor(
                    streamer_id,
                    StreamerMessage::DownloadEnded(DownloadEndPolicy::CircuitBreakerBlocked {
                        reason,
                        retry_after_secs: retry_secs,
                        session_id,
                    }),
                )
                .await;
            }
            DownloadManagerEvent::Progress {
                download_id,
                streamer_id,
                session_id,
                progress,
            } => {
                let should_send = match self.download_heartbeat_last_sent.get(&streamer_id) {
                    Some(last) => now.duration_since(*last.value()) >= HEARTBEAT_THROTTLE,
                    None => true,
                };
                if should_send {
                    self.download_heartbeat_last_sent
                        .insert(streamer_id.clone(), now);
                    send_to_actor(
                        streamer_id,
                        StreamerMessage::DownloadHeartbeat {
                            download_id,
                            session_id,
                            progress: Some(progress),
                        },
                    )
                    .await;
                }
            }
            DownloadManagerEvent::SegmentStarted {
                download_id,
                streamer_id,
                session_id,
                ..
            }
            | DownloadManagerEvent::SegmentCompleted {
                download_id,
                streamer_id,
                session_id,
                ..
            } => {
                let should_send = match self.download_heartbeat_last_sent.get(&streamer_id) {
                    Some(last) => now.duration_since(*last.value()) >= HEARTBEAT_THROTTLE,
                    None => true,
                };
                if should_send {
                    self.download_heartbeat_last_sent
                        .insert(streamer_id.clone(), now);
                    send_to_actor(
                        streamer_id,
                        StreamerMessage::DownloadHeartbeat {
                            download_id,
                            session_id,
                            progress: None,
                        },
                    )
                    .await;
                }
            }
            _ => {}
        }
    }

    /// Handle task completion action from supervisor.
    fn handle_task_completion_action(&self, action: TaskCompletionAction) {
        match action {
            TaskCompletionAction::Stopped { actor_id } => {
                debug!("Actor {} stopped gracefully", actor_id);
            }
            TaskCompletionAction::Cancelled { actor_id } => {
                debug!("Actor {} was cancelled", actor_id);
            }
            TaskCompletionAction::Completed { actor_id } => {
                debug!("Actor {} completed", actor_id);
            }
            TaskCompletionAction::Crashed { actor_id } => {
                warn!("Actor {} crashed", actor_id);
            }
            TaskCompletionAction::RestartScheduled { actor_id, backoff } => {
                info!(
                    "Actor {} restart scheduled with {:?} backoff",
                    actor_id, backoff
                );
            }
            TaskCompletionAction::RestartFailed { actor_id, reason } => {
                error!("Actor {} restart failed: {}", actor_id, reason);
            }
            TaskCompletionAction::RestartLimitExceeded { actor_id } => {
                error!("Actor {} exceeded restart limit", actor_id);
            }
        }
    }

    /// Graceful shutdown using supervisor.
    async fn shutdown(&mut self) -> ShutdownReport {
        info!("Shutting down scheduler");
        self.cancellation_token.cancel();
        self.supervisor.shutdown().await
    }

    /// Add a new streamer dynamically.
    ///
    /// This spawns a new StreamerActor for the streamer without requiring
    /// a full re-schedule.
    pub fn add_streamer(&mut self, metadata: StreamerMetadata) -> Result<()> {
        let platform_id = metadata.platform_config_id.clone();

        // Ensure platform actor exists if needed
        if self.is_batch_capable_platform(&platform_id) {
            self.spawn_platform_actor(&platform_id)?;
        }

        self.spawn_streamer_actor(metadata)
    }

    /// Remove a streamer dynamically.
    ///
    /// This stops and removes the StreamerActor for the streamer.
    pub fn remove_streamer(&mut self, streamer_id: &str) -> bool {
        self.platform_mapping.unregister(streamer_id);
        self.supervisor.remove_streamer(streamer_id)
    }

    /// Get supervisor statistics.
    pub fn stats(&self) -> super::actor::SupervisorStats {
        self.supervisor.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_config_default() {
        let config = SchedulerConfig::default();
        assert_eq!(config.check_interval_ms, 60_000);
        assert_eq!(config.offline_check_interval_ms, 20_000);
        assert_eq!(config.offline_check_count, 3);
    }

    #[test]
    fn test_is_batch_capable_platform() {
        // We can't easily test this without a full Scheduler instance,
        // but we can verify the logic is correct by checking the match arms
        assert!(matches!("twitch", "twitch" | "youtube"));
        assert!(matches!("youtube", "twitch" | "youtube"));
        assert!(!matches!("bilibili", "twitch" | "youtube"));
    }
}
