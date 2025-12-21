use dashmap::DashSet;
use rand::random;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::task::JoinSet;

use rust_srec::database::models::JobDbModel;
use rust_srec::database::repositories::{JobRepository, SqlxJobRepository};
use rust_srec::database::{DbPool, run_migrations};

fn is_sqlite_busy(err: &sqlx::Error) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("database is locked") || msg.contains("database is busy")
}

async fn init_stress_pool(database_url: &str) -> DbPool {
    let connect_options = SqliteConnectOptions::from_str(database_url)
        .unwrap()
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        // Make SQLITE_BUSY surface quickly so retry logic is exercised.
        .busy_timeout(Duration::from_millis(1))
        .foreign_keys(true)
        .create_if_missing(true);

    SqlitePoolOptions::new()
        .max_connections(32)
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("PRAGMA busy_timeout = 1")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("PRAGMA wal_autocheckpoint = 100")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(connect_options)
        .await
        .unwrap()
}

async fn mark_completed_retry(pool: &DbPool, job_id: &str) {
    let mut attempt: u32 = 0;
    loop {
        let now = chrono::Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE job SET status = 'COMPLETED', completed_at = ?, updated_at = ? WHERE id = ? AND status = 'PROCESSING'",
        )
        .bind(&now)
        .bind(&now)
        .bind(job_id)
        .execute(pool)
        .await;

        match res {
            Ok(done) => {
                assert_eq!(
                    done.rows_affected(),
                    1,
                    "job {} completion transition was lost",
                    job_id
                );
                return;
            }
            Err(e) if is_sqlite_busy(&e) && attempt < 50 => {
                let base_ms = 1u64.saturating_mul(1u64 << attempt.min(6));
                let jitter_ms = random::<u64>() % 5;
                tokio::time::sleep(Duration::from_millis((base_ms + jitter_ms).min(50))).await;
                attempt += 1;
            }
            Err(e) => panic!("failed to mark job completed: {e}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "stress test; run explicitly to validate SQLite claim correctness under contention"]
async fn sqlite_claim_stress_no_double_claims_or_lost_transitions() {
    const JOBS: usize = 300;
    const WORKERS: usize = 24;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("stress.db");
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        db_path.to_string_lossy().replace('\\', "/")
    );

    let pool = init_stress_pool(&db_url).await;
    run_migrations(&pool).await.unwrap();

    let repo = Arc::new(SqlxJobRepository::new(pool.clone()));

    // Seed a backlog of PENDING jobs.
    for i in 0..JOBS {
        let mut job = JobDbModel::new_pipeline(
            format!("input-{i}"),
            (i % 10) as i32,
            Some("streamer".to_string()),
            Some("session".to_string()),
            "{}",
        );
        job.job_type = "remux".to_string();
        job.priority = ((i % 5) as i32) - 2;
        repo.create_job(&job).await.unwrap();
    }

    // Background writer that periodically holds the write lock briefly to force SQLITE_BUSY.
    let locker_pool = pool.clone();
    let locker = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if let Ok(mut tx) = locker_pool.begin().await {
                let _ = sqlx::query("UPDATE job SET updated_at = updated_at WHERE id IN (SELECT id FROM job LIMIT 1)")
                    .execute(&mut *tx)
                    .await;
                tokio::time::sleep(Duration::from_millis(5)).await;
                let _ = tx.commit().await;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });

    let claimed_ids = Arc::new(DashSet::<String>::new());

    let mut workers = JoinSet::new();
    for _ in 0..WORKERS {
        let repo = repo.clone();
        let pool = pool.clone();
        let claimed_ids = claimed_ids.clone();
        workers.spawn(async move {
            loop {
                match repo.claim_next_pending_job(None).await.unwrap() {
                    Some(claimed) => {
                        let inserted = claimed_ids.insert(claimed.id.clone());
                        assert!(inserted, "double-claimed job {}", claimed.id);

                        // Add a tiny jitter to increase interleavings.
                        if random::<u8>().is_multiple_of(3) {
                            tokio::task::yield_now().await;
                        } else {
                            tokio::time::sleep(Duration::from_millis((random::<u64>() % 3) as u64))
                                .await;
                        }

                        mark_completed_retry(&pool, &claimed.id).await;
                    }
                    None => {
                        // Avoid "spurious None" under contention by re-checking pending count.
                        if repo.count_pending_jobs(None).await.unwrap() == 0 {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                }
            }
        });
    }

    let joined = tokio::time::timeout(Duration::from_secs(30), async {
        while workers.join_next().await.is_some() {}
    })
    .await;
    assert!(joined.is_ok(), "workers timed out (possible deadlock)");

    let _ = locker.await;

    assert_eq!(claimed_ids.len(), JOBS, "not all jobs were claimed");

    let counts = repo.get_job_counts_by_status().await.unwrap();
    assert_eq!(counts.pending, 0, "pending jobs remain");
    assert_eq!(counts.processing, 0, "processing jobs remain");
    assert_eq!(counts.completed, JOBS as u64, "not all jobs completed");

    let missing_times: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM job WHERE started_at IS NULL OR completed_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(missing_times, 0, "some jobs missing timestamps");
}
