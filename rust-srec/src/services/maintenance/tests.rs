mod output_retention;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jiff::civil::Time;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::api::auth_store::{
    RefreshTokenDbModel, RefreshTokenRepository as _, SqlxRefreshTokenRepository,
};
use crate::database::models::{
    ChannelType, DagExecutionDbModel, DagExecutionStatus, DagStepExecutionDbModel, DagStepStatus,
    JobDbModel, JobExecutionLogDbModel, JobStatus, LiveSessionDbModel, MediaFileType,
    MediaOutputDbModel, NotificationChannelDbModel, NotificationDeadLetterDbModel,
    NotificationEventLogDbModel, SessionEventDbModel, SessionSegmentDbModel, StreamerDbModel,
};
use crate::database::repositories::{
    ConfigRepository as _, DagRepository as _, JobRepository as _, NotificationRepository as _,
    SessionEventRepository as _, SessionRepository as _, SqlxConfigRepository, SqlxDagRepository,
    SqlxJobRepository, SqlxNotificationRepository, SqlxSessionEventRepository,
    SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository as _,
};

struct TestDatabase {
    pool: DbPool,
    config_repository: SqlxConfigRepository,
    dag_repository: SqlxDagRepository,
    job_repository: SqlxJobRepository,
    notification_repository: SqlxNotificationRepository,
    refresh_token_repository: SqlxRefreshTokenRepository,
    session_event_repository: SqlxSessionEventRepository,
    session_repository: SqlxSessionRepository,
    streamer_repository: SqlxStreamerRepository,
    scheduler: Arc<MaintenanceScheduler>,
    _directory: tempfile::TempDir,
}

async fn setup(batch_size: u32) -> TestDatabase {
    setup_with_config(MaintenanceConfig {
        batch_size,
        ..MaintenanceConfig::default()
    })
    .await
}

struct ActiveRecordingEngine;

#[async_trait::async_trait]
impl crate::downloader::engine::DownloadEngine for ActiveRecordingEngine {
    fn engine_type(&self) -> crate::downloader::engine::EngineType {
        crate::downloader::engine::EngineType::Mesio
    }

    async fn run(
        &self,
        handle: Arc<crate::downloader::engine::DownloadHandle>,
    ) -> std::result::Result<(), crate::downloader::engine::EngineStartError> {
        use crate::downloader::engine::{DownloadFailureKind, EngineStartError, SegmentEvent};
        handle.cancellation_token.cancelled().await;
        handle
            .event_tx
            .send(SegmentEvent::DownloadCompleted {
                total_bytes: 0,
                total_duration_secs: 0.0,
                total_segments: 0,
                engine_signal: crate::downloader::EngineEndSignal::Unknown,
            })
            .await
            .map_err(|error| EngineStartError::new(DownloadFailureKind::Other, error.to_string()))
    }

    fn is_available(&self) -> bool {
        true
    }
    fn version(&self) -> Option<String> {
        None
    }
}

