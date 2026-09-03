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
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use crate::database::repositories::{
    ConfigRepository, FilterRepository, SessionRepository, StreamerRepository,
};
use crate::domain::Priority;
use crate::downloader::{
    DownloadManagerEvent, DownloadProgressEvent, DownloadStopCause, DownloadTerminalEvent,
};
use crate::monitor::StreamMonitor;
use crate::streamer::{StreamerManager, StreamerMetadata};

use super::actor::{
    ActorHandle, ConfigRouter, ConfigScope, DownloadEndPolicy, MonitorBatchChecker,
    MonitorStatusChecker, PlatformConfig, PlatformMapping, PlatformMessage, RoutingPlan,
    ShutdownReport, StreamerConfig, StreamerMessage, Supervisor, SupervisorConfig,
    TaskCompletionAction,
};

/// Read-only scheduler state for health and diagnostics consumers.
#[derive(Clone)]
pub(crate) struct SchedulerHandle {
    stats_rx: watch::Receiver<super::actor::SupervisorStats>,
}

impl SchedulerHandle {
    pub(crate) fn stats(&self) -> super::actor::SupervisorStats {
        self.stats_rx.borrow().clone()
    }
}

/// Default check interval (60 seconds).
const DEFAULT_CHECK_INTERVAL_MS: u64 = 60_000;

/// Default offline check interval (20 seconds).
const DEFAULT_OFFLINE_CHECK_INTERVAL_MS: u64 = 20_000;

/// Default offline check count before switching to offline interval.
const DEFAULT_OFFLINE_CHECK_COUNT: u32 = 3;

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
    /// Latest supervisor snapshot for read-only runtime consumers.
    stats_tx: watch::Sender<super::actor::SupervisorStats>,
    /// Platform mapping for config routing.
    platform_mapping: PlatformMapping,
    /// Platform actor handles for batch coordination.
    platform_handles: HashMap<String, ActorHandle<PlatformMessage>>,
    /// Broadcast receiver for download events (direct subscription).
    download_event_rx: Option<broadcast::Receiver<DownloadManagerEvent>>,
    /// Throttle map for forwarding download heartbeats to streamer actors.
    download_heartbeat_last_sent: DashMap<String, Instant>,
}

