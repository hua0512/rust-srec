//! Integration tests for rust-srec database layer.
//!
//! These tests use a real SQLite database (in-memory) to verify
//! repository operations work correctly with the actual schema.

use rust_srec::database::{DbPool, init_pool, run_migrations};

/// Helper to create a test database pool with migrations applied.
async fn setup_test_db() -> DbPool {
    let pool = init_pool("sqlite::memory:")
        .await
        .expect("Failed to create test pool");

    run_migrations(&pool)
        .await
        .expect("Failed to run migrations");

    pool
}

async fn create_test_platform(pool: &DbPool, prefix: &str) -> String {
    use rust_srec::database::models::PlatformConfigDbModel;
    use rust_srec::database::repositories::{ConfigRepository, SqlxConfigRepository};

    let repo = SqlxConfigRepository::new(pool.clone(), pool.clone());
    let config = PlatformConfigDbModel {
        id: uuid::Uuid::new_v4().to_string(),
        platform_name: format!("{}_{}", prefix, uuid::Uuid::new_v4()),
        fetch_delay_ms: Some(60_000),
        download_delay_ms: Some(1_000),
        cookies: None,
        platform_specific_config: None,
        proxy_config: None,
        record_danmu: None,
        output_folder: None,
        output_filename_template: None,
        download_engine: None,
        stream_selection_config: None,
        output_file_format: None,
        min_segment_size_bytes: None,
        max_download_duration_secs: None,
        max_part_size_bytes: None,
        download_retry_policy: None,
        event_hooks: None,
        pipeline: None,
        session_complete_pipeline: None,
        paired_segment_pipeline: None,
        offline_check_count: None,
        offline_check_delay_ms: None,
    };
    let id = config.id.clone();

    repo.create_platform_config(&config)
        .await
        .expect("Failed to create platform config");

    id
}

async fn create_test_streamer(
    pool: &DbPool,
    platform_id: &str,
    name: &str,
    url: &str,
    state: &str,
    priority: &str,
) -> String {
    use rust_srec::database::models::StreamerDbModel;
    use rust_srec::database::repositories::{SqlxStreamerRepository, StreamerRepository};

    let repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
    let mut streamer = StreamerDbModel::new(name, url, platform_id);
    streamer.state = state.to_string();
    streamer.priority = priority.to_string();
    let id = streamer.id.clone();

    repo.create_streamer(&streamer)
        .await
        .expect("Failed to create streamer");

    id
}

async fn create_unique_test_streamer(
    pool: &DbPool,
    platform_id: &str,
    name: &str,
    state: &str,
    priority: &str,
) -> String {
    let url = format!(
        "https://example.com/{}_{}",
        name.to_lowercase(),
        uuid::Uuid::new_v4()
    );
    create_test_streamer(pool, platform_id, name, &url, state, priority).await
}

mod database_tests {
    use super::*;

    #[tokio::test]
    async fn test_database_migrations() {
        let pool = setup_test_db().await;

        // Verify tables exist by querying sqlite_master
        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .expect("Failed to query tables");

        let table_names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();

        // Check essential tables exist
        assert!(
            table_names.contains(&"global_config"),
            "global_config table missing"
        );
        assert!(
            table_names.contains(&"platform_config"),
            "platform_config table missing"
        );
        assert!(
            table_names.contains(&"template_config"),
            "template_config table missing"
        );
        assert!(
            table_names.contains(&"streamers"),
            "streamers table missing"
        );
        assert!(table_names.contains(&"filters"), "filters table missing");
        assert!(
            table_names.contains(&"live_sessions"),
            "live_sessions table missing"
        );
        assert!(
            table_names.contains(&"media_outputs"),
            "media_outputs table missing"
        );
        assert!(table_names.contains(&"job"), "job table missing");
        assert!(
            table_names.contains(&"notification_channel"),
            "notification_channel table missing"
        );
    }

    #[tokio::test]
    async fn test_global_config_schema_drops_session_gap_time() {
        let pool = setup_test_db().await;

        let column_names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('global_config')")
                .fetch_all(&pool)
                .await
                .expect("Failed to inspect global_config columns");
        let column_names: Vec<&str> = column_names.iter().map(String::as_str).collect();

        assert!(
            !column_names.contains(&"session_gap_time_secs"),
            "session_gap_time_secs should not exist after migrations"
        );
        assert!(
            column_names.contains(&"queue_freshness_threshold_ms"),
            "latest global_config columns must be preserved"
        );
    }

