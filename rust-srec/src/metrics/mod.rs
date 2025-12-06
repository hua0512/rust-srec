//! Metrics and monitoring module.
//!
//! Provides Prometheus-compatible metrics collection and health check endpoints
//! for monitoring the streaming recorder system.
//!
//! # Features
//!
//! - Download metrics (active downloads, bytes, duration, errors)
//! - Pipeline metrics (queue depth, jobs, duration)
//! - Streamer metrics (total, live, errors)
//! - System metrics (cache hits/misses, disk space, memory)
//! - Health check endpoints (/health, /ready)
//! - Prometheus metrics endpoint (/metrics)
//!
//! # Example
//!
//! ```ignore
//! use rust_srec::metrics::{MetricsCollector, HealthChecker};
//!
//! let collector = MetricsCollector::new();
//! collector.record_download_started("streamer-1");
//! collector.record_download_bytes(1024 * 1024);
//!
//! let health = HealthChecker::new();
//! let status = health.check_all().await;
//! ```

mod collector;
mod health;
mod prometheus;

pub use collector::{MetricsCollector, MetricsSnapshot};
pub use health::{ComponentHealth, HealthChecker, HealthStatus, SystemHealth};
pub use prometheus::PrometheusExporter;
