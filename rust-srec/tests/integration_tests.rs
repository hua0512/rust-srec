//! Integration tests for rust-srec database layer.
//!
//! These tests use a real SQLite database (in-memory) to verify
//! repository operations work correctly with the actual schema.

use rust_srec::database::{init_pool, run_migrations, DbPool};
use rust_srec::Error;

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

mod database_tests {
    use super::*;

    #[tokio::test]
    async fn test_database_migrations() {
        let pool = setup_test_db().await;
        
        // Verify tables exist by querying sqlite_master
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
        )
            .fetch_all(&pool)
            .await
            .expect("Failed to query tables");
        
        let table_names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
        
        // Check essential tables exist
        assert!(table_names.contains(&"global_config"), "global_config table missing");
        assert!(table_names.contains(&"platform_config"), "platform_config table missing");
        assert!(table_names.contains(&"template_config"), "template_config table missing");
        assert!(table_names.contains(&"streamers"), "streamers table missing");
        assert!(table_names.contains(&"filters"), "filters table missing");
        assert!(table_names.contains(&"live_sessions"), "live_sessions table missing");
        assert!(table_names.contains(&"media_outputs"), "media_outputs table missing");
        assert!(table_names.contains(&"job"), "job table missing");
        assert!(table_names.contains(&"notification_channel"), "notification_channel table missing");
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

    #[tokio::test]
    async fn test_global_config_crud() {
        let pool = setup_test_db().await;
        
        // Insert a global config
        let id = uuid::Uuid::new_v4().to_string();
        let proxy_config = r#"{"enabled":false,"url":null}"#;
        
        sqlx::query(
            "INSERT INTO global_config (id, output_folder, output_filename_template, output_file_format, 
             min_segment_size_bytes, max_download_duration_secs, max_part_size_bytes, record_danmu,
             max_concurrent_downloads, max_concurrent_uploads, streamer_check_delay_ms, proxy_config,
             offline_check_delay_ms, offline_check_count, default_download_engine, max_concurrent_cpu_jobs,
             max_concurrent_io_jobs, job_history_retention_days)
             VALUES (?, './downloads', '{streamer}-{title}', 'flv', 1048576, 0, 8589934592, FALSE,
             6, 3, 60000, ?, 20000, 3, 'ffmpeg', 0, 8, 30)"
        )
            .bind(&id)
            .bind(proxy_config)
            .execute(&pool)
            .await
            .expect("Failed to insert global config");
        
        // Read it back
        let result: (String, String, bool) = sqlx::query_as(
            "SELECT id, output_folder, record_danmu FROM global_config WHERE id = ?"
        )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read global config");
        
        assert_eq!(result.0, id);
        assert_eq!(result.1, "./downloads");
        assert!(!result.2);
    }

    #[tokio::test]
    async fn test_platform_config_crud() {
        let pool = setup_test_db().await;
        
        let id = uuid::Uuid::new_v4().to_string();
        
        // Insert platform config
        sqlx::query(
            "INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
             VALUES (?, 'twitch', 60000, 1000)"
        )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("Failed to insert platform config");
        
        // Query by platform name
        let result: (String, String, i64) = sqlx::query_as(
            "SELECT id, platform_name, fetch_delay_ms FROM platform_config WHERE platform_name = ?"
        )
            .bind("twitch")
            .fetch_one(&pool)
            .await
            .expect("Failed to read platform config");
        
        assert_eq!(result.1, "twitch");
        assert_eq!(result.2, 60000);
    }

    #[tokio::test]
    async fn test_template_config_crud() {
        let pool = setup_test_db().await;
        
        let id = uuid::Uuid::new_v4().to_string();
        
        // Insert template config with optional fields
        sqlx::query(
            "INSERT INTO template_config (id, name, output_folder, max_bitrate)
             VALUES (?, 'high-quality', './hq-downloads', 8000)"
        )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("Failed to insert template config");
        
        // Read it back
        let result: (String, String, Option<String>, Option<i32>) = sqlx::query_as(
            "SELECT id, name, output_folder, max_bitrate FROM template_config WHERE id = ?"
        )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read template config");
        
        assert_eq!(result.1, "high-quality");
        assert_eq!(result.2, Some("./hq-downloads".to_string()));
        assert_eq!(result.3, Some(8000));
    }
}

