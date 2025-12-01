//! Actor metrics for observability.
//!
//! This module provides metrics collection for individual actors and the scheduler,
//! including message processing latency, mailbox size, error counts, and lifecycle events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use tokio::sync::broadcast;

/// Metrics for an individual actor.
#[derive(Debug)]
pub struct ActorMetrics {
    /// Actor identifier.
    actor_id: String,
    /// Total messages processed.
    messages_processed: AtomicU64,
    /// Total errors encountered.
    errors: AtomicU64,
    /// Message processing latency samples (recent).
    latency_samples: RwLock<LatencySamples>,
    /// Current mailbox size.
    mailbox_size: AtomicU64,
    /// Maximum mailbox capacity.
    mailbox_capacity: u64,
    /// When the actor was spawned.
    spawned_at: Instant,
}

/// Recent latency samples for calculating statistics.
#[derive(Debug)]
struct LatencySamples {
    /// Ring buffer of recent latency samples (in microseconds).
    samples: Vec<u64>,
    /// Current write position.
    position: usize,
    /// Number of samples collected.
    count: u64,
    /// Sum of all samples for average calculation.
    sum: u64,
    /// Minimum latency observed.
    min: u64,
    /// Maximum latency observed.
    max: u64,
}

impl LatencySamples {
    fn new(capacity: usize) -> Self {
        Self {
            samples: vec![0; capacity],
            position: 0,
            count: 0,
            sum: 0,
            min: u64::MAX,
            max: 0,
        }
    }

    fn record(&mut self, latency_us: u64) {
        // Update ring buffer
        self.samples[self.position] = latency_us;
        self.position = (self.position + 1) % self.samples.len();

        // Update statistics
        self.count += 1;
        self.sum += latency_us;
        self.min = self.min.min(latency_us);
        self.max = self.max.max(latency_us);
    }

    fn average(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum as f64 / self.count as f64
        }
    }

    fn recent_average(&self) -> f64 {
        let filled = self.count.min(self.samples.len() as u64) as usize;
        if filled == 0 {
            return 0.0;
        }

        let sum: u64 = self.samples[..filled].iter().sum();
        sum as f64 / filled as f64
    }
}

impl ActorMetrics {
    /// Create new metrics for an actor.
    pub fn new(actor_id: impl Into<String>, mailbox_capacity: usize) -> Self {
        Self {
            actor_id: actor_id.into(),
            messages_processed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            latency_samples: RwLock::new(LatencySamples::new(100)), // Keep last 100 samples
            mailbox_size: AtomicU64::new(0),
            mailbox_capacity: mailbox_capacity as u64,
            spawned_at: Instant::now(),
        }
    }

    /// Get the actor ID.
    pub fn actor_id(&self) -> &str {
        &self.actor_id
    }

    /// Record a message being processed.
    pub fn record_message(&self, latency: Duration) {
        self.messages_processed.fetch_add(1, Ordering::Relaxed);
        let latency_us = latency.as_micros() as u64;
        self.latency_samples.write().record(latency_us);
    }

    /// Record an error.
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Update the current mailbox size.
    pub fn update_mailbox_size(&self, size: usize) {
        self.mailbox_size.store(size as u64, Ordering::Relaxed);
    }

    /// Get the total number of messages processed.
    pub fn messages_processed(&self) -> u64 {
        self.messages_processed.load(Ordering::Relaxed)
    }

    /// Get the total number of errors.
    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Get the current mailbox size.
    pub fn mailbox_size(&self) -> u64 {
        self.mailbox_size.load(Ordering::Relaxed)
    }

    /// Get the mailbox capacity.
    pub fn mailbox_capacity(&self) -> u64 {
        self.mailbox_capacity
    }

    /// Get the mailbox usage as a percentage (0.0 to 1.0).
    pub fn mailbox_usage(&self) -> f64 {
        if self.mailbox_capacity == 0 {
            return 0.0;
        }
        self.mailbox_size() as f64 / self.mailbox_capacity as f64
    }

    /// Get the average message processing latency in microseconds.
    pub fn average_latency_us(&self) -> f64 {
        self.latency_samples.read().average()
    }

    /// Get the recent average latency (last N samples) in microseconds.
    pub fn recent_latency_us(&self) -> f64 {
        self.latency_samples.read().recent_average()
    }

