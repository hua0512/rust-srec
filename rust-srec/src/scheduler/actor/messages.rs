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
    /// Notify that a download has started for this streamer.
    ///
    /// This allows external download orchestration to pause status checks
    /// even if the actor hasn't just observed a `Live` check result.
    DownloadStarted {
        /// Download identifier.
        download_id: String,
        /// Download session identifier.
        session_id: String,
    },
    /// Notify that the download has ended (streamer went offline or error).
    /// This triggers the actor to resume status checking.
    DownloadEnded(DownloadEndReason),
    /// Request graceful shutdown.
    Stop,
    /// Query current state (response sent via oneshot channel).
    GetState(oneshot::Sender<StreamerActorState>),
}

/// Reason why a download ended.
///
/// Each variant indicates a different end condition and triggers specific
/// behavior in the StreamerActor. See `StreamerActor::handle_download_ended()`
/// for detailed behavior documentation.
///
/// # Behavior Summary
///
/// - **`StreamerOffline`**: Download ended and indicates offline. Actor continues monitoring,
///   immediately publishes an Offline status to the monitor, and keeps a short post-live
///   polling window to catch quick restarts.
///
/// - **`NetworkError`**: Technical failure (timeout, connection lost). Actor continues
///   monitoring with immediate check to verify status and potentially resume quickly.
///
/// - **`SegmentFailed`**: Segment download failed. Actor continues monitoring with
///   immediate check to verify status.
///
/// - **`Cancelled`**: User cancelled the download. **Actor stops monitoring entirely**
///   by returning a fatal error. The download orchestration layer must update the
///   streamer state to `CANCELLED` in the database before sending this message.
///
/// - **`Other`**: Unknown/unexpected error. Actor continues monitoring with normal
///   scheduling, letting the grace period confirm actual state.
#[derive(Debug, Clone)]
pub enum DownloadEndReason {
    /// Streamer went offline normally.
    ///
    /// The download orchestration confirmed the stream ended. The actor will
    /// publish an Offline status to the monitor immediately and then continue
    /// monitoring using the shorter offline polling interval for a few checks
    /// to detect quick restarts.
    StreamerOffline,

    /// Network error during download (e.g., timeout, connection lost).
    ///
    /// Technical failure that doesn't confirm the streamer is offline. The actor
    /// preserves hysteresis state and checks immediately to quickly resume if
    /// the streamer is still live.
    NetworkError(String),

    /// Download was cancelled by user.
    ///
    /// **This stops the actor entirely.** User intent is "I don't want to monitor
    /// this streamer anymore." The download orchestration layer must update the
    /// streamer state to `CANCELLED` in the database before sending this message.
    Cancelled,

    /// Segment download failed (may indicate offline or network issue).
    ///
    /// Similar to `NetworkError` - the actor preserves hysteresis and checks
    /// immediately to verify status and potentially resume quickly.
    SegmentFailed(String),

    /// Other error (unexpected/unknown reason).
    ///
    /// The actor preserves hysteresis and uses normal scheduling, allowing the
    /// grace period to confirm the actual state through status checks.
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
    /// The raw LiveStatus for process_status() calls.
    /// This allows StreamerActor to apply hysteresis before calling process_status().
    pub status: crate::monitor::LiveStatus,
}

/// Encapsulates offline grace period (hysteresis) logic.
///
/// Hysteresis prevents false offline events from transient network issues.
/// After a streamer goes from Live → NotLive, we wait for N consecutive
/// offline checks before emitting the offline event.
///
/// # State Machine
///
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │                    Initial State                            │
/// │              was_live=false, offline_count=0                │
/// └────────────────────────┬────────────────────────────────────┘
///                          │ Live detected
///                          ▼
/// ┌─────────────────────────────────────────────────────────────┐
/// │                   Live State                                │
/// │              was_live=true, offline_count=0                 │
/// └────────────────────────┬────────────────────────────────────┘
///                          │ NotLive detected
///                          ▼
/// ┌─────────────────────────────────────────────────────────────┐
/// │                 Grace Period                                │
/// │  was_live=true, offline_count increments each check         │
/// │  Events SUPPRESSED until offline_count >= threshold         │
/// └────────────────────────┬────────────────────────────────────┘
///                          │ offline_count >= threshold
///                          ▼
/// ┌─────────────────────────────────────────────────────────────┐
/// │               Confirmed Offline                             │
/// │              was_live=false, offline_count=0                │
/// │              Event EMITTED, back to initial state           │
/// └─────────────────────────────────────────────────────────────┘
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct HysteresisState {
    /// Whether the streamer was live before going offline.
    /// This determines if we should apply the grace period.
    was_live: bool,
    /// Consecutive offline check count during grace period.
    /// Only increments when `was_live` is true and we see NotLive.
    offline_count: u32,
}

impl HysteresisState {
    /// Create a new hysteresis state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create hysteresis state from initial streamer state.
    pub fn from_state(state: StreamerState) -> Self {
        Self {
            was_live: state == StreamerState::Live,
            offline_count: 0,
        }
    }

