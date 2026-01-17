//! Prometheus metrics exporter.
//!
//! Exports metrics in Prometheus text format.

use std::sync::Arc;

use super::collector::MetricsCollector;

/// Prometheus metrics exporter.
pub struct PrometheusExporter {
    collector: Arc<MetricsCollector>,
    namespace: String,
}

impl PrometheusExporter {
    /// Create a new Prometheus exporter.
    pub fn new(collector: Arc<MetricsCollector>) -> Self {
        Self {
            collector,
            namespace: "rust_srec".to_string(),
        }
    }

    /// Create a new Prometheus exporter with custom namespace.
    pub fn with_namespace(collector: Arc<MetricsCollector>, namespace: impl Into<String>) -> Self {
        Self {
            collector,
            namespace: namespace.into(),
        }
    }

    /// Export metrics in Prometheus text format.
    pub fn export(&self) -> String {
        let snapshot = self.collector.snapshot();
        let mut output = String::new();

        // Download metrics
        self.write_gauge(
            &mut output,
            "active_downloads",
            "Number of active downloads",
            snapshot.active_downloads as f64,
        );

        self.write_counter(
            &mut output,
            "download_bytes_total",
            "Total bytes downloaded",
            snapshot.download_bytes_total as f64,
        );

        self.write_counter(
            &mut output,
            "downloads_total",
            "Total number of completed downloads",
            snapshot.download_count as f64,
        );

        for (error_type, count) in &snapshot.download_errors {
            self.write_counter_with_labels(
                &mut output,
                "download_errors_total",
                "Total download errors by type",
                *count as f64,
                &[("error_type", error_type)],
            );
        }

        // Pipeline metrics
        for (worker_type, depth) in &snapshot.pipeline_queue_depth {
            self.write_gauge_with_labels(
                &mut output,
                "pipeline_queue_depth",
                "Pipeline queue depth by worker type",
                *depth as f64,
                &[("worker_type", worker_type)],
            );
        }

        for (status, count) in &snapshot.pipeline_jobs_total {
            self.write_counter_with_labels(
                &mut output,
                "pipeline_jobs_total",
                "Total pipeline jobs by status",
                *count as f64,
                &[("status", status)],
            );
        }

        // Streamer metrics
        let mut total_streamers = 0u64;
        for (state, count) in &snapshot.streamers_by_state {
            total_streamers += count;
            self.write_gauge_with_labels(
                &mut output,
                "streamers_by_state",
                "Number of streamers by state",
                *count as f64,
                &[("state", state)],
            );
        }

        self.write_gauge(
            &mut output,
            "streamers_total",
            "Total number of streamers",
            total_streamers as f64,
        );

        self.write_gauge(
            &mut output,
            "streamers_live",
            "Number of live streamers",
            snapshot
                .streamers_by_state
                .get("Live")
                .copied()
                .unwrap_or(0) as f64,
        );

        for (streamer_id, count) in &snapshot.streamer_errors {
            self.write_counter_with_labels(
                &mut output,
                "streamer_errors_total",
                "Total errors by streamer",
                *count as f64,
                &[("streamer_id", streamer_id)],
            );
        }

        // System metrics
        self.write_counter(
            &mut output,
            "config_cache_hits_total",
            "Total config cache hits",
            snapshot.config_cache_hits as f64,
        );

        self.write_counter(
            &mut output,
            "config_cache_misses_total",
            "Total config cache misses",
            snapshot.config_cache_misses as f64,
        );

        for (path, bytes) in &snapshot.disk_space_bytes {
            self.write_gauge_with_labels(
                &mut output,
                "disk_space_bytes",
                "Available disk space in bytes",
                *bytes as f64,
                &[("path", path)],
            );
        }

        self.write_gauge(
            &mut output,
            "memory_usage_bytes",
            "Memory usage in bytes",
            snapshot.memory_usage_bytes as f64,
        );

        // Web Push metrics
        self.write_counter(
            &mut output,
            "web_push_sent_total",
            "Total Web Push deliveries (successful)",
            snapshot.web_push_sent_total as f64,
        );
        self.write_counter(
            &mut output,
            "web_push_failed_total",
            "Total Web Push delivery failures",
            snapshot.web_push_failed_total as f64,
        );
        self.write_counter(
            &mut output,
            "web_push_throttled_total",
            "Total Web Push throttling responses (HTTP 429)",
            snapshot.web_push_throttled_total as f64,
        );
        self.write_counter(
            &mut output,
            "web_push_stale_deleted_total",
            "Total stale Web Push subscriptions deleted (HTTP 404/410)",
            snapshot.web_push_stale_deleted_total as f64,
        );
        self.write_counter(
            &mut output,
            "web_push_skipped_backoff_total",
            "Total Web Push deliveries skipped due to persisted backoff",
            snapshot.web_push_skipped_backoff_total as f64,
        );
        self.write_gauge(
            &mut output,
            "web_push_delivery_duration_avg_ms",
            "Average Web Push delivery duration in milliseconds",
            snapshot.web_push_delivery_duration_avg_ms,
        );

        output
    }

