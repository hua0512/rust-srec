//! Bounded output retention, with one file and its accounting settled at a time.

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use super::{MaintenanceReport, MaintenanceScheduler};
use crate::database::begin_immediate;
use crate::database::models::MediaOutputDbModel;
use crate::utils::fs::normalize_media_path;
use crate::{Error, Result};

// Age starts after both the output and its session have settled. Retained job
// history also protects recently finished work, including manually retried jobs.
// Unscoped work conservatively protects every session because its inputs may be
// arbitrary paths. Scheduled retries remain live even while their job is FAILED.
const ELIGIBLE: &str = "
    SELECT m.* FROM media_outputs m
    JOIN live_sessions s ON s.id = m.session_id
    WHERE m.created_at < ? AND s.end_time IS NOT NULL AND s.end_time < ?
      AND NOT EXISTS (
        SELECT 1 FROM job j
        WHERE (j.session_id = s.id OR j.session_id IS NULL
          OR j.input = m.file_path
          OR EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(j.input) THEN j.input ELSE '[]' END)
                     WHERE value = m.file_path))
          AND (j.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
               OR j.retry_after IS NOT NULL OR j.updated_at >= ?)
      )
      AND NOT EXISTS (
        SELECT 1 FROM dag_execution d
        WHERE (d.session_id = s.id OR d.session_id IS NULL)
          AND (d.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED') OR d.updated_at >= ?)
      )";

impl MaintenanceScheduler {
    pub(super) async fn prune_outputs(
        &self,
        cutoff: i64,
        delete_files: bool,
        cancellation: &CancellationToken,
        report: &mut MaintenanceReport,
    ) {
        // Keyset pagination advances past failures, so one locked file cannot
        // starve all later candidates. The next sweep retries those failures.
        let mut cursor = (i64::MIN, String::new());
        for _ in 0..self.config.max_batches_per_task.max(1) {
            if cancellation.is_cancelled() {
                return;
            }
            let sql = format!(
                "{ELIGIBLE} AND (m.created_at, m.id) > (?, ?) ORDER BY m.created_at, m.id LIMIT ?"
            );
            let candidates = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(sql))
                .bind(cutoff)
                .bind(cutoff)
                .bind(cutoff)
                .bind(cutoff)
                .bind(cursor.0)
                .bind(&cursor.1)
                .bind(self.config.batch_size.max(1))
                .fetch_all(&self.pool)
                .await;
            let candidates = match candidates {
                Ok(rows) => rows,
                Err(error) => {
                    report.record_failure("list_expired_outputs", error.into());
                    return;
                }
            };
            if candidates.is_empty() {
                return;
            }
            for candidate in candidates {
                cursor = (candidate.created_at, candidate.id.clone());
                if cancellation.is_cancelled() {
                    return;
                }
                let downloads = self.download_manager.upgrade();
                let _admission = if delete_files {
                    match &downloads {
                        Some(downloads) => match downloads.try_admit_maintenance(usize::MAX) {
                            Some(lease) => Some(lease),
                            None => return,
                        },
                        None => None,
                    }
                } else {
                    None
                };
                let _lease = match &self.output_files_gate {
                    Some(gate) => match gate.try_write() {
                        Ok(lease) => Some(lease),
                        // Do not queue an exclusive lock ahead of processors.
                        Err(_) => return,
                    },
                    None if delete_files => {
                        report.record_failure(
                            "prune_outputs",
                            Error::config(
                                "Output file retention requires the pipeline execution gate",
                            ),
                        );
                        return;
                    }
                    None => None,
                };
                // Once admitted, finish unlink/accounting before observing
                // cancellation. Dropping a tokio filesystem future does not stop IO.
                match self
                    .prune_output(&candidate.id, cutoff, delete_files, downloads.as_deref())
                    .await
                {
                    Ok(Some(file_deleted)) => {
                        report.outputs_deleted += 1;
                        report.output_files_deleted += u64::from(file_deleted);
                    }
                    Ok(None) => {}
                    Err(error) => report.record_failure("prune_output", error),
                }
            }
            tokio::task::yield_now().await;
        }
    }

    async fn prune_output(
        &self,
        id: &str,
        cutoff: i64,
        delete_files: bool,
        downloads: Option<&crate::downloader::DownloadManager>,
    ) -> Result<Option<bool>> {
        // The writer reservation serializes the final eligibility check and unlink
        // against job admission/retry and session mutations. Keep it to ONE file;
        // never hold this transaction across a whole batch or a retry sleep.
        let mut tx = begin_immediate(&self.write_pool).await?;
        let sql = format!("{ELIGIBLE} AND m.id = ?");
        let Some(output) = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(sql))
            .bind(cutoff)
            .bind(cutoff)
            .bind(cutoff)
            .bind(cutoff)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
        else {
            return Ok(None);
        };
        let mut file_deleted = false;
        if delete_files {
            // Never unlink a file while another output still owns its path.
            let shared: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM media_outputs WHERE file_path = ? AND id != ?)",
            )
            .bind(&output.file_path)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if shared {
                return Ok(None);
            }
            let path = normalize_media_path(&output.file_path);
            if let Some(downloads) = downloads
                && downloads.output_directory_in_use(&path).await?
            {
                return Ok(None);
            }
            match remove_expired_file(&path, cutoff).await? {
                Some(deleted) => file_deleted = deleted,
                None => return Ok(None),
            }
        }
        crate::database::repositories::session_tx::SessionTxOps::delete_media_output(&mut tx, id)
            .await?;
        tx.commit().await?;
        Ok(Some(file_deleted))
    }
}

/// None means the path was replaced/modified too recently to delete safely.
/// A missing file is reconciled only when its parent is accessible; a missing
/// mount or directory must not silently erase the inventory.
async fn remove_expired_file(path: &Path, cutoff: i64) -> Result<Option<bool>> {
    let absolute = std::path::absolute(path).map_err(|e| Error::io_path("absolute", path, e))?;
    for parent in absolute.ancestors().skip(1) {
        let metadata = tokio::fs::symlink_metadata(parent)
            .await
            .map_err(|e| Error::io_path("inspect output directory", parent, e))?;
        if metadata.is_symlink() {
            return Err(Error::config(format!(
                "Output retention refuses a linked directory: {}",
                parent.display()
            )));
        }
    }
    let metadata = match tokio::fs::symlink_metadata(&absolute).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Some(false)),
        Err(error) => return Err(Error::io_path("inspect output file", &absolute, error)),
    };
    if !metadata.is_file() || metadata.is_symlink() {
        return Err(Error::config(format!(
            "Output retention requires a regular file: {}",
            absolute.display()
        )));
    }
    let modified = metadata
        .modified()
        .map_err(|e| Error::io_path("read modification time", &absolute, e))?;
    let cutoff_time = UNIX_EPOCH + Duration::from_millis(u64::try_from(cutoff).unwrap_or_default());
    if modified >= cutoff_time {
        return Ok(None);
    }
    match tokio::fs::remove_file(&absolute).await {
        Ok(()) => Ok(Some(true)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Some(false)),
        Err(error) => Err(Error::io_path("delete expired output", &absolute, error)),
    }
}