    /// Get the minimum observed latency in microseconds.
    pub fn min_latency_us(&self) -> u64 {
        let samples = self.latency_samples.read();
        if samples.count == 0 {
            0
        } else {
            samples.min
        }
    }

    /// Get the maximum observed latency in microseconds.
    pub fn max_latency_us(&self) -> u64 {
        self.latency_samples.read().max
    }

    /// Get the actor's uptime.
    pub fn uptime(&self) -> Duration {
        self.spawned_at.elapsed()
    }

    /// Get a snapshot of all metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let samples = self.latency_samples.read();
        MetricsSnapshot {
            actor_id: self.actor_id.clone(),
            messages_processed: self.messages_processed(),
            errors: self.errors(),
            mailbox_size: self.mailbox_size(),
            mailbox_capacity: self.mailbox_capacity,
            average_latency_us: samples.average(),
            recent_latency_us: samples.recent_average(),
            min_latency_us: if samples.count == 0 { 0 } else { samples.min },
            max_latency_us: samples.max,
            uptime: self.uptime(),
        }
    }
}

impl Clone for ActorMetrics {
    fn clone(&self) -> Self {
        // Create a new metrics instance with the same ID but fresh counters
        Self::new(self.actor_id.clone(), self.mailbox_capacity as usize)
    }
}

/// A snapshot of actor metrics at a point in time.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    /// Actor identifier.
    pub actor_id: String,
    /// Total messages processed.
    pub messages_processed: u64,
    /// Total errors encountered.
    pub errors: u64,
    /// Current mailbox size.
    pub mailbox_size: u64,
    /// Maximum mailbox capacity.
    pub mailbox_capacity: u64,
    /// Average message processing latency in microseconds.
    pub average_latency_us: f64,
    /// Recent average latency in microseconds.
    pub recent_latency_us: f64,
    /// Minimum observed latency in microseconds.
    pub min_latency_us: u64,
    /// Maximum observed latency in microseconds.
    pub max_latency_us: u64,
    /// Actor uptime.
    pub uptime: Duration,
}

impl MetricsSnapshot {
    /// Get the error rate (errors / messages processed).
    pub fn error_rate(&self) -> f64 {
        if self.messages_processed == 0 {
            0.0
        } else {
            self.errors as f64 / self.messages_processed as f64
        }
    }

    /// Get the message throughput (messages per second).
    pub fn throughput(&self) -> f64 {
        let secs = self.uptime.as_secs_f64();
        if secs == 0.0 {
            0.0
        } else {
            self.messages_processed as f64 / secs
        }
    }
}

/// Shared metrics handle that can be cloned and passed to actors.
pub type SharedActorMetrics = Arc<ActorMetrics>;

/// Create a new shared metrics instance.
pub fn create_metrics(actor_id: impl Into<String>, mailbox_capacity: usize) -> SharedActorMetrics {
    Arc::new(ActorMetrics::new(actor_id, mailbox_capacity))
}

// ============================================================================
// Scheduler Metrics (System-wide)
// ============================================================================

/// Lifecycle events emitted by the scheduler.
#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    /// An actor was spawned.
    ActorSpawned {
        actor_id: String,
        actor_type: ActorType,
        timestamp: Instant,
    },
    /// An actor was stopped.
    ActorStopped {
        actor_id: String,
        actor_type: ActorType,
        graceful: bool,
        timestamp: Instant,
    },
    /// An actor crashed.
    ActorCrashed {
        actor_id: String,
        actor_type: ActorType,
        error: String,
        timestamp: Instant,
    },
    /// An actor was restarted.
    ActorRestarted {
        actor_id: String,
        actor_type: ActorType,
        restart_count: u32,
        timestamp: Instant,
    },
    /// Scheduler shutdown initiated.
    ShutdownInitiated { timestamp: Instant },
    /// Scheduler shutdown completed.
    ShutdownCompleted {
        graceful_stops: usize,
        forced_terminations: usize,
        timestamp: Instant,
    },
}

/// Type of actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorType {
    /// Streamer actor.
    Streamer,
    /// Platform actor.
    Platform,
}

impl std::fmt::Display for ActorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActorType::Streamer => write!(f, "streamer"),
            ActorType::Platform => write!(f, "platform"),
        }
    }
}

