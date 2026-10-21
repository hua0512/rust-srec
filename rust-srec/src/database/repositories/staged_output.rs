//! `staged_outputs` table access.
//!
//! Pipeline attempts record each temporary file before creating it and remove
//! their rows when the worker finalizes them; both writes are best effort in
//! the caller (`StagedOutputRecorder`). Startup recovery marks rows an earlier
//! process left behind as interrupted, and the API lists and settles those.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::WritePool;
use crate::database::models::StagedOutputDbModel;
use crate::database::retry::retry_on_sqlite_busy;
use crate::{Error, Result};

const COLUMNS: &str = "id, path, output_path, job_id, attempt_id, created_at, interrupted_at";

/// A temporary file an attempt is about to create.
#[derive(Debug, Clone)]
pub struct NewStagedOutput<'a> {
    pub path: &'a str,
    pub output_path: &'a str,
    pub job_id: &'a str,
    pub attempt_id: &'a str,
}

/// Repository for temporary files staged by pipeline attempts.
#[async_trait]
pub trait StagedOutputRepository: Send + Sync {
    /// Record a temporary file before it is created.
    async fn record(&self, staged: NewStagedOutput<'_>, now_ms: i64) -> Result<()>;

    /// Forget the rows of a finalized attempt. Interrupted rows are never
    /// touched: they belong to an earlier process, whose attempt IDs this
    /// process does not reuse.
    async fn remove_attempt(&self, attempt_id: &str) -> Result<u64>;

    /// Mark every row of a live attempt as interrupted. Only startup recovery
    /// may call this, before any worker can stage a file.
    async fn mark_interrupted(&self, now_ms: i64) -> Result<u64>;

    /// Interrupted rows, optionally for one job, newest interruption first.
    async fn list_interrupted(
        &self,
        job_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StagedOutputDbModel>>;

    /// One interrupted row.
    async fn get_interrupted(&self, id: &str) -> Result<StagedOutputDbModel>;

    /// Forget an interrupted row. Returns whether it existed.
    async fn forget_interrupted(&self, id: &str) -> Result<bool>;
}

pub struct SqlxStagedOutputRepository {
    pool: SqlitePool,
    write_pool: WritePool,
}

impl SqlxStagedOutputRepository {
    pub fn new(pool: SqlitePool, write_pool: WritePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl StagedOutputRepository for SqlxStagedOutputRepository {
    async fn record(&self, staged: NewStagedOutput<'_>, now_ms: i64) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        retry_on_sqlite_busy("record_staged_output", || async {
            sqlx::query(
                "INSERT INTO staged_outputs (id, path, output_path, job_id, attempt_id, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(staged.path)
            .bind(staged.output_path)
            .bind(staged.job_id)
            .bind(staged.attempt_id)
            .bind(now_ms)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn remove_attempt(&self, attempt_id: &str) -> Result<u64> {
        retry_on_sqlite_busy("remove_staged_outputs", || async {
            let result = sqlx::query(
                "DELETE FROM staged_outputs WHERE attempt_id = ? AND interrupted_at IS NULL",
            )
            .bind(attempt_id)
            .execute(&self.write_pool)
            .await?;
            Ok(result.rows_affected())
        })
        .await
    }

    async fn mark_interrupted(&self, now_ms: i64) -> Result<u64> {
        retry_on_sqlite_busy("mark_staged_outputs_interrupted", || async {
            let result = sqlx::query(
                "UPDATE staged_outputs SET interrupted_at = ? WHERE interrupted_at IS NULL",
            )
            .bind(now_ms)
            .execute(&self.write_pool)
            .await?;
            Ok(result.rows_affected())
        })
        .await
    }

    async fn list_interrupted(
        &self,
        job_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<StagedOutputDbModel>> {
        let rows = sqlx::query_as::<_, StagedOutputDbModel>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM staged_outputs \
             WHERE interrupted_at IS NOT NULL AND (?1 IS NULL OR job_id = ?1) \
             ORDER BY interrupted_at DESC, created_at DESC, id \
             LIMIT ?2"
        )))
        .bind(job_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    async fn get_interrupted(&self, id: &str) -> Result<StagedOutputDbModel> {
        sqlx::query_as::<_, StagedOutputDbModel>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM staged_outputs WHERE id = ? AND interrupted_at IS NOT NULL"
        )))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("Interrupted output", id))
    }

    async fn forget_interrupted(&self, id: &str) -> Result<bool> {
        retry_on_sqlite_busy("forget_staged_output", || async {
            let result = sqlx::query(
                "DELETE FROM staged_outputs WHERE id = ? AND interrupted_at IS NOT NULL",
            )
            .bind(id)
            .execute(&self.write_pool)
            .await?;
            Ok(result.rows_affected() > 0)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{init_pool_with_size, run_migrations};

    async fn repository() -> SqlxStagedOutputRepository {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        SqlxStagedOutputRepository::new(pool.clone(), pool)
    }

    async fn stage(repo: &SqlxStagedOutputRepository, path: &str, job: &str, attempt: &str) {
        repo.record(
            NewStagedOutput {
                path,
                output_path: "/out/video.mp4",
                job_id: job,
                attempt_id: attempt,
            },
            1_000,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn finalizing_an_attempt_leaves_a_successor_and_earlier_leftovers() {
        let repo = repository().await;
        stage(&repo, "/out/a.tmp-1.mp4", "job-1", "attempt-old").await;
        repo.mark_interrupted(2_000).await.unwrap();
        stage(&repo, "/out/a.tmp-2.mp4", "job-1", "attempt-old").await;
        stage(&repo, "/out/a.tmp-3.mp4", "job-1", "attempt-new").await;

        assert_eq!(repo.remove_attempt("attempt-old").await.unwrap(), 1);

        // The successor's row stays live, and the leftover from before the
        // restart stays interrupted even though it shares the attempt ID.
        assert_eq!(repo.mark_interrupted(3_000).await.unwrap(), 1);
        let paths: Vec<_> = repo
            .list_interrupted(Some("job-1"), 10)
            .await
            .unwrap()
            .into_iter()
            .map(|row| (row.path, row.interrupted_at))
            .collect();
        assert_eq!(
            paths,
            [
                ("/out/a.tmp-3.mp4".to_string(), Some(3_000)),
                ("/out/a.tmp-1.mp4".to_string(), Some(2_000)),
            ]
        );
        assert_eq!(repo.list_interrupted(None, 1).await.unwrap().len(), 1);

        // A live row is not a leftover, so it cannot be forgotten.
        stage(&repo, "/out/a.tmp-4.mp4", "job-1", "attempt-next").await;
        let live = sqlx::query_scalar::<_, String>(
            "SELECT id FROM staged_outputs WHERE interrupted_at IS NULL",
        )
        .fetch_one(&repo.pool)
        .await
        .unwrap();
        assert!(!repo.forget_interrupted(&live).await.unwrap());
    }
}
