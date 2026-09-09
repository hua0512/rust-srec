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
//! - Owned configuration work resolves revisions and retains updates for actor delivery

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use crate::database::repositories::{
    ConfigRepository, FilterRepository, SessionRepository, StreamerRepository,
};
use crate::downloader::{
    DownloadManagerEvent, DownloadProgressEvent, DownloadStopCause, DownloadTerminalEvent,
};
use crate::monitor::StreamMonitor;
use crate::streamer::{StreamerManager, StreamerMetadata};

use super::actor::{
    ActorHandle, ActorRemoval, DownloadEndPolicy, MonitorBatchChecker, MonitorStatusChecker,
    PlatformConfig, PlatformMapping, PlatformMessage, ShutdownReport, StreamerConfig,
    StreamerMessage, Supervisor, SupervisorConfig, TaskCompletionAction,
};

/// Work only the scheduler's event loop can do, requested from another task.
///
/// `Scheduler::run` owns the `Supervisor` by value, so everything that needs
/// `&mut Scheduler` has to happen inside the loop. Each variant is answered with
/// a value the requester can then use on its own task; nothing here may await
/// anything slow, or the loop stops reaping actor tasks and consuming the lossy
/// `download_event_rx` broadcast for that long.
pub(crate) enum SchedulerCommand {
    /// Remove a streamer's actor and hand back the receipt for its task's exit.
    RemoveStreamerAwaitable {
        streamer_id: String,
        reply: tokio::sync::oneshot::Sender<ActorRemoval>,
    },
}

/// Scheduler state and control available to consumers that do not own the
/// `Scheduler` itself - which, once `ServiceContainer::start_scheduler` has moved
/// it into its task, is all of them.
#[derive(Clone)]
pub struct SchedulerHandle {
    stats_rx: watch::Receiver<super::actor::SupervisorStats>,
    command_tx: mpsc::Sender<SchedulerCommand>,
    /// True between `Scheduler::run` taking the command receiver and returning.
    ///
    /// A `Scheduler` that has been built but not started still owns the live
    /// receiver, so a send would be buffered by a loop that will never poll it
    /// and the reply would never arrive. `ServiceContainer` holds exactly such a
    /// value in its `Mutex<Option<Scheduler>>` until `start_scheduler` runs.
    loop_running: Arc<AtomicBool>,
}

impl SchedulerHandle {
    pub fn stats(&self) -> super::actor::SupervisorStats {
        self.stats_rx.borrow().clone()
    }

