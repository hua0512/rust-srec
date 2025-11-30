use std::sync::atomic::{AtomicU64, Ordering};
use tracing::info;

/// Performance metrics for HLS pipeline
///
/// Tracks various performance counters for observability and tuning.
/// All counters use atomic operations for thread-safe access.
#[derive(Debug, Default)]
pub struct PerformanceMetrics {
    // Download metrics
    /// Total number of segment downloads
    pub downloads_total: AtomicU64,
    /// Total bytes downloaded
    pub download_bytes_total: AtomicU64,
    /// Sum of download latencies in milliseconds
    pub download_latency_sum_ms: AtomicU64,
    /// Total number of download errors
    pub download_errors: AtomicU64,

    // HTTP version tracking
    /// Number of HTTP/2 requests
    pub http2_requests: AtomicU64,
    /// Number of HTTP/1.x requests
    pub http1_requests: AtomicU64,

    // Decryption metrics
    /// Total number of decryption operations
    pub decryptions_total: AtomicU64,
    /// Sum of decryption times in milliseconds
    pub decryption_time_sum_ms: AtomicU64,
    /// Total bytes decrypted
    pub decryption_bytes_total: AtomicU64,

    // Cache metrics
    /// Number of cache hits
    pub cache_hits: AtomicU64,
    /// Number of cache misses
    pub cache_misses: AtomicU64,

    // Buffer pool metrics
    /// Number of buffer allocations (new buffers created)
    pub buffer_allocations: AtomicU64,
    /// Number of buffer reuses (from pool)
    pub buffer_reuses: AtomicU64,

    // Prefetch metrics
    /// Number of prefetch operations initiated
    pub prefetch_initiated: AtomicU64,
    /// Number of prefetched segments actually used
    pub prefetch_used: AtomicU64,
}

impl PerformanceMetrics {
    /// Create a new PerformanceMetrics instance with all counters at zero
    pub fn new() -> Self {
        Self::default()
    }

    // --- Download metrics recording ---

    /// Record a successful download
    pub fn record_download(&self, bytes: u64, latency_ms: u64) {
        self.downloads_total.fetch_add(1, Ordering::Relaxed);
        self.download_bytes_total
            .fetch_add(bytes, Ordering::Relaxed);
        self.download_latency_sum_ms
            .fetch_add(latency_ms, Ordering::Relaxed);
    }