    /// Record a state transition and determine if event should be emitted.
    ///
    /// Returns `true` if the state change should be propagated downstream
    /// (i.e., `process_status()` should be called).
    ///
    /// # Arguments
    ///
    /// * `prev_state` - The previous streamer state
    /// * `new_state` - The newly detected streamer state
    /// * `threshold` - Number of offline checks before confirming offline (from config)
    pub fn should_emit(
        &mut self,
        prev_state: StreamerState,
        new_state: StreamerState,
        threshold: u32,
    ) -> bool {
        tracing::debug!(
            "HysteresisState::should_emit: prev={:?}, new={:?}, was_live={}, offline_count={}, threshold={}",
            prev_state,
            new_state,
            self.was_live,
            self.offline_count,
            threshold
        );

        match (prev_state, new_state) {
            // Live → Live: No state change, suppress redundant event
            (StreamerState::Live, StreamerState::Live) => {
                tracing::debug!("HysteresisState: Live → Live, suppressing redundant event");
                false
            }

            // Any → Live: Going live, always emit
            (_, StreamerState::Live) => {
                tracing::debug!(
                    "HysteresisState: {:?} → Live, setting was_live=true, resetting offline_count, emitting",
                    prev_state
                );
                self.was_live = true;
                self.offline_count = 0;
                true
            }

            // Was live, now NotLive: Apply grace period
            (_, StreamerState::NotLive) if self.was_live => {
                self.offline_count += 1;

                if self.offline_count >= threshold {
                    // Grace period over - emit and reset
                    tracing::debug!(
                        "HysteresisState: grace period threshold reached ({}/{}), emitting offline event and resetting",
                        self.offline_count,
                        threshold
                    );
                    self.reset();
                    true
                } else {
                    // Still in grace period - suppress event
                    tracing::debug!(
                        "HysteresisState: in grace period, suppressing offline event ({}/{})",
                        self.offline_count,
                        threshold
                    );
                    false
                }
            }

            // Never was live, now NotLive: No hysteresis needed, emit
            (_, StreamerState::NotLive) => {
                tracing::debug!(
                    "HysteresisState: {:?} → NotLive (never was live), emitting immediately",
                    prev_state
                );
                true
            }

            // All other transitions (Error, OutOfSchedule, etc.): emit
            _ => {
                tracing::debug!(
                    "HysteresisState: {:?} → {:?}, emitting (non-standard transition)",
                    prev_state,
                    new_state
                );
                true
            }
        }
    }

    /// Reset hysteresis state to initial.
    ///
    /// Call this when:
    /// - Offline is confirmed (threshold reached)
    /// - You intentionally want to forget recent live state
    pub fn reset(&mut self) {
        tracing::debug!(
            "HysteresisState::reset: was_live={} → false, offline_count={} → 0",
            self.was_live,
            self.offline_count
        );
        self.was_live = false;
        self.offline_count = 0;
    }

    /// Record an offline observation while the streamer is considered "recently live".
    ///
    /// This is useful when an external component (e.g., downloader) provides a strong offline
    /// signal and we want to start the post-live short polling window immediately.
    pub fn mark_offline_observed(&mut self) {
        if !self.was_live {
            tracing::debug!(
                "HysteresisState::mark_offline_observed: was_live=false, offline_count stays {}",
                self.offline_count
            );
            return;
        }

        let prev = self.offline_count;
        self.offline_count = self.offline_count.saturating_add(1);
        tracing::debug!(
            "HysteresisState::mark_offline_observed: was_live=true, offline_count={} → {}",
            prev,
            self.offline_count
        );
    }

    /// Mark as having been live.
    ///
    /// Call this when:
    /// - Download starts (we know they're live)
    /// - External confirmation of live status
    pub fn mark_live(&mut self) {
        tracing::debug!(
            "HysteresisState::mark_live: was_live={} → true, offline_count={} → 0",
            self.was_live,
            self.offline_count
        );
        self.was_live = true;
        self.offline_count = 0;
    }

    /// Check if currently in grace period (suppressing offline events).
    pub fn in_grace_period(&self) -> bool {
        self.was_live && self.offline_count > 0
    }

    /// Check if streamer was previously live.
    pub fn was_live(&self) -> bool {
        self.was_live
    }

    /// Get current offline count (for debugging/logging).
    pub fn offline_count(&self) -> u32 {
        self.offline_count
    }
}

/// State of a StreamerActor for monitoring and persistence.
///
/// This struct contains only runtime-specific state needed for scheduling
/// and hysteresis. Persistent state like error counts is fetched from
/// the metadata store on-demand to avoid drift.
#[derive(Debug, Clone)]
pub struct StreamerActorState {
    /// Current streamer state (Live, NotLive, etc.) - actor's local view for scheduling.
    pub streamer_state: StreamerState,
    /// Next scheduled check time.
    pub next_check: Option<Instant>,
    /// Hysteresis state for offline grace period logic.
    pub hysteresis: HysteresisState,
    /// Last check result.
    pub last_check: Option<CheckResult>,
}

