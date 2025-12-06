//! Resource monitoring for the scheduler.
//!
//! This module handles checking system resources like disk space
//! before allowing downloads to proceed.

use std::path::Path;

use sysinfo::Disks;
use tracing::{debug, warn};

/// Result of a disk space check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskSpaceStatus {
    /// Sufficient space available.
    Ok {
        /// Available space in bytes.
        available_bytes: u64,
    },
    /// Insufficient space.
    InsufficientSpace {
        /// Available space in bytes.
        available_bytes: u64,
        /// Required space in bytes.
        required_bytes: u64,
    },
    /// Could not determine disk space (path doesn't exist, etc.).
    Unknown,
}

impl DiskSpaceStatus {
    /// Check if there is sufficient space.
    pub fn is_ok(&self) -> bool {
        matches!(self, DiskSpaceStatus::Ok { .. })
    }

    /// Check if there is insufficient space.
    pub fn is_insufficient(&self) -> bool {
        matches!(self, DiskSpaceStatus::InsufficientSpace { .. })
    }
}

/// Resource monitor for checking system resources.
#[derive(Debug, Default)]
pub struct ResourceMonitor {
    /// Cached disk information.
    disks: Disks,
}

impl ResourceMonitor {
    /// Create a new resource monitor.
    pub fn new() -> Self {
        Self {
            disks: Disks::new_with_refreshed_list(),
        }
    }

    /// Refresh disk information.
    pub fn refresh(&mut self) {
        self.disks.refresh_list();
    }

    /// Check if there is sufficient disk space for a download.
    ///
    /// # Arguments
    /// * `output_path` - The path where the download will be saved
    /// * `required_bytes` - The minimum required space (e.g., `max_part_size_bytes`)
    ///
    /// # Returns
    /// * `DiskSpaceStatus::Ok` if there is sufficient space
    /// * `DiskSpaceStatus::InsufficientSpace` if space is below required
    /// * `DiskSpaceStatus::Unknown` if the path doesn't exist or can't be checked
    pub fn check_disk_space(&mut self, output_path: &str, required_bytes: u64) -> DiskSpaceStatus {
        // Refresh disk info
        self.refresh();

        let path = Path::new(output_path);

        // Find the disk that contains this path
        let available = self.get_available_space_for_path(path);

        match available {
            Some(available_bytes) => {
                if available_bytes >= required_bytes {
                    debug!(
                        "Disk space OK: {} bytes available, {} bytes required",
                        available_bytes, required_bytes
                    );
                    DiskSpaceStatus::Ok { available_bytes }
                } else {
                    warn!(
                        "Insufficient disk space: {} bytes available, {} bytes required",
                        available_bytes, required_bytes
                    );
                    DiskSpaceStatus::InsufficientSpace {
                        available_bytes,
                        required_bytes,
                    }
                }
            }
            None => {
                warn!("Could not determine disk space for path: {}", output_path);
                DiskSpaceStatus::Unknown
            }
        }
    }

    /// Check if there is sufficient disk space, with a default minimum.
    ///
    /// If `required_bytes` is 0 or None, skips the check and returns Ok.
    pub fn check_disk_space_optional(
        &mut self,
        output_path: &str,
        required_bytes: Option<u64>,
    ) -> DiskSpaceStatus {
        match required_bytes {
            Some(0) | None => {
                // No requirement specified, skip check
                DiskSpaceStatus::Ok { available_bytes: 0 }
            }
            Some(required) => self.check_disk_space(output_path, required),
        }
    }

    /// Get available space for a path.
    fn get_available_space_for_path(&self, path: &Path) -> Option<u64> {
        // Try to find the disk that contains this path
        // We look for the disk with the longest matching mount point

        let path_str = path.to_string_lossy();
        let mut best_match: Option<(&sysinfo::Disk, usize)> = None;

        for disk in self.disks.list() {
            let mount_point = disk.mount_point().to_string_lossy();

            // Check if the path starts with this mount point
            if path_str.starts_with(mount_point.as_ref()) {
                let mount_len = mount_point.len();

                // Keep the longest match (most specific mount point)
                if best_match.is_none_or(|(_, len)| mount_len > len) {
                    best_match = Some((disk, mount_len));
                }
            }
        }

        best_match.map(|(disk, _)| disk.available_space())
    }

    /// Get total available space across all disks.
    pub fn total_available_space(&mut self) -> u64 {
        self.refresh();
        self.disks.list().iter().map(|d| d.available_space()).sum()
    }

    /// Get disk information for logging/debugging.
    pub fn disk_info(&mut self) -> Vec<DiskInfo> {
        self.refresh();
        self.disks
            .list()
            .iter()
            .map(|d| DiskInfo {
                name: d.name().to_string_lossy().to_string(),
                mount_point: d.mount_point().to_string_lossy().to_string(),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
            })
            .collect()
    }
}

/// Information about a disk.
#[derive(Debug, Clone)]
pub struct DiskInfo {
    /// Disk name.
    pub name: String,
    /// Mount point.
    pub mount_point: String,
    /// Total space in bytes.
    pub total_bytes: u64,
    /// Available space in bytes.
    pub available_bytes: u64,
}

impl DiskInfo {
    /// Get the percentage of space used.
    pub fn used_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            let used = self.total_bytes - self.available_bytes;
            (used as f64 / self.total_bytes as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_space_status() {
        let ok = DiskSpaceStatus::Ok {
            available_bytes: 1000,
        };
        assert!(ok.is_ok());
        assert!(!ok.is_insufficient());

        let insufficient = DiskSpaceStatus::InsufficientSpace {
            available_bytes: 100,
            required_bytes: 1000,
        };
        assert!(!insufficient.is_ok());
        assert!(insufficient.is_insufficient());

        let unknown = DiskSpaceStatus::Unknown;
        assert!(!unknown.is_ok());
        assert!(!unknown.is_insufficient());
    }

    #[test]
    fn test_resource_monitor_creation() {
        let monitor = ResourceMonitor::new();
        // Just verify it can be created without panicking
        assert!(monitor.disks.list().len() >= 0);
    }

    #[test]
    fn test_check_disk_space_optional_none() {
        let mut monitor = ResourceMonitor::new();
        let status = monitor.check_disk_space_optional("/tmp", None);
        assert!(status.is_ok());
    }

    #[test]
    fn test_check_disk_space_optional_zero() {
        let mut monitor = ResourceMonitor::new();
        let status = monitor.check_disk_space_optional("/tmp", Some(0));
        assert!(status.is_ok());
    }

    #[test]
    fn test_disk_info_used_percent() {
        let info = DiskInfo {
            name: "test".to_string(),
            mount_point: "/".to_string(),
            total_bytes: 1000,
            available_bytes: 250,
        };
        assert!((info.used_percent() - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_disk_info_used_percent_zero_total() {
        let info = DiskInfo {
            name: "test".to_string(),
            mount_point: "/".to_string(),
            total_bytes: 0,
            available_bytes: 0,
        };
        assert_eq!(info.used_percent(), 0.0);
    }
}
