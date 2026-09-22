use super::*;

const NOW: i64 = 2_000_000_000_000;
const OLD: i64 = NOW - 60 * MILLIS_PER_DAY;

#[tokio::test]
async fn output_retention_migration_preserves_existing_settings_and_defaults_to_disabled() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE media_outputs (file_path TEXT NOT NULL);
                   CREATE TABLE global_config (id TEXT PRIMARY KEY, job_history_retention_days INTEGER NOT NULL);
                   INSERT INTO global_config VALUES ('existing', 90);")
        .execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../migrations/20260922120000_add_output_retention.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let settings: (i32, i32, bool) = sqlx::query_as(
        "SELECT job_history_retention_days, output_retention_days, output_retention_delete_files FROM global_config",
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(settings, (90, 0, false));
    assert!(
        sqlx::query("UPDATE global_config SET output_retention_days = -1")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE global_config SET output_retention_delete_files = 2")
            .execute(&pool)
            .await
            .is_err()
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
}

async fn policy(database: &TestDatabase, days: i32, delete_files: bool) {
    let mut config = database
        .config_repository
        .get_global_config()
        .await
        .unwrap();
    config.output_retention_days = days;
    config.output_retention_delete_files = delete_files;
    database
        .config_repository
        .update_global_config(&config)
        .await
        .unwrap();
}

async fn output(database: &TestDatabase, id: &str) -> PathBuf {
    let path = database._directory.path().join(format!("{id}.mp4"));
    tokio::fs::write(&path, b"video").await.unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(OLD as u64))
        .unwrap();
    database
        .session_repository
        .create_session(&LiveSessionDbModel {
            id: id.to_owned(),
            streamer_id: Some("maintenance-streamer".to_owned()),
            streamer_name: Some("Maintenance".to_owned()),
            start_time: OLD - 1000,
            end_time: Some(OLD),
            titles: None,
            total_size_bytes: 0,
        })
        .await
        .unwrap();
    let mut output = MediaOutputDbModel::new(id, path.to_string_lossy(), MediaFileType::Video, 5);
    output.id = id.to_owned();
    output.created_at = OLD;
    database
        .session_repository
        .create_media_output(&output)
        .await
        .unwrap();
    path
}

async fn has_output(database: &TestDatabase, id: &str) -> bool {
    database
        .session_repository
        .get_media_output(id)
        .await
        .is_ok()
}

#[tokio::test]
async fn output_retention_defaults_off_and_records_only_does_not_unlink() {
    let database = setup(10).await;
    seed_streamer(&database).await;
    let path = output(&database, "record-only").await;
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert_eq!(report.outputs_deleted, 0);
    assert!(has_output(&database, "record-only").await);
    policy(&database, 30, false).await;
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(
        (report.outputs_deleted, report.output_files_deleted),
        (1, 0)
    );
    assert!(path.exists());
    assert!(!has_output(&database, "record-only").await);
    // Switching modes later cannot recover a path after records-only deletion.
    policy(&database, 30, true).await;
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        0
    );
    assert!(path.exists());
}

#[tokio::test]
async fn output_retention_removes_files_and_reconciles_missing_files() {
    let database = setup(1).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    let path = output(&database, "existing").await;
    let missing = output(&database, "missing").await;
    tokio::fs::remove_file(missing).await.unwrap();
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(
        (report.outputs_deleted, report.output_files_deleted),
        (2, 1)
    );
    assert!(!path.exists());
    assert!(!has_output(&database, "existing").await);
    assert!(!has_output(&database, "missing").await);
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        0
    );
    let errors: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&database.pool)
        .await
        .unwrap();
    assert!(errors.is_empty());
}

