//! Internal delivery counters populated by the web-push service.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Counters for web-push delivery outcomes. These are not an HTTP metrics export.
#[derive(Default)]
pub struct MetricsCollector {
    web_push_sent_total: AtomicU64,
    web_push_failed_total: AtomicU64,
    web_push_throttled_total: AtomicU64,
    web_push_stale_deleted_total: AtomicU64,
    web_push_skipped_backoff_total: AtomicU64,
    web_push_delivery_duration_total_ms: AtomicU64,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_web_push_sent(&self, duration_ms: u64) {
        self.web_push_sent_total.fetch_add(1, Ordering::Relaxed);
        self.web_push_delivery_duration_total_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    pub fn record_web_push_failed(&self) {
        self.web_push_failed_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_web_push_throttled(&self) {
        self.web_push_throttled_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_web_push_stale_deleted(&self) {
        self.web_push_stale_deleted_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_web_push_skipped_backoff(&self) {
        self.web_push_skipped_backoff_total
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Relaxed counters provide an approximate snapshot during concurrent delivery.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let sent = self.web_push_sent_total.load(Ordering::Relaxed);
        let duration = self
            .web_push_delivery_duration_total_ms
            .load(Ordering::Relaxed);
        MetricsSnapshot {
            web_push_sent_total: sent,
            web_push_failed_total: self.web_push_failed_total.load(Ordering::Relaxed),
            web_push_throttled_total: self.web_push_throttled_total.load(Ordering::Relaxed),
            web_push_stale_deleted_total: self.web_push_stale_deleted_total.load(Ordering::Relaxed),
            web_push_skipped_backoff_total: self
                .web_push_skipped_backoff_total
                .load(Ordering::Relaxed),
            web_push_delivery_duration_avg_ms: if sent == 0 {
                0.0
            } else {
                duration as f64 / sent as f64
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub web_push_sent_total: u64,
    pub web_push_failed_total: u64,
    pub web_push_throttled_total: u64,
    pub web_push_stale_deleted_total: u64,
    pub web_push_skipped_backoff_total: u64,
    pub web_push_delivery_duration_avg_ms: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_snapshot_preserves_outcomes_and_success_duration_average() {
        let collector = MetricsCollector::new();
        assert_eq!(collector.snapshot().web_push_delivery_duration_avg_ms, 0.0);
        collector.record_web_push_sent(10);
        collector.record_web_push_sent(30);
        collector.record_web_push_failed();
        collector.record_web_push_throttled();
        collector.record_web_push_stale_deleted();
        collector.record_web_push_skipped_backoff();
        let snapshot = collector.snapshot();
        assert_eq!(snapshot.web_push_sent_total, 2);
        assert_eq!(snapshot.web_push_failed_total, 1);
        assert_eq!(snapshot.web_push_throttled_total, 1);
        assert_eq!(snapshot.web_push_stale_deleted_total, 1);
        assert_eq!(snapshot.web_push_skipped_backoff_total, 1);
        assert_eq!(snapshot.web_push_delivery_duration_avg_ms, 20.0);
    }
}