    #[tokio::test]
    async fn test_remove_session_gap_time_migration_preserves_existing_row() {
        let pool = init_pool("sqlite::memory:")
            .await
            .expect("Failed to create test pool");

        sqlx::raw_sql(
            r#"
            CREATE TABLE global_config (
                id TEXT PRIMARY KEY NOT NULL,
                output_folder TEXT NOT NULL,
                output_filename_template TEXT NOT NULL DEFAULT "{streamer}-%Y%m%d-%H%M%S-{title}",
                output_file_format TEXT NOT NULL DEFAULT "flv",
                min_segment_size_bytes INTEGER NOT NULL DEFAULT 1048576,
                max_download_duration_secs INTEGER NOT NULL DEFAULT 0,
                max_part_size_bytes BIGINT NOT NULL DEFAULT 8589934592,
                record_danmu BOOLEAN NOT NULL DEFAULT FALSE,
                max_concurrent_downloads INTEGER NOT NULL DEFAULT 6,
                max_concurrent_uploads INTEGER NOT NULL DEFAULT 3,
                streamer_check_delay_ms INTEGER NOT NULL DEFAULT 60000,
                proxy_config TEXT NOT NULL,
                offline_check_delay_ms INTEGER NOT NULL DEFAULT 20000,
                offline_check_count INTEGER NOT NULL DEFAULT 3,
                default_download_engine TEXT NOT NULL,
                max_concurrent_cpu_jobs INTEGER NOT NULL DEFAULT 0,
                max_concurrent_io_jobs INTEGER NOT NULL DEFAULT 8,
                job_history_retention_days INTEGER NOT NULL DEFAULT 30,
                notification_event_log_retention_days INTEGER NOT NULL DEFAULT 30,
                session_gap_time_secs INTEGER NOT NULL DEFAULT 3600,
                pipeline TEXT,
                log_filter_directive TEXT NOT NULL DEFAULT 'rust_srec=info,sqlx=warn,mesio_engine=info,flv=info,hls=info',
                session_complete_pipeline TEXT,
                paired_segment_pipeline TEXT,
                auto_thumbnail BOOLEAN NOT NULL DEFAULT TRUE,
                pipeline_cpu_job_timeout_secs INTEGER NOT NULL DEFAULT 3600,
                pipeline_io_job_timeout_secs INTEGER NOT NULL DEFAULT 3600,
                pipeline_execute_timeout_secs INTEGER NOT NULL DEFAULT 3600,
                queue_freshness_threshold_ms INTEGER NOT NULL DEFAULT 60000,
                gpu_health_probe_interval_secs INTEGER NOT NULL DEFAULT 30
            );

            INSERT INTO global_config (
                id,
                output_folder,
                output_filename_template,
                output_file_format,
                min_segment_size_bytes,
                max_download_duration_secs,
                max_part_size_bytes,
                record_danmu,
                max_concurrent_downloads,
                max_concurrent_uploads,
                streamer_check_delay_ms,
                proxy_config,
                offline_check_delay_ms,
                offline_check_count,
                default_download_engine,
                max_concurrent_cpu_jobs,
                max_concurrent_io_jobs,
                job_history_retention_days,
                notification_event_log_retention_days,
                session_gap_time_secs,
                pipeline,
                log_filter_directive,
                session_complete_pipeline,
                paired_segment_pipeline,
                auto_thumbnail,
                pipeline_cpu_job_timeout_secs,
                pipeline_io_job_timeout_secs,
                pipeline_execute_timeout_secs,
                queue_freshness_threshold_ms,
                gpu_health_probe_interval_secs
            ) VALUES (
                'global-1',
                '/custom/output',
                '{streamer}',
                'mp4',
                2048,
                3600,
                4096,
                TRUE,
                9,
                4,
                45000,
                '{"enabled":false,"url":null}',
                15000,
                5,
                'ffmpeg',
                2,
                6,
                14,
                21,
                1234,
                '{"name":"global","steps":[]}',
                'rust_srec=debug',
                NULL,
                NULL,
                FALSE,
                111,
                222,
                333,
                444,
                30
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("Failed to seed previous global_config schema");

        sqlx::raw_sql(include_str!(
            "../migrations/20260510000000_remove_session_gap_time_secs.sql"
        ))
        .execute(&pool)
        .await
        .expect("Failed to run session_gap_time_secs removal migration");

        let column_names: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('global_config')")
                .fetch_all(&pool)
                .await
                .expect("Failed to inspect migrated global_config");
        let column_names: Vec<&str> = column_names.iter().map(String::as_str).collect();
        assert!(!column_names.contains(&"session_gap_time_secs"));

        let row: (String, String, String, i64, i64, i64, bool) = sqlx::query_as(
            r#"
            SELECT
                id,
                output_folder,
                default_download_engine,
                offline_check_count,
                pipeline_cpu_job_timeout_secs,
                queue_freshness_threshold_ms,
                auto_thumbnail
            FROM global_config
            "#,
        )
        .fetch_one(&pool)
        .await
        .expect("Failed to read migrated global_config row");

        assert_eq!(row.0, "global-1");
        assert_eq!(row.1, "/custom/output");
        assert_eq!(row.2, "ffmpeg");
        assert_eq!(row.3, 5);
        assert_eq!(row.4, 111);
        assert_eq!(row.5, 444);
        assert!(!row.6);
    }

    #[tokio::test]
    async fn test_wal_mode_enabled() {
        let pool = setup_test_db().await;

        // In-memory databases use "memory" journal mode
        // File-based would use "wal"
        let result: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .expect("Failed to query journal mode");

        // Memory databases can't use WAL, but file-based would
        assert!(result.0 == "memory" || result.0 == "wal");
    }
}

mod config_repository_tests {
    use super::*;
    use rust_srec::database::models::{GlobalConfigDbModel, TemplateConfigDbModel};
    use rust_srec::database::repositories::{ConfigRepository, SqlxConfigRepository};

    #[tokio::test]
    async fn test_global_config_crud() {
        let pool = setup_test_db().await;

        let repo = SqlxConfigRepository::new(pool.clone(), pool.clone());
        let config = GlobalConfigDbModel {
            id: uuid::Uuid::new_v4().to_string(),
            default_download_engine: "ffmpeg".to_string(),
            ..Default::default()
        };
        let id = config.id.clone();

        repo.create_global_config(&config)
            .await
            .expect("Failed to create global config");

        // Read it back
        let result: (String, String, bool) = sqlx::query_as(
            "SELECT id, output_folder, record_danmu FROM global_config WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("Failed to read global config");

        assert_eq!(result.0, id);
        assert_eq!(result.1, "/app/output");
        assert!(!result.2);
    }

    #[tokio::test]
    async fn test_platform_config_crud() {
        let pool = setup_test_db().await;

        let id = create_test_platform(&pool, "test_platform").await;

        // Query by id
        let result: (String, String, i64) = sqlx::query_as(
            "SELECT id, platform_name, fetch_delay_ms FROM platform_config WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("Failed to read platform config");

        assert!(result.1.starts_with("test_platform_"));
        assert_eq!(result.2, 60000);
    }

    #[tokio::test]
    async fn test_template_config_crud() {
        let pool = setup_test_db().await;

        let name = format!("high-quality-{}", uuid::Uuid::new_v4());
        let repo = SqlxConfigRepository::new(pool.clone(), pool.clone());
        let mut template = TemplateConfigDbModel::new(name.clone());
        template.output_folder = Some("./hq-downloads".to_string());
        template.output_file_format = Some("mp4".to_string());
        let id = template.id.clone();

        repo.create_template_config(&template)
            .await
            .expect("Failed to create template config");

        // Read it back
        let result: (String, String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT id, name, output_folder, output_file_format FROM template_config WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("Failed to read template config");

        assert_eq!(result.1, name);
        assert_eq!(result.2, Some("./hq-downloads".to_string()));
        assert_eq!(result.3, Some("mp4".to_string()));
    }
}

mod streamer_repository_tests {
    use super::*;
    use rust_srec::database::repositories::{SqlxStreamerRepository, StreamerRepository};

    async fn setup_platform(pool: &DbPool) -> String {
        create_test_platform(pool, "test_platform").await
    }

    #[tokio::test]
    async fn test_streamer_crud() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = create_test_streamer(
            &pool,
            &platform_id,
            "TestStreamer",
            "https://twitch.tv/test",
            "NOT_LIVE",
            "NORMAL",
        )
        .await
        .to_string();

        // Read it back
        let result: (String, String, String, String) =
            sqlx::query_as("SELECT id, name, state, priority FROM streamers WHERE id = ?")
                .bind(&streamer_id)
                .fetch_one(&pool)
                .await
                .expect("Failed to read streamer");

        assert_eq!(result.1, "TestStreamer");
        assert_eq!(result.2, "NOT_LIVE");
        assert_eq!(result.3, "NORMAL");
    }

    #[tokio::test]
    async fn test_streamer_state_update() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = create_test_streamer(
            &pool,
            &platform_id,
            "TestStreamer",
            "https://twitch.tv/test",
            "NOT_LIVE",
            "NORMAL",
        )
        .await
        .to_string();

        // Update state to LIVE
        let repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        repo.update_streamer_state(&streamer_id, "LIVE")
            .await
            .expect("Failed to update state");

        // Verify state changed
        let result: (String,) = sqlx::query_as("SELECT state FROM streamers WHERE id = ?")
            .bind(&streamer_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read state");

        assert_eq!(result.0, "LIVE");
    }

    #[tokio::test]
    async fn test_streamer_priority_query() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        // Insert streamers with different priorities
        for (name, priority) in [
            ("High1", "HIGH"),
            ("Normal1", "NORMAL"),
            ("Low1", "LOW"),
            ("High2", "HIGH"),
        ] {
            let url = format!("https://twitch.tv/{}", name.to_lowercase());
            create_test_streamer(&pool, &platform_id, name, &url, "NOT_LIVE", priority)
                .await
                .to_string();
        }

        // Query by priority
        let repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let high_priority = repo
            .list_streamers_by_priority("HIGH")
            .await
            .expect("Failed to query high priority");

        assert_eq!(high_priority.len(), 2);
    }
}

mod session_repository_tests {
    use super::*;
    use rust_srec::database::models::LiveSessionDbModel;
    use rust_srec::database::repositories::{SessionRepository, SqlxSessionRepository};

    async fn setup_streamer(pool: &DbPool) -> String {
        let platform_id = create_test_platform(pool, "test_session_platform").await;
        let streamer_url = format!("https://example.com/test_{}", uuid::Uuid::new_v4());
        create_test_streamer(
            pool,
            &platform_id,
            "TestStreamer",
            &streamer_url,
            "NOT_LIVE",
            "NORMAL",
        )
        .await
    }

    #[tokio::test]
    async fn test_live_session_crud() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let repo = SqlxSessionRepository::new(pool.clone(), pool);
        let session = LiveSessionDbModel::new(streamer_id.clone());
        let session_id = session.id.clone();

        repo.create_session(&session)
            .await
            .expect("Failed to create session");

        // Read it back
        let result = repo
            .get_session(&session_id)
            .await
            .expect("Failed to read session");

        assert_eq!(result.id, session_id);
        assert_eq!(result.streamer_id, streamer_id);
        assert!(result.end_time.is_none()); // Session not ended yet
    }

    #[tokio::test]
    async fn test_session_end() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let repo = SqlxSessionRepository::new(pool.clone(), pool);
        let session = LiveSessionDbModel::new(streamer_id);
        let session_id = session.id.clone();
        repo.create_session(&session)
            .await
            .expect("Failed to create session");

        // End session
        let end_time = chrono::Utc::now().timestamp_millis();
        repo.end_session(&session_id, end_time)
            .await
            .expect("Failed to end session");

        // Verify end time is set
        let result = repo
            .get_session(&session_id)
            .await
            .expect("Failed to read session");

        assert_eq!(result.end_time, Some(end_time));
    }

    #[tokio::test]
    async fn test_recent_sessions_query() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let repo = SqlxSessionRepository::new(pool.clone(), pool);

        // Insert multiple sessions (all ended - unique constraint requires only one active session per streamer)
        for i in 0..5 {
            let start_time =
                chrono::DateTime::parse_from_rfc3339(&format!("2024-01-{:02}T12:00:00Z", i + 1))
                    .unwrap()
                    .timestamp_millis();
            let mut session = LiveSessionDbModel::new(streamer_id.clone());
            session.start_time = start_time;
            session.end_time = Some(start_time + chrono::Duration::hours(2).num_milliseconds());
            repo.create_session(&session)
                .await
                .expect("Failed to create session");
        }

        let sessions = repo
            .list_sessions_for_streamer(&streamer_id, 3)
            .await
            .expect("Failed to query sessions");

        assert_eq!(sessions.len(), 3);
        assert!(sessions[0].start_time > sessions[1].start_time);
    }

