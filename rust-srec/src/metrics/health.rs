//! Health check implementation.
//!
//! Provides health checks for system components and resource monitoring.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

use crate::notification::NotificationEvent;

/// Health status of a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Component is healthy.
    Healthy,
    /// Component is degraded but functional.
    Degraded,
    /// Component is unhealthy.
    Unhealthy,
    /// Component status is unknown.
    #[default]
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Health information for a single component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name.
    pub name: String,
    /// Health status.
    pub status: HealthStatus,
    /// Optional message.
    pub message: Option<String>,
    /// Last check time (ISO 8601).
    pub last_check: Option<String>,
    /// Check duration in milliseconds.
    pub check_duration_ms: Option<u64>,
}

impl ComponentHealth {
    /// Create a healthy component.
    pub fn healthy(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            message: None,
            last_check: Some(chrono::Utc::now().to_rfc3339()),
            check_duration_ms: None,
        }
    }

    /// Create an unhealthy component.
    pub fn unhealthy(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
            last_check: Some(chrono::Utc::now().to_rfc3339()),
            check_duration_ms: None,
        }
    }

    /// Create a degraded component.
    pub fn degraded(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded,
            message: Some(message.into()),
            last_check: Some(chrono::Utc::now().to_rfc3339()),
            check_duration_ms: None,
        }
    }

    /// Set the check duration.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.check_duration_ms = Some(duration.as_millis() as u64);
        self
    }
}

/// Overall system health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    /// Overall status.
    pub status: HealthStatus,
    /// Component health details.
    pub components: HashMap<String, ComponentHealth>,
    /// System version.
    pub version: String,
    /// Uptime in seconds.
    pub uptime_secs: u64,
    /// Timestamp of the health check.
    pub timestamp: String,
    /// CPU usage percentage (0-100).
    pub cpu_usage: f32,
    /// Memory usage percentage (0-100).
    pub memory_usage: f32,
}

impl SystemHealth {
    /// Check if the system is ready to serve requests.
    pub fn is_ready(&self) -> bool {
        matches!(self.status, HealthStatus::Healthy | HealthStatus::Degraded)
    }

    /// Check if the system is healthy.
    pub fn is_healthy(&self) -> bool {
        self.status == HealthStatus::Healthy
    }
}

/// Health check function type.
pub type HealthCheckFn = Arc<dyn Fn() -> ComponentHealth + Send + Sync>;

/// Health checker for the system.
pub struct HealthChecker {
    /// Registered health checks.
    checks: RwLock<HashMap<String, HealthCheckFn>>,
    /// System start time.
    start_time: Instant,
    /// System version.
    version: String,
    /// Disk space warning threshold (percentage).
    disk_warning_threshold: f64,
    /// Disk space critical threshold (percentage).
    disk_critical_threshold: f64,
    /// System metrics collector.
    system: Mutex<System>,
}

