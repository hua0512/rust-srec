//! Actor message types for the scheduler actor system.
//!
//! This module defines the message enums for communication between actors:
//! - `StreamerMessage`: Messages sent to StreamerActors
//! - `PlatformMessage`: Messages sent to PlatformActors
//! - `SupervisorMessage`: Messages sent to the Supervisor (Scheduler)

use std::time::Instant;

use chrono::{DateTime, Utc};
use tokio::sync::oneshot;

use crate::domain::{Priority, StreamerState};
use crate::streamer::StreamerMetadata;

/// Messages that can be sent to a StreamerActor.
#[derive(Debug)]
pub enum StreamerMessage {
    /// Trigger a status check.
    CheckStatus,
    /// Update configuration.
    ConfigUpdate(StreamerConfig),
    /// Receive batch detection result from PlatformActor.
    BatchResult(BatchDetectionResult),
    /// Notify that the download has ended (streamer went offline or error).
    /// This triggers the actor to resume status checking.
    DownloadEnded(DownloadEndReason),
    /// Request graceful shutdown.
    Stop,
    /// Query current state (response sent via oneshot channel).
    GetState(oneshot::Sender<StreamerActorState>),
}

/// Reason why a download ended.
#[derive(Debug, Clone)]
pub enum DownloadEndReason {
    /// Streamer went offline normally.
    StreamerOffline,
    /// Network error during download.
    NetworkError(String),
    /// Download was cancelled by user.
    Cancelled,
    /// Segment download failed (may indicate offline or network issue).
    SegmentFailed(String),
    /// Other error.
    Other(String),
}

/// Messages that can be sent to a PlatformActor.
#[derive(Debug)]
pub enum PlatformMessage {
    /// Request batch detection for a streamer.
    RequestCheck {
        /// Streamer ID requesting the check.
        streamer_id: String,
        /// Channel to acknowledge the request was queued.
        reply: oneshot::Sender<()>,
    },
    /// Update platform configuration.
    ConfigUpdate(PlatformConfig),
    /// Request graceful shutdown.
    Stop,
    /// Query current state (response sent via oneshot channel).
    GetState(oneshot::Sender<PlatformActorState>),
}

/// Messages for the Supervisor (Scheduler).
#[derive(Debug)]
pub enum SupervisorMessage {
    /// Spawn a new streamer actor.
    SpawnStreamer(StreamerMetadata),
    /// Remove a streamer actor.
    RemoveStreamer(String),
    /// Actor crashed, needs restart.
    ActorCrashed {
        /// ID of the crashed actor.
        actor_id: String,
        /// Error message describing the crash.
        error: String,
    },
    /// Shutdown all actors.
    Shutdown,
}

/// Configuration for a StreamerActor.
#[derive(Debug, Clone)]
pub struct StreamerConfig {
    /// Check interval in milliseconds.
    pub check_interval_ms: u64,
    /// Offline check interval in milliseconds.
    pub offline_check_interval_ms: u64,
    /// Number of offline checks before using offline interval.
    pub offline_check_count: u32,
    /// Priority level for this streamer.
    pub priority: Priority,
    /// Whether this streamer is on a batch-capable platform.
    pub batch_capable: bool,
}

impl Default for StreamerConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: 60_000,
            offline_check_interval_ms: 20_000,
            offline_check_count: 3,
            priority: Priority::Normal,
            batch_capable: false,
        }
    }
}

/// Configuration for a PlatformActor.
#[derive(Debug, Clone)]
pub struct PlatformConfig {
    /// Platform identifier.
    pub platform_id: String,
    /// Batch window duration in milliseconds.
    pub batch_window_ms: u64,
    /// Maximum batch size.
    pub max_batch_size: usize,
    /// Rate limit (requests per second).
    pub rate_limit: Option<f64>,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            platform_id: String::new(),
            batch_window_ms: 500,
            max_batch_size: 100,
            rate_limit: None,
        }
    }
}

/// Result of a status check for a single streamer.
#[derive(Debug, Clone)]
pub struct CheckResult {
    /// Detected streamer state.
    pub state: StreamerState,
    /// Stream URL if live.
    pub stream_url: Option<String>,
    /// Stream title if available.
    pub title: Option<String>,
    /// Timestamp of the check.
    pub checked_at: DateTime<Utc>,
    /// Error message if check failed.
    pub error: Option<String>,
}

impl CheckResult {
    /// Create a successful check result.
    pub fn success(state: StreamerState) -> Self {
        Self {
            state,
            stream_url: None,
            title: None,
            checked_at: Utc::now(),
            error: None,
        }
    }

    /// Create a failed check result.
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            state: StreamerState::Error,
            stream_url: None,
            title: None,
            checked_at: Utc::now(),
            error: Some(error.into()),
        }
    }

    /// Check if this result indicates an error.
    pub fn is_error(&self) -> bool {
        self.error.is_some() || self.state.is_error()
    }
}

