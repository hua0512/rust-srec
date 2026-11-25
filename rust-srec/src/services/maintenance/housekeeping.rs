//! Windowed SQLite housekeeping: WAL truncation, query-planner optimize, and incremental vacuum.

use std::path::{Path, PathBuf};
use std::time::Instant;

use sysinfo::Disks;
use tracing::{debug, info, warn};

use super::MaintenanceScheduler;
use crate::{Error, Result};

const AUTO_VACUUM_NONE: i64 = 0;
const AUTO_VACUUM_FULL: i64 = 1;
pub(super) const AUTO_VACUUM_INCREMENTAL: i64 = 2;

impl MaintenanceScheduler {
    pub(super) async fn run_optimize(&self) -> Result<()> {
        sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        info!("Database query planner optimized");
        Ok(())
    }

    pub(super) async fn run_wal_checkpoint(&self) -> Result<()> {
        let (busy, log_frames, checkpointed_frames): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(&self.write_pool)
                .await?;
        info!(
            busy,
            log_frames, checkpointed_frames, "WAL checkpoint completed"
        );
        Ok(())
    }

    pub(super) async fn run_vacuum_if_needed(&self) -> Result<bool> {
        let freeable = self.get_freeable_space().await?;
        if freeable < self.config.vacuum_threshold_bytes {
            debug!(
                freeable,
                "Reusable database space is below the vacuum threshold"
            );
            return Ok(false);
        }

        let Some(download_manager) = self.download_manager.upgrade() else {
            debug!("Skipping vacuum without a live download admission owner");
            return Ok(false);
        };
        let Ok(max_active) = usize::try_from(self.config.max_active_downloads_for_vacuum) else {
            warn!("Skipping vacuum with a negative active-download limit");
            return Ok(false);
        };
        // Avoid expensive conversion preflight when recording admission is busy,
        // but release this preliminary check before inspecting filesystems.
        if download_manager.try_admit_maintenance(max_active).is_none() {
            debug!(
                "Skipping vacuum while download admission is busy or recordings exceed the limit"
            );
            return Ok(false);
        }

        let mode: (i64,) = sqlx::query_as("PRAGMA auto_vacuum")
            .fetch_one(&self.pool)
            .await?;
        let before_size = self.get_database_size().await?;
        let conversion_path = match mode.0 {
            AUTO_VACUUM_NONE => Some(self.prepare_incremental_auto_vacuum(before_size).await?),
            AUTO_VACUUM_INCREMENTAL => None,
            AUTO_VACUUM_FULL => {
                debug!("Database already uses full auto-vacuum");
                return Ok(false);
            }
            unexpected => {
                return Err(Error::Database(format!(
                    "unexpected PRAGMA auto_vacuum value {unexpected}"
                )));
            }
        };
        // Recheck after preflight and hold admission through SQLite completion.
        // The production loop awaits this operation through ordinary shutdown;
        // cancelling a SQLx future alone would not stop its SQLite worker.
        let Some(_admission) = download_manager.try_admit_maintenance(max_active) else {
            debug!("Skipping vacuum because download admission changed during preflight");
            return Ok(false);
        };
        let started = Instant::now();

        if let Some(path) = conversion_path {
            self.convert_to_incremental_auto_vacuum(&path).await?;
        } else {
            sqlx::query("PRAGMA incremental_vacuum")
                .execute(&self.write_pool)
                .await?;
        }

        let after_size = self.get_database_size().await?;
        info!(
            elapsed_ms = started.elapsed().as_millis(),
            reclaimed_bytes = before_size.saturating_sub(after_size),
            "Database vacuum completed"
        );
        Ok(true)
    }

    async fn prepare_incremental_auto_vacuum(&self, database_size: i64) -> Result<PathBuf> {
        let path = self.database_path().await?;
        let required_space = u64::try_from(database_size)
            .unwrap_or_default()
            .saturating_add(64 * 1024 * 1024);
        let probe_path = path.clone();
        let available_space =
            tokio::task::spawn_blocking(move || available_space_for_path(&probe_path))
                .await
                .map_err(|error| {
                    Error::Database(format!("Available-space probe task failed: {error}"))
                })?
                .map_err(|error| {
                    Error::Database(format!(
                        "could not list disks to check available space for '{}': {error}",
                        path.display()
                    ))
                })?
                .ok_or_else(|| {
                    Error::Database(format!(
                        "could not determine available space for '{}'",
                        path.display()
                    ))
                })?;

        if available_space < required_space {
            return Err(Error::Database(format!(
                "insufficient space to convert '{}' to incremental auto-vacuum: \
                 need {required_space} bytes, have {available_space} bytes",
                path.display()
            )));
        }

        Ok(path)
    }

    async fn convert_to_incremental_auto_vacuum(&self, path: &Path) -> Result<()> {
        info!(
            path = %path.display(),
            "Converting existing database to incremental auto-vacuum"
        );
        let mut connection = self.write_pool.acquire().await?;
        sqlx::query("PRAGMA auto_vacuum = INCREMENTAL")
            .execute(&mut *connection)
            .await?;
        sqlx::query("VACUUM").execute(&mut *connection).await?;
        Ok(())
    }

    async fn database_path(&self) -> Result<PathBuf> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as("PRAGMA database_list")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .find_map(|(_, name, path)| {
                (name == "main" && !path.is_empty()).then(|| PathBuf::from(path))
            })
            .ok_or_else(|| {
                Error::Database("SQLite main database has no filesystem path".to_string())
            })
    }

    async fn get_freeable_space(&self) -> Result<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT freelist_count * page_size FROM pragma_freelist_count(), pragma_page_size()",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }

    async fn get_database_size(&self) -> Result<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }
}

/// Free space on the longest mount point containing `path`, or `None` when no
/// listed disk contains it.
fn available_space_for_path(path: &Path) -> std::result::Result<Option<u64>, sysinfo::Error> {
    let disks = Disks::new_with_refreshed_list()?;
    let path = path.to_string_lossy();
    Ok(disks
        .iter()
        .filter_map(|disk| {
            let mount = disk.mount_point().to_string_lossy();
            path.starts_with(mount.as_ref())
                .then_some((mount.len(), disk.available_space()))
        })
        .max_by_key(|(mount_len, _)| *mount_len)
        .map(|(_, available)| available))
}
