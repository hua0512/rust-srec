//! Throttle Controller for pipeline backpressure management.
//!
//! This module implements download throttling based on pipeline queue depth.
//! When the queue becomes critically full, the controller reduces concurrent
//! downloads to allow the pipeline to catch up.
//!
//! Requirements: 8.1, 8.2, 8.3, 8.4, 8.5

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::job_queue::JobQueue;

/// Configuration for the throttle controller.
/// Requirements: 8.1, 8.5
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottleConfig {
    /// Enable download throttling based on queue depth.
    /// When false, no throttle events are emitted regardless of queue depth.
    /// Requirements: 8.5
    pub enabled: bool,
    /// Queue depth threshold to activate throttling.
    /// When queue depth exceeds this value, throttling is activated.
    /// Requirements: 8.1
    pub critical_threshold: usize,
    /// Queue depth threshold to deactivate throttling.
    /// When queue depth falls below this value, throttling is deactivated.
    /// Requirements: 8.3
    pub warning_threshold: usize,
    /// Factor to reduce max_concurrent_downloads by when throttling (0.0-1.0).
    /// A value of 0.5 means reduce to 50% of original.
    /// Requirements: 8.2
    #[serde(default = "default_reduction_factor")]
    pub reduction_factor: f32,
    /// Interval in milliseconds between queue depth checks.
    #[serde(default = "default_check_interval_ms")]
    pub check_interval_ms: u64,
}

fn default_reduction_factor() -> f32 {
    0.5
}

fn default_check_interval_ms() -> u64 {
    1000
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            critical_threshold: 500,
            warning_threshold: 100,
            reduction_factor: default_reduction_factor(),
            check_interval_ms: default_check_interval_ms(),
        }
    }
}

/// Events emitted by the throttle controller.
/// Requirements: 8.1, 8.3, 8.4
#[derive(Debug, Clone)]
pub enum ThrottleEvent {
    /// Throttling has been activated due to high queue depth.
    /// Requirements: 8.1, 8.2
    ThrottleActivated {
        /// Current queue depth that triggered activation.
        queue_depth: usize,
        /// New reduced download limit.
        new_limit: usize,
        /// Original download limit before reduction.
        original_limit: usize,
    },
    /// Throttling has been deactivated as queue depth recovered.
    /// Requirements: 8.3
    ThrottleDeactivated {
        /// Current queue depth when deactivated.
        queue_depth: usize,
        /// Restored download limit.
        restored_limit: usize,
    },
}

/// Callback trait for adjusting download limits.
/// Implementations should adjust the actual download manager's concurrent limit.
pub trait DownloadLimitAdjuster: Send + Sync {
    /// Set the maximum concurrent downloads limit.
    fn set_max_concurrent_downloads(&self, limit: usize);

    /// Get the current maximum concurrent downloads limit.
    fn get_max_concurrent_downloads(&self) -> usize;
}

/// The Throttle Controller service.
/// Monitors pipeline queue depth and controls download throttling.
/// Requirements: 8.1, 8.2, 8.3, 8.4, 8.5
pub struct ThrottleController {
    /// Configuration.
    config: ThrottleConfig,
    /// Original max_concurrent_downloads value before throttling.
    original_max_downloads: AtomicUsize,
    /// Whether throttling is currently active.
    is_throttled: AtomicBool,
    /// Event broadcaster.
    event_tx: broadcast::Sender<ThrottleEvent>,
}

impl ThrottleController {
    /// Create a new throttle controller with the given configuration.
    pub fn new(config: ThrottleConfig) -> Self {
        let (event_tx, _) = broadcast::channel(64);

        Self {
            config,
            original_max_downloads: AtomicUsize::new(0),
            is_throttled: AtomicBool::new(false),
            event_tx,
        }
    }