    /// Helper: insert a session with explicit `total_size_bytes` and an
    /// optional `end_time`. Mirrors the production INSERT path in
    /// `SqlxSessionRepository::create_session` plus the increment-on-output
    /// effect on `total_size_bytes`.
    async fn insert_session_with_size(
        pool: &DbPool,
        streamer_id: &str,
        total_size_bytes: i64,
        end_time_ms: Option<i64>,
    ) -> String {
        let repo = SqlxSessionRepository::new(pool.clone(), pool.clone());
        let mut session = LiveSessionDbModel::new(streamer_id);
        session.end_time = end_time_ms;
        session.total_size_bytes = total_size_bytes;
        let session_id = session.id.clone();
        repo.create_session(&session)
            .await
            .expect("Failed to create session with size");
        session_id
    }

    /// Default `list_sessions_filtered` (no `include_empty`) hides ended
    /// sessions whose `total_size_bytes == 0`. These are the connection-blip
    /// 0-byte rows the small-segment guard discarded — noise on the
    /// dashboard.
    #[tokio::test]
    async fn test_list_sessions_filtered_excludes_empty_by_default() {
        use rust_srec::database::models::{Pagination, SessionFilters};
        use rust_srec::database::repositories::{SessionRepository, SqlxSessionRepository};

        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let now_ms = chrono::Utc::now().timestamp_millis();
        let _empty_id = insert_session_with_size(&pool, &streamer_id, 0, Some(now_ms)).await;
        let real_id =
            insert_session_with_size(&pool, &streamer_id, 1_500_000_000, Some(now_ms)).await;

        let repo = SqlxSessionRepository::new(pool.clone(), pool);
        let (sessions, total) = repo
            .list_sessions_filtered(
                &SessionFilters {
                    streamer_id: Some(streamer_id.clone()),
                    ..Default::default()
                },
                &Pagination::new(50, 0),
            )
            .await
            .expect("list_sessions_filtered failed");

        assert_eq!(total, 1, "default filter must hide the empty ended session");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, real_id);
    }

    /// `include_empty: Some(true)` opts in to seeing the discarded rows —
    /// useful for diagnostics ("why did this brief blip happen?").
    #[tokio::test]
    async fn test_list_sessions_filtered_includes_empty_when_opted_in() {
        use rust_srec::database::models::{Pagination, SessionFilters};
        use rust_srec::database::repositories::{SessionRepository, SqlxSessionRepository};

        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let now_ms = chrono::Utc::now().timestamp_millis();
        insert_session_with_size(&pool, &streamer_id, 0, Some(now_ms)).await;
        insert_session_with_size(&pool, &streamer_id, 1_500_000_000, Some(now_ms)).await;

        let repo = SqlxSessionRepository::new(pool.clone(), pool);
        let (_sessions, total) = repo
            .list_sessions_filtered(
                &SessionFilters {
                    streamer_id: Some(streamer_id.clone()),
                    include_empty: Some(true),
                    ..Default::default()
                },
                &Pagination::new(50, 0),
            )
            .await
            .expect("list_sessions_filtered failed");

        assert_eq!(total, 2, "include_empty=true must return both rows");
    }

    /// Active sessions (`end_time IS NULL`) are kept regardless of size.
    /// Their `total_size_bytes == 0` in the brief window between LIVE
    /// detection and the first retained segment; we must NOT hide them.
    #[tokio::test]
    async fn test_list_sessions_filtered_keeps_active_empty_sessions() {
        use rust_srec::database::models::{Pagination, SessionFilters};
        use rust_srec::database::repositories::{SessionRepository, SqlxSessionRepository};

        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let active_id = insert_session_with_size(&pool, &streamer_id, 0, None).await;

        let repo = SqlxSessionRepository::new(pool.clone(), pool);
        let (sessions, total) = repo
            .list_sessions_filtered(
                &SessionFilters {
                    streamer_id: Some(streamer_id.clone()),
                    ..Default::default()
                },
                &Pagination::new(50, 0),
            )
            .await
            .expect("list_sessions_filtered failed");

        assert_eq!(
            total, 1,
            "default filter must keep active sessions even with size 0"
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, active_id);
    }
}

