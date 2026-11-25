//! Metrics and monitoring module.
//!
//! Provides background health snapshots and internal web-push delivery counters.
//! The HTTP surface is `/api/health`, `/api/health/ready` and `/api/health/live`;
//! there is no Prometheus exporter or `/metrics` route.
//!
//! # Features
//!
//! - System, disk and registered component health
//! - GPU health monitoring
//! - Internal web-push outcomes and delivery duration
//!
//! # Example
//!
//! ```ignore
//! use rust_srec::metrics::HealthChecker;
//!
//! let health = HealthChecker::new();
//! let status = health.current();
//! ```

mod collector;
pub mod gpu_health;
mod health;

pub use collector::{MetricsCollector, MetricsSnapshot};
pub use gpu_health::{
    DEFAULT_PROBE_INTERVAL_SECS as DEFAULT_GPU_PROBE_INTERVAL_SECS, GpuHealthMonitor,
};
pub use health::{
    ComponentHealth, DiskSnapshot, DiskUsage, HealthChecker, HealthProbe, HealthStatus,
    SystemHealth, SystemMetricsSnapshot,
};