    /// Create a new throttle controller with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ThrottleConfig::default())
    }

    /// Subscribe to throttle events.
    pub fn subscribe(&self) -> broadcast::Receiver<ThrottleEvent> {
        self.event_tx.subscribe()
    }

    /// Check if throttling is currently active.
    pub fn is_throttled(&self) -> bool {
        self.is_throttled.load(Ordering::SeqCst)
    }

    /// Check if throttling is enabled in configuration.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the current configuration.
    pub fn config(&self) -> &ThrottleConfig {
        &self.config
    }

    /// Get the original max downloads value (before throttling).
    pub fn original_max_downloads(&self) -> usize {
        self.original_max_downloads.load(Ordering::SeqCst)
    }

    /// Calculate the throttled limit based on the original limit.
    /// Requirements: 8.2
    pub fn calculate_throttled_limit(&self, original: usize) -> usize {
        let reduced = (original as f32 * self.config.reduction_factor) as usize;
        // Ensure at least 1 concurrent download
        reduced.max(1)
    }

    /// Check queue depth and update throttle state.
    /// Returns Some(event) if a state transition occurred.
    /// Requirements: 8.1, 8.2, 8.3, 8.4, 8.5
    pub fn check_and_update<A: DownloadLimitAdjuster + ?Sized>(
        &self,
        queue_depth: usize,
        adjuster: &A,
    ) -> Option<ThrottleEvent> {
        // If throttling is disabled, never emit events
        // Requirements: 8.5
        if !self.config.enabled {
            return None;
        }

        let currently_throttled = self.is_throttled.load(Ordering::SeqCst);

        if !currently_throttled && queue_depth > self.config.critical_threshold {
            // Activate throttling
            // Requirements: 8.1, 8.2
            let original = adjuster.get_max_concurrent_downloads();
            self.original_max_downloads
                .store(original, Ordering::SeqCst);

            let new_limit = self.calculate_throttled_limit(original);
            adjuster.set_max_concurrent_downloads(new_limit);
            self.is_throttled.store(true, Ordering::SeqCst);

            // Log the transition
            // Requirements: 8.4
            warn!(
                "Throttling activated: queue_depth={}, reducing max_concurrent_downloads from {} to {}",
                queue_depth, original, new_limit
            );

            let event = ThrottleEvent::ThrottleActivated {
                queue_depth,
                new_limit,
                original_limit: original,
            };
            let _ = self.event_tx.send(event.clone());
            return Some(event);
        } else if currently_throttled && queue_depth < self.config.warning_threshold {
            // Deactivate throttling
            // Requirements: 8.3
            let original = self.original_max_downloads.load(Ordering::SeqCst);
            adjuster.set_max_concurrent_downloads(original);
            self.is_throttled.store(false, Ordering::SeqCst);

            // Log the transition
            // Requirements: 8.4
            info!(
                "Throttling deactivated: queue_depth={}, restoring max_concurrent_downloads to {}",
                queue_depth, original
            );

            let event = ThrottleEvent::ThrottleDeactivated {
                queue_depth,
                restored_limit: original,
            };
            let _ = self.event_tx.send(event.clone());
            return Some(event);
        }

        None
    }

    /// Start background monitoring of queue depth.
    /// This spawns a task that periodically checks queue depth and adjusts throttling.
    pub fn start_monitoring(
        self: Arc<Self>,
        job_queue: Arc<JobQueue>,
        adjuster: Arc<dyn DownloadLimitAdjuster>,
        cancellation_token: CancellationToken,
    ) {
        if !self.config.enabled {
            debug!("Throttle controller disabled, not starting monitoring");
            return;
        }

        let check_interval = Duration::from_millis(self.config.check_interval_ms);

        tokio::spawn(async move {
            info!("Throttle controller monitoring started");

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Throttle controller monitoring shutting down");

                        // Restore original limit if we're currently throttled
                        if self.is_throttled.load(Ordering::SeqCst) {
                            let original = self.original_max_downloads.load(Ordering::SeqCst);
                            adjuster.set_max_concurrent_downloads(original);
                            info!("Restored max_concurrent_downloads to {} on shutdown", original);
                        }
                        break;
                    }
                    _ = tokio::time::sleep(check_interval) => {
                        let queue_depth = job_queue.depth();
                        self.check_and_update(queue_depth, adjuster.as_ref());
                    }
                }
            }

            info!("Throttle controller monitoring stopped");
        });
    }
}

impl Default for ThrottleController {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Mock adjuster for testing.
    struct MockAdjuster {
        limit: AtomicUsize,
    }

    impl MockAdjuster {
        fn new(initial: usize) -> Self {
            Self {
                limit: AtomicUsize::new(initial),
            }
        }
    }

    impl DownloadLimitAdjuster for MockAdjuster {
        fn set_max_concurrent_downloads(&self, limit: usize) {
            self.limit.store(limit, Ordering::SeqCst);
        }