/// VACUUM runs only when a live, idle download manager admits it: never
/// without one (before wiring, or after shutdown dropped it), while another
/// admission holds it, while a recording runs, or with a negative activity
/// limit. SQL job rows are not a second source of recording activity.
#[tokio::test]
async fn vacuum_runs_only_when_a_live_idle_admission_owner_admits_it() {
    let mut database = setup_with_config(MaintenanceConfig {
        vacuum_threshold_bytes: 0,
        ..MaintenanceConfig::default()
    })
    .await;
    assert!(!database.scheduler.run_vacuum_if_needed().await.unwrap());
    let manager = Arc::new(DownloadManager::new());
    manager.register_engine(Arc::new(ActiveRecordingEngine));
    Arc::get_mut(&mut database.scheduler)
        .unwrap()
        .download_manager = Arc::downgrade(&manager);
    let admission = manager.try_admit_maintenance(0).unwrap();
    assert!(
        !database.scheduler.run_vacuum_if_needed().await.unwrap(),
        "another admission owner defers vacuum"
    );
    drop(admission);

    let config = crate::downloader::engine::DownloadConfig::new(
        "https://invalid.test/live",
        database._directory.path().join("recording"),
        "streamer",
        "Streamer",
        "session",
    );
    let id = tokio::time::timeout(
        Duration::from_secs(5),
        manager.start_download(config, Some("MESIO".to_owned()), false),
    )
    .await
    .unwrap()
    .unwrap();
    let (jobs,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM job")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
    assert_eq!(manager.active_count(), 1);
    assert!(!database.scheduler.run_vacuum_if_needed().await.unwrap());
    tokio::time::timeout(Duration::from_secs(2), manager.stop_download(&id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(manager.active_count(), 0);
    assert!(database.scheduler.run_vacuum_if_needed().await.unwrap());
    insert_job(&database, "synthetic-download", JobStatus::Pending, 0).await;
    sqlx::query("UPDATE job SET job_type = 'DOWNLOAD' WHERE id = 'synthetic-download'")
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(database.scheduler.run_vacuum_if_needed().await.unwrap());

    Arc::get_mut(&mut database.scheduler)
        .unwrap()
        .config
        .max_active_downloads_for_vacuum = -1;
    assert!(!database.scheduler.run_vacuum_if_needed().await.unwrap());
    Arc::get_mut(&mut database.scheduler)
        .unwrap()
        .config
        .max_active_downloads_for_vacuum = 0;
    drop(manager);
    assert!(!database.scheduler.run_vacuum_if_needed().await.unwrap());
}

/// Without the container's download manager as its admission owner, the
/// scheduler would skip VACUUM forever.
#[tokio::test]
async fn service_builder_wires_the_actual_download_admission_owner() {
    let database = setup(1).await;
    let container = crate::services::ServiceContainer::new(
        database.pool.clone(),
        database.scheduler.write_pool.clone(),
    )
    .await
    .unwrap();
    let owner = container
        .maintenance_scheduler
        .download_manager
        .upgrade()
        .expect("production maintenance must have an admission owner");
    assert!(Arc::ptr_eq(&owner, &container.download_manager));
}

async fn setup_with_config(config: MaintenanceConfig) -> TestDatabase {
    // macOS temporary paths can have symlinked ancestors, which output
    // retention deliberately rejects. Give fixtures a physical root.
    let temp_root = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temporary root");
    // SQLite URLs cannot contain the Windows extended-length path prefix.
    let temp_root = crate::utils::fs::normalize_media_path(&temp_root.to_string_lossy());
    let directory = tempfile::tempdir_in(temp_root).expect("temporary database directory");
    let path = directory.path().join("maintenance.db");
    let url = format!(
        "sqlite:{}?mode=rwc",
        path.to_string_lossy().replace('\\', "/")
    );
    srec_sqlite::test_support::write_migrated_database(&path).await;
    let pool = crate::database::init_pool_with_size(&url, 1)
        .await
        .expect("read pool");
    let write_pool = crate::database::init_write_pool(&url)
        .await
        .expect("write pool");
    let scheduler = Arc::new(
        MaintenanceScheduler::new(pool.clone(), write_pool.clone(), config)
            .with_output_files_gate(Arc::new(tokio::sync::RwLock::new(()))),
    );
    TestDatabase {
        config_repository: SqlxConfigRepository::new(pool.clone(), write_pool.clone()),
        dag_repository: SqlxDagRepository::new(pool.clone(), write_pool.clone()),
        job_repository: SqlxJobRepository::new(pool.clone(), write_pool.clone()),
        notification_repository: SqlxNotificationRepository::new(pool.clone(), write_pool.clone()),
        refresh_token_repository: SqlxRefreshTokenRepository::new(pool.clone(), write_pool.clone()),
        session_event_repository: SqlxSessionEventRepository::new(pool.clone(), write_pool.clone()),
        session_repository: SqlxSessionRepository::new(pool.clone(), write_pool.clone()),
        streamer_repository: SqlxStreamerRepository::new(pool.clone(), write_pool),
        pool,
        scheduler,
        _directory: directory,
    }
}

async fn set_retention(database: &TestDatabase, job_days: i32, event_days: i32) {
    let mut config = database
        .config_repository
        .get_global_config()
        .await
        .expect("global config");
    config.job_history_retention_days = job_days;
    config.notification_event_log_retention_days = event_days;
    database
        .config_repository
        .update_global_config(&config)
        .await
        .expect("retention config");
}

async fn seed_streamer(database: &TestDatabase) {
    let mut streamer = StreamerDbModel::new(
        "Maintenance",
        "https://example.com/maintenance",
        "platform-twitch",
    );
    streamer.id = "maintenance-streamer".to_string();
    database
        .streamer_repository
        .create_streamer(&streamer)
        .await
        .expect("streamer");
}

async fn create_job(
    database: &TestDatabase,
    id: &str,
    status: JobStatus,
    updated_at: i64,
    dag_step_execution_id: Option<&str>,
) {
    let mut job = JobDbModel::new("PROCESS", "{}");
    job.id = id.to_string();
    job.status = status.as_str().to_string();
    job.created_at = updated_at;
    job.updated_at = updated_at;
    job.completed_at = status.is_terminal().then_some(updated_at);
    job.dag_step_execution_id = dag_step_execution_id.map(str::to_string);
    database.job_repository.create_job(&job).await.expect("job");
}

async fn insert_job(database: &TestDatabase, id: &str, status: JobStatus, updated_at: i64) {
    create_job(database, id, status, updated_at, None).await;
}

async fn insert_dag(
    database: &TestDatabase,
    id: &str,
    status: DagExecutionStatus,
    updated_at: i64,
) {
    let dag = DagExecutionDbModel {
        id: id.to_string(),
        dag_definition: "{}".to_string(),
        status: status.as_str().to_string(),
        streamer_id: None,
        session_id: None,
        segment_index: None,
        segment_source: None,
        created_at: updated_at,
        updated_at,
        completed_at: status.is_terminal().then_some(updated_at),
        error: None,
        total_steps: 1,
        completed_steps: i32::from(status == DagExecutionStatus::Completed),
        failed_steps: i32::from(status == DagExecutionStatus::Failed),
        input_manifest: None,
    };
    database.dag_repository.create_dag(&dag).await.expect("DAG");
}

struct DagJobFixture<'a> {
    dag_id: &'a str,
    dag_status: DagExecutionStatus,
    dag_updated_at: i64,
    step_id: &'a str,
    job_id: &'a str,
    job_status: JobStatus,
    job_updated_at: i64,
}

async fn insert_dag_job(database: &TestDatabase, fixture: DagJobFixture<'_>) {
    let DagJobFixture {
        dag_id,
        dag_status,
        dag_updated_at,
        step_id,
        job_id,
        job_status,
        job_updated_at,
    } = fixture;
    insert_dag(database, dag_id, dag_status, dag_updated_at).await;
    let step = DagStepExecutionDbModel {
        id: step_id.to_string(),
        dag_id: dag_id.to_string(),
        step_id: step_id.to_string(),
        job_id: None,
        status: DagStepStatus::Pending.as_str().to_string(),
        depends_on_step_ids: "[]".to_string(),
        outputs: None,
        created_at: dag_updated_at,
        updated_at: dag_updated_at,
    };
    database
        .dag_repository
        .create_step(&step)
        .await
        .expect("DAG step");
    create_job(database, job_id, job_status, job_updated_at, Some(step_id)).await;
    sqlx::query("UPDATE dag_step_execution SET status = ?, job_id = ? WHERE id = ?")
        .bind(DagStepStatus::Completed.as_str())
        .bind(job_id)
        .bind(step_id)
        .execute(&database.pool)
        .await
        .expect("DAG step job link");
}

#[tokio::test]
async fn maintenance_window_supports_normal_and_overnight_ranges() {
    let mut config = MaintenanceConfig::default();
    let scheduler = MaintenanceScheduler::new(
        sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
        sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
        config.clone(),
    );
    assert!(scheduler.is_in_maintenance_window(Time::new(3, 0, 0, 0).unwrap_or(Time::MIN)));
    assert!(!scheduler.is_in_maintenance_window(Time::new(10, 0, 0, 0).unwrap_or(Time::MIN)));

    config.window_start = Time::new(22, 0, 0, 0).unwrap_or(Time::MIN);
    config.window_end = Time::new(2, 0, 0, 0).unwrap_or(Time::MIN);
    let scheduler = MaintenanceScheduler::new(
        sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
        sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
        config,
    );
    assert!(scheduler.is_in_maintenance_window(Time::new(23, 0, 0, 0).unwrap_or(Time::MIN)));
    assert!(scheduler.is_in_maintenance_window(Time::new(1, 0, 0, 0).unwrap_or(Time::MIN)));
}

#[tokio::test]
async fn maintenance_prunes_bounded_history_and_preserves_live_data() {
    let database = setup(1).await;
    let now = 2_000_000_000_000_i64;
    let old = now - 31 * MILLIS_PER_DAY;
    let recent = now - 29 * MILLIS_PER_DAY;
    set_retention(&database, 30, 30).await;
    seed_streamer(&database).await;

    for (id, status) in [
        ("old-failed", JobStatus::Failed),
        ("old-cancelled", JobStatus::Cancelled),
    ] {
        insert_job(&database, id, status, old).await;
    }
    insert_job(&database, "old-pending", JobStatus::Pending, old).await;
    insert_dag_job(
        &database,
        DagJobFixture {
            dag_id: "old-dag",
            dag_status: DagExecutionStatus::Completed,
            dag_updated_at: old,
            step_id: "old-step",
            job_id: "old-completed",
            job_status: JobStatus::Completed,
            job_updated_at: old,
        },
    )
    .await;
    insert_dag_job(
        &database,
        DagJobFixture {
            dag_id: "protected-dag",
            dag_status: DagExecutionStatus::Completed,
            dag_updated_at: old,
            step_id: "protected-step",
            job_id: "recent-completed",
            job_status: JobStatus::Completed,
            job_updated_at: recent,
        },
    )
    .await;
    insert_dag_job(
        &database,
        DagJobFixture {
            dag_id: "active-dag",
            dag_status: DagExecutionStatus::Processing,
            dag_updated_at: old,
            step_id: "active-step",
            job_id: "active-dag-old-job",
            job_status: JobStatus::Completed,
            job_updated_at: old,
        },
    )
    .await;
    let mut log = JobExecutionLogDbModel::new("old-completed", "{}");
    log.id = "old-log".to_string();
    log.created_at = old;
    database
        .job_repository
        .add_execution_log(&log)
        .await
        .expect("job log");

    let user_id = "default-admin-00000000-0000-0000-0000-000000000001";
    for (id, expires_at, revoked_at) in [
        ("expired-token", old, None),
        ("active-token", now + MILLIS_PER_DAY, None),
        (
            "revoked-token",
            now + MILLIS_PER_DAY,
            Some(now - MILLIS_PER_DAY),
        ),
    ] {
        database
            .refresh_token_repository
            .create(&RefreshTokenDbModel {
                session_id: None,
                id: id.to_string(),
                user_id: user_id.to_string(),
                token_hash: format!("hash-{id}"),
                expires_at,
                created_at: old,
                revoked_at,
                device_info: None,
            })
            .await
            .expect("refresh token");
    }

    for (id, created_at) in [("old-event", old), ("recent-event", recent)] {
        database
            .notification_repository
            .add_event_log(&NotificationEventLogDbModel {
                id: id.to_string(),
                event_type: "test".to_string(),
                priority: 5,
                payload: "{}".to_string(),
                streamer_id: None,
                created_at,
            })
            .await
            .expect("notification event");
    }
    let mut channel = NotificationChannelDbModel::new("Channel", ChannelType::Webhook, "{}");
    channel.id = "channel".to_string();
    database
        .notification_repository
        .create_channel(&channel)
        .await
        .expect("notification channel");
    database
        .notification_repository
        .add_to_dead_letter(&NotificationDeadLetterDbModel {
            id: "old-dead-letter".to_string(),
            channel_id: channel.id,
            event_name: "test".to_string(),
            event_payload: "{}".to_string(),
            error_message: "failed".to_string(),
            retry_count: 0,
            first_attempt_at: old,
            last_attempt_at: old,
            created_at: old,
        })
        .await
        .expect("dead letter");
    sqlx::query(
        "INSERT INTO monitor_event_outbox (\
            streamer_id, event_type, payload, created_at, delivered_at\
         ) VALUES ('maintenance-streamer', 'test', '{}', ?, ?), \
                  ('maintenance-streamer', 'test', '{}', ?, NULL)",
    )
    .bind(old)
    .bind(old)
    .bind(old)
    .execute(&database.pool)
    .await
    .expect("monitor outbox");

    let old_empty_end = now - 10 * 60 * 1000;
    for (id, end_time, total_size) in [
        ("old-empty", old_empty_end, 0_i64),
        ("recent-empty", now, 0_i64),
        ("old-real", old, 1024_i64),
    ] {
        database
            .session_repository
            .create_session(&LiveSessionDbModel {
                id: id.to_string(),
                streamer_id: Some("maintenance-streamer".to_string()),
                streamer_name: Some("Maintenance Streamer".to_string()),
                start_time: end_time - 1000,
                end_time: Some(end_time),
                titles: Some("[]".to_string()),
                total_size_bytes: total_size,
            })
            .await
            .expect("session");
    }
    database
        .session_event_repository
        .insert(&SessionEventDbModel {
            id: 0,
            session_id: "old-empty".to_string(),
            streamer_id: "maintenance-streamer".to_string(),
            kind: "session_ended".to_string(),
            occurred_at: old_empty_end,
            payload: Some("{}".to_string()),
        })
        .await
        .expect("session event");
    database
        .session_repository
        .create_session_segment(&SessionSegmentDbModel {
            id: "old-segment".to_string(),
            session_id: "old-empty".to_string(),
            segment_index: 0,
            file_path: "old.flv".to_string(),
            duration_secs: 1.0,
            size_bytes: 0,
            split_reason_code: None,
            split_reason_details_json: None,
            created_at: Some(old_empty_end - 1000),
            completed_at: Some(old_empty_end),
            persisted_at: old_empty_end,
        })
        .await
        .expect("session segment");
    let mut output = MediaOutputDbModel::new("old-empty", "old.flv", MediaFileType::Video, 0);
    output.id = "old-output".to_string();
    output.created_at = old_empty_end;
    database
        .session_repository
        .create_media_output(&output)
        .await
        .expect("media output");

    let report = database.scheduler.run_maintenance_at(now).await;
    assert!(report.failures.is_empty(), "{:#?}", report.failures);
    assert_eq!(report.jobs_deleted, 3);
    assert_eq!(report.dags_deleted, 1);
    assert_eq!(report.refresh_tokens_deleted, 1);
    assert_eq!(report.dead_letters_deleted, 1);
    assert_eq!(report.delivered_outbox_deleted, 1);
    assert_eq!(report.notification_events_deleted, 1);
    assert_eq!(report.empty_sessions_deleted, 1);

    let remaining_jobs: Vec<String> = sqlx::query_scalar("SELECT id FROM job ORDER BY id")
        .fetch_all(&database.pool)
        .await
        .expect("remaining jobs");
    assert_eq!(
        remaining_jobs,
        vec!["active-dag-old-job", "old-pending", "recent-completed"]
    );
    let old_logs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM job_execution_logs WHERE job_id = 'old-completed'",
    )
    .fetch_one(&database.pool)
    .await
    .expect("old job logs");
    assert_eq!(old_logs, 0);
    let remaining_dags: Vec<String> =
        sqlx::query_scalar("SELECT id FROM dag_execution ORDER BY id")
            .fetch_all(&database.pool)
            .await
            .expect("remaining DAGs");
    assert_eq!(remaining_dags, vec!["active-dag", "protected-dag"]);
    let remaining_tokens: Vec<String> =
        sqlx::query_scalar("SELECT id FROM refresh_tokens ORDER BY id")
            .fetch_all(&database.pool)
            .await
            .expect("remaining refresh tokens");
    assert_eq!(remaining_tokens, vec!["active-token", "revoked-token"]);
    let undelivered: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM monitor_event_outbox WHERE delivered_at IS NULL")
            .fetch_one(&database.pool)
            .await
            .expect("undelivered outbox");
    assert_eq!(undelivered, 1);
    let remaining_sessions: Vec<String> =
        sqlx::query_scalar("SELECT id FROM live_sessions ORDER BY id")
            .fetch_all(&database.pool)
            .await
            .expect("remaining sessions");
    assert_eq!(remaining_sessions, vec!["old-real", "recent-empty"]);
    let old_session_children: i64 = sqlx::query_scalar(
        "SELECT \
            (SELECT COUNT(*) FROM session_events WHERE session_id = 'old-empty') + \
            (SELECT COUNT(*) FROM session_segments WHERE session_id = 'old-empty') + \
            (SELECT COUNT(*) FROM media_outputs WHERE session_id = 'old-empty')",
    )
    .fetch_one(&database.pool)
    .await
    .expect("old session children");
    assert_eq!(old_session_children, 0);
    let foreign_key_errors: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&database.pool)
            .await
            .expect("foreign key check");
    assert!(foreign_key_errors.is_empty());
}

