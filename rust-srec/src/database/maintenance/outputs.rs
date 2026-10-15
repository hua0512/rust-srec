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
// Materialize the indexed protection set once per statement before evaluating
// input paths; the transactional per-file check rebuilds it after writer admission.
// Suffixes also use numbered parameters: SQLx does not advance anonymous bind
// positions past a numbered parameter, so mixing the two binds the wrong values.
const ELIGIBLE: &str = "
    WITH protected_job_ids AS MATERIALIZED (
        SELECT id FROM job WHERE status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
        UNION
        SELECT id FROM job WHERE retry_after IS NOT NULL
        UNION
        SELECT id FROM job WHERE updated_at >= ?1
    )
    SELECT m.* FROM media_outputs m
    JOIN live_sessions s ON s.id = m.session_id
    WHERE m.created_at < ?1 AND s.end_time IS NOT NULL AND s.end_time < ?1
      AND NOT EXISTS (
        SELECT 1 FROM job j
        WHERE j.id IN (SELECT id FROM protected_job_ids)
          AND (j.session_id = s.id OR j.session_id IS NULL
          OR j.input = m.file_path
          OR EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(j.input) THEN j.input ELSE '[]' END)
                     WHERE value = m.file_path))
      )
      AND NOT EXISTS (
        SELECT 1 FROM dag_execution d
        WHERE (d.session_id = s.id OR d.session_id IS NULL)
          AND (d.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED') OR d.updated_at >= ?1)
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
                "{ELIGIBLE} AND (m.created_at, m.id) > (?2, ?3) ORDER BY m.created_at, m.id LIMIT ?4"
            );
            let candidates = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(sql))
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
        let sql = format!("{ELIGIBLE} AND m.id = ?2");
        let Some(output) = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(sql))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{MaintenanceConfig, init_pool_with_size, run_migrations};

    const FILE_PATH: &str = "/archive/video.mp4";
    const CUTOFF: i64 = 100;

    async fn fixture() -> (sqlx::SqlitePool, MaintenanceScheduler) {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO live_sessions(id, start_time, end_time, total_size_bytes) \
             VALUES ('session', 0, 1, 5), ('other', 0, 1, 0); \
             INSERT INTO media_outputs(id, session_id, file_path, file_type, size_bytes, created_at) \
             VALUES ('output', 'session', '/archive/video.mp4', 'VIDEO', 5, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let scheduler =
            MaintenanceScheduler::new(pool.clone(), pool.clone(), MaintenanceConfig::default());
        (pool, scheduler)
    }

    #[tokio::test]
    async fn output_retention_protection_includes_future_statuses_and_path_inputs() {
        let (pool, _) = fixture().await;
        let json_input = serde_json::to_string(&[FILE_PATH]).unwrap();
        let scalar_input = serde_json::to_string(FILE_PATH).unwrap();
        for (status, session, input, updated_at, retry_after, protected) in [
            ("PENDING", Some("session"), None, 1, None, true),
            ("PROCESSING", Some("session"), None, 1, None, true),
            ("SUSPENDED", Some("session"), None, 1, None, true),
            ("COMPLETED", Some("session"), None, CUTOFF, None, true),
            ("FAILED", Some("session"), None, 1, Some(200_i64), true),
            ("CANCELLED", Some("session"), None, 1, None, false),
            ("PENDING", Some("other"), Some(FILE_PATH), 1, None, true),
            (
                "PENDING",
                Some("other"),
                Some(json_input.as_str()),
                1,
                None,
                true,
            ),
            (
                "PENDING",
                Some("other"),
                Some(scalar_input.as_str()),
                1,
                None,
                true,
            ),
            ("PENDING", Some("other"), Some("{invalid"), 1, None, false),
            ("PENDING", Some("other"), None, 1, None, false),
            ("PENDING", None, None, 1, None, true),
            ("COMPLETED", None, None, 1, None, false),
            (
                "FAILED",
                Some("other"),
                Some(json_input.as_str()),
                1,
                None,
                false,
            ),
        ] {
            let mut connection = pool.acquire().await.unwrap();
            sqlx::query("DELETE FROM job")
                .execute(&mut *connection)
                .await
                .unwrap();
            // Exercise a future schema's nonterminal value without rebuilding the fixture.
            sqlx::query("PRAGMA ignore_check_constraints = ON")
                .execute(&mut *connection)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO job(id, job_type, status, config, state, created_at, updated_at, \
                 session_id, input, retry_after) VALUES ('protector', 'remux', ?, '{}', '{}', 1, ?, ?, ?, ?)",
            )
            .bind(status).bind(updated_at).bind(session).bind(input).bind(retry_after)
            .execute(&mut *connection).await.unwrap();
            sqlx::query("PRAGMA ignore_check_constraints = OFF")
                .execute(&mut *connection)
                .await
                .unwrap();
            drop(connection);

            let eligible = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(format!(
                "{ELIGIBLE} AND m.id = ?2"
            )))
            .bind(CUTOFF)
            .bind("output")
            .fetch_optional(&pool)
            .await
            .unwrap();
            assert_eq!(
                eligible.is_none(),
                protected,
                "{status}, {session:?}, {input:?}"
            );
        }
    }

    #[tokio::test]
    async fn output_retention_final_recheck_observes_new_job_protection() {
        let (pool, scheduler) = fixture().await;
        let candidates = sqlx::query_as::<_, MediaOutputDbModel>(sqlx::AssertSqlSafe(ELIGIBLE))
            .bind(CUTOFF)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        sqlx::query(
            "INSERT INTO job(id, job_type, status, config, state, created_at, updated_at, session_id) \
             VALUES ('new-job', 'remux', 'PENDING', '{}', '{}', 1, 1, 'session')",
        )
        .execute(&pool).await.unwrap();
        assert_eq!(
            scheduler
                .prune_output("output", CUTOFF, false, None)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media_outputs")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn output_retention_plan_uses_indexed_protection_instead_of_correlated_history_scans() {
        use sqlx::Row;

        let (pool, _) = fixture().await;
        sqlx::raw_sql(
            "WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<2000) \
             INSERT INTO job(id, job_type, status, config, state, created_at, updated_at, session_id) \
             SELECT 'old-' || i, 'remux', 'COMPLETED', '{}', '{}', 1, 1, 'other' FROM n;",
        ).execute(&pool).await.unwrap();
        let plan = sqlx::query(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {ELIGIBLE}"
        )))
        .bind(CUTOFF)
        .fetch_all(&pool)
        .await
        .unwrap();
        let details: Vec<String> = plan.iter().map(|row| row.get("detail")).collect();
        let plan = details.join("\n");
        for index in [
            "idx_job_nonterminal_id",
            "idx_job_retry_after",
            "idx_job_updated_at",
        ] {
            assert!(plan.contains(index), "{plan}");
        }
        assert!(
            !details
                .iter()
                .any(|detail| detail == "SCAN j" || detail.starts_with("SCAN j USING")),
            "{plan}"
        );
    }
}
