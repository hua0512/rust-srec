//! Metrics collector implementation.
//!
//! Collects and stores metrics for the streaming recorder system.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// Metrics collector for the streaming recorder system.
#[derive(Debug)]
pub struct MetricsCollector {
    // Download metrics
    active_downloads: AtomicU64,
    download_bytes_total: AtomicU64,
    download_duration_total_ms: AtomicU64,
    download_count: AtomicU64,
    download_errors: DashMap<String, AtomicU64>,

    // Pipeline metrics
    pipeline_queue_depth: DashMap<String, AtomicU64>,
    pipeline_jobs_total: DashMap<String, AtomicU64>,
    pipeline_job_duration_total_ms: DashMap<String, AtomicU64>,
    pipeline_job_count: DashMap<String, AtomicU64>,

    // Streamer metrics
    streamers_by_state: DashMap<String, AtomicU64>,
    streamer_errors: DashMap<String, AtomicU64>,

    // System metrics
    config_cache_hits: AtomicU64,
    config_cache_misses: AtomicU64,
    disk_space_bytes: DashMap<String, AtomicU64>,
    memory_usage_bytes: AtomicU64,

    // Web Push metrics
    web_push_sent_total: AtomicU64,
    web_push_failed_total: AtomicU64,
    web_push_throttled_total: AtomicU64,
    web_push_stale_deleted_total: AtomicU64,
    web_push_skipped_backoff_total: AtomicU64,
    web_push_delivery_duration_total_ms: AtomicU64,
    web_push_delivery_count: AtomicU64,

    // Custom labels
    labels: RwLock<HashMap<String, String>>,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new() -> Self {
        Self {
            active_downloads: AtomicU64::new(0),
            download_bytes_total: AtomicU64::new(0),
            download_duration_total_ms: AtomicU64::new(0),
            download_count: AtomicU64::new(0),
            download_errors: DashMap::new(),
            pipeline_queue_depth: DashMap::new(),
            pipeline_jobs_total: DashMap::new(),
            pipeline_job_duration_total_ms: DashMap::new(),
            pipeline_job_count: DashMap::new(),
            streamers_by_state: DashMap::new(),
            streamer_errors: DashMap::new(),
            config_cache_hits: AtomicU64::new(0),
            config_cache_misses: AtomicU64::new(0),
            disk_space_bytes: DashMap::new(),
            memory_usage_bytes: AtomicU64::new(0),
            web_push_sent_total: AtomicU64::new(0),
            web_push_failed_total: AtomicU64::new(0),
            web_push_throttled_total: AtomicU64::new(0),
            web_push_stale_deleted_total: AtomicU64::new(0),
            web_push_skipped_backoff_total: AtomicU64::new(0),
            web_push_delivery_duration_total_ms: AtomicU64::new(0),
            web_push_delivery_count: AtomicU64::new(0),
            labels: RwLock::new(HashMap::new()),
        }
    }

    /// Set a custom label for metrics.
    pub fn set_label(&self, key: impl Into<String>, value: impl Into<String>) {
        self.labels.write().insert(key.into(), value.into());
    }

    // ========== Download Metrics ==========

