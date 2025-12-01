//! Restart tracker for managing actor restart backoff.
//!
//! The `RestartTracker` tracks restart history per actor and calculates
//! exponential backoff delays to prevent restart storms.
//!
//! # Backoff Algorithm
//!
//! - First 3 failures within the failure window: no backoff (immediate restart)
//! - After 3 failures: backoff = base * 2^(failures - 3)
//! - Backoff is capped at max_backoff
//! - Failure count resets when the failure window expires

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

/// Default base backoff duration (1 second).
pub const DEFAULT_BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Default maximum backoff duration (5 minutes).
pub const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Default failure window (60 seconds).
pub const DEFAULT_FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// Default failure threshold before applying backoff.
pub const DEFAULT_FAILURE_THRESHOLD: usize = 3;

/// Maximum exponent to prevent overflow.
const MAX_EXPONENT: u32 = 10;

/// Configuration for the restart tracker.
#[derive(Debug, Clone)]
pub struct RestartTrackerConfig {
    /// Base backoff duration.
    pub base_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
    /// Window for counting failures.
    pub failure_window: Duration,
    /// Number of failures before applying backoff.
    pub failure_threshold: usize,
}

impl Default for RestartTrackerConfig {
    fn default() -> Self {
        Self {
            base_backoff: DEFAULT_BASE_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            failure_window: DEFAULT_FAILURE_WINDOW,
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
        }
    }
}


/// Restart history for a single actor.
#[derive(Debug, Clone)]
struct RestartHistory {
    /// Timestamps of recent failures.
    failures: Vec<Instant>,
    /// Total restart count (for metrics).
    total_restarts: u64,
    /// Last restart time.
    last_restart: Option<Instant>,
}

impl RestartHistory {
    fn new() -> Self {
        Self {
            failures: Vec::new(),
            total_restarts: 0,
            last_restart: None,
        }
    }

    /// Record a failure and return the number of recent failures.
    fn record_failure(&mut self, now: Instant, window: Duration) -> usize {
        // Remove old failures outside the window
        self.failures.retain(|&t| now.duration_since(t) < window);

        // Add new failure
        self.failures.push(now);
        self.total_restarts += 1;
        self.last_restart = Some(now);

        self.failures.len()
    }

    /// Get the number of recent failures within the window.
    fn recent_failures(&self, now: Instant, window: Duration) -> usize {
        self.failures
            .iter()
            .filter(|&&t| now.duration_since(t) < window)
            .count()
    }

    /// Clear failure history (e.g., after successful operation).
    fn clear_failures(&mut self) {
        self.failures.clear();
    }
}

/// Tracks restart history and calculates backoff for actors.
///
/// The tracker maintains per-actor failure history and calculates
/// exponential backoff delays to prevent restart storms.
pub struct RestartTracker {
    /// Restart history by actor ID.
    history: HashMap<String, RestartHistory>,
    /// Configuration.
    config: RestartTrackerConfig,
}

impl RestartTracker {
    /// Create a new restart tracker with default configuration.
    pub fn new() -> Self {
        Self::with_config(RestartTrackerConfig::default())
    }

    /// Create a new restart tracker with custom configuration.
    pub fn with_config(config: RestartTrackerConfig) -> Self {
        Self {
            history: HashMap::new(),
            config,
        }
    }

    /// Record a failure for an actor and get the backoff duration.
    ///
    /// Returns the duration to wait before restarting the actor.
    pub fn record_failure(&mut self, actor_id: &str) -> Duration {
        let now = Instant::now();
        let history = self
            .history
            .entry(actor_id.to_string())
            .or_insert_with(RestartHistory::new);

        let failures = history.record_failure(now, self.config.failure_window);
        let backoff = self.calculate_backoff(failures);

        if backoff.is_zero() {
            debug!(
                "Actor {} failed ({} times in window), immediate restart",
                actor_id, failures
            );
        } else {
            info!(
                "Actor {} failed ({} times in window), backoff: {:?}",
                actor_id, failures, backoff
            );
        }

        backoff
    }

    /// Get the backoff duration for an actor without recording a failure.
    pub fn get_backoff(&self, actor_id: &str) -> Duration {
        let now = Instant::now();
        let failures = self
            .history
            .get(actor_id)
            .map(|h| h.recent_failures(now, self.config.failure_window))
            .unwrap_or(0);

        self.calculate_backoff(failures)
    }