mod job_repository_tests {
    use super::*;
    use rust_srec::database::models::{JobDbModel, JobStatus};
    use rust_srec::database::repositories::{JobRepository, SqlxJobRepository};

    async fn create_job(pool: &DbPool, job_type: &str, status: JobStatus) -> JobDbModel {
        let repo = SqlxJobRepository::new(pool.clone(), pool.clone());
        let mut job = JobDbModel::new(job_type, "{}");
        job.status = status.as_str().to_string();
        repo.create_job(&job).await.expect("Failed to create job");
        job
    }

    #[tokio::test]
    async fn test_job_crud() {
        let pool = setup_test_db().await;

        let job = create_job(&pool, "DOWNLOAD", JobStatus::Pending).await;

        // Read it back
        let result: (String, String, String) =
            sqlx::query_as("SELECT id, job_type, status FROM job WHERE id = ?")
                .bind(&job.id)
                .fetch_one(&pool)
                .await
                .expect("Failed to read job");

        assert_eq!(result.1, "DOWNLOAD");
        assert_eq!(result.2, "PENDING");
    }

    #[tokio::test]
    async fn test_job_status_update() {
        let pool = setup_test_db().await;

        let repo = SqlxJobRepository::new(pool.clone(), pool.clone());
        let job = create_job(&pool, "DOWNLOAD", JobStatus::Pending).await;

        // Update status
        repo.update_job_status(&job.id, JobStatus::Processing)
            .await
            .expect("Failed to update status");

        // Verify
        let result: (String,) = sqlx::query_as("SELECT status FROM job WHERE id = ?")
            .bind(&job.id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read status");

        assert_eq!(result.0, "PROCESSING");
    }

    #[tokio::test]
    async fn test_pending_jobs_query() {
        let pool = setup_test_db().await;
        let repo = SqlxJobRepository::new(pool.clone(), pool.clone());

        // Insert jobs with different statuses
        for (status, job_type) in [
            (JobStatus::Pending, "DOWNLOAD"),
            (JobStatus::Pending, "PIPELINE"),
            (JobStatus::Processing, "DOWNLOAD"),
            (JobStatus::Completed, "DOWNLOAD"),
        ] {
            create_job(&pool, job_type, status).await;
        }

        // Query pending download jobs
        let pending = repo
            .list_pending_jobs("DOWNLOAD")
            .await
            .expect("Failed to query pending jobs");

        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_cancelled_jobs_are_not_reset_by_processing_recovery() {
        let pool = setup_test_db().await;
        let repo = SqlxJobRepository::new(pool.clone(), pool.clone());

        for _ in 0..3 {
            create_job(&pool, "DOWNLOAD", JobStatus::Cancelled).await;
        }

        let reset_count = repo
            .reset_processing_jobs()
            .await
            .expect("Failed to reset jobs");

        assert_eq!(reset_count, 0);

        let cancelled = repo
            .list_jobs_by_status(JobStatus::Cancelled)
            .await
            .expect("Failed to query");

        assert_eq!(cancelled.len(), 3);
    }
}

mod notification_repository_tests {
    use super::*;
    use rust_srec::database::models::{
        ChannelType, NotificationChannelDbModel, NotificationDeadLetterDbModel,
    };
    use rust_srec::database::repositories::{NotificationRepository, SqlxNotificationRepository};

    async fn create_channel(pool: &DbPool, name: &str, channel_type: ChannelType) -> String {
        let repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let channel = NotificationChannelDbModel::new(name, channel_type, "{}");
        let id = channel.id.clone();
        repo.create_channel(&channel)
            .await
            .expect("Failed to create notification channel");
        id
    }

    #[tokio::test]
    async fn test_notification_channel_crud() {
        let pool = setup_test_db().await;
        let repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());

        let settings = r#"{"webhook_url":"https://discord.com/api/webhooks/123"}"#;
        let channel =
            NotificationChannelDbModel::new("Discord Alerts", ChannelType::Discord, settings);
        let channel_id = channel.id.clone();

        // Insert channel
        repo.create_channel(&channel)
            .await
            .expect("Failed to create channel");

        // Read it back
        let result: (String, String, String) =
            sqlx::query_as("SELECT id, name, channel_type FROM notification_channel WHERE id = ?")
                .bind(&channel_id)
                .fetch_one(&pool)
                .await
                .expect("Failed to read channel");

        assert_eq!(result.1, "Discord Alerts");
        assert_eq!(result.2, "Discord");
    }

    #[tokio::test]
    async fn test_notification_subscription() {
        let pool = setup_test_db().await;

        let repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let channel_id = create_channel(&pool, "Test Channel", ChannelType::Webhook).await;

        // Subscribe to events
        for event in ["streamer.online", "streamer.offline", "download.complete"] {
            repo.subscribe(&channel_id, event)
                .await
                .expect("Failed to insert subscription");
        }

        // Query subscriptions
        let subs = repo
            .get_subscriptions_for_channel(&channel_id)
            .await
            .expect("Failed to query subscriptions");

        assert_eq!(subs.len(), 3);
    }

    #[tokio::test]
    async fn test_dead_letter_queue() {
        let pool = setup_test_db().await;

        let repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let channel_id = create_channel(&pool, "Test Channel", ChannelType::Webhook).await;

        // Insert dead letter entries
        for i in 0..3 {
            let entry = NotificationDeadLetterDbModel::new(
                &channel_id,
                "test.event",
                "{}",
                "Connection timeout",
                i + 1,
                rust_srec::database::time::now_ms(),
            );
            repo.add_to_dead_letter(&entry)
                .await
                .expect("Failed to insert dead letter");
        }

        // Query dead letters
        let dead_letters = repo
            .list_dead_letters(Some(&channel_id), 10)
            .await
            .expect("Failed to query dead letters");

        assert_eq!(dead_letters.len(), 3);
    }
}

mod filter_repository_tests {
    use super::*;
    use rust_srec::database::models::{FilterDbModel, FilterType};
    use rust_srec::database::repositories::{
        FilterRepository, SqlxFilterRepository, SqlxStreamerRepository, StreamerRepository,
    };

