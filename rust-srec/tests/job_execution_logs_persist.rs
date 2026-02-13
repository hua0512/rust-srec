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
