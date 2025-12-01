//! Scheduler service implementation.
//!
//! The Scheduler orchestrates monitoring tasks for all active streamers,
//! reacting to configuration changes and managing task lifecycle.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigUpdateEvent};
use crate::streamer::{StreamerManager, StreamerMetadata};

use super::batch::{BatchGroup, group_by_platform};
use super::task::{MonitoringTask, TaskHandle, TaskStatus};

/// Default check interval (60 seconds).
const DEFAULT_CHECK_INTERVAL_MS: u64 = 60_000;

/// Default offline check interval (20 seconds).
const DEFAULT_OFFLINE_CHECK_INTERVAL_MS: u64 = 20_000;

/// Default offline check count before switching to offline interval.
const DEFAULT_OFFLINE_CHECK_COUNT: u32 = 3;

/// Re-scheduling interval (5 minutes).
const RESCHEDULE_INTERVAL: Duration = Duration::from_secs(300);

/// Scheduler configuration.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Default check interval in milliseconds.
    pub check_interval_ms: u64,
    /// Offline check interval in milliseconds.
    pub offline_check_interval_ms: u64,
    /// Number of offline checks before using offline interval.
    pub offline_check_count: u32,
    /// Re-scheduling interval.
    pub reschedule_interval: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: DEFAULT_CHECK_INTERVAL_MS,
            offline_check_interval_ms: DEFAULT_OFFLINE_CHECK_INTERVAL_MS,
            offline_check_count: DEFAULT_OFFLINE_CHECK_COUNT,
            reschedule_interval: RESCHEDULE_INTERVAL,
        }
    }
}

/// The Scheduler orchestrates monitoring tasks for all active streamers.
pub struct Scheduler<R: crate::database::repositories::StreamerRepository + Send + Sync + 'static> {
    /// Streamer manager for accessing streamer state.
    streamer_manager: Arc<StreamerManager<R>>,
    /// Event broadcaster for config updates.
    event_broadcaster: ConfigEventBroadcaster,
    /// Scheduler configuration.
    config: SchedulerConfig,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Active task handles by streamer ID.
    active_tasks: HashMap<String, TaskHandle>,
    /// JoinSet for managing spawned tasks.
    task_set: JoinSet<TaskResult>,
}

/// Result of a monitoring task.
#[derive(Debug)]
pub struct TaskResult {
    /// Streamer ID.
    pub streamer_id: String,
    /// Task status.
    pub status: TaskStatus,
    /// Error message if failed.
    pub error: Option<String>,
}