    async fn setup_streamer(pool: &DbPool) -> String {
        let platform_id = create_test_platform(pool, "test_filter_platform").await;
        let streamer_url = format!("https://example.com/filter_test_{}", uuid::Uuid::new_v4());
        create_test_streamer(
            pool,
            &platform_id,
            "TestStreamer",
            &streamer_url,
            "NOT_LIVE",
            "NORMAL",
        )
        .await
    }

    #[tokio::test]
    async fn test_filter_crud() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;

        let repo = SqlxFilterRepository::new(pool.clone(), pool.clone());
        let config = r#"{"include":["gaming"],"exclude":["ads"]}"#;
        let filter = FilterDbModel::new(&streamer_id, FilterType::Keyword, config);
        let filter_id = filter.id.clone();

        // Insert filter
        repo.create_filter(&filter)
            .await
            .expect("Failed to insert filter");

        // Read it back
        let result: (String, String, String) =
            sqlx::query_as("SELECT id, filter_type, config FROM filters WHERE id = ?")
                .bind(&filter_id)
                .fetch_one(&pool)
                .await
                .expect("Failed to read filter");

        assert_eq!(result.1, "KEYWORD");
    }

    #[tokio::test]
    async fn test_filter_cascade_delete() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;
        let filter_repo = SqlxFilterRepository::new(pool.clone(), pool.clone());

        // Insert filters
        for _i in 0..3 {
            let filter = FilterDbModel::new(&streamer_id, FilterType::Keyword, "{}");
            filter_repo
                .create_filter(&filter)
                .await
                .expect("Failed to insert filter");
        }

        // Verify filters exist
        let before = filter_repo
            .get_filters_for_streamer(&streamer_id)
            .await
            .expect("Failed to query filters");

        assert_eq!(before.len(), 3);

        // Delete streamer (should cascade delete filters)
        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        streamer_repo
            .delete_streamer(&streamer_id)
            .await
            .expect("Failed to delete streamer");

        // Verify filters are deleted
        let after = filter_repo
            .get_filters_for_streamer(&streamer_id)
            .await
            .expect("Failed to query filters");

        assert_eq!(after.len(), 0);
    }
}

mod concurrent_access_tests {
    use super::*;
    use rust_srec::database::repositories::{ConfigRepository, SqlxConfigRepository};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_concurrent_reads() {
        let pool = Arc::new(setup_test_db().await);

        let platform_id = create_test_platform(&pool, "test_platform").await;
        let repo = Arc::new(SqlxConfigRepository::new(
            pool.as_ref().clone(),
            pool.as_ref().clone(),
        ));

        // Spawn multiple concurrent read tasks
        let mut handles = vec![];
        for _ in 0..10 {
            let platform_id_clone = platform_id.clone();
            let repo_clone = repo.clone();
            handles.push(tokio::spawn(async move {
                let platform = repo_clone
                    .get_platform_config(&platform_id_clone)
                    .await
                    .expect("Failed to read");
                platform.platform_name
            }));
        }

        // All reads should succeed
        for handle in handles {
            let result = handle.await.expect("Task failed");
            assert!(result.starts_with("test_platform_"));
        }
    }
}

mod streamer_manager_tests {
    use super::*;
    use rust_srec::config::ConfigEventBroadcaster;
    use rust_srec::database::repositories::streamer::SqlxStreamerRepository;
    use rust_srec::domain::StreamerState;
    use rust_srec::streamer::StreamerManager;
    use std::sync::Arc;

    async fn setup_platform(pool: &DbPool) -> String {
        create_test_platform(pool, "test_mgr_platform").await
    }

    async fn insert_streamer(
        pool: &DbPool,
        platform_id: &str,
        name: &str,
        state: &str,
        priority: &str,
    ) -> String {
        create_unique_test_streamer(pool, platform_id, name, state, priority).await
    }

    #[tokio::test]
    async fn test_streamer_manager_hydration() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        // Insert some streamers
        insert_streamer(&pool, &platform_id, "Streamer1", "NOT_LIVE", "NORMAL").await;
        insert_streamer(&pool, &platform_id, "Streamer2", "LIVE", "HIGH").await;
        insert_streamer(&pool, &platform_id, "Streamer3", "NOT_LIVE", "LOW").await;

