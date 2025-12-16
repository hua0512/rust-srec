//! Monitoring task definitions.
//!
//! This module defines the task structures used by the scheduler
//! to track and manage monitoring operations.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use crate::domain::StreamerState;
use crate::streamer::StreamerMetadata;

/// Status of a monitoring task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Task is pending execution.
    Pending,
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task was cancelled.
    Cancelled,
    /// Task failed with an error.
    Failed,
}

/// A handle to a spawned monitoring task.
#[derive(Debug)]
pub struct TaskHandle {
    /// Unique task identifier.
    pub id: String,
    /// Streamer ID being monitored.
    pub streamer_id: String,
    /// Platform ID for grouping.
    pub platform_id: String,
    /// Current task status.
    pub status: TaskStatus,
    /// When the task was created.
    pub created_at: Instant,
    /// Cancellation token for this task.
    pub cancellation_token: CancellationToken,
}

impl TaskHandle {
    /// Create a new task handle.
    pub fn new(streamer_id: String, platform_id: String, parent_token: &CancellationToken) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            streamer_id,
            platform_id,
            status: TaskStatus::Pending,
            created_at: Instant::now(),
            cancellation_token: parent_token.child_token(),
        }
    }

    /// Cancel this task.
    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    /// Check if this task is cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Get the elapsed time since task creation.
    pub fn elapsed(&self) -> Duration {
        self.created_at.elapsed()
    }
}

/// A monitoring task for a single streamer.
#[derive(Debug, Clone)]
pub struct MonitoringTask {
    /// Streamer metadata.
    pub streamer: StreamerMetadata,
    /// Check interval in milliseconds.
    pub check_interval_ms: u64,
    /// Offline check interval in milliseconds.
    pub offline_check_interval_ms: u64,
    /// Number of consecutive offline checks before using offline interval.
    pub offline_check_count: u32,
    /// Current consecutive offline count.
    pub current_offline_count: u32,
    /// Last check time.
    pub last_check_time: Option<DateTime<Utc>>,
    /// Last detected state.
    pub last_state: StreamerState,
}

impl MonitoringTask {
    /// Create a new monitoring task from streamer metadata.
    pub fn new(
        streamer: StreamerMetadata,
        check_interval_ms: u64,
        offline_check_interval_ms: u64,
        offline_check_count: u32,
    ) -> Self {
        Self {
            last_state: streamer.state,
            streamer,
            check_interval_ms,
            offline_check_interval_ms,
            offline_check_count,
            current_offline_count: 0,
            last_check_time: None,
        }
    }

    /// Get the current check interval based on state.
    pub fn current_interval(&self) -> Duration {
        if self.last_state == StreamerState::NotLive
            && self.current_offline_count >= self.offline_check_count
        {
            Duration::from_millis(self.offline_check_interval_ms)
        } else {
            Duration::from_millis(self.check_interval_ms)
        }
    }

    /// Record an offline check result.
    pub fn record_offline(&mut self) {
        self.current_offline_count += 1;
        self.last_state = StreamerState::NotLive;
        self.last_check_time = Some(Utc::now());
    }

    /// Record a live check result.
    pub fn record_live(&mut self) {
        self.current_offline_count = 0;
        self.last_state = StreamerState::Live;
        self.last_check_time = Some(Utc::now());
    }

    /// Record an out-of-schedule check result.
    pub fn record_out_of_schedule(&mut self) {
        self.current_offline_count = 0;
        self.last_state = StreamerState::OutOfSchedule;
        self.last_check_time = Some(Utc::now());
    }

    /// Check if enough time has passed for the next check.
    pub fn is_due_for_check(&self) -> bool {
        match self.last_check_time {
            Some(last) => {
                let elapsed = Utc::now().signed_duration_since(last);
                elapsed >= chrono::Duration::from_std(self.current_interval()).unwrap_or_default()
            }
            None => true, // Never checked, so due immediately
        }
    }

    /// Get time until next check.
    pub fn time_until_next_check(&self) -> Duration {
        match self.last_check_time {
            Some(last) => {
                let elapsed = Utc::now().signed_duration_since(last);
                let interval =
                    chrono::Duration::from_std(self.current_interval()).unwrap_or_default();
                if elapsed >= interval {
                    Duration::ZERO
                } else {
                    (interval - elapsed).to_std().unwrap_or(Duration::ZERO)
                }
            }
            None => Duration::ZERO,
        }
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
            avatar_url: None,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
            last_error: None,
            streamer_specific_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_task_handle_creation() {
        let parent_token = CancellationToken::new();
        let handle = TaskHandle::new(
            "streamer-1".to_string(),
            "twitch".to_string(),
            &parent_token,
        );

        assert_eq!(handle.streamer_id, "streamer-1");
        assert_eq!(handle.platform_id, "twitch");
        assert_eq!(handle.status, TaskStatus::Pending);
        assert!(!handle.is_cancelled());
    }

    #[test]
    fn test_task_handle_cancellation() {
        let parent_token = CancellationToken::new();
        let handle = TaskHandle::new(
            "streamer-1".to_string(),
            "twitch".to_string(),
            &parent_token,
        );

        assert!(!handle.is_cancelled());
        handle.cancel();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn test_parent_cancellation_propagates() {
        let parent_token = CancellationToken::new();
        let handle = TaskHandle::new(
            "streamer-1".to_string(),
            "twitch".to_string(),
            &parent_token,
        );

        assert!(!handle.is_cancelled());
        parent_token.cancel();
        assert!(handle.is_cancelled());
    }

    #[test]
    fn test_monitoring_task_interval() {
        let metadata = create_test_metadata();
        let mut task = MonitoringTask::new(metadata, 60000, 20000, 3);

        // Initially should use normal interval
        assert_eq!(task.current_interval(), Duration::from_millis(60000));

        // After 3 offline checks, should use offline interval
        task.record_offline();
        task.record_offline();
        task.record_offline();
        assert_eq!(task.current_interval(), Duration::from_millis(20000));

        // Going live should reset
        task.record_live();
        assert_eq!(task.current_interval(), Duration::from_millis(60000));
    }

    #[test]
    fn test_monitoring_task_due_for_check() {
        let metadata = create_test_metadata();
        let task = MonitoringTask::new(metadata, 60000, 20000, 3);

        // Never checked, should be due
        assert!(task.is_due_for_check());
    }
}