impl<R: crate::database::repositories::StreamerRepository + Send + Sync + 'static> Scheduler<R> {
    /// Create a new scheduler.
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

    /// Create a new scheduler with custom configuration.
    pub fn with_config(
        streamer_manager: Arc<StreamerManager<R>>,
        event_broadcaster: ConfigEventBroadcaster,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            streamer_manager,
            event_broadcaster,
            config,
            cancellation_token: CancellationToken::new(),
            active_tasks: HashMap::new(),
            task_set: JoinSet::new(),
        }
    }

    /// Get the cancellation token for this scheduler.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Start the scheduler event loop.
    ///
    /// This method runs until the cancellation token is triggered.
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting scheduler");

        // Subscribe to config update events
        let mut config_receiver = self.event_broadcaster.subscribe();

        // Initial scheduling
        self.schedule_all_streamers().await?;

        // Create reschedule interval
        let mut reschedule_interval = tokio::time::interval(self.config.reschedule_interval);

        loop {
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

                // Periodic re-scheduling
                _ = reschedule_interval.tick() => {
                    debug!("Periodic re-scheduling");
                    if let Err(e) = self.schedule_all_streamers().await {
                        error!("Failed to re-schedule streamers: {}", e);
                    }
                }

                // Handle completed tasks
                Some(result) = self.task_set.join_next() => {
                    match result {
                        Ok(task_result) => {
                            self.handle_task_completion(task_result);
                        }
                        Err(e) => {
                            error!("Task panicked: {}", e);
                        }
                    }
                }
            }
        }

        // Graceful shutdown
        self.shutdown().await;

        info!("Scheduler stopped");
        Ok(())
    }

    /// Schedule monitoring tasks for all active streamers.
    async fn schedule_all_streamers(&mut self) -> Result<()> {
        let streamers = self.streamer_manager.get_ready_for_check();
        info!("Scheduling {} streamers for monitoring", streamers.len());

        // Group by platform
        let groups = group_by_platform(streamers);

        for (_platform_id, group) in groups {
            if group.supports_batch {
                self.schedule_batch_task(group).await;
            } else {
                for streamer in group.streamers {
                    self.schedule_individual_task(streamer).await;
                }
            }
        }

        Ok(())
    }

    /// Schedule a batch monitoring task for a platform.
    async fn schedule_batch_task(&mut self, group: BatchGroup) {
        let platform_id = group.platform_id.clone();
        let streamer_count = group.len();

        debug!(
            "Scheduling batch task for platform {} with {} streamers",
            platform_id, streamer_count
        );

        // Create task handle
        let handle = TaskHandle::new(
            format!("batch-{}", platform_id),
            platform_id.clone(),
            &self.cancellation_token,
        );

        let task_id = handle.id.clone();
        let cancellation_token = handle.cancellation_token.clone();

        // Store handle
        self.active_tasks.insert(task_id.clone(), handle);

        // Spawn batch monitoring task
        let config = self.config.clone();
        self.task_set.spawn(async move {
            // Placeholder for actual batch monitoring logic
            // This will be implemented in the monitor module
            let result = run_batch_monitoring_task(group, config, cancellation_token).await;

            TaskResult {
                streamer_id: task_id,
                status: if result.is_ok() {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                },
                error: result.err().map(|e| e.to_string()),
            }
        });
    }

    /// Schedule an individual monitoring task for a streamer.
    async fn schedule_individual_task(&mut self, streamer: StreamerMetadata) {
        let streamer_id = streamer.id.clone();

        // Skip if already scheduled
        if self.active_tasks.contains_key(&streamer_id) {
            debug!("Streamer {} already has an active task", streamer_id);
            return;
        }

        debug!("Scheduling individual task for streamer {}", streamer_id);

        // Create task handle
        let handle = TaskHandle::new(
            streamer_id.clone(),
            streamer.platform_config_id.clone(),
            &self.cancellation_token,
        );

        let task_id = handle.id.clone();
        let cancellation_token = handle.cancellation_token.clone();

        // Store handle
        self.active_tasks.insert(streamer_id.clone(), handle);

        // Create monitoring task
        let task = MonitoringTask::new(
            streamer,
            self.config.check_interval_ms,
            self.config.offline_check_interval_ms,
            self.config.offline_check_count,
        );

        // Spawn individual monitoring task
        self.task_set.spawn(async move {
            // Placeholder for actual monitoring logic
            // This will be implemented in the monitor module
            let result = run_individual_monitoring_task(task, cancellation_token).await;

            TaskResult {
                streamer_id: task_id,
                status: if result.is_ok() {
                    TaskStatus::Completed
                } else {
                    TaskStatus::Failed
                },
                error: result.err().map(|e| e.to_string()),
            }
        });
    }

    /// Handle a configuration update event.
    async fn handle_config_event(&mut self, event: ConfigUpdateEvent) {
        debug!("Handling config event: {}", event.description());

        match event {
            ConfigUpdateEvent::GlobalUpdated => {
                // Global update affects all streamers - reschedule all
                info!("Global config updated, rescheduling all streamers");
                self.cancel_all_tasks();
                if let Err(e) = self.schedule_all_streamers().await {
                    error!("Failed to reschedule after global update: {}", e);
                }
            }
            ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                // Platform update affects streamers on that platform
                info!(
                    "Platform {} config updated, rescheduling affected streamers",
                    platform_id
                );
                self.cancel_platform_tasks(&platform_id);
                // Streamers will be rescheduled on next periodic check
            }
            ConfigUpdateEvent::TemplateUpdated { template_id } => {
                // Template update affects streamers using that template
                debug!("Template {} config updated", template_id);
                // Streamers will pick up new config on next check
            }
            ConfigUpdateEvent::StreamerUpdated { streamer_id } => {
                // Single streamer update
                debug!("Streamer {} config updated", streamer_id);
                self.cancel_streamer_task(&streamer_id);
                // Streamer will be rescheduled on next periodic check
            }
            ConfigUpdateEvent::EngineUpdated { engine_id } => {
                debug!("Engine {} config updated", engine_id);
                // Engine updates don't affect scheduling
            }
        }
    }

    /// Handle task completion.
    fn handle_task_completion(&mut self, result: TaskResult) {
        debug!(
            "Task {} completed with status {:?}",
            result.streamer_id, result.status
        );

        // Remove from active tasks
        self.active_tasks.remove(&result.streamer_id);

        if let Some(error) = result.error {
            warn!("Task {} failed: {}", result.streamer_id, error);
        }
    }

    /// Cancel all active tasks.
    fn cancel_all_tasks(&mut self) {
        info!("Cancelling all {} active tasks", self.active_tasks.len());
        for (_, handle) in self.active_tasks.drain() {
            handle.cancel();
        }
    }

    /// Cancel tasks for a specific platform.
    fn cancel_platform_tasks(&mut self, platform_id: &str) {
        let to_cancel: Vec<String> = self
            .active_tasks
            .iter()
            .filter(|(_, h)| h.platform_id == platform_id)
            .map(|(id, _)| id.clone())
            .collect();

        debug!(
            "Cancelling {} tasks for platform {}",
            to_cancel.len(),
            platform_id
        );

        for id in to_cancel {
            if let Some(handle) = self.active_tasks.remove(&id) {
                handle.cancel();
            }
        }
    }

    /// Cancel task for a specific streamer.
    fn cancel_streamer_task(&mut self, streamer_id: &str) {
        if let Some(handle) = self.active_tasks.remove(streamer_id) {
            debug!("Cancelling task for streamer {}", streamer_id);
            handle.cancel();
        }
    }

    /// Graceful shutdown.
    async fn shutdown(&mut self) {
        info!("Shutting down scheduler");

        // Cancel all tasks
        self.cancellation_token.cancel();
        self.cancel_all_tasks();

        // Wait for all tasks to complete with timeout
        let shutdown_timeout = Duration::from_secs(10);
        let deadline = tokio::time::Instant::now() + shutdown_timeout;

        while !self.task_set.is_empty() {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    warn!("Shutdown timeout reached, {} tasks still running", self.task_set.len());
                    self.task_set.abort_all();
                    break;
                }
                result = self.task_set.join_next() => {
                    if result.is_none() {
                        break;
                    }
                }
            }
        }

        info!("Scheduler shutdown complete");
    }

    /// Get the number of active tasks.
    pub fn active_task_count(&self) -> usize {
        self.active_tasks.len()
    }

    /// Check if the scheduler is running.
    pub fn is_running(&self) -> bool {
        !self.cancellation_token.is_cancelled()
    }
}

/// Placeholder for batch monitoring task.
/// This will be replaced with actual implementation in the monitor module.
async fn run_batch_monitoring_task(
    _group: BatchGroup,
    _config: SchedulerConfig,
    cancellation_token: CancellationToken,
) -> Result<()> {
    // Wait for cancellation or timeout
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            debug!("Batch task cancelled");
        }
        _ = tokio::time::sleep(Duration::from_secs(60)) => {
            debug!("Batch task completed");
        }
    }
    Ok(())
}

/// Placeholder for individual monitoring task.
/// This will be replaced with actual implementation in the monitor module.
async fn run_individual_monitoring_task(
    task: MonitoringTask,
    cancellation_token: CancellationToken,
) -> Result<()> {
    let interval = task.current_interval();

    // Wait for cancellation or interval
    tokio::select! {
        _ = cancellation_token.cancelled() => {
            debug!("Individual task for {} cancelled", task.streamer.id);
        }
        _ = tokio::time::sleep(interval) => {
            debug!("Individual task for {} completed", task.streamer.id);
        }
    }
    Ok(())
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
}