        // Create manager and hydrate
        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(repo, broadcaster);

        let count = manager.hydrate().await.expect("Failed to hydrate");
        assert_eq!(count, 3);
        assert_eq!(manager.count(), 3);
    }

    #[tokio::test]
    async fn test_streamer_manager_get_all_active() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        // Insert streamers with different states
        insert_streamer(&pool, &platform_id, "Active1", "NOT_LIVE", "NORMAL").await;
        insert_streamer(&pool, &platform_id, "Active2", "LIVE", "HIGH").await;
        insert_streamer(&pool, &platform_id, "Inactive1", "CANCELLED", "NORMAL").await;
        insert_streamer(&pool, &platform_id, "Inactive2", "FATAL_ERROR", "NORMAL").await;

        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(repo, broadcaster);
        manager.hydrate().await.expect("Failed to hydrate");

        let active = manager.get_all_active();
        assert_eq!(active.len(), 2);
    }

    #[tokio::test]
    async fn test_streamer_manager_update_state() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;
        let streamer_id =
            insert_streamer(&pool, &platform_id, "TestStreamer", "NOT_LIVE", "NORMAL").await;

        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(repo, broadcaster);
        manager.hydrate().await.expect("Failed to hydrate");

        // Update state
        manager
            .update_state(&streamer_id, StreamerState::Live)
            .await
            .expect("Failed to update state");

        // Verify in-memory
        let metadata = manager
            .get_streamer(&streamer_id)
            .expect("Streamer not found");
        assert_eq!(metadata.state, StreamerState::Live);

        // Verify in database
        let result: (String,) = sqlx::query_as("SELECT state FROM streamers WHERE id = ?")
            .bind(&streamer_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to query");
        assert_eq!(result.0, "LIVE");
    }

    #[tokio::test]
    async fn test_streamer_manager_error_backoff() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;
        let streamer_id =
            insert_streamer(&pool, &platform_id, "TestStreamer", "NOT_LIVE", "NORMAL").await;

        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::with_error_threshold(repo, broadcaster, 2);
        manager.hydrate().await.expect("Failed to hydrate");

        // Record errors until backoff triggers
        manager
            .record_error(&streamer_id, "Error 1")
            .await
            .expect("Failed to record error");
        assert!(!manager.is_disabled(&streamer_id));

        manager
            .record_error(&streamer_id, "Error 2")
            .await
            .expect("Failed to record error");
        assert!(manager.is_disabled(&streamer_id));

        // Verify in database
        let result: (Option<i64>,) =
            sqlx::query_as("SELECT disabled_until FROM streamers WHERE id = ?")
                .bind(&streamer_id)
                .fetch_one(&pool)
                .await
                .expect("Failed to query");
        assert!(result.0.is_some());
    }

    #[tokio::test]
    async fn test_streamer_manager_record_success() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;
        let streamer_id =
            insert_streamer(&pool, &platform_id, "TestStreamer", "NOT_LIVE", "NORMAL").await;

        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::with_error_threshold(repo, broadcaster, 1);
        manager.hydrate().await.expect("Failed to hydrate");

        // Trigger backoff
        manager
            .record_error(&streamer_id, "Error")
            .await
            .expect("Failed to record error");
        assert!(manager.is_disabled(&streamer_id));

        // Record success
        manager
            .record_success(&streamer_id, true)
            .await
            .expect("Failed to record success");
        assert!(!manager.is_disabled(&streamer_id));

        // Verify last_live_time is set
        let metadata = manager
            .get_streamer(&streamer_id)
            .expect("Streamer not found");
        assert!(metadata.last_live_time.is_some());
    }

    #[tokio::test]
    async fn test_streamer_manager_concurrent_access() {
        let pool = Arc::new(setup_test_db().await);
        let platform_id = setup_platform(&pool).await;

        // Insert streamers
        for i in 0..10 {
            insert_streamer(
                &pool,
                &platform_id,
                &format!("Streamer{}", i),
                "NOT_LIVE",
                "NORMAL",
            )
            .await;
        }

        let repo = Arc::new(SqlxStreamerRepository::new(
            (*pool).clone(),
            (*pool).clone(),
        ));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = Arc::new(StreamerManager::new(repo, broadcaster));
        manager.hydrate().await.expect("Failed to hydrate");

        // Spawn concurrent read tasks
        let mut handles = vec![];
        for _ in 0..20 {
            let manager_clone = manager.clone();
            handles.push(tokio::spawn(async move {
                let all = manager_clone.get_all();
                all.len()
            }));
        }

        // All reads should succeed
        for handle in handles {
            let count = handle.await.expect("Task failed");
            assert_eq!(count, 10);
        }
    }

    #[tokio::test]
    async fn test_streamer_manager_get_by_platform() {
        let pool = setup_test_db().await;
        let platform1 = setup_platform(&pool).await;

        // Create second platform
        let platform2 = create_test_platform(&pool, "test_platform2").await;

        // Insert streamers on different platforms
        insert_streamer(&pool, &platform1, "P1S1", "NOT_LIVE", "NORMAL").await;
        insert_streamer(&pool, &platform1, "P1S2", "NOT_LIVE", "NORMAL").await;
        insert_streamer(&pool, &platform2, "P2S1", "NOT_LIVE", "NORMAL").await;

        let repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();
        let manager = StreamerManager::new(repo, broadcaster);
        manager.hydrate().await.expect("Failed to hydrate");

        let p1_streamers = manager.get_by_platform(&platform1);
        assert_eq!(p1_streamers.len(), 2);

        let p2_streamers = manager.get_by_platform(&platform2);
        assert_eq!(p2_streamers.len(), 1);
    }
}

/// End-to-end verification tests for Sprint 3.
/// These tests verify the complete flow: add streamer → detect status → update state → emit events.
mod end_to_end_tests {
    use super::*;
    use chrono::Utc;
    use rust_srec::config::{ConfigCache, ConfigEventBroadcaster, ConfigService};
    use rust_srec::database::repositories::SessionLifecycleRepository;
    use rust_srec::database::repositories::config::SqlxConfigRepository;
    use rust_srec::database::repositories::filter::SqlxFilterRepository;
    use rust_srec::database::repositories::session::SqlxSessionRepository;
    use rust_srec::database::repositories::streamer::SqlxStreamerRepository;
    use rust_srec::domain::StreamerState;
    use rust_srec::monitor::{FilterReason, LiveStatus, MonitorEvent, StreamMonitor};
    use rust_srec::session::{OfflineClassifier, SessionLifecycle};
    use rust_srec::streamer::{StreamerManager, StreamerMetadata};
    use std::sync::Arc;