    /// Record a download started.
    pub fn record_download_started(&self) {
        self.active_downloads.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a download completed.
    pub fn record_download_completed(&self, bytes: u64, duration_ms: u64) {
        self.active_downloads.fetch_sub(1, Ordering::Relaxed);
        self.download_bytes_total
            .fetch_add(bytes, Ordering::Relaxed);
        self.download_duration_total_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
        self.download_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record download bytes (incremental).
    pub fn record_download_bytes(&self, bytes: u64) {
        self.download_bytes_total
            .fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record a download error.
    pub fn record_download_error(&self, error_type: impl Into<String>) {
        self.active_downloads.fetch_sub(1, Ordering::Relaxed);
        let error_type = error_type.into();
        self.download_errors
            .entry(error_type)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get active download count.
    pub fn active_downloads(&self) -> u64 {
        self.active_downloads.load(Ordering::Relaxed)
    }

    // ========== Pipeline Metrics ==========

    /// Set pipeline queue depth.
    pub fn set_pipeline_queue_depth(&self, worker_type: impl Into<String>, depth: u64) {
        let worker_type = worker_type.into();
        self.pipeline_queue_depth
            .entry(worker_type)
            .or_insert_with(|| AtomicU64::new(0))
            .store(depth, Ordering::Relaxed);
    }

    /// Record a pipeline job completed.
    pub fn record_pipeline_job_completed(
        &self,
        job_type: impl Into<String>,
        status: impl Into<String>,
        duration_ms: u64,
    ) {
        let job_type = job_type.into();
        let status = status.into();

        // Increment job count by status
        self.pipeline_jobs_total
            .entry(status)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);

        // Record duration
        self.pipeline_job_duration_total_ms
            .entry(job_type.clone())
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(duration_ms, Ordering::Relaxed);

        self.pipeline_job_count
            .entry(job_type)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    // ========== Streamer Metrics ==========

    /// Set streamer count by state.
    pub fn set_streamers_by_state(&self, state: impl Into<String>, count: u64) {
        let state = state.into();
        self.streamers_by_state
            .entry(state)
            .or_insert_with(|| AtomicU64::new(0))
            .store(count, Ordering::Relaxed);
    }

    /// Record a streamer error.
    pub fn record_streamer_error(&self, streamer_id: impl Into<String>) {
        let streamer_id = streamer_id.into();
        self.streamer_errors
            .entry(streamer_id)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Get live streamer count.
    pub fn live_streamers(&self) -> u64 {
        self.streamers_by_state
            .get("Live")
            .map(|v| v.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    // ========== System Metrics ==========

    /// Record a config cache hit.
    pub fn record_cache_hit(&self) {
        self.config_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a config cache miss.
    pub fn record_cache_miss(&self) {
        self.config_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Set disk space for a path.
    pub fn set_disk_space(&self, path: impl Into<String>, bytes: u64) {
        let path = path.into();
        self.disk_space_bytes
            .entry(path)
            .or_insert_with(|| AtomicU64::new(0))
            .store(bytes, Ordering::Relaxed);
    }

    /// Set memory usage.
    pub fn set_memory_usage(&self, bytes: u64) {
        self.memory_usage_bytes.store(bytes, Ordering::Relaxed);
    }

    // ========== Web Push Metrics ==========

    pub fn record_web_push_sent(&self, duration_ms: u64) {
        self.web_push_sent_total.fetch_add(1, Ordering::Relaxed);
        self.web_push_delivery_count.fetch_add(1, Ordering::Relaxed);
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

    // ========== Snapshot ==========

    /// Get a snapshot of all metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            active_downloads: self.active_downloads.load(Ordering::Relaxed),
            download_bytes_total: self.download_bytes_total.load(Ordering::Relaxed),
            download_duration_avg_ms: self.avg_download_duration_ms(),
            download_count: self.download_count.load(Ordering::Relaxed),
            download_errors: self
                .download_errors
                .iter()
                .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
                .collect(),
            pipeline_queue_depth: self
                .pipeline_queue_depth
                .iter()
                .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
                .collect(),
            pipeline_jobs_total: self
                .pipeline_jobs_total
                .iter()
                .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
                .collect(),
            pipeline_job_duration_avg_ms: self.avg_pipeline_job_duration_ms(),
            streamers_by_state: self
                .streamers_by_state
                .iter()
                .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
                .collect(),
            streamer_errors: self
                .streamer_errors
                .iter()
                .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
                .collect(),
            config_cache_hits: self.config_cache_hits.load(Ordering::Relaxed),
            config_cache_misses: self.config_cache_misses.load(Ordering::Relaxed),
            disk_space_bytes: self
                .disk_space_bytes
                .iter()
                .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
                .collect(),
            memory_usage_bytes: self.memory_usage_bytes.load(Ordering::Relaxed),
            web_push_sent_total: self.web_push_sent_total.load(Ordering::Relaxed),
            web_push_failed_total: self.web_push_failed_total.load(Ordering::Relaxed),
            web_push_throttled_total: self.web_push_throttled_total.load(Ordering::Relaxed),
            web_push_stale_deleted_total: self.web_push_stale_deleted_total.load(Ordering::Relaxed),
            web_push_skipped_backoff_total: self
                .web_push_skipped_backoff_total
                .load(Ordering::Relaxed),
            web_push_delivery_duration_avg_ms: self.avg_web_push_duration_ms(),
        }
    }

    fn avg_web_push_duration_ms(&self) -> f64 {
        let count = self.web_push_delivery_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let total = self
            .web_push_delivery_duration_total_ms
            .load(Ordering::Relaxed);
        total as f64 / count as f64
    }

    fn avg_download_duration_ms(&self) -> f64 {
        let count = self.download_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let total = self.download_duration_total_ms.load(Ordering::Relaxed);
        total as f64 / count as f64
    }

    fn avg_pipeline_job_duration_ms(&self) -> HashMap<String, f64> {
        self.pipeline_job_count
            .iter()
            .map(|e| {
                let job_type = e.key().clone();
                let count = e.value().load(Ordering::Relaxed);
                let total = self
                    .pipeline_job_duration_total_ms
                    .get(&job_type)
                    .map(|v| v.load(Ordering::Relaxed))
                    .unwrap_or(0);
                let avg = if count > 0 {
                    total as f64 / count as f64
                } else {
                    0.0
                };
                (job_type, avg)
            })
            .collect()
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// A snapshot of all metrics at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    // Download metrics
    pub active_downloads: u64,
    pub download_bytes_total: u64,
    pub download_duration_avg_ms: f64,
    pub download_count: u64,
    pub download_errors: HashMap<String, u64>,

    // Pipeline metrics
    pub pipeline_queue_depth: HashMap<String, u64>,
    pub pipeline_jobs_total: HashMap<String, u64>,
    pub pipeline_job_duration_avg_ms: HashMap<String, f64>,

    // Streamer metrics
    pub streamers_by_state: HashMap<String, u64>,
    pub streamer_errors: HashMap<String, u64>,

    // System metrics
    pub config_cache_hits: u64,
    pub config_cache_misses: u64,
    pub disk_space_bytes: HashMap<String, u64>,
    pub memory_usage_bytes: u64,

    // Web Push metrics
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
    fn test_metrics_collector_creation() {
        let collector = MetricsCollector::new();
        assert_eq!(collector.active_downloads(), 0);
    }

    #[test]
    fn test_download_metrics() {
        let collector = MetricsCollector::new();

        collector.record_download_started();
        assert_eq!(collector.active_downloads(), 1);

        collector.record_download_bytes(1024);
        collector.record_download_completed(2048, 5000);

        assert_eq!(collector.active_downloads(), 0);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.download_bytes_total, 3072);
        assert_eq!(snapshot.download_count, 1);
    }

    #[test]
    fn test_download_error_metrics() {
        let collector = MetricsCollector::new();

        collector.record_download_started();
        collector.record_download_error("network");
        collector.record_download_started();
        collector.record_download_error("network");
        collector.record_download_started();
        collector.record_download_error("timeout");

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.download_errors.get("network"), Some(&2));
        assert_eq!(snapshot.download_errors.get("timeout"), Some(&1));
    }

    #[test]
    fn test_pipeline_metrics() {
        let collector = MetricsCollector::new();

        collector.set_pipeline_queue_depth("cpu", 5);
        collector.set_pipeline_queue_depth("io", 10);
        collector.record_pipeline_job_completed("remux", "completed", 1000);
        collector.record_pipeline_job_completed("remux", "completed", 2000);
        collector.record_pipeline_job_completed("upload", "failed", 500);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.pipeline_queue_depth.get("cpu"), Some(&5));
        assert_eq!(snapshot.pipeline_queue_depth.get("io"), Some(&10));
        assert_eq!(snapshot.pipeline_jobs_total.get("completed"), Some(&2));
        assert_eq!(snapshot.pipeline_jobs_total.get("failed"), Some(&1));
    }

    #[test]
    fn test_streamer_metrics() {
        let collector = MetricsCollector::new();

        collector.set_streamers_by_state("Live", 5);
        collector.set_streamers_by_state("NotLive", 10);
        collector.record_streamer_error("streamer-1");
        collector.record_streamer_error("streamer-1");

        assert_eq!(collector.live_streamers(), 5);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.streamers_by_state.get("Live"), Some(&5));
        assert_eq!(snapshot.streamer_errors.get("streamer-1"), Some(&2));
    }

    #[test]
    fn test_system_metrics() {
        let collector = MetricsCollector::new();

        collector.record_cache_hit();
        collector.record_cache_hit();
        collector.record_cache_miss();
        collector.set_disk_space("/data", 1024 * 1024 * 1024);
        collector.set_memory_usage(512 * 1024 * 1024);

        let snapshot = collector.snapshot();
        assert_eq!(snapshot.config_cache_hits, 2);
        assert_eq!(snapshot.config_cache_misses, 1);
        assert_eq!(
            snapshot.disk_space_bytes.get("/data"),
            Some(&(1024 * 1024 * 1024))
        );
        assert_eq!(snapshot.memory_usage_bytes, 512 * 1024 * 1024);
    }
}