impl Default for StreamerActorState {
    fn default() -> Self {
        Self {
            streamer_state: StreamerState::NotLive,
            next_check: None,
            hysteresis: HysteresisState::default(),
            last_check: None,
        }
    }
}

impl StreamerActorState {
    /// Create a new state from streamer metadata.
    pub fn from_metadata(metadata: &StreamerMetadata) -> Self {
        Self {
            streamer_state: metadata.state,
            next_check: Some(Instant::now()), // Due immediately
            hysteresis: HysteresisState::from_state(metadata.state),
            last_check: None,
        }
    }

    /// Record a check result and update state.
    ///
    /// Returns `true` if events should be emitted (e.g., `process_status()` should be called).
    /// Returns `false` if hysteresis is suppressing the event (during offline grace period).
    ///
    /// # Arguments
    ///
    /// * `result` - The check result
    /// * `config` - Actor configuration
    /// * `error_count` - Current consecutive error count from metadata (for scheduling)
    pub fn record_check(
        &mut self,
        result: CheckResult,
        config: &StreamerConfig,
        error_count: u32,
    ) -> bool {
        let prev_state = self.streamer_state;
        self.streamer_state = result.state;

        // Delegate hysteresis logic to HysteresisState
        let should_emit =
            self.hysteresis
                .should_emit(prev_state, result.state, config.offline_check_count);

        self.last_check = Some(result);
        self.schedule_next_check(config, error_count);

        should_emit
    }

    /// Schedule the next check based on current state and config.
    ///
    /// When the streamer is live, we normally don't schedule checks because we're actively
    /// downloading and know they're online. Checks typically resume when:
    /// - The download fails/ends (streamer goes offline or network issues)
    /// - The state changes to NotLive
    ///
    /// Note: the `StreamerActor` run loop may still perform an occasional watchdog check
    /// while live to avoid getting stuck if external download components fail to notify.
    ///
    /// The check interval strategy:
    /// - Normal case (streamer was never live): Use `check_interval_ms` (longer)
    /// - Streamer in grace period (was_live): Use `offline_check_interval_ms` (shorter)
    ///   for quick re-detection
    ///
    /// # Arguments
    ///
    /// * `config` - Actor configuration
    /// * `error_count` - Current consecutive error count from metadata
    pub fn schedule_next_check(&mut self, config: &StreamerConfig, error_count: u32) {
        // Don't schedule checks when streamer is live - we're downloading and know they're online
        if self.streamer_state == StreamerState::Live {
            self.next_check = None;
            return;
        }

        // Use shorter interval when streamer was previously live and is in grace period
        // This enables quick re-detection when a stream ends unexpectedly
        // For streamers that were never live, use the longer interval
        let use_short_interval = self.hysteresis.was_live()
            && (self.streamer_state == StreamerState::NotLive
                || (self.streamer_state == StreamerState::Error
                    && error_count < config.offline_check_count));

        let interval_ms = if use_short_interval {
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
        assert_eq!(state.hysteresis.offline_count(), 0);
        assert!(!state.hysteresis.was_live());
        assert!(state.last_check.is_none());
    }

    #[test]
    fn test_streamer_actor_state_record_check() {
        let mut state = StreamerActorState::default();
        let config = StreamerConfig::default();

        // Record a live check - this sets was_live to true via hysteresis
        state.record_check(CheckResult::success(StreamerState::Live), &config, 0);
        assert_eq!(state.streamer_state, StreamerState::Live);
        assert_eq!(state.hysteresis.offline_count(), 0);
        assert!(state.hysteresis.was_live());

        // Record offline checks - offline_count increments because was_live is true
        state.record_check(CheckResult::success(StreamerState::NotLive), &config, 0);
        assert_eq!(state.hysteresis.offline_count(), 1);
        assert!(state.hysteresis.was_live());

        state.record_check(CheckResult::success(StreamerState::NotLive), &config, 0);
        assert_eq!(state.hysteresis.offline_count(), 2);
        assert!(state.hysteresis.was_live());

        // After 3 offline checks (>= offline_check_count), hysteresis resets
        state.record_check(CheckResult::success(StreamerState::NotLive), &config, 0);
        assert_eq!(state.hysteresis.offline_count(), 0);
        assert!(!state.hysteresis.was_live());
    }

    #[test]
    fn test_streamer_actor_state_schedule_next_check() {
        let mut state = StreamerActorState::default();
        let config = StreamerConfig::default();

        // Normal interval for offline streamer
        state.schedule_next_check(&config, 0);
        assert!(state.next_check.is_some());

        // When hysteresis indicates not in grace period, should use normal interval
        state.streamer_state = StreamerState::NotLive;
        state.schedule_next_check(&config, 0);
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
        state.schedule_next_check(&config, 0);
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
