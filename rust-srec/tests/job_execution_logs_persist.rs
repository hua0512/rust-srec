use std::sync::Arc;

use rust_srec::database;
use rust_srec::database::models::Pagination;
use rust_srec::database::repositories::JobRepository;
use rust_srec::database::repositories::SqlxJobRepository;
use rust_srec::pipeline::{
    Job, JobExecutionInfo, JobLogEntry, JobQueue, JobQueueConfig, JobResult, LogLevel,
};
use tempfile::TempDir;

async fn log_queue() -> (TempDir, JobQueue, Arc<SqlxJobRepository>, String) {
    let dir = TempDir::new().unwrap();
    let db_url = format!(
        "sqlite:{}?mode=rwc",
        dir.path()
            .join("logs.db")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let pool = database::init_pool(&db_url).await.unwrap();
    database::run_migrations(&pool).await.unwrap();
    let repo = Arc::new(SqlxJobRepository::new(pool.clone(), pool));
    let queue = JobQueue::with_repository(JobQueueConfig::default(), repo.clone());
    let job_id = queue
        .enqueue(Job::new("remux", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    (dir, queue, repo, job_id)
}

#[tokio::test]
async fn beginning_fresh_and_legacy_empty_execution_persists_processor_without_logs() {
    let (_dir, queue, repo, job_id) = log_queue().await;
    assert_eq!(repo.get_job_execution_info(&job_id).await.unwrap(), None);
    assert_eq!(
        repo.get_job_execution_info("missing-job").await.unwrap(),
        None
    );
    for legacy_empty in [false, true] {
        if legacy_empty {
            repo.update_job_execution_info(&job_id, "").await.unwrap();
        }
        queue
            .begin_execution(&job_id, "fresh-processor")
            .await
            .unwrap();
        let info: JobExecutionInfo =
            serde_json::from_str(&repo.get_job_execution_info(&job_id).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(info.current_processor.as_deref(), Some("fresh-processor"));
        assert!(info.logs.is_empty());
        assert_eq!(
            repo.list_execution_logs(&job_id, &Pagination::new(100, 0))
                .await
                .unwrap()
                .1,
            0
        );
    }
}

#[tokio::test]
async fn beginning_retry_preserves_metadata_and_never_replays_historical_log_rows() {
    let (_dir, queue, repo, job_id) = log_queue().await;
    let prior = JobLogEntry::info("previous attempt log");
    queue
        .append_log_entry(&job_id, std::slice::from_ref(&prior))
        .await
        .unwrap();
    let mut info = JobExecutionInfo::new()
        .with_processor("previous")
        .with_step(2, 4)
        .with_input_size(123)
        .with_output_size(456);
    info.logs.push_back(prior);
    info.log_lines_total = 1;
    info.items_produced.push("published.mp4".to_owned());
    let now = chrono::Utc::now();
    info.record_step_duration(1, "previous", 2.5, now, now);
    queue
        .update_execution_info(&job_id, info.clone())
        .await
        .unwrap();
    let mut expected = serde_json::to_value(&info).unwrap();
    expected["extension_metadata"] = serde_json::json!({"preserve": true});
    repo.update_job_execution_info(&job_id, &expected.to_string())
        .await
        .unwrap();
    for processor in ["retry-one", "retry-two"] {
        queue.begin_execution(&job_id, processor).await.unwrap();
        expected["current_processor"] = serde_json::json!(processor);
        let stored: serde_json::Value =
            serde_json::from_str(&repo.get_job_execution_info(&job_id).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(stored, expected);
        let (_, total) = repo
            .list_execution_logs(&job_id, &Pagination::new(100, 0))
            .await
            .unwrap();
        assert_eq!(
            total, 1,
            "starting attempts must not reinsert historical snapshots"
        );
    }
    repo.update_job_execution_info(&job_id, "[invalid metadata]")
        .await
        .unwrap();
    assert!(queue.begin_execution(&job_id, "retry-three").await.is_err());
    assert_eq!(
        repo.get_job_execution_info(&job_id)
            .await
            .unwrap()
            .as_deref(),
        Some("[invalid metadata]")
    );
}

#[tokio::test]
async fn retry_terminal_updates_preserve_history_and_extension_metadata() {
    for fail in [false, true] {
        let (_dir, queue, repo, job_id) = log_queue().await;
        let prior_log = JobLogEntry::warn("prior attempt warning");
        queue
            .append_log_entry(&job_id, std::slice::from_ref(&prior_log))
            .await
            .unwrap();
        let mut prior = JobExecutionInfo::new()
            .with_processor("previous")
            .with_step(2, 4)
            .with_input_size(123)
            .with_output_size(456);
        prior.logs.push_back(prior_log);
        prior.log_lines_total = 1;
        prior.log_warn_count = 1;
        prior.items_produced.push("published.mp4".to_owned());
        let now = chrono::Utc::now();
        prior.record_step_duration(1, "previous", 2.5, now, now);
        let mut document = serde_json::to_value(&prior).unwrap();
        document["extension_metadata"] = serde_json::json!({"attempts": [{"kept": true}]});
        repo.update_job_execution_info(&job_id, &document.to_string())
            .await
            .unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        queue.begin_execution(&job_id, "retry").await.unwrap();
        queue
            .record_produced_items(&job_id, &["new.mp4".to_owned()])
            .await
            .unwrap();
        if fail {
            queue
                .fail_with_step_info(&job_id, "retry failed", Some("retry"), Some(2), Some(4))
                .await
                .unwrap();
        } else {
            queue
                .complete(
                    &job_id,
                    JobResult {
                        outputs: vec!["new.mp4".to_owned()],
                        duration_secs: 3.0,
                        metadata: None,
                        uploads: vec![],
                        logs: vec![JobLogEntry::info("retry completed")],
                    },
                )
                .await
                .unwrap();
        }
        let row = repo.get_job(&job_id).await.unwrap();
        assert_eq!(row.status, if fail { "FAILED" } else { "COMPLETED" });
        let stored: serde_json::Value =
            serde_json::from_str(row.execution_info.as_deref().unwrap()).unwrap();
        for field in [
            "current_step",
            "total_steps",
            "input_size_bytes",
            "output_size_bytes",
            "step_durations",
            "extension_metadata",
        ] {
            assert_eq!(stored[field], document[field], "lost {field}");
        }
        assert_eq!(stored["current_processor"], "retry");
        assert_eq!(
            stored["items_produced"],
            serde_json::json!(["published.mp4", "new.mp4"])
        );
        assert_eq!(stored["logs"].as_array().unwrap().len(), 2);
        assert_eq!(stored["log_lines_total"], 2);
        assert_eq!(stored["log_warn_count"], 1);
        assert_eq!(stored["log_error_count"], if fail { 1 } else { 0 });
        let (logs, total) = repo
            .list_execution_logs(&job_id, &Pagination::new(100, 0))
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            logs.iter()
                .filter(|log| log.message.as_deref() == Some("prior attempt warning"))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn disjoint_batches_preserve_same_timestamp_and_identical_messages() {
    let (_dir, queue, repo, job_id) = log_queue().await;
    let timestamp = chrono::Utc::now();
    let entry = |message: &str| {
        let mut entry = JobLogEntry::info(message);
        entry.timestamp = timestamp;
        entry
    };
    let first = entry("repeated");
    let second = entry("repeated");
    let third = entry("distinct");
    queue
        .append_log_entry(&job_id, std::slice::from_ref(&first))
        .await
        .unwrap();
    queue
        .append_log_entry(&job_id, &[second.clone(), third.clone()])
        .await
        .unwrap();

    let mut snapshot = JobExecutionInfo::new();
    snapshot.logs.extend([first, second, third]);
    queue
        .update_execution_info(&job_id, snapshot.clone())
        .await
        .unwrap();
    // A new occurrence with exactly the same public fields must still be inserted.
    snapshot.logs.push_back(entry("repeated"));
    queue
        .update_execution_info(&job_id, snapshot.clone())
        .await
        .unwrap();
    queue
        .update_execution_info(&job_id, snapshot)
        .await
        .unwrap();

    let (rows, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 4);
    assert_eq!(
        rows.iter()
            .filter(|row| row.message.as_deref() == Some("repeated"))
            .count(),
        3
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.message.as_deref() == Some("distinct"))
            .count(),
        1
    );
}

#[tokio::test]
async fn rehydrated_execution_snapshot_does_not_reinsert_stored_logs() {
    let (_dir, queue, repo, job_id) = log_queue().await;
    let mut snapshot = JobExecutionInfo::new();
    snapshot.logs.push_back(JobLogEntry::info("stored"));
    queue
        .update_execution_info(&job_id, snapshot)
        .await
        .unwrap();
    let mut recovered = queue
        .get_job(&job_id)
        .await
        .unwrap()
        .unwrap()
        .execution_info
        .unwrap();
    recovered.logs.push_back(JobLogEntry::info("new"));
    queue
        .update_execution_info(&job_id, recovered.clone())
        .await
        .unwrap();
    queue
        .update_execution_info(&job_id, recovered)
        .await
        .unwrap();
    let (_, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 2);
}

#[tokio::test]
async fn legacy_snapshot_logs_survive_the_first_new_log_row() {
    let (_dir, queue, repo, job_id) = log_queue().await;
    let mut legacy = JobExecutionInfo::new();
    legacy.logs.push_back(JobLogEntry::info("legacy"));
    repo.update_job_execution_info(&job_id, &serde_json::to_string(&legacy).unwrap())
        .await
        .unwrap();
    let mut recovered = queue
        .get_job(&job_id)
        .await
        .unwrap()
        .unwrap()
        .execution_info
        .unwrap();
    recovered.logs.push_back(JobLogEntry::info("new"));
    queue
        .update_execution_info(&job_id, recovered.clone())
        .await
        .unwrap();
    queue
        .update_execution_info(&job_id, recovered)
        .await
        .unwrap();
    let (logs, total) = queue
        .list_job_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 2);
    assert!(logs.iter().any(|entry| entry.message == "legacy"));
    assert!(logs.iter().any(|entry| entry.message == "new"));
}

#[tokio::test]
async fn concurrent_snapshots_persist_each_occurrence_once() {
    let (_dir, queue, repo, job_id) = log_queue().await;
    let queue = Arc::new(queue);
    let mut snapshot = JobExecutionInfo::new();
    snapshot
        .logs
        .push_back(JobLogEntry::info("shared occurrence"));
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let queue = queue.clone();
        let job_id = job_id.clone();
        let snapshot = snapshot.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            barrier.wait().await;
            queue
                .update_execution_info(&job_id, snapshot)
                .await
                .unwrap();
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        barrier.wait().await;
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    })
    .await
    .unwrap();
    let (_, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(0, 0))
        .await
        .unwrap();
    assert_eq!(total, 1);
}

#[tokio::test]
async fn concurrent_appends_preserve_cap_after_queue_recreation() {
    const CAP: usize = 5000;
    let (_dir, queue, repo, job_id) = log_queue().await;
    let entries: Vec<_> = (0..CAP - 1)
        .map(|i| JobLogEntry::info(format!("old-{i}")))
        .collect();
    queue.append_log_entry(&job_id, &entries).await.unwrap();
    drop(queue);
    let queue = Arc::new(JobQueue::with_repository(
        JobQueueConfig::default(),
        repo.clone(),
    ));
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..2 {
        let queue = queue.clone();
        let job_id = job_id.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            let entry = JobLogEntry::info(format!("new-{i}"));
            barrier.wait().await;
            queue.append_log_entry(&job_id, &[entry]).await.unwrap();
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        barrier.wait().await;
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    })
    .await
    .unwrap();
    let (logs, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(CAP as u32, 0))
        .await
        .unwrap();
    assert_eq!(total, CAP as u64);
    assert_eq!(
        logs.iter()
            .filter(|log| log
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("new-")))
            .count(),
        2
    );
}

#[tokio::test]
async fn completion_snapshot_does_not_replay_streamed_logs_after_ring_trimming() {
    const CAP: usize = 5000;
    let (_dir, queue, repo, job_id) = log_queue().await;
    queue.dequeue(None).await.unwrap().unwrap();
    let base = chrono::Utc::now();
    let mut retained_warning = JobLogEntry::warn("early warning");
    retained_warning.timestamp = base;
    queue
        .append_log_entry(&job_id, std::slice::from_ref(&retained_warning))
        .await
        .unwrap();
    let batch: Vec<_> = (1..=CAP + 1)
        .map(|i| {
            let mut entry = JobLogEntry::info(format!("line-{i}"));
            entry.timestamp = base + chrono::Duration::milliseconds(i as i64);
            entry
        })
        .collect();
    queue.append_log_entry(&job_id, &batch).await.unwrap();
    let mut final_entry = JobLogEntry::info("completion only");
    final_entry.timestamp = base + chrono::Duration::milliseconds((CAP + 2) as i64);
    queue
        .complete(
            &job_id,
            JobResult {
                outputs: vec![],
                duration_secs: 0.0,
                metadata: None,
                uploads: vec![],
                logs: vec![retained_warning, batch.last().unwrap().clone(), final_entry],
            },
        )
        .await
        .unwrap();
    let (rows, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(CAP as u32, 0))
        .await
        .unwrap();
    assert_eq!(total, CAP as u64);
    assert!(
        !rows
            .iter()
            .any(|row| row.message.as_deref() == Some("early warning"))
    );
    // The newly appended completion line is the newest row after trimming.
    let (last, _) = repo
        .list_execution_logs(&job_id, &Pagination::new(1, (CAP - 1) as u32))
        .await
        .unwrap();
    assert_eq!(last[0].message.as_deref(), Some("completion only"));
    let (first, _) = repo
        .list_execution_logs(&job_id, &Pagination::new(1, 0))
        .await
        .unwrap();
    assert_eq!(first[0].message.as_deref(), Some("line-3"));
}

#[test]
fn log_persistence_provenance_is_not_serialized() {
    let entry = JobLogEntry::info("public log");
    let value = serde_json::to_value(&entry).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert_eq!(value["message"], "public log");
    assert_eq!(value["level"], "info");
    assert!(value.get("timestamp").is_some());
    let decoded: JobLogEntry = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[tokio::test]
async fn failed_append_does_not_mark_snapshot_entries_as_persisted() {
    let (dir, queue, repo, job_id) = log_queue().await;
    let db_url = format!(
        "sqlite:{}?mode=rw",
        dir.path()
            .join("logs.db")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_logs BEFORE INSERT ON job_execution_logs
         BEGIN SELECT RAISE(FAIL, 'injected log failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let entry = JobLogEntry::info("retry me");
    assert!(
        queue
            .append_log_entry(&job_id, std::slice::from_ref(&entry))
            .await
            .is_err()
    );
    sqlx::query("DROP TRIGGER reject_logs")
        .execute(&pool)
        .await
        .unwrap();
    let mut snapshot = JobExecutionInfo::new();
    snapshot.logs.push_back(entry);
    queue
        .update_execution_info(&job_id, snapshot.clone())
        .await
        .unwrap();
    queue
        .update_execution_info(&job_id, snapshot)
        .await
        .unwrap();
    let (_, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(100, 0))
        .await
        .unwrap();
    assert_eq!(total, 1);
    pool.close().await;
}

#[tokio::test]
async fn successful_trim_retries_excess_left_by_a_failed_trim() {
    const CAP: usize = 5000;
    let (dir, queue, repo, job_id) = log_queue().await;
    let db_url = format!(
        "sqlite:{}?mode=rw",
        dir.path()
            .join("logs.db")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let pool = sqlx::SqlitePool::connect(&db_url).await.unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_log_trim BEFORE DELETE ON job_execution_logs
         BEGIN SELECT RAISE(FAIL, 'injected trim failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let entries: Vec<_> = (0..CAP + 1)
        .map(|i| JobLogEntry::info(format!("line-{i}")))
        .collect();
    queue.append_log_entry(&job_id, &entries).await.unwrap();
    let (_, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(0, 0))
        .await
        .unwrap();
    assert_eq!(
        total,
        (CAP + 1) as u64,
        "successful insertion survives trim failure"
    );
    sqlx::query("DROP TRIGGER reject_log_trim")
        .execute(&pool)
        .await
        .unwrap();
    queue
        .append_log_entry(&job_id, &[JobLogEntry::info("after trim recovery")])
        .await
        .unwrap();
    let (rows, total) = repo
        .list_execution_logs(&job_id, &Pagination::new(CAP as u32, 0))
        .await
        .unwrap();
    assert_eq!(
        total, CAP as u64,
        "both accumulated excess rows must be removed"
    );
    assert!(
        rows.iter()
            .any(|row| row.message.as_deref() == Some("after trim recovery"))
    );
    pool.close().await;
}

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