#[tokio::test]
async fn empty_session_cleanup_preserves_materialized_children_and_active_work() {
    let database = setup(10).await;
    let now = 2_000_000_000_000_i64;
    let ended_at = now - 10 * 60 * 1000;
    seed_streamer(&database).await;

    for id in [
        "stale-media-size",
        "materialized-segment",
        "active-job-session",
        "active-dag-session",
    ] {
        database
            .session_repository
            .create_session(&LiveSessionDbModel {
                id: id.to_string(),
                streamer_id: Some("maintenance-streamer".to_string()),
                streamer_name: Some("Maintenance Streamer".to_string()),
                start_time: ended_at - 1000,
                end_time: Some(ended_at),
                titles: Some("[]".to_string()),
                total_size_bytes: 0,
            })
            .await
            .expect("session");
    }

    let mut output = MediaOutputDbModel::new(
        "stale-media-size",
        "retained.flv",
        MediaFileType::Video,
        1024,
    );
    output.created_at = ended_at;
    database
        .session_repository
        .create_media_output(&output)
        .await
        .expect("media output");
    sqlx::query("UPDATE live_sessions SET total_size_bytes = 0 WHERE id = 'stale-media-size'")
        .execute(&database.pool)
        .await
        .expect("simulate stale session size");

    database
        .session_repository
        .create_session_segment(&SessionSegmentDbModel {
            id: "retained-segment".to_string(),
            session_id: "materialized-segment".to_string(),
            segment_index: 0,
            file_path: "segment.flv".to_string(),
            duration_secs: 1.0,
            size_bytes: 1024,
            split_reason_code: None,
            split_reason_details_json: None,
            created_at: Some(ended_at - 1000),
            completed_at: Some(ended_at),
            persisted_at: ended_at,
        })
        .await
        .expect("session segment");

    let mut job = JobDbModel::new("PROCESS", "{}");
    job.id = "active-session-job".to_string();
    job.session_id = Some("active-job-session".to_string());
    job.created_at = ended_at;
    job.updated_at = ended_at;
    database
        .job_repository
        .create_job(&job)
        .await
        .expect("active session job");

    database
        .dag_repository
        .create_dag(&DagExecutionDbModel {
            id: "active-session-dag".to_string(),
            dag_definition: "{}".to_string(),
            status: DagExecutionStatus::Processing.as_str().to_string(),
            streamer_id: Some("maintenance-streamer".to_string()),
            session_id: Some("active-dag-session".to_string()),
            segment_index: None,
            segment_source: None,
            created_at: ended_at,
            updated_at: ended_at,
            completed_at: None,
            error: None,
            total_steps: 1,
            completed_steps: 0,
            failed_steps: 0,
            input_manifest: None,
        })
        .await
        .expect("active session DAG");

    let report = database.scheduler.run_maintenance_at(now).await;
    assert!(report.failures.is_empty(), "{:#?}", report.failures);
    assert_eq!(report.empty_sessions_deleted, 0);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM live_sessions WHERE id IN (\
            'stale-media-size', 'materialized-segment', \
            'active-job-session', 'active-dag-session'\
        )",
    )
    .fetch_one(&database.pool)
    .await
    .expect("protected sessions");
    assert_eq!(remaining, 4);
}