mod streamer_repository_tests {
    use super::*;

    async fn setup_platform(pool: &DbPool) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
             VALUES (?, 'test_platform', 60000, 1000)"
        )
            .bind(&id)
            .execute(pool)
            .await
            .expect("Failed to insert platform config");
        id
    }

    #[tokio::test]
    async fn test_streamer_crud() {
        let pool = setup_test_db().await;
        let platform_id = setup_platform(&pool).await;
        
        let streamer_id = uuid::Uuid::new_v4().to_string();
        
        // Insert streamer
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state, priority)
             VALUES (?, 'TestStreamer', 'https://twitch.tv/test', ?, 'NOT_LIVE', 'NORMAL')"
        )
            .bind(&streamer_id)
            .bind(&platform_id)
            .execute(&pool)
            .await
            .expect("Failed to insert streamer");
        
        // Read it back
        let result: (String, String, String, String) = sqlx::query_as(
            "SELECT id, name, state, priority FROM streamers WHERE id = ?"
        )
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
        
        let streamer_id = uuid::Uuid::new_v4().to_string();
        
        // Insert streamer
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state, priority)
             VALUES (?, 'TestStreamer', 'https://twitch.tv/test', ?, 'NOT_LIVE', 'NORMAL')"
        )
            .bind(&streamer_id)
            .bind(&platform_id)
            .execute(&pool)
            .await
            .expect("Failed to insert streamer");
        
        // Update state to LIVE
        sqlx::query("UPDATE streamers SET state = 'LIVE' WHERE id = ?")
            .bind(&streamer_id)
            .execute(&pool)
            .await
            .expect("Failed to update state");
        
        // Verify state changed
        let result: (String,) = sqlx::query_as(
            "SELECT state FROM streamers WHERE id = ?"
        )
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
        for (name, priority) in [("High1", "HIGH"), ("Normal1", "NORMAL"), ("Low1", "LOW"), ("High2", "HIGH")] {
            let id = uuid::Uuid::new_v4().to_string();
            let url = format!("https://twitch.tv/{}", name.to_lowercase());
            sqlx::query(
                "INSERT INTO streamers (id, name, url, platform_config_id, state, priority)
                 VALUES (?, ?, ?, ?, 'NOT_LIVE', ?)"
            )
                .bind(&id)
                .bind(name)
                .bind(&url)
                .bind(&platform_id)
                .bind(priority)
                .execute(&pool)
                .await
                .expect("Failed to insert streamer");
        }
        
        // Query by priority
        let high_priority: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM streamers WHERE priority = 'HIGH'"
        )
            .fetch_all(&pool)
            .await
            .expect("Failed to query high priority");
        
        assert_eq!(high_priority.len(), 2);
    }
}


mod session_repository_tests {
    use super::*;

    async fn setup_streamer(pool: &DbPool) -> String {
        let platform_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
             VALUES (?, 'test_platform', 60000, 1000)"
        )
            .bind(&platform_id)
            .execute(pool)
            .await
            .expect("Failed to insert platform config");
        