#[tokio::test]
async fn output_retention_preserves_active_recent_and_retry_dependencies() {
    let database = setup(20).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    for id in [
        "active-session",
        "recent-output",
        "recent-session",
        "pending-job",
        "retry-job",
        "recent-job",
        "active-dag",
        "recent-dag",
    ] {
        output(&database, id).await;
    }
    sqlx::query("UPDATE live_sessions SET end_time = NULL WHERE id = 'active-session'")
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE live_sessions SET end_time = ? WHERE id = 'recent-session'")
        .bind(NOW)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE media_outputs SET created_at = ? WHERE id = 'recent-output'")
        .bind(NOW)
        .execute(&database.pool)
        .await
        .unwrap();
    for (id, status, updated, retry) in [
        ("pending-job", JobStatus::Pending, OLD, None),
        (
            "retry-job",
            JobStatus::Failed,
            OLD,
            Some(NOW + MILLIS_PER_DAY),
        ),
        ("recent-job", JobStatus::Completed, NOW, None),
    ] {
        let mut job = JobDbModel::new("PROCESS", "{}");
        job.id = id.to_owned();
        job.session_id = Some(id.to_owned());
        job.status = status.as_str().to_owned();
        job.created_at = OLD;
        job.updated_at = updated;
        job.retry_after = retry;
        database.job_repository.create_job(&job).await.unwrap();
    }
    for (id, status, updated) in [
        ("active-dag", DagExecutionStatus::Processing, OLD),
        ("recent-dag", DagExecutionStatus::Completed, NOW),
    ] {
        let mut dag = DagExecutionDbModel::new(
            &crate::database::models::DagPipelineDefinition::new("retention", vec![]),
            None,
            Some(id.to_owned()),
        );
        dag.id = id.to_owned();
        dag.session_id = Some(id.to_owned());
        dag.status = status.as_str().to_owned();
        dag.created_at = OLD;
        dag.updated_at = updated;
        database.dag_repository.create_dag(&dag).await.unwrap();
    }
    for _ in 0..2 {
        let report = database.scheduler.run_maintenance_at(NOW).await;
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.outputs_deleted, 0);
    }
    assert!(database.job_repository.get_job("retry-job").await.is_ok());
}

#[tokio::test]
async fn output_retention_advances_past_failures_and_retries_without_double_accounting() {
    let database = setup(1).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    let blocked = output(&database, "a-blocked").await;
    tokio::fs::remove_file(&blocked).await.unwrap();
    tokio::fs::create_dir(&blocked).await.unwrap();
    let good = output(&database, "b-good").await;
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert_eq!(report.outputs_deleted, 1, "{:?}", report.failures);
    assert_eq!(report.failures.len(), 1, "{:?}", report.failures);
    assert!(has_output(&database, "a-blocked").await);
    assert!(!good.exists());
    assert_eq!(
        database
            .session_repository
            .get_session("a-blocked")
            .await
            .unwrap()
            .total_size_bytes,
        5
    );
    tokio::fs::remove_dir(&blocked).await.unwrap();
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert_eq!(
        (report.outputs_deleted, report.output_files_deleted),
        (1, 0)
    );
    assert!(report.failures.is_empty());
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        0
    );
}

#[tokio::test]
async fn output_retention_defers_while_a_processor_owns_files() {
    let database = setup(10).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    let path = output(&database, "leased").await;
    let gate = database.scheduler.output_files_gate.as_ref().unwrap();
    let lease = gate.read().await;
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        0
    );
    assert!(path.exists());
    drop(lease);
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.outputs_deleted, 1);
}