#[tokio::test]
async fn cancelled_sweep_stops_before_deleting_rows() {
    let database = setup(1).await;
    let now = 2_000_000_000_000_i64;
    let old = now - 31 * MILLIS_PER_DAY;
    set_retention(&database, 30, 30).await;
    insert_job(&database, "cancelled-sweep-job", JobStatus::Failed, old).await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let report = database
        .scheduler
        .run_maintenance_with_cancellation(now, &cancellation)
        .await;
    assert!(report.cancelled);
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE id = 'cancelled-sweep-job'")
            .fetch_one(&database.pool)
            .await
            .expect("cancelled sweep job");
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn zero_retention_preserves_pipeline_and_notification_history() {
    let database = setup(10).await;
    let now = 2_000_000_000_000_i64;
    let old = now - 365 * MILLIS_PER_DAY;
    set_retention(&database, 0, 0).await;
    insert_job(&database, "forever-job", JobStatus::Cancelled, old).await;
    insert_dag(&database, "forever-dag", DagExecutionStatus::Failed, old).await;
    database
        .notification_repository
        .add_event_log(&NotificationEventLogDbModel {
            id: "forever-event".to_string(),
            event_type: "test".to_string(),
            priority: 5,
            payload: "{}".to_string(),
            streamer_id: None,
            created_at: old,
        })
        .await
        .expect("notification event");

    let report = database.scheduler.run_maintenance_at(now).await;
    assert_eq!(report.jobs_deleted, 0);
    assert_eq!(report.dags_deleted, 0);
    assert_eq!(report.notification_events_deleted, 0);
    let count: i64 = sqlx::query_scalar(
        "SELECT \
            (SELECT COUNT(*) FROM job WHERE id = 'forever-job') + \
            (SELECT COUNT(*) FROM dag_execution WHERE id = 'forever-dag') + \
            (SELECT COUNT(*) FROM notification_event_log WHERE id = 'forever-event')",
    )
    .fetch_one(&database.pool)
    .await
    .expect("retained history");
    assert_eq!(count, 3);
}

/// A failing policy skips only its own task. One unparsable retention field
/// disables only that policy, and a policy source that cannot be read at all
/// still leaves token cleanup running.
#[tokio::test]
async fn a_failing_retention_policy_skips_only_its_own_task() {
    let database = setup(10).await;
    let now = 2_000_000_000_000_i64;
    let old = now - 31 * MILLIS_PER_DAY;
    sqlx::query(
        "UPDATE global_config SET job_history_retention_days = -1, \
         notification_event_log_retention_days = 30",
    )
    .execute(&database.pool)
    .await
    .expect("corrupt one retention field");
    insert_job(
        &database,
        "preserved-invalid-policy",
        JobStatus::Failed,
        old,
    )
    .await;
    database
        .notification_repository
        .add_event_log(&NotificationEventLogDbModel {
            id: "pruned-valid-policy".to_string(),
            event_type: "test".to_string(),
            priority: 5,
            payload: "{}".to_string(),
            streamer_id: None,
            created_at: old,
        })
        .await
        .expect("notification event");

    let report = database.scheduler.run_maintenance_at(now).await;
    assert_eq!(report.jobs_deleted, 0);
    assert_eq!(report.notification_events_deleted, 1);
    assert!(
        report
            .failures
            .iter()
            .any(|failure| failure.task == "job_history_retention_policy")
    );
    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE id = 'preserved-invalid-policy'")
            .fetch_one(&database.pool)
            .await
            .expect("preserved job");
    assert_eq!(job_count, 1);

    database
        .refresh_token_repository
        .create(&RefreshTokenDbModel {
            session_id: None,
            id: "expired-token".to_string(),
            user_id: "default-admin-00000000-0000-0000-0000-000000000001".to_string(),
            token_hash: "expired-hash".to_string(),
            expires_at: now - 1,
            created_at: now - MILLIS_PER_DAY,
            revoked_at: None,
            device_info: None,
        })
        .await
        .expect("expired token");
    sqlx::query("DROP TABLE global_config")
        .execute(&database.pool)
        .await
        .expect("drop policy source");

    let report = database.scheduler.run_maintenance_at(now).await;
    assert_eq!(report.refresh_tokens_deleted, 1);
    assert!(
        report
            .failures
            .iter()
            .any(|failure| failure.task == "load_retention_policy")
    );
}

/// The desktop app relies on `run_migrations` alone to choose incremental
/// auto-vacuum for a new database file, so this test migrates a fresh file
/// rather than copying the migrated template the other fixtures use.
#[tokio::test]
async fn new_database_enables_incremental_auto_vacuum() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let url = format!(
        "sqlite:{}?mode=rwc",
        directory
            .path()
            .join("fresh.db")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let pool = crate::database::init_pool_with_size(&url, 1)
        .await
        .expect("pool");
    crate::database::run_migrations(&pool)
        .await
        .expect("migrations");
    let (mode,): (i64,) = sqlx::query_as("PRAGMA auto_vacuum")
        .fetch_one(&pool)
        .await
        .expect("auto-vacuum mode");
    assert_eq!(mode, AUTO_VACUUM_INCREMENTAL);
}