/// Result of a batch detection from PlatformActor.
#[derive(Debug, Clone)]
pub struct BatchDetectionResult {
    /// Streamer ID this result is for.
    pub streamer_id: String,
    /// The check result.
    pub result: CheckResult,
}

/// State of a StreamerActor for monitoring and persistence.
#[derive(Debug, Clone)]
pub struct StreamerActorState {
    /// Current streamer state (Live, NotLive, etc.).
    pub streamer_state: StreamerState,
    /// Next scheduled check time.
    pub next_check: Option<Instant>,
    /// Consecutive offline count (after going offline from live).
    pub offline_count: u32,
    /// Whether the streamer was previously live (used for quick re-detection).
    pub was_live: bool,
    /// Last check result.
    pub last_check: Option<CheckResult>,
    /// Error count for circuit breaker.
    pub error_count: u32,
}

impl Default for StreamerActorState {
    fn default() -> Self {
        Self {
            streamer_state: StreamerState::NotLive,
            next_check: None,
            offline_count: 0,
            was_live: false,
            last_check: None,
            error_count: 0,
        }
    }
}

impl StreamerActorState {
    /// Create a new state from streamer metadata.
    pub fn from_metadata(metadata: &StreamerMetadata) -> Self {
        Self {
            streamer_state: metadata.state,
            next_check: Some(Instant::now()), // Due immediately
            offline_count: 0,
            was_live: metadata.state == StreamerState::Live,
            last_check: None,
            error_count: metadata.consecutive_error_count as u32,
        }
    }

    /// Record a check result and update state.
    pub fn record_check(&mut self, result: CheckResult, config: &StreamerConfig) {
        self.streamer_state = result.state;

        if result.is_error() {
            self.error_count += 1;
        } else {
            self.error_count = 0;
        }

        // Track if streamer was live for quick re-detection logic
        if result.state == StreamerState::Live {
            self.was_live = true;
            self.offline_count = 0;
        } else if result.state == StreamerState::NotLive {
            // Only count offline checks if streamer was previously live
            if self.was_live {
                self.offline_count += 1;
                // Reset was_live after enough offline checks
                if self.offline_count >= config.offline_check_count {
                    self.was_live = false;
                    self.offline_count = 0;
                }
            }
        }

        self.last_check = Some(result);
        self.schedule_next_check(config);
    }

    /// Schedule the next check based on current state and config.
    ///
    /// When the streamer is live, we don't schedule checks because we're actively
    /// downloading and know they're online. Checks will resume when:
    /// - The download fails/ends (streamer goes offline or network issues)
    /// - The state changes to NotLive
    ///
    /// The check interval strategy:
    /// - Normal case (streamer was never live): Use `check_interval_ms` (longer)
    /// - Streamer just went offline (was_live && offline_count < offline_check_count):
    ///   Use `offline_check_interval_ms` (shorter) for quick re-detection
    /// - After several offline checks: Use `check_interval_ms` (longer)
    pub fn schedule_next_check(&mut self, config: &StreamerConfig) {
        // Don't schedule checks when streamer is live - we're downloading and know they're online
        if self.streamer_state == StreamerState::Live {
            self.next_check = None;
            return;
        }

        // Use shorter interval only when streamer was previously live and just went offline
        // This enables quick re-detection when a stream ends unexpectedly
        // For streamers that were never live, use the longer interval
        let interval_ms = if self.was_live
            && self.streamer_state == StreamerState::NotLive
            && self.offline_count < config.offline_check_count
        {
            config.offline_check_interval_ms
        } else {
            config.check_interval_ms
        };

        self.next_check = Some(Instant::now() + std::time::Duration::from_millis(interval_ms));
    }

    /// Force schedule a check (used when download fails or streamer goes offline).
    ///
    /// This bypasses the live state check and schedules an immediate check.
    pub fn schedule_immediate_check(&mut self) {
        self.next_check = Some(Instant::now());
    }

    /// Check if a check is due.
    pub fn is_check_due(&self) -> bool {
        self.next_check.map(|t| Instant::now() >= t).unwrap_or(true)
    }

    /// Get time until next check.
    ///
    /// Returns `None` if no check is scheduled (e.g., when streamer is live).
    /// Returns `Some(Duration::ZERO)` if a check is due immediately.
    pub fn time_until_next_check(&self) -> Option<std::time::Duration> {
        self.next_check
            .map(|t| t.saturating_duration_since(Instant::now()))
    }
}

/// State of a PlatformActor for monitoring.
#[derive(Debug, Clone)]
pub struct PlatformActorState {
    /// Number of registered streamers.
    pub streamer_count: usize,
    /// Pending batch requests.
    pub pending_count: usize,
    /// Last batch execution time.
    pub last_batch: Option<Instant>,
    /// Batch success rate (0.0 to 1.0).
    pub success_rate: f64,
    /// Total batches executed.
    pub total_batches: u64,
    /// Total successful batches.
    pub successful_batches: u64,
}