#[tokio::test]
async fn output_retention_protects_active_recording_directories_before_segment_events() {
    let mut database = setup(10).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    let original = output(&database, "recording-directory").await;
    let directory = database._directory.path().join("live");
    tokio::fs::create_dir(&directory).await.unwrap();
    let protected = directory.join("previous-session.mp4");
    tokio::fs::rename(original, &protected).await.unwrap();
    sqlx::query("UPDATE media_outputs SET file_path = ? WHERE id = 'recording-directory'")
        .bind(protected.to_string_lossy().as_ref())
        .execute(&database.pool)
        .await
        .unwrap();
    let unrelated = output(&database, "unrelated-directory").await;
    let manager = Arc::new(DownloadManager::new());
    manager.register_engine(Arc::new(ActiveRecordingEngine));
    Arc::get_mut(&mut database.scheduler)
        .unwrap()
        .download_manager = Arc::downgrade(&manager);
    let config = crate::downloader::engine::DownloadConfig::new(
        "https://invalid.test/live",
        &directory,
        "streamer",
        "Streamer",
        "new-session",
    );
    let id = tokio::time::timeout(
        Duration::from_secs(5),
        manager.start_download(config, Some("MESIO".to_owned()), false),
    )
    .await
    .unwrap()
    .unwrap();
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.outputs_deleted, 1);
    assert!(protected.exists());
    assert!(!unrelated.exists());
    tokio::time::timeout(Duration::from_secs(2), manager.stop_download(&id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        1
    );
    assert!(!protected.exists());
}

#[tokio::test]
async fn output_retention_preserves_shared_paths_and_unavailable_directories() {
    let database = setup(10).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    let modified = output(&database, "recently-modified").await;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&modified)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(NOW as u64))
        .unwrap();
    let shared = output(&database, "shared").await;
    let mut other =
        MediaOutputDbModel::new("shared", shared.to_string_lossy(), MediaFileType::Video, 0);
    other.created_at = NOW;
    database
        .session_repository
        .create_media_output(&other)
        .await
        .unwrap();
    output(&database, "offline").await;
    sqlx::query("UPDATE media_outputs SET file_path = ? WHERE id = 'offline'")
        .bind(
            database
                ._directory
                .path()
                .join("unavailable/file.mp4")
                .to_string_lossy()
                .as_ref(),
        )
        .execute(&database.pool)
        .await
        .unwrap();
    let report = database.scheduler.run_maintenance_at(NOW).await;
    assert_eq!(report.outputs_deleted, 0);
    assert_eq!(report.failures.len(), 1);
    assert!(shared.exists());
    assert!(modified.exists());
    assert!(has_output(&database, "recently-modified").await);
    assert!(has_output(&database, "offline").await);
}

#[tokio::test]
async fn output_retention_honors_cancellation_and_per_sweep_budget() {
    let database = setup_with_config(MaintenanceConfig {
        batch_size: 1,
        max_batches_per_task: 1,
        ..MaintenanceConfig::default()
    })
    .await;
    seed_streamer(&database).await;
    policy(&database, 30, false).await;
    output(&database, "first").await;
    output(&database, "second").await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let report = database
        .scheduler
        .run_maintenance_with_cancellation(NOW, &cancellation)
        .await;
    assert!(report.cancelled);
    assert_eq!(report.outputs_deleted, 0);
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        1
    );
    assert_eq!(
        database
            .scheduler
            .run_maintenance_at(NOW)
            .await
            .outputs_deleted,
        1
    );
}

#[tokio::test]
async fn output_retention_keeps_retry_dag_history_until_retry_settles() {
    let database = setup(10).await;
    seed_streamer(&database).await;
    policy(&database, 30, true).await;
    let path = output(&database, "retry-session").await;
    insert_dag_job(
        &database,
        DagJobFixture {
            dag_id: "retry-dag",
            dag_status: DagExecutionStatus::Failed,
            dag_updated_at: OLD,
            step_id: "retry-step",
            job_id: "retry-job",
            job_status: JobStatus::Failed,
            job_updated_at: OLD,
        },
    )
    .await;
    sqlx::query(
        "UPDATE job SET retry_after = ?, session_id = 'retry-session' WHERE id = 'retry-job'",
    )
    .bind(NOW + MILLIS_PER_DAY)
    .execute(&database.pool)
    .await
    .unwrap();
    for _ in 0..2 {
        let report = database.scheduler.run_maintenance_at(NOW).await;
        assert_eq!(
            (
                report.outputs_deleted,
                report.jobs_deleted,
                report.dags_deleted
            ),
            (0, 0, 0)
        );
        assert!(report.failures.is_empty(), "{:?}", report.failures);
    }
    assert!(path.exists());
}