    /// Record a download error
    pub fn record_download_error(&self) {
        self.download_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Record HTTP version used for a request
    pub fn record_http_version(&self, is_http2: bool) {
        if is_http2 {
            self.http2_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.http1_requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    // --- Decryption metrics recording ---

    /// Record a decryption operation
    pub fn record_decryption(&self, bytes: u64, duration_ms: u64) {
        self.decryptions_total.fetch_add(1, Ordering::Relaxed);
        self.decryption_bytes_total
            .fetch_add(bytes, Ordering::Relaxed);
        self.decryption_time_sum_ms
            .fetch_add(duration_ms, Ordering::Relaxed);
    }

    // --- Cache metrics recording ---

    /// Record a cache hit
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a cache miss
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    // --- Buffer pool metrics recording ---

    /// Record a buffer allocation (new buffer created)
    pub fn record_buffer_allocation(&self) {
        self.buffer_allocations.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a buffer reuse (from pool)
    pub fn record_buffer_reuse(&self) {
        self.buffer_reuses.fetch_add(1, Ordering::Relaxed);
    }

    // --- Prefetch metrics recording ---

    /// Record a prefetch initiation
    pub fn record_prefetch_initiated(&self) {
        self.prefetch_initiated.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a prefetched segment being used
    pub fn record_prefetch_used(&self) {
        self.prefetch_used.fetch_add(1, Ordering::Relaxed);
    }

    // --- Helper methods for computed metrics ---

    /// Get average download latency in milliseconds
    ///
    /// Returns None if no downloads have been recorded
    pub fn average_download_latency_ms(&self) -> Option<f64> {
        let total = self.downloads_total.load(Ordering::Relaxed);
        if total == 0 {
            return None;
        }
        let sum = self.download_latency_sum_ms.load(Ordering::Relaxed);
        Some(sum as f64 / total as f64)
    }

    /// Get average throughput in bytes per second
    ///
    /// Returns None if no downloads have been recorded or total latency is zero
    pub fn average_throughput(&self) -> Option<f64> {
        let total_bytes = self.download_bytes_total.load(Ordering::Relaxed);
        let total_latency_ms = self.download_latency_sum_ms.load(Ordering::Relaxed);

        if total_latency_ms == 0 {
            return None;
        }

        // Convert ms to seconds for bytes/sec calculation
        let total_latency_sec = total_latency_ms as f64 / 1000.0;
        Some(total_bytes as f64 / total_latency_sec)
    }

    /// Get cache hit rate as a percentage (0.0 to 1.0)
    ///
    /// Returns 0.0 if no cache operations have been recorded
    pub fn cache_hit_rate(&self) -> f64 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;

        if total == 0 {
            return 0.0;
        }

        hits as f64 / total as f64
    }

    /// Get buffer pool reuse rate as a percentage (0.0 to 1.0)
    ///
    /// Returns 0.0 if no buffer operations have been recorded
    pub fn buffer_reuse_rate(&self) -> f64 {
        let allocations = self.buffer_allocations.load(Ordering::Relaxed);
        let reuses = self.buffer_reuses.load(Ordering::Relaxed);
        let total = allocations + reuses;

        if total == 0 {
            return 0.0;
        }

        reuses as f64 / total as f64
    }

    /// Get prefetch effectiveness rate as a percentage (0.0 to 1.0)
    ///
    /// Returns 0.0 if no prefetch operations have been initiated
    pub fn prefetch_effectiveness(&self) -> f64 {
        let initiated = self.prefetch_initiated.load(Ordering::Relaxed);
        let used = self.prefetch_used.load(Ordering::Relaxed);

        if initiated == 0 {
            return 0.0;
        }

        used as f64 / initiated as f64
    }

    /// Get HTTP/2 usage rate as a percentage (0.0 to 1.0)
    ///
    /// Returns 0.0 if no HTTP requests have been recorded
    pub fn http2_rate(&self) -> f64 {
        let http2 = self.http2_requests.load(Ordering::Relaxed);
        let http1 = self.http1_requests.load(Ordering::Relaxed);
        let total = http2 + http1;

        if total == 0 {
            return 0.0;
        }

        http2 as f64 / total as f64
    }

    /// Log a performance summary using tracing
    ///
    /// Logs key metrics including download stats, cache performance,
    /// and resource utilization.
    pub fn log_summary(&self) {
        let downloads = self.downloads_total.load(Ordering::Relaxed);
        let download_bytes = self.download_bytes_total.load(Ordering::Relaxed);
        let download_errors = self.download_errors.load(Ordering::Relaxed);
        let http2 = self.http2_requests.load(Ordering::Relaxed);
        let http1 = self.http1_requests.load(Ordering::Relaxed);
        let decryptions = self.decryptions_total.load(Ordering::Relaxed);
        let decryption_bytes = self.decryption_bytes_total.load(Ordering::Relaxed);
        let cache_hits = self.cache_hits.load(Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(Ordering::Relaxed);
        let buffer_allocs = self.buffer_allocations.load(Ordering::Relaxed);
        let buffer_reuses = self.buffer_reuses.load(Ordering::Relaxed);
        let prefetch_init = self.prefetch_initiated.load(Ordering::Relaxed);
        let prefetch_used = self.prefetch_used.load(Ordering::Relaxed);

        let avg_latency = self
            .average_download_latency_ms()
            .map(|l| format!("{:.2}ms", l))
            .unwrap_or_else(|| "N/A".to_string());

        let avg_throughput = self
            .average_throughput()
            .map(format_bytes_per_sec)
            .unwrap_or_else(|| "N/A".to_string());

        let cache_rate = self.cache_hit_rate() * 100.0;
        let buffer_rate = self.buffer_reuse_rate() * 100.0;
        let prefetch_rate = self.prefetch_effectiveness() * 100.0;
        let http2_rate = self.http2_rate() * 100.0;

        info!(
            downloads = downloads,
            download_bytes = download_bytes,
            download_errors = download_errors,
            avg_latency = %avg_latency,
            avg_throughput = %avg_throughput,
            http2_requests = http2,
            http1_requests = http1,
            http2_rate = format!("{:.1}%", http2_rate),
            decryptions = decryptions,
            decryption_bytes = decryption_bytes,
            cache_hits = cache_hits,
            cache_misses = cache_misses,
            cache_hit_rate = format!("{:.1}%", cache_rate),
            buffer_allocations = buffer_allocs,
            buffer_reuses = buffer_reuses,
            buffer_reuse_rate = format!("{:.1}%", buffer_rate),
            prefetch_initiated = prefetch_init,
            prefetch_used = prefetch_used,
            prefetch_effectiveness = format!("{:.1}%", prefetch_rate),
            "HLS Performance Summary"
        );
    }

    /// Get a snapshot of all current metric values
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            downloads_total: self.downloads_total.load(Ordering::Relaxed),
            download_bytes_total: self.download_bytes_total.load(Ordering::Relaxed),
            download_latency_sum_ms: self.download_latency_sum_ms.load(Ordering::Relaxed),
            download_errors: self.download_errors.load(Ordering::Relaxed),
            http2_requests: self.http2_requests.load(Ordering::Relaxed),
            http1_requests: self.http1_requests.load(Ordering::Relaxed),
            decryptions_total: self.decryptions_total.load(Ordering::Relaxed),
            decryption_time_sum_ms: self.decryption_time_sum_ms.load(Ordering::Relaxed),
            decryption_bytes_total: self.decryption_bytes_total.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            buffer_allocations: self.buffer_allocations.load(Ordering::Relaxed),
            buffer_reuses: self.buffer_reuses.load(Ordering::Relaxed),
            prefetch_initiated: self.prefetch_initiated.load(Ordering::Relaxed),
            prefetch_used: self.prefetch_used.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time snapshot of all metrics values
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub downloads_total: u64,
    pub download_bytes_total: u64,
    pub download_latency_sum_ms: u64,
    pub download_errors: u64,
    pub http2_requests: u64,
    pub http1_requests: u64,
    pub decryptions_total: u64,
    pub decryption_time_sum_ms: u64,
    pub decryption_bytes_total: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub buffer_allocations: u64,
    pub buffer_reuses: u64,
    pub prefetch_initiated: u64,
    pub prefetch_used: u64,
}

/// Format bytes per second in human-readable form
fn format_bytes_per_sec(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1_000_000_000.0 {
        format!("{:.2} GB/s", bytes_per_sec / 1_000_000_000.0)
    } else if bytes_per_sec >= 1_000_000.0 {
        format!("{:.2} MB/s", bytes_per_sec / 1_000_000.0)
    } else if bytes_per_sec >= 1_000.0 {
        format!("{:.2} KB/s", bytes_per_sec / 1_000.0)
    } else {
        format!("{:.2} B/s", bytes_per_sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_default_metrics() {
        let metrics = PerformanceMetrics::default();
        assert_eq!(metrics.downloads_total.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.download_bytes_total.load(Ordering::Relaxed), 0);
        assert!(metrics.average_download_latency_ms().is_none());
        assert!(metrics.average_throughput().is_none());
        assert_eq!(metrics.cache_hit_rate(), 0.0);
    }

    #[test]
    fn test_record_download() {
        let metrics = PerformanceMetrics::new();
        metrics.record_download(1000, 50);
        metrics.record_download(2000, 100);

        assert_eq!(metrics.downloads_total.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.download_bytes_total.load(Ordering::Relaxed), 3000);
        assert_eq!(metrics.download_latency_sum_ms.load(Ordering::Relaxed), 150);
    }

    #[test]
    fn test_average_latency() {
        let metrics = PerformanceMetrics::new();
        metrics.record_download(1000, 100);
        metrics.record_download(1000, 200);
        metrics.record_download(1000, 300);

        let avg = metrics.average_download_latency_ms().unwrap();
        assert!((avg - 200.0).abs() < 0.001);
    }

    #[test]
    fn test_average_throughput() {
        let metrics = PerformanceMetrics::new();
        // 1000 bytes in 100ms = 10000 bytes/sec
        metrics.record_download(1000, 100);

        let throughput = metrics.average_throughput().unwrap();
        assert!((throughput - 10000.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_hit_rate() {
        let metrics = PerformanceMetrics::new();
        metrics.record_cache_hit();
        metrics.record_cache_hit();
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        let rate = metrics.cache_hit_rate();
        assert!((rate - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_buffer_reuse_rate() {
        let metrics = PerformanceMetrics::new();
        metrics.record_buffer_allocation();
        metrics.record_buffer_reuse();
        metrics.record_buffer_reuse();
        metrics.record_buffer_reuse();

        let rate = metrics.buffer_reuse_rate();
        assert!((rate - 0.75).abs() < 0.001);
    }

    #[test]
    fn test_http2_rate() {
        let metrics = PerformanceMetrics::new();

        // Test with no requests - should return 0.0
        assert_eq!(metrics.http2_rate(), 0.0);

        // Record 3 HTTP/2 requests and 1 HTTP/1.x request
        metrics.record_http_version(true); // HTTP/2
        metrics.record_http_version(true); // HTTP/2
        metrics.record_http_version(true); // HTTP/2
        metrics.record_http_version(false); // HTTP/1.x

        let rate = metrics.http2_rate();
        assert!((rate - 0.75).abs() < 0.001);

        // Verify raw counts
        assert_eq!(metrics.http2_requests.load(Ordering::Relaxed), 3);
        assert_eq!(metrics.http1_requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_snapshot() {
        let metrics = PerformanceMetrics::new();
        metrics.record_download(1000, 50);
        metrics.record_cache_hit();
        metrics.record_decryption(500, 10);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.downloads_total, 1);
        assert_eq!(snapshot.download_bytes_total, 1000);
        assert_eq!(snapshot.cache_hits, 1);
        assert_eq!(snapshot.decryptions_total, 1);
    }

    // Property-based tests

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        /// **Feature: hls-performance-optimization, Property 10: Metrics accuracy**
        ///
        ///
        /// For any sequence of download operations, the metrics SHALL accurately reflect:
        /// - downloads_total equals count of download calls
        /// - download_bytes_total equals sum of downloaded bytes
        /// - average latency equals sum of latencies divided by count
        #[test]
        fn prop_metrics_accuracy(
            downloads in prop::collection::vec((1u64..10_000_000, 1u64..10_000), 1..50)
        ) {
            let metrics = PerformanceMetrics::new();

            let mut expected_count = 0u64;
            let mut expected_bytes = 0u64;
            let mut expected_latency_sum = 0u64;

            for (bytes, latency) in &downloads {
                metrics.record_download(*bytes, *latency);
                expected_count += 1;
                expected_bytes += bytes;
                expected_latency_sum += latency;
            }

            // Verify downloads_total equals count of download calls
            prop_assert_eq!(
                metrics.downloads_total.load(Ordering::Relaxed),
                expected_count,
                "downloads_total should equal count of download calls"
            );

            // Verify download_bytes_total equals sum of downloaded bytes
            prop_assert_eq!(
                metrics.download_bytes_total.load(Ordering::Relaxed),
                expected_bytes,
                "download_bytes_total should equal sum of downloaded bytes"
            );

            // Verify download_latency_sum_ms equals sum of latencies
            prop_assert_eq!(
                metrics.download_latency_sum_ms.load(Ordering::Relaxed),
                expected_latency_sum,
                "download_latency_sum_ms should equal sum of latencies"
            );

            // Verify average latency equals sum of latencies divided by count
            let expected_avg = expected_latency_sum as f64 / expected_count as f64;
            let actual_avg = metrics.average_download_latency_ms().unwrap();
            prop_assert!(
                (actual_avg - expected_avg).abs() < 0.0001,
                "average latency should equal sum/count: expected {}, got {}",
                expected_avg,
                actual_avg
            );
        }
    }
}
