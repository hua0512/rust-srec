use std::sync::Arc;

use rust_srec::database;
use rust_srec::database::models::Pagination;
use rust_srec::database::repositories::JobRepository;
use rust_srec::database::repositories::SqlxJobRepository;
use rust_srec::pipeline::{Job, JobExecutionInfo, JobLogEntry, JobQueue, JobQueueConfig, LogLevel};
use tempfile::TempDir;

#[tokio::test]
async fn update_execution_info_persists_logs_to_job_execution_logs() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("job_logs.db");
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        db_path.to_string_lossy().replace('\\', "/")
    );

    let pool = database::init_pool(&db_url).await.unwrap();
    database::run_migrations(&pool).await.unwrap();

    let repo = Arc::new(SqlxJobRepository::new(pool.clone(), pool));
    let queue = JobQueue::with_repository(JobQueueConfig::default(), repo.clone());

    let job = Job::new(
        "remux",
        vec!["/input.flv".to_string()],
        vec![],
        "streamer-1",
        "session-1",
    );
    let job_id = queue.enqueue(job).await.unwrap();

    let mut exec_info = JobExecutionInfo::new();
    exec_info.add_log(JobLogEntry::new(LogLevel::Info, "hello"));
    queue
        .update_execution_info(&job_id, exec_info.clone())
        .await
        .unwrap();

    let (rows, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows.len(), 1);

    queue
        .update_execution_info(&job_id, exec_info.clone())
        .await
        .unwrap();
    let (_rows, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 1, "should dedupe identical updates");

    exec_info.add_log(JobLogEntry::new(LogLevel::Warn, "second"));
    queue
        .update_execution_info(&job_id, exec_info)
        .await
        .unwrap();

    let (_rows, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 2);
}

#[tokio::test]
async fn persisted_logs_are_ring_buffered_per_job() {
    // Mirrors MAX_PERSISTED_LOG_ROWS_PER_JOB in pipeline::job_queue; the
    // assertions below pin that contract for one job run.
    const CAP: usize = 5000;

    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("job_logs_ring.db");
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        db_path.to_string_lossy().replace('\\', "/")
    );

    let pool = database::init_pool(&db_url).await.unwrap();
    database::run_migrations(&pool).await.unwrap();

    let repo = Arc::new(SqlxJobRepository::new(pool.clone(), pool));
    let queue = JobQueue::with_repository(JobQueueConfig::default(), repo.clone());

    let job = Job::new(
        "rclone",
        vec!["/input.mp4".to_string()],
        vec![],
        "streamer-1",
        "session-1",
    );
    let job_id = queue.enqueue(job).await.unwrap();

    // Strictly increasing timestamps so "oldest" is well-defined for the
    // trim's created_at ordering.
    let base = chrono::Utc::now();
    let entry_at = |i: usize| {
        let mut entry = JobLogEntry::new(LogLevel::Info, format!("line-{i:04}"));
        entry.timestamp = base + chrono::Duration::milliseconds(i as i64);
        entry
    };

    let first_batch: Vec<JobLogEntry> = (0..CAP + 10).map(entry_at).collect();
    queue.append_log_entry(&job_id, &first_batch).await.unwrap();

    let (oldest, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(1, 0))
        .await
        .unwrap();
    assert_eq!(total as usize, CAP, "rows stay at the cap after overflow");
    assert_eq!(
        oldest[0].message.as_deref(),
        Some("line-0010"),
        "the oldest rows are the ones trimmed"
    );

    // Later flushes keep rotating: 5 new rows in, the 5 oldest out.
    let second_batch: Vec<JobLogEntry> = (CAP + 10..CAP + 15).map(entry_at).collect();
    queue
        .append_log_entry(&job_id, &second_batch)
        .await
        .unwrap();

    let (oldest, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(1, 0))
        .await
        .unwrap();
    assert_eq!(total as usize, CAP);
    assert_eq!(oldest[0].message.as_deref(), Some("line-0015"));
}
