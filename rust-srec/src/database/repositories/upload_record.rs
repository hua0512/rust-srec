//! `upload_records` table access.
//!
//! Writes come from the pipeline at terminal job transitions
//! (`JobQueue::persist_upload_records`) and are best-effort there — the
//! caller logs and continues on failure because a bookkeeping miss must not
//! fail an otherwise successful upload job. Reads serve the API
//! (`GET /api/pipeline/jobs/{id}/uploads` and the `list_outputs` annotation).

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::WritePool;
use crate::database::models::UploadRecordDbModel;
use crate::database::retry::retry_on_sqlite_busy;

/// Upper bound per IN-clause chunk in [`UploadRecordRepository::list_by_local_paths`].
/// The outputs page fetches at most 100 rows per page, so one chunk covers a
/// full page; the chunking only matters if a caller ever passes more.
const LOCAL_PATH_CHUNK: usize = 100;

const UPSERT_SQL: &str = r#"
    INSERT INTO upload_records (
        id, job_id, streamer_id, session_id, uploader, local_path,
        remote_path, status, size_bytes, error, created_at, updated_at, completed_at
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT (job_id, local_path) DO UPDATE SET
        remote_path  = excluded.remote_path,
        status       = excluded.status,
        size_bytes   = COALESCE(excluded.size_bytes, upload_records.size_bytes),
        error        = excluded.error,
        updated_at   = excluded.updated_at,
        completed_at = excluded.completed_at
"#;

const LIST_BY_JOB_SQL: &str = r#"
    SELECT id, job_id, streamer_id, session_id, uploader, local_path,
           remote_path, status, size_bytes, error, created_at, updated_at, completed_at
    FROM upload_records
    WHERE job_id = ?
    ORDER BY local_path ASC
"#;

/// Repository for durable per-file upload results.
#[async_trait]
pub trait UploadRecordRepository: Send + Sync {
    /// Insert or update records in one transaction.
    ///
    /// The `ON CONFLICT (job_id, local_path)` upsert makes retries idempotent:
    /// `JobQueue::retry_job` reuses the job id, so a retry flips the earlier
    /// FAILED rows instead of duplicating them. `size_bytes` keeps the stored
    /// value when the new one is NULL — an rclone move-resume retry cannot
    /// re-stat a source file that was already moved.
    async fn upsert_records(&self, records: &[UploadRecordDbModel]) -> Result<()>;

    /// All records for one job, ordered by local path.
    async fn list_by_job(&self, job_id: &str) -> Result<Vec<UploadRecordDbModel>>;

    /// Records whose `local_path` is in `paths`, newest-updated first.
    /// Callers dedupe per `(local_path, uploader)` if they need one row per
    /// file (DAG retries create fresh job ids, so a file can have rows from
    /// several jobs).
    async fn list_by_local_paths(&self, paths: &[String]) -> Result<Vec<UploadRecordDbModel>>;
}

/// Sqlx implementation backed by separate read / write pools (matches the
/// pattern used by [`crate::database::repositories::SqlxStreamerCheckHistoryRepository`]).
pub struct SqlxUploadRecordRepository {
    pool: SqlitePool,
    write_pool: WritePool,
}

impl SqlxUploadRecordRepository {
    pub fn new(pool: SqlitePool, write_pool: WritePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl UploadRecordRepository for SqlxUploadRecordRepository {
    async fn upsert_records(&self, records: &[UploadRecordDbModel]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        retry_on_sqlite_busy("upsert_upload_records", || async {
            // One transaction so a batch is all-or-nothing; readers never see
            // a half-written batch for a job.
            let mut tx = self.write_pool.begin().await?;
            for record in records {
                sqlx::query(UPSERT_SQL)
                    .bind(&record.id)
                    .bind(record.job_id.as_deref())
                    .bind(record.streamer_id.as_deref())
                    .bind(record.session_id.as_deref())
                    .bind(&record.uploader)
                    .bind(&record.local_path)
                    .bind(record.remote_path.as_deref())
                    .bind(&record.status)
                    .bind(record.size_bytes)
                    .bind(record.error.as_deref())
                    .bind(record.created_at)
                    .bind(record.updated_at)
                    .bind(record.completed_at)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn list_by_job(&self, job_id: &str) -> Result<Vec<UploadRecordDbModel>> {
        let rows = sqlx::query_as::<_, UploadRecordDbModel>(LIST_BY_JOB_SQL)
            .bind(job_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    async fn list_by_local_paths(&self, paths: &[String]) -> Result<Vec<UploadRecordDbModel>> {
        let mut rows = Vec::new();
        for chunk in paths.chunks(LOCAL_PATH_CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let sql = format!(
                "SELECT id, job_id, streamer_id, session_id, uploader, local_path, \
                        remote_path, status, size_bytes, error, created_at, updated_at, completed_at \
                 FROM upload_records \
                 WHERE local_path IN ({placeholders}) \
                 ORDER BY updated_at DESC"
            );
            let mut query = sqlx::query_as::<_, UploadRecordDbModel>(sqlx::AssertSqlSafe(sql));
            for path in chunk {
                query = query.bind(path);
            }
            rows.extend(query.fetch_all(&self.pool).await?);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::JobDbModel;
    use crate::database::models::upload_record::upload_status;
    use crate::database::repositories::{JobRepository as _, SqlxJobRepository};
    use crate::database::{init_pool_with_size, run_migrations};
    use sqlx::SqlitePool;

    async fn setup_pool() -> SqlitePool {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        // upload_records.job_id has a FOREIGN KEY on job(id); create the jobs
        // the test records reference.
        let job_repo = SqlxJobRepository::new(pool.clone(), pool.clone());
        for job_id in ["job-1", "job-2"] {
            let mut job = JobDbModel::new("rclone", "{}");
            job.id = job_id.to_string();
            job_repo.create_job(&job).await.unwrap();
        }
        pool
    }

    fn record(job_id: &str, local_path: &str, status: &str) -> UploadRecordDbModel {
        UploadRecordDbModel {
            id: uuid::Uuid::new_v4().to_string(),
            job_id: Some(job_id.to_string()),
            streamer_id: Some("streamer-1".to_string()),
            session_id: Some("session-1".to_string()),
            uploader: "rclone".to_string(),
            local_path: local_path.to_string(),
            remote_path: Some(format!("remote:bucket/{local_path}")),
            status: status.to_string(),
            size_bytes: Some(1024),
            error: None,
            created_at: 1_000,
            updated_at: 1_000,
            completed_at: (status == upload_status::COMPLETED).then_some(1_000),
        }
    }

    #[tokio::test]
    async fn upsert_and_list_round_trip_each_status() {
        let pool = setup_pool().await;
        let repo = SqlxUploadRecordRepository::new(pool.clone(), pool.clone());

        // One record per status asserts the CHECK constraint accepts every
        // documented variant — guards against the migration and the
        // `upload_status` constants drifting apart.
        let records: Vec<_> = upload_status::ALL
            .iter()
            .enumerate()
            .map(|(i, status)| record("job-1", &format!("/tmp/file-{i}.mp4"), status))
            .collect();
        repo.upsert_records(&records).await.unwrap();

        let rows = repo.list_by_job("job-1").await.unwrap();
        assert_eq!(rows.len(), upload_status::ALL.len());
        // Ordered by local_path, so index order matches insertion order here.
        assert_eq!(rows[0].status, upload_status::COMPLETED);
    }

    #[tokio::test]
    async fn upsert_rejects_unknown_status() {
        let pool = setup_pool().await;
        let repo = SqlxUploadRecordRepository::new(pool.clone(), pool.clone());

        let err = repo
            .upsert_records(&[record("job-1", "/tmp/a.mp4", "typo")])
            .await;
        assert!(err.is_err(), "CHECK constraint must reject unknown status");
    }

    #[tokio::test]
    async fn retry_upsert_flips_failed_to_completed_and_keeps_size() {
        let pool = setup_pool().await;
        let repo = SqlxUploadRecordRepository::new(pool.clone(), pool.clone());

        let mut failed = record("job-1", "/tmp/a.mp4", upload_status::FAILED);
        failed.error = Some("connection reset".to_string());
        failed.completed_at = None;
        repo.upsert_records(&[failed]).await.unwrap();

        // Retry after a move-resume: the source file is gone, so the retry
        // reports size_bytes = None. COALESCE must keep the original size.
        let mut retried = record("job-1", "/tmp/a.mp4", upload_status::COMPLETED);
        retried.size_bytes = None;
        retried.updated_at = 2_000;
        retried.completed_at = Some(2_000);
        repo.upsert_records(&[retried]).await.unwrap();

        let rows = repo.list_by_job("job-1").await.unwrap();
        assert_eq!(rows.len(), 1, "same (job_id, local_path) must upsert");
        assert_eq!(rows[0].status, upload_status::COMPLETED);
        assert_eq!(rows[0].size_bytes, Some(1024));
        assert_eq!(rows[0].completed_at, Some(2_000));
        assert!(rows[0].error.is_none());
    }

    #[tokio::test]
    async fn list_by_local_paths_returns_newest_first() {
        let pool = setup_pool().await;
        let repo = SqlxUploadRecordRepository::new(pool.clone(), pool.clone());

        let mut old = record("job-1", "/tmp/a.mp4", upload_status::FAILED);
        old.updated_at = 1_000;
        let mut new = record("job-2", "/tmp/a.mp4", upload_status::COMPLETED);
        new.updated_at = 2_000;
        repo.upsert_records(&[old, new]).await.unwrap();

        let rows = repo
            .list_by_local_paths(&["/tmp/a.mp4".to_string(), "/tmp/missing.mp4".to_string()])
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, upload_status::COMPLETED, "newest first");
    }
}