    /// Calculate backoff duration based on failure count.
    ///
    /// Formula: base * 2^(failures - threshold) for failures >= threshold
    fn calculate_backoff(&self, failures: usize) -> Duration {
        if failures < self.config.failure_threshold {
            return Duration::ZERO;
        }

        let exponent = (failures - self.config.failure_threshold).min(MAX_EXPONENT as usize) as u32;
        let multiplier = 2u32.saturating_pow(exponent);
        let backoff = self.config.base_backoff.saturating_mul(multiplier);

        // Cap at max backoff
        backoff.min(self.config.max_backoff)
    }

    /// Get the number of recent failures for an actor.
    pub fn recent_failures(&self, actor_id: &str) -> usize {
        let now = Instant::now();
        self.history
            .get(actor_id)
            .map(|h| h.recent_failures(now, self.config.failure_window))
            .unwrap_or(0)
    }

    /// Get the total restart count for an actor.
    pub fn total_restarts(&self, actor_id: &str) -> u64 {
        self.history.get(actor_id).map(|h| h.total_restarts).unwrap_or(0)
    }

    /// Clear failure history for an actor.
    ///
    /// Call this when an actor has been running successfully for a while.
    pub fn clear_failures(&mut self, actor_id: &str) {
        if let Some(history) = self.history.get_mut(actor_id) {
            debug!("Clearing failure history for actor {}", actor_id);
            history.clear_failures();
        }
    }

    /// Remove an actor from tracking.
    pub fn remove(&mut self, actor_id: &str) {
        self.history.remove(actor_id);
    }

    /// Check if an actor should be restarted.
    ///
    /// Returns `true` if the actor has not exceeded a reasonable restart limit.
    pub fn should_restart(&self, actor_id: &str) -> bool {
        let failures = self.recent_failures(actor_id);
        // Allow restart if under a reasonable limit (e.g., 10 failures in window)
        failures < 10
    }

    /// Get statistics for all tracked actors.
    pub fn stats(&self) -> RestartTrackerStats {
        let now = Instant::now();
        let total_actors = self.history.len();
        let actors_with_failures = self
            .history
            .values()
            .filter(|h| h.recent_failures(now, self.config.failure_window) > 0)
            .count();
        let total_restarts: u64 = self.history.values().map(|h| h.total_restarts).sum();

        RestartTrackerStats {
            total_actors,
            actors_with_failures,
            total_restarts,
        }
    }
}

impl Default for RestartTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics from the restart tracker.
#[derive(Debug, Clone)]
pub struct RestartTrackerStats {
    /// Total number of tracked actors.
    pub total_actors: usize,
    /// Number of actors with recent failures.
    pub actors_with_failures: usize,
    /// Total restart count across all actors.
    pub total_restarts: u64,
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_restart_tracker_new() {
        let tracker = RestartTracker::new();
        assert_eq!(tracker.recent_failures("test"), 0);
        assert_eq!(tracker.get_backoff("test"), Duration::ZERO);
    }

    #[test]
    fn test_restart_tracker_no_backoff_under_threshold() {
        let mut tracker = RestartTracker::new();

        // Failures 1 and 2 should have no backoff (under threshold of 3)
        for i in 0..2 {
            let backoff = tracker.record_failure("test");
            assert_eq!(backoff, Duration::ZERO, "Failure {} should have no backoff", i + 1);
        }

        assert_eq!(tracker.recent_failures("test"), 2);
    }

    #[test]
    fn test_restart_tracker_exponential_backoff() {
        let config = RestartTrackerConfig {
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(300),
            failure_window: Duration::from_secs(60),
            failure_threshold: 3,
        };
        let mut tracker = RestartTracker::with_config(config);

        // First 2 failures: no backoff (under threshold)
        for _ in 0..2 {
            let backoff = tracker.record_failure("test");
            assert_eq!(backoff, Duration::ZERO);
        }

        // 3rd failure: base * 2^0 = 1s (at threshold)
        let backoff = tracker.record_failure("test");
        assert_eq!(backoff, Duration::from_secs(1));

        // 4th failure: base * 2^1 = 2s
        let backoff = tracker.record_failure("test");
        assert_eq!(backoff, Duration::from_secs(2));

        // 5th failure: base * 2^2 = 4s
        let backoff = tracker.record_failure("test");
        assert_eq!(backoff, Duration::from_secs(4));

        // 6th failure: base * 2^3 = 8s
        let backoff = tracker.record_failure("test");
        assert_eq!(backoff, Duration::from_secs(8));
    }