    /// Ask the scheduler's event loop to remove `streamer_id`'s actor and return
    /// the receipt for its task.
    ///
    /// `None` when the scheduler's event loop is not running - it has not been
    /// started, or it has already returned - which callers must not read as "the
    /// actor stopped": nothing observed it. Only building the receipt happens in
    /// the loop; awaiting it with [`ActorRemoval::wait`] happens on the caller's
    /// task, so a caller that waits minutes does not stall the loop.
    pub async fn remove_streamer_awaitable(&self, streamer_id: &str) -> Option<ActorRemoval> {
        // Racing a `run` that is about to start yields a spurious `None`, which
        // is the same safe answer as any other unobserved removal.
        if !self.loop_running.load(Ordering::Acquire) {
            return None;
        }

        let (reply, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(SchedulerCommand::RemoveStreamerAwaitable {
                streamer_id: streamer_id.to_string(),
                reply,
            })
            .await
            .ok()?;
        rx.await.ok()
    }
}

/// Depth of the `SchedulerCommand` channel. Commands are answered without
/// awaiting anything, so the queue only absorbs bursts while the loop is busy
/// with another branch.
const SCHEDULER_COMMAND_CAPACITY: usize = 64;

/// Default check interval (60 seconds).
const DEFAULT_CHECK_INTERVAL_MS: u64 = 60_000;

/// Default offline check interval (20 seconds).
const DEFAULT_OFFLINE_CHECK_INTERVAL_MS: u64 = 20_000;

/// Default offline check count before switching to offline interval.
const DEFAULT_OFFLINE_CHECK_COUNT: u32 = 3;

#[async_trait::async_trait]
trait ActorConfigResolver: Send + Sync {
    async fn resolve(&self, streamer_id: &str, base: StreamerConfig) -> Result<StreamerConfig>;
}

#[async_trait::async_trait]
impl<C, S> ActorConfigResolver for crate::config::ConfigService<C, S>
where
    C: ConfigRepository + Send + Sync + 'static,
    S: StreamerRepository + Send + Sync + 'static,
{
    async fn resolve(&self, streamer_id: &str, mut base: StreamerConfig) -> Result<StreamerConfig> {
        // Metadata publication and the container's resolver fan-out are separate subscribers.
        // Read current layers directly without using the pre-publication cached context.
        let config = self.get_fresh_config_for_streamer(streamer_id).await?;
        base.offline_check_count = config.offline_check_count;
        base.offline_check_interval_ms = config.offline_check_delay_ms;
        Ok(base)
    }
}

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
/// Actors manage their own scheduling internally.
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
    config_resolver: Option<Arc<dyn ActorConfigResolver>>,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Supervisor for managing actor lifecycle.
    supervisor: Supervisor,
    /// Latest supervisor snapshot for read-only runtime consumers.
    stats_tx: watch::Sender<super::actor::SupervisorStats>,
    /// Sender cloned into every `SchedulerHandle`; kept here so `handle()` can
    /// be called at any point before `run` takes the receiver.
    command_tx: mpsc::Sender<SchedulerCommand>,
    /// Receiver for `SchedulerCommand`, taken by `run`.
    command_rx: Option<mpsc::Receiver<SchedulerCommand>>,
    /// Shared with every `SchedulerHandle`; see `SchedulerHandle::loop_running`.
    loop_running: Arc<AtomicBool>,
    /// Platform mapping for config routing.
    platform_mapping: PlatformMapping,
    /// Platform actor handles for batch coordination.
    platform_handles: HashMap<String, ActorHandle<PlatformMessage>>,
    /// Broadcast receiver for download events (direct subscription).
    download_event_rx: Option<broadcast::Receiver<DownloadManagerEvent>>,
    feedback: Arc<super::feedback::SchedulerFeedback>,
    reliable_feedback: bool,
    configuration: configuration::ConfigurationWork,
    /// Throttle map for forwarding download heartbeats to streamer actors.
    download_heartbeat_last_sent: DashMap<String, Instant>,
}