/// System-wide metrics for the scheduler.
///
/// Tracks actor counts, message throughput, and emits lifecycle events.
#[derive(Debug)]
pub struct SchedulerMetrics {
    /// Number of streamer actors.
    streamer_count: AtomicU64,
    /// Number of platform actors.
    platform_count: AtomicU64,
    /// Total messages processed across all actors.
    total_messages_processed: AtomicU64,
    /// Total errors across all actors.
    total_errors: AtomicU64,
    /// Actor spawn count.
    actors_spawned: AtomicU64,
    /// Actor stop count.
    actors_stopped: AtomicU64,
    /// Actor crash count.
    actors_crashed: AtomicU64,
    /// Actor restart count.
    actors_restarted: AtomicU64,
    /// When the scheduler started.
    started_at: Instant,
    /// Lifecycle event broadcaster.
    event_sender: broadcast::Sender<LifecycleEvent>,
    /// Per-actor metrics storage.
    actor_metrics: RwLock<HashMap<String, SharedActorMetrics>>,
}

impl SchedulerMetrics {
    /// Create new scheduler metrics.
    pub fn new() -> Self {
        let (event_sender, _) = broadcast::channel(256);
        Self {
            streamer_count: AtomicU64::new(0),
            platform_count: AtomicU64::new(0),
            total_messages_processed: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            actors_spawned: AtomicU64::new(0),
            actors_stopped: AtomicU64::new(0),
            actors_crashed: AtomicU64::new(0),
            actors_restarted: AtomicU64::new(0),
            started_at: Instant::now(),
            event_sender,
            actor_metrics: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribe to lifecycle events.
    pub fn subscribe(&self) -> broadcast::Receiver<LifecycleEvent> {
        self.event_sender.subscribe()
    }

    /// Record an actor being spawned.
    pub fn record_actor_spawned(&self, actor_id: &str, actor_type: ActorType) {
        self.actors_spawned.fetch_add(1, Ordering::Relaxed);
        match actor_type {
            ActorType::Streamer => {
                self.streamer_count.fetch_add(1, Ordering::Relaxed);
            }
            ActorType::Platform => {
                self.platform_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        let event = LifecycleEvent::ActorSpawned {
            actor_id: actor_id.to_string(),
            actor_type,
            timestamp: Instant::now(),
        };
        let _ = self.event_sender.send(event);
    }

    /// Record an actor being stopped.
    pub fn record_actor_stopped(&self, actor_id: &str, actor_type: ActorType, graceful: bool) {
        self.actors_stopped.fetch_add(1, Ordering::Relaxed);
        match actor_type {
            ActorType::Streamer => {
                self.streamer_count.fetch_sub(1, Ordering::Relaxed);
            }
            ActorType::Platform => {
                self.platform_count.fetch_sub(1, Ordering::Relaxed);
            }
        }

        // Remove actor metrics
        self.actor_metrics.write().remove(actor_id);

        let event = LifecycleEvent::ActorStopped {
            actor_id: actor_id.to_string(),
            actor_type,
            graceful,
            timestamp: Instant::now(),
        };
        let _ = self.event_sender.send(event);
    }

    /// Record an actor crash.
    pub fn record_actor_crashed(&self, actor_id: &str, actor_type: ActorType, error: &str) {
        self.actors_crashed.fetch_add(1, Ordering::Relaxed);
        self.total_errors.fetch_add(1, Ordering::Relaxed);

        let event = LifecycleEvent::ActorCrashed {
            actor_id: actor_id.to_string(),
            actor_type,
            error: error.to_string(),
            timestamp: Instant::now(),
        };
        let _ = self.event_sender.send(event);
    }

    /// Record an actor restart.
    pub fn record_actor_restarted(&self, actor_id: &str, actor_type: ActorType, restart_count: u32) {
        self.actors_restarted.fetch_add(1, Ordering::Relaxed);

        let event = LifecycleEvent::ActorRestarted {
            actor_id: actor_id.to_string(),
            actor_type,
            restart_count,
            timestamp: Instant::now(),
        };
        let _ = self.event_sender.send(event);
    }

    /// Record a message being processed.
    pub fn record_message_processed(&self) {
        self.total_messages_processed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record an error.
    pub fn record_error(&self) {
        self.total_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record shutdown initiated.
    pub fn record_shutdown_initiated(&self) {
        let event = LifecycleEvent::ShutdownInitiated {
            timestamp: Instant::now(),
        };
        let _ = self.event_sender.send(event);
    }

    /// Record shutdown completed.
    pub fn record_shutdown_completed(&self, graceful_stops: usize, forced_terminations: usize) {
        let event = LifecycleEvent::ShutdownCompleted {
            graceful_stops,
            forced_terminations,
            timestamp: Instant::now(),
        };
        let _ = self.event_sender.send(event);
    }

    /// Register actor metrics for tracking.
    pub fn register_actor_metrics(&self, actor_id: &str, metrics: SharedActorMetrics) {
        self.actor_metrics.write().insert(actor_id.to_string(), metrics);
    }

    /// Get actor metrics by ID.
    pub fn get_actor_metrics(&self, actor_id: &str) -> Option<SharedActorMetrics> {
        self.actor_metrics.read().get(actor_id).cloned()
    }

    /// Update mailbox size for an actor.
    pub fn update_mailbox_size(&self, actor_id: &str, size: usize) {
        if let Some(metrics) = self.actor_metrics.read().get(actor_id) {
            metrics.update_mailbox_size(size);
        }
    }

    /// Get the number of streamer actors.
    pub fn streamer_count(&self) -> u64 {
        self.streamer_count.load(Ordering::Relaxed)
    }

    /// Get the number of platform actors.
    pub fn platform_count(&self) -> u64 {
        self.platform_count.load(Ordering::Relaxed)
    }

    /// Get the total actor count.
    pub fn total_actor_count(&self) -> u64 {
        self.streamer_count() + self.platform_count()
    }

    /// Get the total messages processed.
    pub fn total_messages_processed(&self) -> u64 {
        self.total_messages_processed.load(Ordering::Relaxed)
    }

    /// Get the total errors.
    pub fn total_errors(&self) -> u64 {
        self.total_errors.load(Ordering::Relaxed)
    }

    /// Get the scheduler uptime.
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Get the message throughput (messages per second).
    pub fn message_throughput(&self) -> f64 {
        let secs = self.uptime().as_secs_f64();
        if secs == 0.0 {
            0.0
        } else {
            self.total_messages_processed() as f64 / secs
        }
    }

    /// Get the error rate (errors / messages processed).
    pub fn error_rate(&self) -> f64 {
        let messages = self.total_messages_processed();
        if messages == 0 {
            0.0
        } else {
            self.total_errors() as f64 / messages as f64
        }
    }

    /// Get a snapshot of scheduler metrics.
    pub fn snapshot(&self) -> SchedulerMetricsSnapshot {
        SchedulerMetricsSnapshot {
            streamer_count: self.streamer_count(),
            platform_count: self.platform_count(),
            total_messages_processed: self.total_messages_processed(),
            total_errors: self.total_errors(),
            actors_spawned: self.actors_spawned.load(Ordering::Relaxed),
            actors_stopped: self.actors_stopped.load(Ordering::Relaxed),
            actors_crashed: self.actors_crashed.load(Ordering::Relaxed),
            actors_restarted: self.actors_restarted.load(Ordering::Relaxed),
            uptime: self.uptime(),
            message_throughput: self.message_throughput(),
            error_rate: self.error_rate(),
        }
    }
}

impl Default for SchedulerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A snapshot of scheduler metrics at a point in time.
#[derive(Debug, Clone)]
pub struct SchedulerMetricsSnapshot {
    /// Number of streamer actors.
    pub streamer_count: u64,
    /// Number of platform actors.
    pub platform_count: u64,
    /// Total messages processed.
    pub total_messages_processed: u64,
    /// Total errors.
    pub total_errors: u64,
    /// Total actors spawned.
    pub actors_spawned: u64,
    /// Total actors stopped.
    pub actors_stopped: u64,
    /// Total actors crashed.
    pub actors_crashed: u64,
    /// Total actors restarted.
    pub actors_restarted: u64,
    /// Scheduler uptime.
    pub uptime: Duration,
    /// Message throughput (messages per second).
    pub message_throughput: f64,
    /// Error rate.
    pub error_rate: f64,
}

impl SchedulerMetricsSnapshot {
    /// Get the total actor count.
    pub fn total_actor_count(&self) -> u64 {
        self.streamer_count + self.platform_count
    }
}

/// Shared scheduler metrics handle.
pub type SharedSchedulerMetrics = Arc<SchedulerMetrics>;

/// Create a new shared scheduler metrics instance.
pub fn create_scheduler_metrics() -> SharedSchedulerMetrics {
    Arc::new(SchedulerMetrics::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_actor_metrics_new() {
        let metrics = ActorMetrics::new("test-actor", 256);
        assert_eq!(metrics.actor_id(), "test-actor");
        assert_eq!(metrics.messages_processed(), 0);
        assert_eq!(metrics.errors(), 0);
        assert_eq!(metrics.mailbox_capacity(), 256);
    }

    #[test]
    fn test_actor_metrics_record_message() {
        let metrics = ActorMetrics::new("test-actor", 256);

        metrics.record_message(Duration::from_micros(100));
        assert_eq!(metrics.messages_processed(), 1);

        metrics.record_message(Duration::from_micros(200));
        assert_eq!(metrics.messages_processed(), 2);

        // Average should be 150
        assert!((metrics.average_latency_us() - 150.0).abs() < 0.1);
    }

    #[test]
    fn test_actor_metrics_record_error() {
        let metrics = ActorMetrics::new("test-actor", 256);

        metrics.record_error();
        assert_eq!(metrics.errors(), 1);

        metrics.record_error();
        assert_eq!(metrics.errors(), 2);
    }

    #[test]
    fn test_actor_metrics_mailbox_size() {
        let metrics = ActorMetrics::new("test-actor", 256);

        metrics.update_mailbox_size(100);
        assert_eq!(metrics.mailbox_size(), 100);
        assert!((metrics.mailbox_usage() - (100.0 / 256.0)).abs() < 0.01);
    }

    #[test]
    fn test_actor_metrics_latency_stats() {
        let metrics = ActorMetrics::new("test-actor", 256);

        metrics.record_message(Duration::from_micros(50));
        metrics.record_message(Duration::from_micros(100));
        metrics.record_message(Duration::from_micros(150));

        assert_eq!(metrics.min_latency_us(), 50);
        assert_eq!(metrics.max_latency_us(), 150);
        assert!((metrics.average_latency_us() - 100.0).abs() < 0.1);
    }

    #[test]
    fn test_metrics_snapshot() {
        let metrics = ActorMetrics::new("test-actor", 256);

        metrics.record_message(Duration::from_micros(100));
        metrics.record_message(Duration::from_micros(200));
        metrics.record_error();
        metrics.update_mailbox_size(50);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.actor_id, "test-actor");
        assert_eq!(snapshot.messages_processed, 2);
        assert_eq!(snapshot.errors, 1);
        assert_eq!(snapshot.mailbox_size, 50);
        assert_eq!(snapshot.mailbox_capacity, 256);
        assert!((snapshot.error_rate() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_create_shared_metrics() {
        let metrics = create_metrics("test-actor", 256);
        assert_eq!(metrics.actor_id(), "test-actor");

        // Can be cloned
        let metrics2 = Arc::clone(&metrics);
        metrics.record_message(Duration::from_micros(100));
        assert_eq!(metrics2.messages_processed(), 1);
    }

    // ========== SchedulerMetrics Tests ==========

    #[test]
    fn test_scheduler_metrics_new() {
        let metrics = SchedulerMetrics::new();
        assert_eq!(metrics.streamer_count(), 0);
        assert_eq!(metrics.platform_count(), 0);
        assert_eq!(metrics.total_messages_processed(), 0);
        assert_eq!(metrics.total_errors(), 0);
    }

    #[test]
    fn test_scheduler_metrics_actor_spawned() {
        let metrics = SchedulerMetrics::new();

        metrics.record_actor_spawned("streamer-1", ActorType::Streamer);
        assert_eq!(metrics.streamer_count(), 1);
        assert_eq!(metrics.platform_count(), 0);
        assert_eq!(metrics.total_actor_count(), 1);

        metrics.record_actor_spawned("platform-1", ActorType::Platform);
        assert_eq!(metrics.streamer_count(), 1);
        assert_eq!(metrics.platform_count(), 1);
        assert_eq!(metrics.total_actor_count(), 2);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.actors_spawned, 2);
    }

    #[test]
    fn test_scheduler_metrics_actor_stopped() {
        let metrics = SchedulerMetrics::new();

        metrics.record_actor_spawned("streamer-1", ActorType::Streamer);
        metrics.record_actor_spawned("streamer-2", ActorType::Streamer);
        assert_eq!(metrics.streamer_count(), 2);

        metrics.record_actor_stopped("streamer-1", ActorType::Streamer, true);
        assert_eq!(metrics.streamer_count(), 1);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.actors_stopped, 1);
    }

    #[test]
    fn test_scheduler_metrics_actor_crashed() {
        let metrics = SchedulerMetrics::new();

        metrics.record_actor_crashed("streamer-1", ActorType::Streamer, "test error");
        assert_eq!(metrics.total_errors(), 1);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.actors_crashed, 1);
    }

    #[test]
    fn test_scheduler_metrics_actor_restarted() {
        let metrics = SchedulerMetrics::new();

        metrics.record_actor_restarted("streamer-1", ActorType::Streamer, 1);
        metrics.record_actor_restarted("streamer-1", ActorType::Streamer, 2);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.actors_restarted, 2);
    }

    #[test]
    fn test_scheduler_metrics_message_throughput() {
        let metrics = SchedulerMetrics::new();

        metrics.record_message_processed();
        metrics.record_message_processed();
        metrics.record_message_processed();

        assert_eq!(metrics.total_messages_processed(), 3);
        // Throughput should be positive (messages / time)
        assert!(metrics.message_throughput() > 0.0);
    }

    #[test]
    fn test_scheduler_metrics_error_rate() {
        let metrics = SchedulerMetrics::new();

        metrics.record_message_processed();
        metrics.record_message_processed();
        metrics.record_error();

        // 1 error / 2 messages = 0.5
        assert!((metrics.error_rate() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_scheduler_metrics_register_actor_metrics() {
        let scheduler_metrics = SchedulerMetrics::new();
        let actor_metrics = create_metrics("test-actor", 256);

        scheduler_metrics.register_actor_metrics("test-actor", actor_metrics);

        let retrieved = scheduler_metrics.get_actor_metrics("test-actor");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().actor_id(), "test-actor");
    }

    #[test]
    fn test_scheduler_metrics_update_mailbox_size() {
        let scheduler_metrics = SchedulerMetrics::new();
        let actor_metrics = create_metrics("test-actor", 256);

        scheduler_metrics.register_actor_metrics("test-actor", Arc::clone(&actor_metrics));
        scheduler_metrics.update_mailbox_size("test-actor", 100);

        assert_eq!(actor_metrics.mailbox_size(), 100);
    }

    #[tokio::test]
    async fn test_scheduler_metrics_lifecycle_events() {
        let metrics = SchedulerMetrics::new();
        let mut receiver = metrics.subscribe();

        metrics.record_actor_spawned("streamer-1", ActorType::Streamer);

        // Should receive the spawn event
        let event = receiver.try_recv();
        assert!(event.is_ok());
        match event.unwrap() {
            LifecycleEvent::ActorSpawned { actor_id, actor_type, .. } => {
                assert_eq!(actor_id, "streamer-1");
                assert_eq!(actor_type, ActorType::Streamer);
            }
            _ => panic!("Expected ActorSpawned event"),
        }
    }

    #[test]
    fn test_scheduler_metrics_snapshot() {
        let metrics = SchedulerMetrics::new();

        metrics.record_actor_spawned("streamer-1", ActorType::Streamer);
        metrics.record_actor_spawned("platform-1", ActorType::Platform);
        metrics.record_message_processed();
        metrics.record_message_processed();
        metrics.record_error();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.streamer_count, 1);
        assert_eq!(snapshot.platform_count, 1);
        assert_eq!(snapshot.total_messages_processed, 2);
        assert_eq!(snapshot.total_errors, 1);
        assert_eq!(snapshot.actors_spawned, 2);
        assert_eq!(snapshot.total_actor_count(), 2);
    }

    #[test]
    fn test_create_scheduler_metrics() {
        let metrics = create_scheduler_metrics();
        assert_eq!(metrics.streamer_count(), 0);

        // Can be cloned
        let metrics2 = Arc::clone(&metrics);
        metrics.record_actor_spawned("test", ActorType::Streamer);
        assert_eq!(metrics2.streamer_count(), 1);
    }

    #[test]
    fn test_actor_type_display() {
        assert_eq!(format!("{}", ActorType::Streamer), "streamer");
        assert_eq!(format!("{}", ActorType::Platform), "platform");
    }
}