    #[test]
    fn test_restart_tracker_max_backoff() {
        let config = RestartTrackerConfig {
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(10), // Low max for testing
            failure_window: Duration::from_secs(60),
            failure_threshold: 3,
        };
        let mut tracker = RestartTracker::with_config(config);

        // Record many failures
        for _ in 0..20 {
            tracker.record_failure("test");
        }

        // Backoff should be capped at max
        let backoff = tracker.get_backoff("test");
        assert_eq!(backoff, Duration::from_secs(10));
    }

    #[test]
    fn test_restart_tracker_clear_failures() {
        let mut tracker = RestartTracker::new();

        // Record some failures
        for _ in 0..5 {
            tracker.record_failure("test");
        }
        assert_eq!(tracker.recent_failures("test"), 5);

        // Clear failures
        tracker.clear_failures("test");
        assert_eq!(tracker.recent_failures("test"), 0);
        assert_eq!(tracker.get_backoff("test"), Duration::ZERO);

        // Total restarts should still be tracked
        assert_eq!(tracker.total_restarts("test"), 5);
    }

    #[test]
    fn test_restart_tracker_remove() {
        let mut tracker = RestartTracker::new();

        tracker.record_failure("test");
        assert_eq!(tracker.recent_failures("test"), 1);

        tracker.remove("test");
        assert_eq!(tracker.recent_failures("test"), 0);
        assert_eq!(tracker.total_restarts("test"), 0);
    }

    #[test]
    fn test_restart_tracker_multiple_actors() {
        let mut tracker = RestartTracker::new();

        // Record failures for different actors
        for _ in 0..5 {
            tracker.record_failure("actor-1");
        }
        for _ in 0..2 {
            tracker.record_failure("actor-2");
        }

        assert_eq!(tracker.recent_failures("actor-1"), 5);
        assert_eq!(tracker.recent_failures("actor-2"), 2);

        // actor-1 should have backoff (5 >= 3), actor-2 should not (2 < 3)
        assert!(tracker.get_backoff("actor-1") > Duration::ZERO);
        assert_eq!(tracker.get_backoff("actor-2"), Duration::ZERO);
    }

    #[test]
    fn test_restart_tracker_should_restart() {
        let mut tracker = RestartTracker::new();

        // Should restart with few failures
        for _ in 0..5 {
            tracker.record_failure("test");
        }
        assert!(tracker.should_restart("test"));

        // Should not restart after too many failures
        for _ in 0..10 {
            tracker.record_failure("test");
        }
        assert!(!tracker.should_restart("test"));
    }

    #[test]
    fn test_restart_tracker_stats() {
        let mut tracker = RestartTracker::new();

        tracker.record_failure("actor-1");
        tracker.record_failure("actor-1");
        tracker.record_failure("actor-2");

        let stats = tracker.stats();
        assert_eq!(stats.total_actors, 2);
        assert_eq!(stats.actors_with_failures, 2);
        assert_eq!(stats.total_restarts, 3);
    }

    #[test]
    fn test_calculate_backoff_formula() {
        let config = RestartTrackerConfig {
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(60),
            failure_window: Duration::from_secs(60),
            failure_threshold: 3,
        };
        let tracker = RestartTracker::with_config(config);

        // Under threshold: no backoff
        assert_eq!(tracker.calculate_backoff(0), Duration::ZERO);
        assert_eq!(tracker.calculate_backoff(1), Duration::ZERO);
        assert_eq!(tracker.calculate_backoff(2), Duration::ZERO);

        // At and above threshold: exponential backoff
        // failures=3: base * 2^0 = 100ms
        assert_eq!(tracker.calculate_backoff(3), Duration::from_millis(100));
        // failures=4: base * 2^1 = 200ms
        assert_eq!(tracker.calculate_backoff(4), Duration::from_millis(200));
        // failures=5: base * 2^2 = 400ms
        assert_eq!(tracker.calculate_backoff(5), Duration::from_millis(400));
        // failures=6: base * 2^3 = 800ms
        assert_eq!(tracker.calculate_backoff(6), Duration::from_millis(800));
    }
}