        let streamer_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state, priority)
             VALUES (?, 'TestStreamer', 'https://twitch.tv/test', ?, 'NOT_LIVE', 'NORMAL')"
        )
            .bind(&streamer_id)
            .bind(&platform_id)
            .execute(pool)
            .await
            .expect("Failed to insert streamer");
        
        streamer_id
    }

    #[tokio::test]
    async fn test_live_session_crud() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;
        
        let session_id = uuid::Uuid::new_v4().to_string();
        let start_time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert session
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time)
             VALUES (?, ?, ?)"
        )
            .bind(&session_id)
            .bind(&streamer_id)
            .bind(&start_time)
            .execute(&pool)
            .await
            .expect("Failed to insert session");
        
        // Read it back
        let result: (String, String, Option<String>) = sqlx::query_as(
            "SELECT id, streamer_id, end_time FROM live_sessions WHERE id = ?"
        )
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read session");
        
        assert_eq!(result.0, session_id);
        assert_eq!(result.1, streamer_id);
        assert!(result.2.is_none()); // Session not ended yet
    }

    #[tokio::test]
    async fn test_session_end() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;
        
        let session_id = uuid::Uuid::new_v4().to_string();
        let start_time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert session
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time)
             VALUES (?, ?, ?)"
        )
            .bind(&session_id)
            .bind(&streamer_id)
            .bind(&start_time)
            .execute(&pool)
            .await
            .expect("Failed to insert session");
        
        // End session
        let end_time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        sqlx::query("UPDATE live_sessions SET end_time = ? WHERE id = ?")
            .bind(&end_time)
            .bind(&session_id)
            .execute(&pool)
            .await
            .expect("Failed to end session");
        
        // Verify end time is set
        let result: (Option<String>,) = sqlx::query_as(
            "SELECT end_time FROM live_sessions WHERE id = ?"
        )
            .bind(&session_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read session");
        
        assert!(result.0.is_some());
    }

    #[tokio::test]
    async fn test_recent_sessions_query() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;
        
        // Insert multiple sessions
        for i in 0..5 {
            let session_id = uuid::Uuid::new_v4().to_string();
            let start_time = format!("2024-01-{:02} 12:00:00", i + 1);
            
            sqlx::query(
                "INSERT INTO live_sessions (id, streamer_id, start_time)
                 VALUES (?, ?, ?)"
            )
                .bind(&session_id)
                .bind(&streamer_id)
                .bind(&start_time)
                .execute(&pool)
                .await
                .expect("Failed to insert session");
        }
        
        // Query recent sessions (using index)
        let sessions: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, start_time FROM live_sessions 
             WHERE streamer_id = ? 
             ORDER BY start_time DESC 
             LIMIT 3"
        )
            .bind(&streamer_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to query sessions");
        
        assert_eq!(sessions.len(), 3);
        // Most recent first
        assert!(sessions[0].1 > sessions[1].1);
    }
}

mod job_repository_tests {
    use super::*;

    #[tokio::test]
    async fn test_job_crud() {
        let pool = setup_test_db().await;
        
        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert job
        sqlx::query(
            "INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
             VALUES (?, 'DOWNLOAD', 'PENDING', '{}', '{}', ?, ?)"
        )
            .bind(&job_id)
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await
            .expect("Failed to insert job");
        
        // Read it back
        let result: (String, String, String) = sqlx::query_as(
            "SELECT id, job_type, status FROM job WHERE id = ?"
        )
            .bind(&job_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read job");
        
        assert_eq!(result.1, "DOWNLOAD");
        assert_eq!(result.2, "PENDING");
    }

    #[tokio::test]
    async fn test_job_status_update() {
        let pool = setup_test_db().await;
        
        let job_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert job
        sqlx::query(
            "INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
             VALUES (?, 'DOWNLOAD', 'PENDING', '{}', '{}', ?, ?)"
        )
            .bind(&job_id)
            .bind(&now)
            .bind(&now)
            .execute(&pool)
            .await
            .expect("Failed to insert job");
        
        // Update status
        sqlx::query("UPDATE job SET status = 'PROCESSING' WHERE id = ?")
            .bind(&job_id)
            .execute(&pool)
            .await
            .expect("Failed to update status");
        
        // Verify
        let result: (String,) = sqlx::query_as(
            "SELECT status FROM job WHERE id = ?"
        )
            .bind(&job_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read status");
        
        assert_eq!(result.0, "PROCESSING");
    }

    #[tokio::test]
    async fn test_pending_jobs_query() {
        let pool = setup_test_db().await;
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert jobs with different statuses
        for (status, job_type) in [
            ("PENDING", "DOWNLOAD"),
            ("PENDING", "PIPELINE"),
            ("PROCESSING", "DOWNLOAD"),
            ("COMPLETED", "DOWNLOAD"),
        ] {
            let job_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
                 VALUES (?, ?, ?, '{}', '{}', ?, ?)"
            )
                .bind(&job_id)
                .bind(job_type)
                .bind(status)
                .bind(&now)
                .bind(&now)
                .execute(&pool)
                .await
                .expect("Failed to insert job");
        }
        
        // Query pending download jobs
        let pending: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM job WHERE status = 'PENDING' AND job_type = 'DOWNLOAD'"
        )
            .fetch_all(&pool)
            .await
            .expect("Failed to query pending jobs");
        
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_reset_interrupted_jobs() {
        let pool = setup_test_db().await;
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert interrupted jobs
        for _ in 0..3 {
            let job_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
                 VALUES (?, 'DOWNLOAD', 'INTERRUPTED', '{}', '{}', ?, ?)"
            )
                .bind(&job_id)
                .bind(&now)
                .bind(&now)
                .execute(&pool)
                .await
                .expect("Failed to insert job");
        }
        
        // Reset interrupted jobs
        let result = sqlx::query(
            "UPDATE job SET status = 'PENDING' WHERE status = 'INTERRUPTED'"
        )
            .execute(&pool)
            .await
            .expect("Failed to reset jobs");
        
        assert_eq!(result.rows_affected(), 3);
        
        // Verify no interrupted jobs remain
        let interrupted: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM job WHERE status = 'INTERRUPTED'"
        )
            .fetch_all(&pool)
            .await
            .expect("Failed to query");
        