impl HealthChecker {
    /// Create a new health checker.
    pub fn new() -> Self {
        Self {
            checks: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            disk_warning_threshold: 0.80,
            disk_critical_threshold: 0.95,
            system: Mutex::new(System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_memory(MemoryRefreshKind::everything()),
            )),
        }
    }

    /// Create a new health checker with custom thresholds.
    pub fn with_thresholds(disk_warning: f64, disk_critical: f64) -> Self {
        Self {
            checks: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            disk_warning_threshold: disk_warning,
            disk_critical_threshold: disk_critical,
            system: Mutex::new(System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_memory(MemoryRefreshKind::everything()),
            )),
        }
    }

    /// Register a health check.
    pub async fn register(&self, name: impl Into<String>, check: HealthCheckFn) {
        self.checks.write().await.insert(name.into(), check);
    }

    /// Unregister a health check.
    ///
    /// Returns true if the check was removed, false if it didn't exist.
    pub async fn unregister(&self, name: &str) -> bool {
        self.checks.write().await.remove(name).is_some()
    }

    /// Run all health checks.
    pub async fn check_all(&self) -> SystemHealth {
        let checks = self.checks.read().await;
        let mut components = HashMap::new();
        let mut overall_status = HealthStatus::Healthy;

        // Collect system metrics
        let (cpu_usage, memory_usage) = {
            let mut system = self.system.lock().await;
            system.refresh_cpu_all();
            system.refresh_memory();

            let cpu = system.global_cpu_usage();
            let total_mem = system.total_memory();
            let used_mem = system.used_memory();
            let mem_usage = if total_mem > 0 {
                (used_mem as f64 / total_mem as f64 * 100.0) as f32
            } else {
                0.0
            };
            (cpu, mem_usage)
        };

        for (name, check) in checks.iter() {
            let start = Instant::now();
            let mut health = check();
            health.check_duration_ms = Some(start.elapsed().as_millis() as u64);

            // Update overall status
            match health.status {
                HealthStatus::Unhealthy => {
                    overall_status = HealthStatus::Unhealthy;
                }
                HealthStatus::Degraded if overall_status == HealthStatus::Healthy => {
                    overall_status = HealthStatus::Degraded;
                }
                _ => {}
            }

            components.insert(name.clone(), health);
        }

        SystemHealth {
            status: overall_status,
            components,
            version: self.version.clone(),
            uptime_secs: self.start_time.elapsed().as_secs(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            cpu_usage,
            memory_usage,
        }
    }

    /// Check readiness (for Kubernetes probes).
    pub async fn check_ready(&self) -> bool {
        let health = self.check_all().await;
        health.is_ready()
    }

    /// Check disk space and return health status.
    pub fn check_disk_space(&self, path: &str, available: u64, total: u64) -> ComponentHealth {
        if total == 0 {
            return ComponentHealth::unhealthy("disk", "Unable to determine disk space");
        }

        let used_ratio = 1.0 - (available as f64 / total as f64);

        if used_ratio >= self.disk_critical_threshold {
            warn!(
                "Disk space critical on {}: {:.1}% used",
                path,
                used_ratio * 100.0
            );
            ComponentHealth::unhealthy(
                format!("disk:{}", path),
                format!(
                    "Disk space critical: {:.1}% used ({} available)",
                    used_ratio * 100.0,
                    format_bytes(available)
                ),
            )
        } else if used_ratio >= self.disk_warning_threshold {
            warn!(
                "Disk space warning on {}: {:.1}% used",
                path,
                used_ratio * 100.0
            );
            ComponentHealth::degraded(
                format!("disk:{}", path),
                format!(
                    "Disk space warning: {:.1}% used ({} available)",
                    used_ratio * 100.0,
                    format_bytes(available)
                ),
            )
        } else {
            debug!("Disk space OK on {}: {:.1}% used", path, used_ratio * 100.0);
            ComponentHealth::healthy(format!("disk:{}", path))
        }
    }

    /// Generate notification event for disk space issues.
    pub fn disk_space_notification(
        &self,
        path: &str,
        available: u64,
        total: u64,
    ) -> Option<NotificationEvent> {
        if total == 0 {
            return None;
        }

        let used_ratio = 1.0 - (available as f64 / total as f64);
        let threshold = if used_ratio >= self.disk_critical_threshold {
            (total as f64 * (1.0 - self.disk_critical_threshold)) as u64
        } else if used_ratio >= self.disk_warning_threshold {
            (total as f64 * (1.0 - self.disk_warning_threshold)) as u64
        } else {
            return None;
        };

        Some(NotificationEvent::OutOfSpace {
            path: path.to_string(),
            available_bytes: available,
            threshold_bytes: threshold,
            timestamp: chrono::Utc::now(),
        })
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Format bytes into human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_status_default() {
        assert_eq!(HealthStatus::default(), HealthStatus::Unknown);
    }

    #[test]
    fn test_component_health_healthy() {
        let health = ComponentHealth::healthy("test");
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.message.is_none());
    }

    #[test]
    fn test_component_health_unhealthy() {
        let health = ComponentHealth::unhealthy("test", "Something went wrong");
        assert_eq!(health.status, HealthStatus::Unhealthy);
        assert_eq!(health.message, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_component_health_with_duration() {
        let health = ComponentHealth::healthy("test").with_duration(Duration::from_millis(100));
        assert_eq!(health.check_duration_ms, Some(100));
    }

    #[tokio::test]
    async fn test_health_checker_creation() {
        let checker = HealthChecker::new();
        let health = checker.check_all().await;
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.components.is_empty());
    }

    #[tokio::test]
    async fn test_health_checker_register() {
        let checker = HealthChecker::new();
        checker
            .register("test", Arc::new(|| ComponentHealth::healthy("test")))
            .await;

        let health = checker.check_all().await;
        assert!(health.components.contains_key("test"));
    }

    #[tokio::test]
    async fn test_health_checker_unhealthy_component() {
        let checker = HealthChecker::new();
        checker
            .register(
                "failing",
                Arc::new(|| ComponentHealth::unhealthy("failing", "Error")),
            )
            .await;

        let health = checker.check_all().await;
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_disk_space_check_healthy() {
        let checker = HealthChecker::new();
        let health =
            checker.check_disk_space("/data", 50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_disk_space_check_warning() {
        let checker = HealthChecker::new();
        // 85% used = 15% available
        let health =
            checker.check_disk_space("/data", 15 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(health.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_disk_space_check_critical() {
        let checker = HealthChecker::new();
        // 97% used = 3% available
        let health =
            checker.check_disk_space("/data", 3 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_system_health_is_ready() {
        let health = SystemHealth {
            status: HealthStatus::Healthy,
            components: HashMap::new(),
            version: "0.1.0".to_string(),
            uptime_secs: 100,
            timestamp: chrono::Utc::now().to_rfc3339(),
            cpu_usage: 0.0,
            memory_usage: 0.0,
        };
        assert!(health.is_ready());
        assert!(health.is_healthy());

        let degraded = SystemHealth {
            status: HealthStatus::Degraded,
            ..health.clone()
        };
        assert!(degraded.is_ready());
        assert!(!degraded.is_healthy());

        let unhealthy = SystemHealth {
            status: HealthStatus::Unhealthy,
            ..health
        };
        assert!(!unhealthy.is_ready());
    }
}
