//! Throttle Controller for pipeline backpressure management.
//!
//! This module implements download throttling based on pipeline queue depth.
//! When the queue becomes critically full, the controller reduces concurrent
//! downloads to allow the pipeline to catch up.
//!

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::job_queue::JobQueue;

/// Configuration for the throttle controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottleConfig {
    /// Enable download throttling based on queue depth.
    /// When false, no throttle events are emitted regardless of queue depth.
    pub enabled: bool,
    /// Queue depth threshold to activate throttling.
    /// When queue depth exceeds this value, throttling is activated.
    pub critical_threshold: usize,
    /// Queue depth threshold to deactivate throttling.
    /// When queue depth falls below this value, throttling is deactivated.
    pub warning_threshold: usize,
    /// Factor to reduce max_concurrent_downloads by when throttling (0.0-1.0).
    /// A value of 0.5 uses 50% of the latest configured normal-slot limit.
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

impl ThrottleConfig {
    pub(crate) fn validate(&self) -> crate::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.warning_threshold == 0 || self.warning_threshold > self.critical_threshold {
            return Err(crate::Error::config(
                "Throttle warning threshold must be positive and no greater than the critical threshold",
            ));
        }
        if !self.reduction_factor.is_finite() || !(0.0..=1.0).contains(&self.reduction_factor) {
            return Err(crate::Error::config(
                "Throttle reduction factor must be finite and between 0 and 1",
            ));
        }
        if self.check_interval_ms == 0 {
            return Err(crate::Error::config(
                "Throttle check interval must be positive",
            ));
        }
        Ok(())
    }
}

/// Events emitted by the throttle controller.
#[derive(Debug, Clone)]
pub enum ThrottleEvent {
    /// Throttling has been activated due to high queue depth.
    ThrottleActivated {
        /// Current queue depth that triggered activation.
        queue_depth: usize,
        /// New reduced download limit.
        new_limit: usize,
        /// Original download limit before reduction.
        original_limit: usize,
    },
    /// Throttling has been deactivated as queue depth recovered.
    ThrottleDeactivated {
        /// Current queue depth when deactivated.
        queue_depth: usize,
        /// Restored download limit.
        restored_limit: usize,
    },
}

/// Applies a temporary reduction while preserving the configured download limit.
pub trait DownloadLimitAdjuster: Send + Sync {
    /// Apply a factor, or release the reduction with None. Return the configured
    /// and effective normal-slot limits from the same serialized update.
    fn set_throttle_factor(&self, factor: Option<f32>) -> (usize, usize);
}

impl DownloadLimitAdjuster for crate::downloader::DownloadManager {
    fn set_throttle_factor(&self, factor: Option<f32>) -> (usize, usize) {
        self.set_download_throttle(factor)
    }
}

/// Forced task aborts must release the reduction just like cooperative shutdown.
struct ThrottleReset<'a> {
    controller: &'a ThrottleController,
    adjuster: &'a dyn DownloadLimitAdjuster,
}

impl Drop for ThrottleReset<'_> {
    fn drop(&mut self) {
        if self.controller.is_throttled.swap(false, Ordering::SeqCst) {
            let (_, restored) = self.adjuster.set_throttle_factor(None);
            info!(
                restored_limit = restored,
                "Released download throttle on monitor shutdown"
            );
        }
    }
}