    fn make_session_lifecycle(pool: &DbPool) -> Arc<SessionLifecycle> {
        Arc::new(SessionLifecycle::with_default_capacity(
            Arc::new(SessionLifecycleRepository::new(pool.clone())),
            Arc::new(OfflineClassifier::new()),
        ))
    }

    async fn setup_platform(pool: &DbPool) -> String {
        create_test_platform(pool, "test_e2e_platform").await
    }

    async fn setup_streamer(pool: &DbPool, platform_id: &str, name: &str, url: &str) -> String {
        create_test_streamer(pool, platform_id, name, url, "NOT_LIVE", "NORMAL").await
    }

    fn create_test_metadata(
        id: &str,
        name: &str,
        url: &str,
        platform_id: &str,
        state: StreamerState,
    ) -> StreamerMetadata {
        StreamerMetadata {
            id: id.to_string(),
            name: name.to_string(),
            url: url.to_string(),
            platform_config_id: platform_id.to_string(),
            template_config_id: None,
            state,
            priority: rust_srec::domain::Priority::Normal,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
            avatar_url: None,
            streamer_specific_config: None,
            last_error: None,
            effective_offline_check_count: 3,
            effective_offline_check_delay_ms: 20_000,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_e2e_live_status_processing() {
        // Setup database
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = setup_streamer(
            &pool,
            &platform_id,
            "TestStreamer",
            "https://twitch.tv/teststreamer",
        )
        .await;

        // Create services
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();

        let streamer_manager = Arc::new(StreamerManager::new(streamer_repo.clone(), broadcaster));
        streamer_manager.hydrate().await.expect("Failed to hydrate");

        // Create config service
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let cache = ConfigCache::new();
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo,
            streamer_repo.clone(),
            cache,
        ));

        // Create monitor
        let monitor = StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo,
            config_service,
            pool.clone(),
            make_session_lifecycle(&pool),
        );

        // Subscribe to events
        let mut event_rx = monitor.subscribe_events();

        // Create test metadata
        let metadata = create_test_metadata(
            &streamer_id,
            "TestStreamer",
            "https://twitch.tv/teststreamer",
            &platform_id,
            StreamerState::NotLive,
        );

        // Simulate live status detection
        let live_status = LiveStatus::Live {
            title: "Playing Rust!".to_string(),
            category: Some("Gaming".to_string()),
            avatar: None,
            started_at: Some(Utc::now()),
            viewer_count: Some(1000),
            streams: vec![],
            media_headers: None,
            media_extras: None,
            next_check_hint: None,
            candidates: vec![],
        };

        // Process the status
        monitor
            .process_status(&metadata, live_status)
            .await
            .expect("Failed to process status");

        // Verify state was updated
        let updated = streamer_manager
            .get_streamer(&streamer_id)
            .expect("Streamer not found");
        assert_eq!(updated.state, StreamerState::Live);