impl Default for PlatformActorState {
    fn default() -> Self {
        Self {
            streamer_count: 0,
            pending_count: 0,
            last_batch: None,
            success_rate: 1.0,
            total_batches: 0,
            successful_batches: 0,
        }
    }
}

impl PlatformActorState {
    /// Record a batch execution result.
    pub fn record_batch(&mut self, success: bool) {
        self.total_batches += 1;
        if success {
            self.successful_batches += 1;
        }
        self.success_rate = self.successful_batches as f64 / self.total_batches as f64;
        self.last_batch = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streamer_config_default() {
        let config = StreamerConfig::default();
        assert_eq!(config.check_interval_ms, 60_000);
        assert_eq!(config.offline_check_interval_ms, 20_000);
        assert_eq!(config.offline_check_count, 3);
        assert_eq!(config.priority, Priority::Normal);
        assert!(!config.batch_capable);
    }

    #[test]
    fn test_platform_config_default() {
        let config = PlatformConfig::default();
        assert_eq!(config.batch_window_ms, 500);
        assert_eq!(config.max_batch_size, 100);
        assert!(config.rate_limit.is_none());
    }

    #[test]
    fn test_check_result_success() {
        let result = CheckResult::success(StreamerState::Live);
        assert_eq!(result.state, StreamerState::Live);
        assert!(!result.is_error());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_check_result_failure() {
        let result = CheckResult::failure("Network error");
        assert!(result.is_error());
        assert_eq!(result.error, Some("Network error".to_string()));
    }

    #[test]
    fn test_streamer_actor_state_default() {
        let state = StreamerActorState::default();
        assert_eq!(state.streamer_state, StreamerState::NotLive);
        assert_eq!(state.offline_count, 0);
        assert_eq!(state.error_count, 0);
        assert!(state.last_check.is_none());
    }

    #[test]
    fn test_streamer_actor_state_record_check() {
        let mut state = StreamerActorState::default();
        let config = StreamerConfig::default();

        // Record a live check - this sets was_live to true
        state.record_check(CheckResult::success(StreamerState::Live), &config);
        assert_eq!(state.streamer_state, StreamerState::Live);
        assert_eq!(state.offline_count, 0);
        assert_eq!(state.error_count, 0);
        assert!(state.was_live);

        // Record offline checks - offline_count increments because was_live is true
        state.record_check(CheckResult::success(StreamerState::NotLive), &config);
        assert_eq!(state.offline_count, 1);
        assert!(state.was_live);

        state.record_check(CheckResult::success(StreamerState::NotLive), &config);
        assert_eq!(state.offline_count, 2);
        assert!(state.was_live);

        // After 3 offline checks (>= offline_check_count), was_live resets to false
        // and offline_count resets to 0
        state.record_check(CheckResult::success(StreamerState::NotLive), &config);
        assert_eq!(state.offline_count, 0);
        assert!(!state.was_live);
    }

    #[test]
    fn test_streamer_actor_state_schedule_next_check() {
        let mut state = StreamerActorState::default();
        let config = StreamerConfig::default();

        // Normal interval for offline streamer
        state.schedule_next_check(&config);
        assert!(state.next_check.is_some());

        // After enough offline checks, should use offline interval
        state.streamer_state = StreamerState::NotLive;
        state.offline_count = 3;
        state.schedule_next_check(&config);
        // The next check should be scheduled
        assert!(state.next_check.is_some());
    }

    #[test]
    fn test_streamer_actor_state_no_check_when_live() {
        let mut state = StreamerActorState::default();
        let config = StreamerConfig::default();

        // When streamer is live, no check should be scheduled
        // (we're downloading and know they're online)
        state.streamer_state = StreamerState::Live;
        state.schedule_next_check(&config);
        assert!(state.next_check.is_none());
    }

    #[test]
    fn test_streamer_actor_state_schedule_immediate_check() {
        let mut state = StreamerActorState::default();

        // Even when live, schedule_immediate_check should work
        state.streamer_state = StreamerState::Live;
        state.schedule_immediate_check();
        assert!(state.next_check.is_some());

        // Check should be due immediately
        assert!(state.is_check_due());
    }

    #[test]
    fn test_platform_actor_state_record_batch() {
        let mut state = PlatformActorState::default();

        state.record_batch(true);
        assert_eq!(state.total_batches, 1);
        assert_eq!(state.successful_batches, 1);
        assert_eq!(state.success_rate, 1.0);

        state.record_batch(false);
        assert_eq!(state.total_batches, 2);
        assert_eq!(state.successful_batches, 1);
        assert_eq!(state.success_rate, 0.5);
    }
}