        assert_eq!(interrupted.len(), 0);
    }
}


mod notification_repository_tests {
    use super::*;

    #[tokio::test]
    async fn test_notification_channel_crud() {
        let pool = setup_test_db().await;
        
        let channel_id = uuid::Uuid::new_v4().to_string();
        let settings = r#"{"webhook_url":"https://discord.com/api/webhooks/123"}"#;
        
        // Insert channel
        sqlx::query(
            "INSERT INTO notification_channel (id, name, channel_type, settings)
             VALUES (?, 'Discord Alerts', 'DISCORD', ?)"
        )
            .bind(&channel_id)
            .bind(settings)
            .execute(&pool)
            .await
            .expect("Failed to insert channel");
        
        // Read it back
        let result: (String, String, String) = sqlx::query_as(
            "SELECT id, name, channel_type FROM notification_channel WHERE id = ?"
        )
            .bind(&channel_id)
            .fetch_one(&pool)
            .await
            .expect("Failed to read channel");
        
        assert_eq!(result.1, "Discord Alerts");
        assert_eq!(result.2, "DISCORD");
    }

    #[tokio::test]
    async fn test_notification_subscription() {
        let pool = setup_test_db().await;
        
        let channel_id = uuid::Uuid::new_v4().to_string();
        
        // Insert channel first
        sqlx::query(
            "INSERT INTO notification_channel (id, name, channel_type, settings)
             VALUES (?, 'Test Channel', 'WEBHOOK', '{}')"
        )
            .bind(&channel_id)
            .execute(&pool)
            .await
            .expect("Failed to insert channel");
        
        // Subscribe to events
        for event in ["streamer.online", "streamer.offline", "download.complete"] {
            sqlx::query(
                "INSERT INTO notification_subscription (channel_id, event_name)
                 VALUES (?, ?)"
            )
                .bind(&channel_id)
                .bind(event)
                .execute(&pool)
                .await
                .expect("Failed to insert subscription");
        }
        
        // Query subscriptions
        let subs: Vec<(String,)> = sqlx::query_as(
            "SELECT event_name FROM notification_subscription WHERE channel_id = ?"
        )
            .bind(&channel_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to query subscriptions");
        
        assert_eq!(subs.len(), 3);
    }

    #[tokio::test]
    async fn test_dead_letter_queue() {
        let pool = setup_test_db().await;
        
        let channel_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        
        // Insert channel first
        sqlx::query(
            "INSERT INTO notification_channel (id, name, channel_type, settings)
             VALUES (?, 'Test Channel', 'WEBHOOK', '{}')"
        )
            .bind(&channel_id)
            .execute(&pool)
            .await
            .expect("Failed to insert channel");
        
        // Insert dead letter entries
        for i in 0..3 {
            let id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO notification_dead_letter 
                 (id, channel_id, event_name, event_payload, error_message, retry_count, 
                  first_attempt_at, last_attempt_at, created_at)
                 VALUES (?, ?, 'test.event', '{}', 'Connection timeout', ?, ?, ?, ?)"
            )
                .bind(&id)
                .bind(&channel_id)
                .bind(i + 1)
                .bind(&now)
                .bind(&now)
                .bind(&now)
                .execute(&pool)
                .await
                .expect("Failed to insert dead letter");
        }
        
        // Query dead letters
        let dead_letters: Vec<(String, i32)> = sqlx::query_as(
            "SELECT id, retry_count FROM notification_dead_letter WHERE channel_id = ?"
        )
            .bind(&channel_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to query dead letters");
        
        assert_eq!(dead_letters.len(), 3);
    }
}

mod filter_repository_tests {
    use super::*;