    fn write_gauge(&self, output: &mut String, name: &str, help: &str, value: f64) {
        let full_name = format!("{}_{}", self.namespace, name);
        output.push_str(&format!("# HELP {} {}\n", full_name, help));
        output.push_str(&format!("# TYPE {} gauge\n", full_name));
        output.push_str(&format!("{} {}\n", full_name, value));
    }

    fn write_gauge_with_labels(
        &self,
        output: &mut String,
        name: &str,
        help: &str,
        value: f64,
        labels: &[(&str, &str)],
    ) {
        let full_name = format!("{}_{}", self.namespace, name);
        output.push_str(&format!("# HELP {} {}\n", full_name, help));
        output.push_str(&format!("# TYPE {} gauge\n", full_name));

        let labels_str = labels
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"", k, v))
            .collect::<Vec<_>>()
            .join(",");

        output.push_str(&format!("{}{{{}}} {}\n", full_name, labels_str, value));
    }

    fn write_counter(&self, output: &mut String, name: &str, help: &str, value: f64) {
        let full_name = format!("{}_{}", self.namespace, name);
        output.push_str(&format!("# HELP {} {}\n", full_name, help));
        output.push_str(&format!("# TYPE {} counter\n", full_name));
        output.push_str(&format!("{} {}\n", full_name, value));
    }

    fn write_counter_with_labels(
        &self,
        output: &mut String,
        name: &str,
        help: &str,
        value: f64,
        labels: &[(&str, &str)],
    ) {
        let full_name = format!("{}_{}", self.namespace, name);
        output.push_str(&format!("# HELP {} {}\n", full_name, help));
        output.push_str(&format!("# TYPE {} counter\n", full_name));

        let labels_str = labels
            .iter()
            .map(|(k, v)| format!("{}=\"{}\"", k, v))
            .collect::<Vec<_>>()
            .join(",");

        output.push_str(&format!("{}{{{}}} {}\n", full_name, labels_str, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_exporter_creation() {
        let collector = Arc::new(MetricsCollector::new());
        let exporter = PrometheusExporter::new(collector);
        assert_eq!(exporter.namespace, "rust_srec");
    }

    #[test]
    fn test_prometheus_export_empty() {
        let collector = Arc::new(MetricsCollector::new());
        let exporter = PrometheusExporter::new(collector);
        let output = exporter.export();

        assert!(output.contains("# HELP rust_srec_active_downloads"));
        assert!(output.contains("# TYPE rust_srec_active_downloads gauge"));
        assert!(output.contains("rust_srec_active_downloads 0"));
    }

    #[test]
    fn test_prometheus_export_with_data() {
        let collector = Arc::new(MetricsCollector::new());
        collector.record_download_started();
        collector.record_download_started();
        collector.record_download_bytes(1024);
        collector.set_streamers_by_state("Live", 5);
        collector.set_disk_space("/data", 1024 * 1024 * 1024);

        let exporter = PrometheusExporter::new(collector);
        let output = exporter.export();

        assert!(output.contains("rust_srec_active_downloads 2"));
        assert!(output.contains("rust_srec_download_bytes_total 1024"));
        assert!(output.contains("rust_srec_streamers_live 5"));
        assert!(output.contains("rust_srec_disk_space_bytes{path=\"/data\"}"));
    }

    #[test]
    fn test_prometheus_export_with_labels() {
        let collector = Arc::new(MetricsCollector::new());
        collector.record_download_started();
        collector.record_download_error("network");
        collector.set_pipeline_queue_depth("cpu", 10);

        let exporter = PrometheusExporter::new(collector);
        let output = exporter.export();

        assert!(output.contains("rust_srec_download_errors_total{error_type=\"network\"}"));
        assert!(output.contains("rust_srec_pipeline_queue_depth{worker_type=\"cpu\"}"));
    }

    #[test]
    fn test_prometheus_custom_namespace() {
        let collector = Arc::new(MetricsCollector::new());
        let exporter = PrometheusExporter::with_namespace(collector, "custom");
        let output = exporter.export();

        assert!(output.contains("custom_active_downloads"));
        assert!(!output.contains("rust_srec_"));
    }
}