/// The Throttle Controller service.
/// Monitors pipeline queue depth and controls download throttling.
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

    /// Configured normal-slot limit at the most recent activation (a historical snapshot).
    pub fn original_max_downloads(&self) -> usize {
        self.original_max_downloads.load(Ordering::SeqCst)
    }

    /// Check queue depth and update throttle state.
    /// Returns Some(event) if a state transition occurred.
    pub fn check_and_update<A: DownloadLimitAdjuster + ?Sized>(
        &self,
        queue_depth: usize,
        adjuster: &A,
    ) -> Option<ThrottleEvent> {
        // If throttling is disabled, never emit events
        if !self.config.enabled {
            return None;
        }

        let currently_throttled = self.is_throttled.load(Ordering::SeqCst);

        if !currently_throttled && queue_depth > self.config.critical_threshold {
            // Activate throttling
            let (original, new_limit) =
                adjuster.set_throttle_factor(Some(self.config.reduction_factor));
            self.original_max_downloads
                .store(original, Ordering::SeqCst);

            self.is_throttled.store(true, Ordering::SeqCst);

            // Log the transition
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
            let (_, restored) = adjuster.set_throttle_factor(None);
            self.is_throttled.store(false, Ordering::SeqCst);

            // Log the transition
            info!(
                "Throttling deactivated: queue_depth={}, restoring max_concurrent_downloads to {}",
                queue_depth, restored
            );

            let event = ThrottleEvent::ThrottleDeactivated {
                queue_depth,
                restored_limit: restored,
            };
            let _ = self.event_tx.send(event.clone());
            return Some(event);
        }

        None
    }

    /// Start background monitoring of queue depth and return its owned task.
    pub fn start_monitoring(
        self: Arc<Self>,
        job_queue: Arc<JobQueue>,
        adjuster: Arc<dyn DownloadLimitAdjuster>,
        cancellation_token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(self.run_monitoring(job_queue, adjuster, cancellation_token))
    }

    /// Run monitoring until cancellation. PipelineManager owns this future so
    /// its shutdown barrier can join the task.
    pub(crate) async fn run_monitoring(
        self: Arc<Self>,
        job_queue: Arc<JobQueue>,
        adjuster: Arc<dyn DownloadLimitAdjuster>,
        cancellation_token: CancellationToken,
    ) {
        if !self.config.enabled {
            debug!("Throttle controller disabled, not starting monitoring");
            return;
        }

        let _reset = ThrottleReset {
            controller: &self,
            adjuster: adjuster.as_ref(),
        };
        let check_interval = Duration::from_millis(self.config.check_interval_ms.max(1));

        info!("Throttle controller monitoring started");

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Throttle controller monitoring shutting down");

                    break;
                }
                _ = tokio::time::sleep(check_interval) => {
                    let queue_depth = job_queue.depth();
                    self.check_and_update(queue_depth, adjuster.as_ref());
                }
            }
        }

        info!("Throttle controller monitoring stopped");
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
        configured: usize,
        limit: AtomicUsize,
    }

    impl MockAdjuster {
        fn new(initial: usize) -> Self {
            Self {
                configured: initial,
                limit: AtomicUsize::new(initial),
            }
        }

        fn get_max_concurrent_downloads(&self) -> usize {
            self.limit.load(Ordering::SeqCst)
        }
    }

    impl DownloadLimitAdjuster for MockAdjuster {
        fn set_throttle_factor(&self, factor: Option<f32>) -> (usize, usize) {
            let limit = factor.map_or(self.configured, |factor| {
                ((self.configured as f32 * factor) as usize).max(1)
            });
            self.limit.store(limit, Ordering::SeqCst);
            (self.configured, limit)
        }
    }

    #[test]
    fn test_throttle_controller_creation() {
        let controller = ThrottleController::with_defaults();
        assert!(!controller.is_throttled());
        assert!(!controller.is_enabled());
    }

    #[test]
    fn enabled_throttle_config_rejects_unrecoverable_or_expanding_limits() {
        let valid = ThrottleConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(valid.validate().is_ok());
        for invalid in [
            ThrottleConfig {
                warning_threshold: 0,
                ..valid.clone()
            },
            ThrottleConfig {
                warning_threshold: valid.critical_threshold + 1,
                ..valid.clone()
            },
            ThrottleConfig {
                reduction_factor: -0.1,
                ..valid.clone()
            },
            ThrottleConfig {
                reduction_factor: 1.1,
                ..valid.clone()
            },
            ThrottleConfig {
                reduction_factor: f32::NAN,
                ..valid.clone()
            },
            ThrottleConfig {
                check_interval_ms: 0,
                ..valid.clone()
            },
        ] {
            assert!(invalid.validate().is_err());
            assert!(
                ThrottleConfig {
                    enabled: false,
                    ..invalid
                }
                .validate()
                .is_ok()
            );
        }
    }

    #[test]
    fn test_throttle_disabled_no_events() {
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

    #[tokio::test]
    async fn aborted_throttle_monitor_releases_its_reduction() {
        let controller = Arc::new(ThrottleController::new(ThrottleConfig {
            enabled: true,
            critical_threshold: 1,
            warning_threshold: 1,
            check_interval_ms: 10,
            ..Default::default()
        }));
        let queue = Arc::new(JobQueue::new());
        for _ in 0..2 {
            queue
                .enqueue(crate::pipeline::Job::new("held", vec![], vec![], "", ""))
                .await
                .unwrap();
        }
        let adjuster = Arc::new(MockAdjuster::new(10));
        let mut events = controller.subscribe();
        let monitor =
            controller
                .clone()
                .start_monitoring(queue, adjuster.clone(), CancellationToken::new());
        tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        monitor.abort();
        assert!(monitor.await.unwrap_err().is_cancelled());
        assert_eq!(
            (
                controller.is_throttled(),
                adjuster.get_max_concurrent_downloads()
            ),
            (false, 10)
        );
    }
}