        // Verify event was emitted
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .expect("Timed out waiting for event")
            .expect("No event received");
        match event {
            MonitorEvent::StreamerLive {
                streamer_name,
                title,
                ..
            } => {
                assert_eq!(streamer_name, "TestStreamer");
                assert_eq!(title, "Playing Rust!");
            }
            _ => panic!("Expected StreamerLive event"),
        }
    }

    #[tokio::test]
    async fn test_e2e_fatal_error_processing() {
        // Setup database
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = setup_streamer(
            &pool,
            &platform_id,
            "MissingStreamer",
            "https://twitch.tv/missingstreamer",
        )
        .await;

        // Create services
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();

        let streamer_manager = Arc::new(StreamerManager::new(streamer_repo.clone(), broadcaster));
        streamer_manager.hydrate().await.expect("Failed to hydrate");

        // Create config service
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let cache = ConfigCache::new();
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo,
            streamer_repo.clone(),
            cache,
        ));

        // Create monitor
        let monitor = StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo,
            config_service,
            pool.clone(),
            make_session_lifecycle(&pool),
        );

        // Subscribe to events
        let mut event_rx = monitor.subscribe_events();

        // Create test metadata
        let metadata = create_test_metadata(
            &streamer_id,
            "MissingStreamer",
            "https://twitch.tv/missingstreamer",
            &platform_id,
            StreamerState::NotLive,
        );

        // Process NotFound status (fatal error)
        monitor
            .process_status(&metadata, LiveStatus::NotFound)
            .await
            .expect("Failed to process status");

        // Verify state was updated to NotFound
        let updated = streamer_manager
            .get_streamer(&streamer_id)
            .expect("Streamer not found");
        assert_eq!(updated.state, StreamerState::NotFound);

        // Verify fatal error event was emitted
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .expect("Timed out waiting for event")
            .expect("No event received");
        match event {
            MonitorEvent::FatalError {
                streamer_name,
                error_type,
                new_state,
                ..
            } => {
                assert_eq!(streamer_name, "MissingStreamer");
                assert_eq!(error_type, rust_srec::monitor::FatalErrorType::NotFound);
                assert_eq!(new_state, StreamerState::NotFound);
            }
            _ => panic!("Expected FatalError event"),
        }
    }

    #[tokio::test]
    async fn test_e2e_filter_evaluation() {
        // Setup database
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = setup_streamer(
            &pool,
            &platform_id,
            "FilteredStreamer",
            "https://twitch.tv/filteredstreamer",
        )
        .await;

        // Create services
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();

        let streamer_manager = Arc::new(StreamerManager::new(streamer_repo.clone(), broadcaster));
        streamer_manager.hydrate().await.expect("Failed to hydrate");

        // Create config service
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let cache = ConfigCache::new();
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo,
            streamer_repo.clone(),
            cache,
        ));

        // Create monitor
        let monitor = StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo,
            config_service,
            pool.clone(),
            make_session_lifecycle(&pool),
        );

        // Create test metadata
        let metadata = create_test_metadata(
            &streamer_id,
            "FilteredStreamer",
            "https://twitch.tv/filteredstreamer",
            &platform_id,
            StreamerState::NotLive,
        );

        // Process filtered status (out of schedule)
        let filtered_status = LiveStatus::Filtered {
            reason: FilterReason::OutOfSchedule {
                next_available: None,
            },
            title: "Late Night Stream".to_string(),
            category: Some("Just Chatting".to_string()),
        };

        monitor
            .process_status(&metadata, filtered_status)
            .await
            .expect("Failed to process status");

        // Verify state was updated to OutOfSchedule
        let updated = streamer_manager
            .get_streamer(&streamer_id)
            .expect("Streamer not found");
        assert_eq!(updated.state, StreamerState::OutOfSchedule);
    }

    #[tokio::test]
    async fn test_e2e_transient_error_handling() {
        // Setup database
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = setup_streamer(
            &pool,
            &platform_id,
            "ErrorStreamer",
            "https://twitch.tv/errorstreamer",
        )
        .await;

        // Create services
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        let broadcaster = ConfigEventBroadcaster::new();

        let streamer_manager = Arc::new(StreamerManager::new(streamer_repo.clone(), broadcaster));
        streamer_manager.hydrate().await.expect("Failed to hydrate");

        // Create config service
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let cache = ConfigCache::new();
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo,
            streamer_repo.clone(),
            cache.clone(),
        ));

        // Create monitor
        let monitor = StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo,
            config_service,
            pool.clone(),
            make_session_lifecycle(&pool),
        );

        // Subscribe to events
        let mut event_rx = monitor.subscribe_events();

        // Create test metadata
        let metadata = create_test_metadata(
            &streamer_id,
            "ErrorStreamer",
            "https://twitch.tv/errorstreamer",
            &platform_id,
            StreamerState::NotLive,
        );

        // Handle transient error
        monitor
            .handle_error(&metadata, "Network timeout")
            .await
            .expect("Failed to handle error");

        // Verify error count was incremented
        let updated = streamer_manager
            .get_streamer(&streamer_id)
            .expect("Streamer not found");
        assert_eq!(updated.consecutive_error_count, 1);

        // Verify transient error event was emitted
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), event_rx.recv())
            .await
            .expect("Timed out waiting for event")
            .expect("No event received");
        match event {
            MonitorEvent::TransientError {
                streamer_name,
                error_message,
                consecutive_errors,
                ..
            } => {
                assert_eq!(streamer_name, "ErrorStreamer");
                assert_eq!(error_message, "Network timeout");
                assert_eq!(consecutive_errors, 1);
            }
            _ => panic!("Expected TransientError event"),
        }
    }

    /// Regression: disable a Live streamer with an active session via the
    /// lifecycle's `end_for_disable`, then re-trigger LiveDetected. The
    /// pre-fix `force_end_active_session` wrote DB-only and left the
    /// in-memory FSM stuck in Hysteresis, so re-enable took the
    /// `resume_from_hysteresis` short-circuit and silently restarted a
    /// download under an already-ended session_id. With the fix, a new
    /// `Created` outcome with a fresh session_id must be produced and the
    /// old row must be ended in DB.
    #[tokio::test]
    async fn test_disable_then_reenable_creates_fresh_session() {
        use rust_srec::database::repositories::StartSessionOutcome;
        use rust_srec::session::LiveDetectedArgs;

        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;

        let streamer_id = setup_streamer(
            &pool,
            &platform_id,
            "TestStreamer",
            "https://twitch.tv/teststreamer",
        )
        .await;

        let lifecycle = make_session_lifecycle(&pool);

        let now = Utc::now();
        let streams = Vec::new();
        let live_args = LiveDetectedArgs {
            streamer_id: &streamer_id,
            streamer_name: "TestStreamer",
            streamer_url: "https://twitch.tv/teststreamer",
            current_avatar: None,
            new_avatar: None,
            title: "first session",
            category: None,
            streams: &streams,
            media_headers: None,
            media_extras: None,
            now,
        };

        // Step 1: streamer goes live → fresh session.
        let first = lifecycle
            .on_live_detected(live_args)
            .await
            .expect("first live");
        let first_id = first.session_id().to_string();
        assert!(matches!(first, StartSessionOutcome::Created { .. }));

        // Step 2: user disables — lifecycle tears down. (In production the
        // download's CleanDisconnect would have parked the session in
        // Hysteresis first; we test the simpler path here. The hysteresis
        // path is covered by lifecycle unit tests for user-disabled teardown.)
        let resolved = lifecycle
            .end_for_disable(&streamer_id, "TestStreamer")
            .await
            .expect("end_for_disable");
        assert_eq!(resolved.as_deref(), Some(first_id.as_str()));

        // DB end_time set on the first session.
        let end_time: Option<i64> =
            sqlx::query_scalar("SELECT end_time FROM live_sessions WHERE id = ?")
                .bind(&first_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(end_time.is_some(), "first session must be ended in DB");

        // In-memory state must be Ended (the bug we're fixing was that
        // force_end_active_session only wrote DB, leaving in-memory state
        // stale — re-enable would then take the resume_from_hysteresis
        // short-circuit instead of creating a new session).
        let snap = lifecycle
            .session_snapshot(&first_id)
            .expect("session in memory after disable");
        assert!(
            snap.is_ended(),
            "in-memory state must be Ended after disable"
        );

        // Step 3: re-enable triggers fresh LiveDetected. Because the
        // in-memory state is Ended (not Hysteresis), the lifecycle's
        // `on_live_detected` falls through to `start_or_resume` which
        // creates a fresh session_id.
        let later = now + chrono::Duration::seconds(5);
        let streams2 = Vec::new();
        let relive_args = LiveDetectedArgs {
            streamer_id: &streamer_id,
            streamer_name: "TestStreamer",
            streamer_url: "https://twitch.tv/teststreamer",
            current_avatar: None,
            new_avatar: None,
            title: "second session",
            category: None,
            streams: &streams2,
            media_headers: None,
            media_extras: None,
            now: later,
        };
        let second = lifecycle
            .on_live_detected(relive_args)
            .await
            .expect("re-live");
        let second_id = second.session_id().to_string();

        assert!(
            matches!(second, StartSessionOutcome::Created { .. }),
            "re-enable must create a fresh session, got {second:?}"
        );
        assert_ne!(
            first_id, second_id,
            "fresh session must have a different id than the disabled one"
        );

        // Two rows in live_sessions: the first ended, the second active.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM live_sessions WHERE streamer_id = ?")
                .bind(&streamer_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 2);

        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL",
        )
        .bind(&streamer_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            active_count, 1,
            "exactly one active session after re-enable"
        );
    }
}