pub(super) fn download_end_policy_for_stop(cause: DownloadStopCause) -> DownloadEndPolicy {
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
        let (command_tx, command_rx) = mpsc::channel(SCHEDULER_COMMAND_CAPACITY);

        Self {
            streamer_manager,
            event_broadcaster,
            config,
            config_repo: None,
            config_resolver: None,
            cancellation_token,
            supervisor,
            stats_tx,
            command_tx,
            command_rx: Some(command_rx),
            loop_running: Arc::new(AtomicBool::new(false)),
            platform_mapping: PlatformMapping::new(),
            platform_handles: HashMap::new(),
            download_event_rx: None,
            feedback: super::feedback::SchedulerFeedback::new(),
            reliable_feedback: false,
            configuration: configuration::ConfigurationWork::default(),
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
        let config_resolver = monitor.config_service();
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
        let (command_tx, command_rx) = mpsc::channel(SCHEDULER_COMMAND_CAPACITY);

        Self {
            streamer_manager,
            event_broadcaster,
            config,
            config_repo: None,
            config_resolver: Some(config_resolver),
            cancellation_token,
            supervisor,
            stats_tx,
            command_tx,
            command_rx: Some(command_rx),
            loop_running: Arc::new(AtomicBool::new(false)),
            platform_mapping: PlatformMapping::new(),
            platform_handles: HashMap::new(),
            download_event_rx: None,
            feedback: super::feedback::SchedulerFeedback::new(),
            reliable_feedback: false,
            configuration: configuration::ConfigurationWork::default(),
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
    pub fn handle(&self) -> SchedulerHandle {
        SchedulerHandle {
            stats_rx: self.stats_tx.subscribe(),
            command_tx: self.command_tx.clone(),
            loop_running: self.loop_running.clone(),
        }
    }

    fn publish_stats(&self) {
        self.stats_tx.send_replace(self.supervisor.stats());
    }

    /// Set the download event receiver.
    ///
    /// This should be called before `run()` to enable download event handling.
    /// Connect recording lifecycle ownership and the lossy progress observer.
    /// Wire once before starting this scheduler or admitting recordings.
    pub fn connect_download_manager(
        &mut self,
        manager: &crate::downloader::DownloadManager,
    ) -> Result<()> {
        manager.set_scheduler_feedback(self.reliable_feedback())?;
        self.set_download_receiver(manager.subscribe());
        Ok(())
    }

    pub(crate) fn reliable_feedback(&mut self) -> Arc<super::feedback::SchedulerFeedback> {
        self.reliable_feedback = true;
        self.supervisor.set_feedback(self.feedback.clone());
        self.feedback.clone()
    }

    /// Attach a compatibility observer; use `connect_download_manager` for reliable lifecycle delivery.
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

    /// Resolve effective timing before spawning or replacing the actor's restart config.
    async fn create_streamer_config(&self, metadata: &StreamerMetadata) -> Result<StreamerConfig> {
        let base = StreamerConfig {
            check_interval_ms: self.config.check_interval_ms,
            offline_check_interval_ms: self.config.offline_check_interval_ms,
            offline_check_count: self.config.offline_check_count,
            priority: metadata.priority,
            batch_capable: self.is_batch_capable_platform(&metadata.platform_config_id),
        };
        match &self.config_resolver {
            Some(resolver) => tokio::select! {
                biased;
                _ = self.cancellation_token.cancelled() => Err(crate::Error::Other("Scheduler configuration resolution cancelled".to_owned())),
                result = resolver.resolve(&metadata.id, base) => result,
            },
            // No-monitor constructors are also used by callers that supply explicit timing.
            None => Ok(base),
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

        // Taken, not borrowed, so the select arm below does not hold a borrow of
        // `self` that its handler needs mutably.
        let mut command_rx = self.command_rx.take();
        // Set before the first await so no command can be buffered for a loop
        // that is not yet polling; cleared below once it stops.
        self.loop_running.store(true, Ordering::Release);

        // Initial actor spawning for all active streamers
        self.queue_reconciliation();
        self.pump_configuration();
        self.publish_stats();

        info!(
            "Scheduler started with {} streamer actors and {} platform actors",
            self.supervisor.registry().streamer_count(),
            self.supervisor.registry().platform_count()
        );

        let mut feedback_failure = None;
        loop {
            // Calculate next restart time for pending restarts
            let next_restart = self.supervisor.next_restart_time();
            let next_configuration = self.next_configuration_time();
            let feedback = self.feedback.clone();

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
                            self.queue_configuration(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Scheduler lagged {} config events; reconciling current state", n);
                            self.queue_configuration(ConfigUpdateEvent::GlobalUpdated);
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
                                let streamer = task_result.actor_type == "streamer";
                                let action = self.supervisor.handle_task_completion(task_result);
                                self.handle_task_completion_action(action, streamer);
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

                // Handle commands from `SchedulerHandle`
                command = async {
                    match &mut command_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match command {
                        Some(command) => self.handle_command(command),
                        None => {
                            // Every `SchedulerHandle` was dropped. `self.command_tx`
                            // keeps the channel open, so this is unreachable while
                            // the scheduler exists; stop polling either way.
                            command_rx = None;
                        }
                    }
                }

                result = self.configuration.jobs.join_next(), if !self.configuration.jobs.is_empty() => {
                    if let Some(result) = result { self.finish_configuration(result); }
                }
                _ = Self::wait_for_restart(next_configuration) => self.pump_configuration(),
                failure = feedback.failure() => {
                    self.cancellation_token.cancel();
                    error!(%failure, "Reliable scheduler feedback failed");
                    feedback_failure = Some(failure);
                    break;
                }

                // Process pending restarts
                _ = Self::wait_for_restart(next_restart) => {
                    self.queue_due_restarts();
                }
            }
            self.feedback
                .update_targets(self.supervisor.registry().streamer_handles_map());
            self.pump_configuration();
            self.publish_stats();
        }

        self.loop_running.store(false, Ordering::Release);
        self.configuration.jobs.abort_all();
        while self.configuration.jobs.join_next().await.is_some() {}
        self.feedback.shutdown().await;

        // Graceful shutdown
        let report = self.shutdown().await;
        self.publish_stats();
        info!(
            "Scheduler stopped: {} graceful, {} forced",
            report.graceful_stops, report.forced_terminations
        );

        match feedback_failure.or_else(|| self.configuration.failure.take()) {
            Some(error) => Err(crate::Error::Other(error)),
            None => Ok(()),
        }
    }

    /// Answer a `SchedulerCommand`. Must not await: the loop is not reaping
    /// actor tasks or draining `download_event_rx` while this runs.
    fn handle_command(&mut self, command: SchedulerCommand) {
        match command {
            SchedulerCommand::RemoveStreamerAwaitable { streamer_id, reply } => {
                let removal = self.remove_streamer_awaitable(&streamer_id);
                // The requester gave up before the loop got here; the actor is
                // removed and cancelled regardless.
                let _ = reply.send(removal);
            }
        }
    }

    /// Wait for the next restart time, or forever if none pending.
    async fn wait_for_restart(next_restart: Option<tokio::time::Instant>) {
        match next_restart {
            Some(instant) => tokio::time::sleep_until(instant).await,
            None => std::future::pending().await,
        }
    }

    #[cfg(test)]
    async fn process_pending_restarts(&mut self) -> usize {
        let before = self.supervisor.registry().streamer_count();
        self.queue_due_restarts();
        self.drain_configuration().await;
        self.supervisor
            .registry()
            .streamer_count()
            .saturating_sub(before)
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
    async fn spawn_streamer_actor(&mut self, metadata: StreamerMetadata) -> Result<()> {
        let streamer_id = metadata.id.clone();

        if self.supervisor.registry().has_streamer(&streamer_id) {
            debug!("Streamer actor {} already exists", streamer_id);
            return Ok(());
        }

        let config = self.create_streamer_config(&metadata).await?;
        self.spawn_streamer_resolved(metadata, config)
    }

    fn spawn_streamer_resolved(
        &mut self,
        metadata: StreamerMetadata,
        config: StreamerConfig,
    ) -> Result<()> {
        let streamer_id = metadata.id.clone();
        let platform_id = metadata.platform_config_id.clone();

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
                self.feedback.activate(&streamer_id);
                self.feedback
                    .update_targets(self.supervisor.registry().streamer_handles_map());
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

    #[cfg(test)]
    async fn handle_config_event(&mut self, event: ConfigUpdateEvent) {
        self.queue_configuration(event);
        self.drain_configuration().await;
    }

    /// Process a download event (internal).
    async fn process_download_event(&self, event: DownloadManagerEvent) {
        if self.reliable_feedback
            && !matches!(
                event,
                DownloadManagerEvent::Progress(
                    DownloadProgressEvent::Progress { .. }
                        | DownloadProgressEvent::SegmentStarted { .. }
                        | DownloadProgressEvent::SegmentCompleted { .. }
                )
            )
        {
            return;
        }
        const HEARTBEAT_THROTTLE: Duration = Duration::from_secs(30);

        let send_to_actor = |streamer_id: String, msg: StreamerMessage| async move {
            trace!(
                "Handling download event for streamer {}: {:?}",
                streamer_id, msg
            );
            if let Some(handle) = self.supervisor.registry().get_streamer(&streamer_id) {
                if let Err(e) = handle.try_send(msg) {
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
                if should_send
                    && (!self.reliable_feedback
                        || self.feedback.is_current_download(
                            &streamer_id,
                            &download_id,
                            &session_id,
                        ))
                {
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
                if should_send
                    && (!self.reliable_feedback
                        || self.feedback.is_current_download(
                            &streamer_id,
                            &download_id,
                            &session_id,
                        ))
                {
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
    fn handle_task_completion_action(&self, action: TaskCompletionAction, streamer: bool) {
        match action {
            TaskCompletionAction::Stopped { actor_id } => {
                if streamer {
                    self.feedback.retire(&actor_id);
                }
                debug!("Actor {} stopped gracefully", actor_id);
            }
            TaskCompletionAction::Cancelled { actor_id } => {
                if streamer {
                    self.feedback.retire(&actor_id);
                }
                debug!("Actor {} was cancelled", actor_id);
            }
            TaskCompletionAction::Completed { actor_id } => {
                if streamer {
                    self.feedback.retire(&actor_id);
                }
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
                if streamer {
                    self.feedback.unavailable(&actor_id);
                }
                error!("Actor {} restart failed: {}", actor_id, reason);
            }
            TaskCompletionAction::RestartLimitExceeded { actor_id } => {
                if streamer {
                    self.feedback.unavailable(&actor_id);
                }
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
    pub async fn add_streamer(&mut self, metadata: StreamerMetadata) -> Result<()> {
        let platform_id = metadata.platform_config_id.clone();

        // Ensure platform actor exists if needed
        if self.is_batch_capable_platform(&platform_id) {
            self.spawn_platform_actor(&platform_id)?;
        }

        self.spawn_streamer_actor(metadata).await
    }

    /// Remove a streamer dynamically.
    ///
    /// This stops and removes the StreamerActor for the streamer.
    pub fn remove_streamer(&mut self, streamer_id: &str) -> bool {
        self.invalidate_configuration(streamer_id);
        self.feedback.retire(streamer_id);
        self.platform_mapping.unregister(streamer_id);
        self.supervisor.remove_streamer(streamer_id)
    }

    /// Remove a streamer dynamically and return a receipt for its actor task.
    ///
    /// `remove_streamer` returns while the actor may still be inside a
    /// `check_status` call that writes streamer state. The receipt lets a caller
    /// that is retiring the streamer wait for that call to finish before it
    /// touches the same rows.
    ///
    /// Building the receipt needs `&mut Scheduler`, which after
    /// `ServiceContainer::start_scheduler` exists only inside `run`'s own
    /// handlers - so awaiting it here would stall `join_next`, config events and
    /// the lossy `download_event_rx` broadcast for the whole wait. Off-loop
    /// callers go through `SchedulerHandle::remove_streamer_awaitable`, which has
    /// the loop build the receipt and awaits it on the caller's task.
    pub fn remove_streamer_awaitable(&mut self, streamer_id: &str) -> ActorRemoval {
        self.invalidate_configuration(streamer_id);
        self.feedback.retire(streamer_id);
        self.platform_mapping.unregister(streamer_id);
        self.supervisor.remove_streamer_awaitable(streamer_id)
    }

    /// Get supervisor statistics.
    pub fn stats(&self) -> super::actor::SupervisorStats {
        self.supervisor.stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::StreamerState;
    use crate::scheduler::actor::ActorRemovalOutcome;
    use chrono::Utc;

    /// A scheduler whose actors use `NoOpStatusChecker`, with `streamer_ids`
    /// already in the shared metadata store so initial reconciliation gives each
    /// of them an actor.
    ///
    /// The repository is real but unused by this path: nothing here applies a
    /// config event, and `StreamerActor` persists through its `StatusChecker`,
    /// not through `StreamerManager`.
    async fn scheduler_with_streamers(
        streamer_ids: &[&str],
    ) -> Scheduler<crate::database::repositories::SqlxStreamerRepository> {
        let dir = tempfile::TempDir::new().unwrap();
        let db_url = format!(
            "sqlite:{}?mode=rwc",
            dir.path().join("scheduler_test.db").to_string_lossy()
        );
        let pool = crate::database::init_pool(&db_url).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        // The database outlives the guard on purpose; `keep` expresses that
        // without leaking the guard itself.
        let _ = dir.keep();

        let repo = Arc::new(crate::database::repositories::SqlxStreamerRepository::new(
            pool.clone(),
            pool,
        ));
        let broadcaster = ConfigEventBroadcaster::new();
        let streamer_manager = Arc::new(StreamerManager::new(repo, broadcaster.clone()));

        let store = streamer_manager.metadata_store();
        for id in streamer_ids {
            store.insert(
                (*id).to_string(),
                StreamerMetadata {
                    id: (*id).to_string(),
                    name: format!("Test {id}"),
                    url: format!("https://twitch.tv/{id}"),
                    platform_config_id: "twitch".to_string(),
                    template_config_id: None,
                    state: StreamerState::NotLive,
                    priority: crate::domain::Priority::Normal,
                    avatar_url: None,
                    consecutive_error_count: 0,
                    disabled_until: None,
                    last_live_time: None,
                    last_error: None,
                    streamer_specific_config: None,
                    offline_check_count: 3,
                    offline_check_delay_ms: 20_000,
                    created_at: Utc::now(),
                    deleted_at: None,
                    updated_at: Utc::now(),
                },
            );
        }

        Scheduler::with_full_config(
            streamer_manager,
            broadcaster,
            SchedulerConfig::default(),
            CancellationToken::new(),
        )
    }

    /// Retry until the spawned `Scheduler::run` has entered its loop.
    ///
    /// `SchedulerHandle::remove_streamer_awaitable` answers `None` until then,
    /// and a `None` removes nothing, so retrying is free of side effects.
    async fn removal_once_running(handle: &SchedulerHandle, streamer_id: &str) -> ActorRemoval {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if handle.stats().streamer_count > 0
                    && let Some(removal) = handle.remove_streamer_awaitable(streamer_id).await
                {
                    return removal;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the scheduler loop should start and answer the command")
    }

    /// `Scheduler::run` owns the supervisor, so `SchedulerHandle` is the only way
    /// an off-loop caller can get a removal receipt at all. The receipt has to
    /// come back answered, and awaiting it must not be what the loop is doing.
    #[tokio::test]
    async fn scheduler_handle_gets_a_removal_receipt_from_the_event_loop() {
        let mut scheduler = scheduler_with_streamers(&["test-1"]).await;
        let handle = scheduler.handle();
        let token = scheduler.cancellation_token.clone();
        let runner = tokio::spawn(async move { scheduler.run().await });

        let removal = removal_once_running(&handle, "test-1").await;
        assert_eq!(removal.actor_id(), "test-1");
        assert!(removal.generation().is_some());
        assert_eq!(
            removal.wait(Duration::from_secs(5)).await,
            ActorRemovalOutcome::Stopped
        );

        // The loop is still serving commands after the removal, which it could
        // not be if answering one meant awaiting the actor's exit.
        let again = handle
            .remove_streamer_awaitable("test-1")
            .await
            .expect("the scheduler loop should still be running");
        assert_eq!(
            again.wait(Duration::ZERO).await,
            ActorRemovalOutcome::NotRegistered
        );

        token.cancel();
        runner.await.unwrap().unwrap();
    }

    /// A handle reports `None` rather than an outcome whenever no event loop
    /// looked at the request, so a caller cannot mistake "nobody looked" for
    /// "the actor stopped". A scheduler that was built but never started owns a
    /// live command receiver nothing polls, so it has to answer `None` too
    /// rather than leaving the caller waiting for a reply that cannot come.
    #[tokio::test]
    async fn scheduler_handle_reports_no_answer_without_a_running_loop() {
        let scheduler = scheduler_with_streamers(&[]).await;
        let handle = scheduler.handle();

        let never_started = tokio::time::timeout(
            Duration::from_secs(5),
            handle.remove_streamer_awaitable("test-1"),
        )
        .await
        .expect("a scheduler that was never started must not hang the caller");
        assert!(never_started.is_none());

        drop(scheduler);
        assert!(handle.remove_streamer_awaitable("test-1").await.is_none());
    }

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
        let (command_tx, _command_rx) = mpsc::channel(SCHEDULER_COMMAND_CAPACITY);
        let handle = SchedulerHandle {
            stats_rx,
            command_tx,
            loop_running: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(handle.stats().streamer_count, 1);

        stats_tx.send_replace(stats(2));
        assert_eq!(handle.stats().streamer_count, 2);
    }
}

#[cfg(test)]
mod config_tests;

mod configuration;

impl<R: StreamerRepository + Send + Sync + 'static> Drop for Scheduler<R> {
    fn drop(&mut self) {
        self.feedback.stop_monitoring();
    }
}

#[cfg(test)]
mod reliability_tests;