fn download_end_policy_for_stop(cause: DownloadStopCause) -> DownloadEndPolicy {
    match cause {
        DownloadStopCause::User => DownloadEndPolicy::UserCancelled,
        DownloadStopCause::StreamerOffline => DownloadEndPolicy::StreamerOffline,
        DownloadStopCause::OutOfSchedule => DownloadEndPolicy::OutOfSchedule,
        other => DownloadEndPolicy::Stopped(other),
    }
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
        let (stats_tx, _) = watch::channel(supervisor.stats());

        Self {
            streamer_manager,
            event_broadcaster,
            config,
            config_repo: None,
            cancellation_token,
            supervisor,
            stats_tx,
            platform_mapping: PlatformMapping::new(),
            platform_handles: HashMap::new(),
            download_event_rx: None,
            download_heartbeat_last_sent: DashMap::new(),
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
        Self::with_monitor_history_and_config(
            streamer_manager,
            event_broadcaster,
            monitor,
            None,
            config,
            cancellation_token,
        )
    }

    /// Like [`Self::with_monitor_and_config`] but additionally wires a
    /// best-effort check-history writer into the per-streamer status checker
    /// so each poll outcome lands in the `streamer_check_history` ring
    /// buffer that powers the streamer details page's check-history strip.
    ///
    /// Pass `history_writer = None` to disable persistence (the polling path
    /// is unaffected either way; the writer is purely diagnostic).
    pub(crate) fn with_monitor_history_and_config<SR, FR, SSR, CR>(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        monitor: Arc<StreamMonitor<SR, FR, SSR, CR>>,
        history_writer: Option<crate::monitor::CheckHistoryWriter>,
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
        let status_checker = Arc::new(match history_writer {
            Some(writer) => MonitorStatusChecker::with_history_writer(monitor.clone(), writer),
            None => MonitorStatusChecker::new(monitor.clone()),
        });
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
        let (stats_tx, _) = watch::channel(supervisor.stats());

        Self {
            streamer_manager,
            event_broadcaster,
            config,
            config_repo: None,
            cancellation_token,
            supervisor,
            stats_tx,
            platform_mapping: PlatformMapping::new(),
            platform_handles: HashMap::new(),
            download_event_rx: None,
            download_heartbeat_last_sent: DashMap::new(),
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

    /// Create a cheap read-only handle for scheduler diagnostics.
    pub(crate) fn handle(&self) -> SchedulerHandle {
        SchedulerHandle {
            stats_rx: self.stats_tx.subscribe(),
        }
    }

    fn publish_stats(&self) {
        self.stats_tx.send_replace(self.supervisor.stats());
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
    ///
    /// `offline_check_*` come from the metadata's resolved per-streamer
    /// values (populated by the resolver at registration / config-update
    /// time). Falls back to the global scheduler config when the metadata
    /// hasn't been resolved yet (e.g. defensive default during early boot).
    fn create_streamer_config(&self, metadata: &StreamerMetadata) -> StreamerConfig {
        StreamerConfig {
            check_interval_ms: self.config.check_interval_ms,
            offline_check_interval_ms: metadata.offline_check_delay_ms,
            offline_check_count: metadata.offline_check_count,
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
    ///
    /// No platform has a batch API implementation
    /// (`BatchDetector::check_batch_internal` errors unconditionally), so no
    /// streamer routes checks through `PlatformMessage::RequestCheck`
    /// delegation; every check runs individually via `perform_check`.
    fn is_batch_capable_platform(&self, _platform_id: &str) -> bool {
        false
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
        self.publish_stats();

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
                                // `ActorRegistry::spawn_streamer` runs the actor
                                // inside `catch_unwind`, so this is never a panic
                                // from `run`: either the task was cancelled with
                                // the runtime, or the `ActorTaskResult`
                                // construction outside the guard unwound.
                                if e.is_cancelled() {
                                    debug!("Actor task cancelled before reporting: {}", e);
                                } else {
                                    error!("Actor task failed to join: {}", e);
                                }
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
            self.publish_stats();
        }

        // Graceful shutdown
        let report = self.shutdown().await;
        self.publish_stats();
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
    ///
    /// Uses `get_all_active` so streamers still inside their `disabled_until`
    /// error backoff also get an actor; the actor's `initiate_check` guard
    /// defers the first real check until the backoff expires, which keeps
    /// them monitored once it does.
    async fn spawn_initial_actors(&mut self) -> Result<()> {
        let streamers = self.streamer_manager.get_all_active();
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

        // Handle event-specific side effects and early returns first.
        match &event {
            ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                // Filters can affect OutOfSchedule smart-wake hints. Force a fresh check soon so
                // the StreamerActor picks up the new filter behavior immediately.
                if let Some(handle) = self.supervisor.registry().get_streamer(streamer_id)
                    && let Err(error) = handle.send(StreamerMessage::CheckStatus).await
                {
                    debug!(streamer_id, %error, "Streamer actor stopped before forced check");
                }
            }
            ConfigUpdateEvent::GlobalUpdated => match self.refresh_timing_config_from_db().await {
                Ok(true) => {}
                Ok(false) => {
                    // Avoid broadcasting config updates to every actor if global changes don't
                    // affect scheduler timing (e.g., log filter changes).
                    return;
                }
                Err(error) => {
                    warn!("Failed to refresh scheduler timing config: {}", error);
                }
            },
            ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                if self.remove_streamer(streamer_id) {
                    info!("Removed actor for deleted streamer: {}", streamer_id);
                }
                return;
            }
            ConfigUpdateEvent::StreamerStateSyncedFromDb {
                streamer_id,
                is_active,
            } => {
                self.handle_state_sync(streamer_id, *is_active).await;
                return;
            }
            _ => {}
        }

        let scope = ConfigScope::from_event(&event);

        // Ensure actor state/platform mapping for streamer-scoped updates.
        if let ConfigScope::Streamer(streamer_id) = &scope
            && !self.ensure_streamer_actor_state(streamer_id)
        {
            return;
        }

        // Snapshot semantics: compute once, update restart caches, then deliver.
        //
        // Note: we intentionally keep the router borrows scoped to avoid holding an immutable
        // borrow of self.supervisor across the restart-cache mutation.
        let plan = {
            let registry = self.supervisor.registry();
            let router = ConfigRouter::new(
                registry.streamer_handles_map(),
                registry.platform_handles_map(),
                &self.platform_mapping,
            );
            router.plan_with_scope(
                &scope,
                |id| self.build_streamer_config(id),
                |id| self.create_platform_config(id),
            )
        };

        self.update_restart_cache_from_plan(&plan);

        let result = {
            let registry = self.supervisor.registry();
            let router = ConfigRouter::new(
                registry.streamer_handles_map(),
                registry.platform_handles_map(),
                &self.platform_mapping,
            );
            router.deliver_plan(plan).await
        };

        if !result.all_succeeded() {
            warn!(
                "Config routing had {} failures: {:?}",
                result.failed, result.failed_actors
            );
        } else {
            debug!("Config update delivered to {} actors", result.delivered);
        }
    }

    fn build_streamer_config(&self, streamer_id: &str) -> StreamerConfig {
        let metadata = self.streamer_manager.get_streamer(streamer_id);
        let priority = metadata
            .as_ref()
            .map(|m| m.priority)
            .unwrap_or(Priority::Normal);
        let batch_capable = metadata
            .as_ref()
            .map(|m| self.is_batch_capable_platform(&m.platform_config_id))
            .unwrap_or(false);
        // Per-streamer offline-check cadence is cached on metadata (set by
        // the resolver fan-out). Fall back to the scheduler-wide global
        // when no metadata is registered yet.
        let offline_check_interval_ms = metadata
            .as_ref()
            .map(|m| m.offline_check_delay_ms)
            .unwrap_or(self.config.offline_check_interval_ms);
        let offline_check_count = metadata
            .as_ref()
            .map(|m| m.offline_check_count)
            .unwrap_or(self.config.offline_check_count);

        StreamerConfig {
            check_interval_ms: self.config.check_interval_ms,
            offline_check_interval_ms,
            offline_check_count,
            priority,
            batch_capable,
        }
    }

    fn ensure_streamer_actor_state(&mut self, streamer_id: &str) -> bool {
        let Some(metadata) = self.streamer_manager.get_streamer(streamer_id) else {
            // Streamer not found - might have been deleted, remove actor if exists
            if self.remove_streamer(streamer_id) {
                info!("Removed actor for unknown streamer: {}", streamer_id);
            } else {
                debug!("Streamer {} not found in manager", streamer_id);
            }
            return false;
        };

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
            if !self.supervisor.registry().has_streamer(streamer_id) {
                info!(
                    "Spawning missing actor for active streamer: {}",
                    streamer_id
                );
                if let Err(e) = self.spawn_streamer_actor(metadata) {
                    warn!("Failed to spawn actor for {}: {}", streamer_id, e);
                }
            }
            true
        } else {
            // Streamer is inactive (disabled, cancelled, etc.) - remove actor if exists.
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
            false
        }
    }

    // NOTE: restart cache update now uses a Router-generated RoutingPlan to preserve
    // snapshot semantics and avoid recomputing configs after any awaited sends.

    fn update_restart_cache_from_plan(&mut self, plan: &RoutingPlan) {
        for (streamer_id, cfg) in &plan.streamers {
            self.supervisor
                .update_streamer_restart_config(streamer_id, cfg.clone());
        }
        for (platform_id, cfg) in &plan.platforms {
            self.supervisor
                .update_platform_restart_config(platform_id, cfg.clone());
        }
    }

    async fn handle_state_sync(&mut self, streamer_id: &str, is_active: bool) {
        if is_active {
            // Streamer became active - spawn actor if missing.
            if let Some(metadata) = self.streamer_manager.get_streamer(streamer_id)
                && !self.supervisor.registry().has_streamer(streamer_id)
            {
                // Ensure platform actor exists for batch-capable platforms.
                if self.is_batch_capable_platform(&metadata.platform_config_id)
                    && let Err(e) = self.spawn_platform_actor(&metadata.platform_config_id)
                {
                    warn!(
                        platform_id = %metadata.platform_config_id,
                        error = %e,
                        "Failed to spawn platform actor during state sync"
                    );
                }

                info!(
                    "Spawning actor for newly active streamer: {} (state: {})",
                    streamer_id, metadata.state
                );
                if let Err(e) = self.spawn_streamer_actor(metadata) {
                    warn!("Failed to spawn actor for {}: {}", streamer_id, e);
                }
            }
        } else {
            // Streamer became inactive - remove actor if exists.
            if self.remove_streamer(streamer_id) {
                info!("Removed actor for inactive streamer: {}", streamer_id);
            }
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
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadStarted {
                streamer_id,
                download_id,
                session_id,
                ..
            }) => {
                send_to_actor(
                    streamer_id,
                    StreamerMessage::DownloadStarted {
                        download_id,
                        session_id,
                    },
                )
                .await;
            }
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Completed {
                streamer_id,
                stop_cause,
                ..
            }) => {
                let policy = stop_cause
                    .map(download_end_policy_for_stop)
                    .unwrap_or(DownloadEndPolicy::Completed);
                send_to_actor(streamer_id, StreamerMessage::DownloadEnded(policy)).await;
            }
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Failed {
                streamer_id,
                error,
                ..
            }) => {
                send_to_actor(
                    streamer_id,
                    StreamerMessage::DownloadEnded(DownloadEndPolicy::SegmentFailed(error)),
                )
                .await;
            }
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
                streamer_id,
                cause,
                ..
            }) => {
                let policy = download_end_policy_for_stop(cause);
                send_to_actor(streamer_id, StreamerMessage::DownloadEnded(policy)).await;
            }
            DownloadManagerEvent::Terminal(DownloadTerminalEvent::Rejected {
                streamer_id,
                reason,
                retry_after_secs,
                session_id,
                kind,
                ..
            }) => {
                let retry_secs = retry_after_secs.unwrap_or(60);
                let policy = match kind {
                    crate::downloader::DownloadRejectedKind::CircuitBreaker => {
                        DownloadEndPolicy::CircuitBreakerBlocked {
                            reason,
                            retry_after_secs: retry_secs,
                            session_id,
                        }
                    }
                    crate::downloader::DownloadRejectedKind::OutputRootUnavailable {
                        path,
                        io_kind,
                    } => DownloadEndPolicy::OutputRootBlocked {
                        path,
                        io_kind,
                        retry_after_secs: retry_secs,
                        session_id,
                    },
                    crate::downloader::DownloadRejectedKind::StreamerBackoff => {
                        DownloadEndPolicy::StreamerBackoffBlocked {
                            reason,
                            retry_after_secs: retry_secs,
                            session_id,
                        }
                    }
                };
                send_to_actor(streamer_id, StreamerMessage::DownloadEnded(policy)).await;
            }
            DownloadManagerEvent::Progress(DownloadProgressEvent::Progress {
                download_id,
                streamer_id,
                session_id,
                progress,
                ..
            }) => {
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
            DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
                download_id,
                streamer_id,
                session_id,
                ..
            })
            | DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                download_id,
                streamer_id,
                session_id,
                ..
            }) => {
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
            TaskCompletionAction::Superseded { actor_id } => {
                debug!("Actor {} was replaced before its task finished", actor_id);
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
    fn clean_completion_preserves_the_requested_stop_policy() {
        assert!(matches!(
            download_end_policy_for_stop(DownloadStopCause::User),
            DownloadEndPolicy::UserCancelled
        ));
        assert!(matches!(
            download_end_policy_for_stop(DownloadStopCause::StreamerOffline),
            DownloadEndPolicy::StreamerOffline
        ));
        assert!(matches!(
            download_end_policy_for_stop(DownloadStopCause::OutOfSchedule),
            DownloadEndPolicy::OutOfSchedule
        ));
        assert!(matches!(
            download_end_policy_for_stop(DownloadStopCause::DanmuStreamClosed),
            DownloadEndPolicy::Stopped(DownloadStopCause::DanmuStreamClosed)
        ));
    }

    #[test]
    fn scheduler_handle_reads_latest_stats_snapshot() {
        fn stats(streamer_count: usize) -> crate::scheduler::actor::SupervisorStats {
            crate::scheduler::actor::SupervisorStats {
                streamer_count,
                platform_count: 0,
                pending_restarts: 0,
                restart_stats: crate::scheduler::actor::RestartTrackerStats {
                    total_actors: streamer_count,
                    actors_with_failures: 0,
                    total_restarts: 0,
                },
            }
        }

        let (stats_tx, stats_rx) = watch::channel(stats(1));
        let handle = SchedulerHandle { stats_rx };
        assert_eq!(handle.stats().streamer_count, 1);

        stats_tx.send_replace(stats(2));
        assert_eq!(handle.stats().streamer_count, 2);
    }
}