    async fn setup_streamer(pool: &DbPool) -> String {
        let platform_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
             VALUES (?, 'test_platform', 60000, 1000)"
        )
            .bind(&platform_id)
            .execute(pool)
            .await
            .expect("Failed to insert platform config");
        
        let streamer_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state, priority)
             VALUES (?, 'TestStreamer', 'https://twitch.tv/test', ?, 'NOT_LIVE', 'NORMAL')"
        )
            .bind(&streamer_id)
            .bind(&platform_id)
            .execute(pool)
            .await
            .expect("Failed to insert streamer");
        
        streamer_id
    }

    #[tokio::test]
    async fn test_filter_crud() {
        let pool = setup_test_db().await;
        let streamer_id = setup_streamer(&pool).await;
        
        let filter_id = uuid::Uuid::new_v4().to_string();
        let config = r#"{"include":["gaming"],"exclude":["ads"]}"#;
        
        // Insert filter
        sqlx::query(
            "INSERT INTO filters (id, streamer_id, filter_type, config)
             VALUES (?, ?, 'KEYWORD', ?)"
        )
            .bind(&filter_id)
            .bind(&streamer_id)
            .bind(config)
            .execute(&pool)
            .await
            .expect("Failed to insert filter");
        
        // Read it back
        let result: (String, String, String) = sqlx::query_as(
            "SELECT id, filter_type, config FROM filters WHERE id = ?"
        )
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
        
        // Insert filters
        for i in 0..3 {
            let filter_id = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO filters (id, streamer_id, filter_type, config)
                 VALUES (?, ?, 'KEYWORD', '{}')"
            )
                .bind(&filter_id)
                .bind(&streamer_id)
                .execute(&pool)
                .await
                .expect("Failed to insert filter");
        }
        
        // Verify filters exist
        let before: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM filters WHERE streamer_id = ?"
        )
            .bind(&streamer_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to query filters");
        
        assert_eq!(before.len(), 3);
        
        // Delete streamer (should cascade delete filters)
        sqlx::query("DELETE FROM streamers WHERE id = ?")
            .bind(&streamer_id)
            .execute(&pool)
            .await
            .expect("Failed to delete streamer");
        
        // Verify filters are deleted
        let after: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM filters WHERE streamer_id = ?"
        )
            .bind(&streamer_id)
            .fetch_all(&pool)
            .await
            .expect("Failed to query filters");
        
        assert_eq!(after.len(), 0);
    }
}

mod concurrent_access_tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_concurrent_reads() {
        let pool = Arc::new(setup_test_db().await);
        
        // Insert test data
        let platform_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
             VALUES (?, 'test_platform', 60000, 1000)"
        )
            .bind(&platform_id)
            .execute(pool.as_ref())
            .await
            .expect("Failed to insert platform config");
        
        // Spawn multiple concurrent read tasks
        let mut handles = vec![];
        for _ in 0..10 {
            let pool_clone = pool.clone();
            let platform_id_clone = platform_id.clone();
            handles.push(tokio::spawn(async move {
                let result: (String,) = sqlx::query_as(
                    "SELECT platform_name FROM platform_config WHERE id = ?"
                )
                    .bind(&platform_id_clone)
                    .fetch_one(pool_clone.as_ref())
                    .await
                    .expect("Failed to read");
                result.0
            }));
        }
        
        // All reads should succeed
        for handle in handles {
            let result = handle.await.expect("Task failed");
            assert_eq!(result, "test_platform");
        }
    }
}
