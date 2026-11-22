//! Centralized database retention and storage maintenance.
//!
//! Lightweight retention runs on startup and at a fixed cadence. Operations
//! that can block readers, such as vacuuming and WAL truncation, remain gated
//! by the configured maintenance window.

mod config;
mod housekeeping;
mod outputs;
mod pruning;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Weak};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use self::config::{MaintenanceConfig, MaintenanceFailure, MaintenanceReport};
use self::pruning::MaintenanceRepository;
use crate::database::{DbPool, WritePool};
use crate::downloader::DownloadManager;

#[cfg(test)]
use self::housekeeping::AUTO_VACUUM_INCREMENTAL;
#[cfg(test)]
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// Coordinates lightweight retention and windowed SQLite maintenance.
pub struct MaintenanceScheduler {
    pool: DbPool,
    write_pool: WritePool,
    repository: MaintenanceRepository,
    config: MaintenanceConfig,
    download_manager: Weak<DownloadManager>,
    output_files_gate: Option<Arc<tokio::sync::RwLock<()>>>,
}

impl MaintenanceScheduler {
    /// Creates a scheduler for the supplied SQLite pools.
    pub fn new(pool: DbPool, write_pool: WritePool, config: MaintenanceConfig) -> Self {
        let repository = MaintenanceRepository::new(
            pool.clone(),
            write_pool.clone(),
            config.batch_size,
            config.max_batches_per_task,
        );
        Self {
            pool,
            write_pool,
            repository,
            config,
            download_manager: Weak::new(),
            output_files_gate: None,
        }
    }

    /// Attach the recording admission owner. Without a live owner, vacuuming
    /// is skipped; lightweight retention remains available independently.
    pub fn with_download_manager(mut self, download_manager: Weak<DownloadManager>) -> Self {
        self.download_manager = download_manager;
        self
    }

    pub(crate) fn with_output_files_gate(mut self, gate: Arc<tokio::sync::RwLock<()>>) -> Self {
        self.output_files_gate = Some(gate);
        self
    }

    /// Starts maintenance and returns its cancellation-aware background task.
    pub fn start(self: Arc<Self>, cancellation: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move { self.run_loop(cancellation).await })
    }
}