        fn get_max_concurrent_downloads(&self) -> usize {
            self.limit.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn test_throttle_config_default() {
        let config = ThrottleConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.critical_threshold, 500);
        assert_eq!(config.warning_threshold, 100);
        assert!((config.reduction_factor - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_throttle_controller_creation() {
        let controller = ThrottleController::with_defaults();
        assert!(!controller.is_throttled());
        assert!(!controller.is_enabled());
    }

    #[test]
    fn test_calculate_throttled_limit() {
        let config = ThrottleConfig {
            enabled: true,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);

        assert_eq!(controller.calculate_throttled_limit(10), 5);
        assert_eq!(controller.calculate_throttled_limit(6), 3);
        assert_eq!(controller.calculate_throttled_limit(1), 1); // Minimum of 1
        assert_eq!(controller.calculate_throttled_limit(0), 1); // Minimum of 1
    }

    #[test]
    fn test_throttle_disabled_no_events() {
        // Requirements: 8.5
        let config = ThrottleConfig {
            enabled: false,
            critical_threshold: 100,
            warning_threshold: 50,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // Even with high queue depth, no event should be emitted
        let event = controller.check_and_update(200, &adjuster);
        assert!(event.is_none());
        assert!(!controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 10);
    }

    #[test]
    fn test_throttle_activation() {
        // Requirements: 8.1, 8.2
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // Queue depth exceeds critical threshold
        let event = controller.check_and_update(150, &adjuster);

        assert!(event.is_some());
        match event.unwrap() {
            ThrottleEvent::ThrottleActivated {
                queue_depth,
                new_limit,
                original_limit,
            } => {
                assert_eq!(queue_depth, 150);
                assert_eq!(new_limit, 5); // 10 * 0.5
                assert_eq!(original_limit, 10);
            }
            _ => panic!("Expected ThrottleActivated event"),
        }

        assert!(controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 5);
        assert_eq!(controller.original_max_downloads(), 10);
    }

    #[test]
    fn test_throttle_deactivation() {
        // Requirements: 8.3
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // First activate throttling
        controller.check_and_update(150, &adjuster);
        assert!(controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 5);

        // Queue depth falls below warning threshold
        let event = controller.check_and_update(30, &adjuster);

        assert!(event.is_some());
        match event.unwrap() {
            ThrottleEvent::ThrottleDeactivated {
                queue_depth,
                restored_limit,
            } => {
                assert_eq!(queue_depth, 30);
                assert_eq!(restored_limit, 10);
            }
            _ => panic!("Expected ThrottleDeactivated event"),
        }

        assert!(!controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 10);
    }

    #[test]
    fn test_throttle_hysteresis() {
        // Test that throttling doesn't flip-flop between states
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // Activate throttling
        controller.check_and_update(150, &adjuster);
        assert!(controller.is_throttled());

        // Queue depth between warning and critical - should stay throttled
        let event = controller.check_and_update(75, &adjuster);
        assert!(event.is_none());
        assert!(controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 5);

        // Queue depth at exactly warning threshold - should stay throttled
        let event = controller.check_and_update(50, &adjuster);
        assert!(event.is_none());
        assert!(controller.is_throttled());

        // Queue depth below warning threshold - should deactivate
        let event = controller.check_and_update(49, &adjuster);
        assert!(event.is_some());
        assert!(!controller.is_throttled());
    }

    #[test]
    fn test_throttle_round_trip() {
        // Requirements: 8.1, 8.2, 8.3 - Property 10: Throttling round-trip
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // Initial state
        assert!(!controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 10);

        // Activate: queue exceeds critical threshold
        let event = controller.check_and_update(150, &adjuster);
        assert!(matches!(
            event,
            Some(ThrottleEvent::ThrottleActivated { .. })
        ));
        assert!(controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 5); // Reduced by 50%

        // Deactivate: queue falls below warning threshold
        let event = controller.check_and_update(30, &adjuster);
        assert!(matches!(
            event,
            Some(ThrottleEvent::ThrottleDeactivated { .. })
        ));
        assert!(!controller.is_throttled());
        assert_eq!(adjuster.get_max_concurrent_downloads(), 10); // Restored to original
    }

    #[test]
    fn test_no_event_when_already_throttled_and_still_high() {
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // Activate throttling
        let event = controller.check_and_update(150, &adjuster);
        assert!(event.is_some());

        // Check again with still high queue - no new event
        let event = controller.check_and_update(200, &adjuster);
        assert!(event.is_none());
        assert!(controller.is_throttled());
    }

    #[test]
    fn test_no_event_when_not_throttled_and_still_low() {
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        // Check with low queue - no event
        let event = controller.check_and_update(30, &adjuster);
        assert!(event.is_none());
        assert!(!controller.is_throttled());

        // Check again with still low queue - no event
        let event = controller.check_and_update(20, &adjuster);
        assert!(event.is_none());
        assert!(!controller.is_throttled());
    }

    #[test]
    fn test_event_subscription() {
        let config = ThrottleConfig {
            enabled: true,
            critical_threshold: 100,
            warning_threshold: 50,
            reduction_factor: 0.5,
            ..Default::default()
        };
        let controller = ThrottleController::new(config);
        let adjuster = MockAdjuster::new(10);

        let mut receiver = controller.subscribe();

        // Activate throttling
        controller.check_and_update(150, &adjuster);

        // Should receive the event
        let event = receiver.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
            ThrottleEvent::ThrottleActivated { queue_depth, .. } => {
                assert_eq!(queue_depth, 150);
            }
            _ => panic!("Expected ThrottleActivated event"),
        }
    }
}
