mod credential_profiles;
mod email_validation;
mod proxies;
mod timezone_import;

use std::sync::Arc;

use sqlx::SqlitePool;

use super::snapshot::{ImportSnapshot, validate_import};
use super::streamers::{clears_error_tracking, imported_state};
use super::*;
use crate::api::auth_service::MAX_USERNAME_LENGTH;
use crate::config::ConfigService;
use crate::config::backup::{ConfigExport, GlobalConfigExport, ImportMode, UserExport};
use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, JobPreset, PipelinePreset,
    PlatformConfigDbModel, StreamerDbModel, TemplateConfigDbModel, UserDbModel,
};
use crate::database::repositories::config::{
    SqlxConfigRepository, import_template as persist_template,
};
use crate::database::repositories::credential_selections;
use crate::database::repositories::preset::{
    import_job_preset as persist_job_preset, import_pipeline_preset as persist_pipeline_preset,
};
use crate::database::repositories::streamer::{
    SqlxStreamerRepository, import_streamer_row as persist_streamer,
};
use crate::database::repositories::user::import_user as persist_user;
use crate::database::{begin_immediate, init_pool_with_size, run_migrations};
use crate::domain::StreamerState;

#[tokio::test]
async fn timestamp_import_persistence_and_snapshot_use_integer_milliseconds() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let at = jiff::fmt::temporal::DateTimeParser::new()
        .parse_timestamp("2026-01-02T03:04:05.123456Z")
        .unwrap();
    let mut template = TemplateConfigDbModel::new("timestamp-import-template");
    template.created_at = at;
    template.updated_at = at;
    let mut job = JobPreset::new("timestamp-import-job", "remux", serde_json::json!({}));
    job.created_at = at;
    job.updated_at = at;
    let mut pipeline = PipelinePreset::new(
        "timestamp-import-pipeline",
        crate::database::models::DagPipelineDefinition::new("test", vec![]),
    );
    pipeline.created_at = at;
    pipeline.updated_at = at;
    let mut tx = begin_immediate(&pool).await.unwrap();
    for updated in [at, at + jiff::SignedDuration::from_millis(7)] {
        template.updated_at = updated;
        job.updated_at = updated;
        pipeline.updated_at = updated;
        persist_template(&mut tx, &template).await.unwrap();
        persist_job_preset(&mut tx, &job).await.unwrap();
        persist_pipeline_preset(&mut tx, &pipeline).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        for (table, id, created_at, updated_at) in [
            (
                "template_config",
                &template.id,
                snapshot.templates[&template.name].created_at,
                snapshot.templates[&template.name].updated_at,
            ),
            (
                "job_presets",
                &job.id,
                snapshot.job_presets[&job.name].created_at,
                snapshot.job_presets[&job.name].updated_at,
            ),
            (
                "pipeline_presets",
                &pipeline.id,
                snapshot.pipeline_presets[&pipeline.name].created_at,
                snapshot.pipeline_presets[&pipeline.name].updated_at,
            ),
        ] {
            assert_eq!(created_at.as_millisecond(), at.as_millisecond());
            assert_eq!(updated_at.as_millisecond(), updated.as_millisecond());
            let storage: (String, String) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT typeof(created_at), typeof(updated_at) FROM {table} WHERE id = ?"
            )))
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            assert_eq!(storage, ("integer".to_owned(), "integer".to_owned()));
        }
    }
    tx.commit().await.unwrap();
}

const VALID_PASSWORD_HASH: &str = concat!(
    "$argon2id$v=19$m=19456,t=2,p=1$",
    "c2FsdHNhbHRzYWx0c2FsdA$",
    "TiYF50X49Bmdr2XJZ6yBn4ozsFFV8gGZJIXZ3Jg9pWg"
);

fn import_config(global: &GlobalConfigDbModel) -> ConfigExport {
    ConfigExport {
        version: "0.1.7".to_string(),
        exported_at: jiff::Timestamp::now().to_string(),
        global_config: GlobalConfigExport {
            output_folder: global.output_folder.clone(),
            output_filename_template: global.output_filename_template.clone(),
            output_file_format: global.output_file_format.clone(),
            min_segment_size_bytes: global.min_segment_size_bytes,
            max_download_duration_secs: global.max_download_duration_secs,
            max_part_size_bytes: global.max_part_size_bytes,
            record_danmu: global.record_danmu,
            danmu_statistics: None,
            max_concurrent_downloads: global.max_concurrent_downloads,
            max_concurrent_uploads: global.max_concurrent_uploads,
            streamer_check_delay_ms: global.streamer_check_delay_ms,
            proxy_config: serde_json::json!({"enabled": false}),
            proxy_route: None,
            offline_check_delay_ms: global.offline_check_delay_ms,
            offline_check_count: global.offline_check_count,
            default_download_engine: global.default_download_engine.clone(),
            default_extractor: None,
            max_concurrent_cpu_jobs: global.max_concurrent_cpu_jobs,
            max_concurrent_io_jobs: global.max_concurrent_io_jobs,
            job_history_retention_days: global.job_history_retention_days,
            notification_event_log_retention_days: global.notification_event_log_retention_days,
            output_retention_days: global.output_retention_days,
            output_retention_delete_files: global.output_retention_delete_files,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            log_filter_directive: Some(global.log_filter_directive.clone()),
            auto_thumbnail: global.auto_thumbnail,
            pipeline_cpu_job_timeout_secs: global.pipeline_cpu_job_timeout_secs,
            pipeline_io_job_timeout_secs: global.pipeline_io_job_timeout_secs,
            pipeline_execute_timeout_secs: global.pipeline_execute_timeout_secs,
            queue_freshness_threshold_ms: global.queue_freshness_threshold_ms,
            gpu_health_probe_interval_secs: global.gpu_health_probe_interval_secs,
            stream_proxy_allow_private_targets: global.stream_proxy_allow_private_targets,
        },
        templates: Vec::new(),
        streamers: Vec::new(),
        engines: Vec::new(),
        platforms: Vec::new(),
        notification_channels: Vec::new(),
        job_presets: Vec::new(),
        pipeline_presets: Vec::new(),
        users: Vec::new(),
        credential_profiles: Vec::new(),
        proxies: Vec::new(),
    }
}

fn imported_user(username: &str, roles: Vec<String>, is_active: bool) -> UserExport {
    UserExport {
        id: format!("{username}-id"),
        username: username.to_string(),
        password_hash: VALID_PASSWORD_HASH.to_string(),
        email: None,
        roles,
        is_active,
        must_change_password: false,
        last_login_at: None,
        created_at: 1_767_225_600_000,
        updated_at: 1_767_225_600_000,
    }
}

fn imported_streamer(url: &str, platform: &str) -> crate::config::backup::StreamerExport {
    crate::config::backup::StreamerExport {
        name: "imported".to_string(),
        url: url.to_string(),
        platform: platform.to_string(),
        template: None,
        priority: "NORMAL".to_string(),
        state: "NOT_LIVE".to_string(),
        avatar_url: None,
        streamer_specific_config: None,
        proxy_route: None,
        filters: Vec::new(),
    }
}

async fn seed_case_duplicate_streamers(pool: &SqlitePool) -> PlatformConfigDbModel {
    sqlx::query("DROP TABLE streamers")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        CREATE TABLE streamers (
            id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            url TEXT NOT NULL UNIQUE,
            platform_config_id TEXT NOT NULL,
            template_config_id TEXT,
            state TEXT NOT NULL,
            priority TEXT NOT NULL DEFAULT 'NORMAL',
            last_live_time INTEGER,
            streamer_specific_config TEXT,
            platform_metadata TEXT,
            consecutive_error_count INTEGER DEFAULT 0,
            last_error TEXT,
            disabled_until INTEGER,
            avatar TEXT,
            created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
            updated_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
            deleted_at INTEGER,
            proxy_route TEXT NOT NULL DEFAULT 'inherit',
            proxy_id TEXT
        )
        "#,
    )
    .execute(pool)
    .await
    .unwrap();

    let platform: PlatformConfigDbModel =
        sqlx::query_as("SELECT * FROM platform_config WHERE platform_name = 'huya'")
            .fetch_one(pool)
            .await
            .unwrap();
    let mut lower = StreamerDbModel::new("lower", "https://example.com/live/alpha", &platform.id);
    lower.id = "streamer-a".to_string();
    let mut mixed = StreamerDbModel::new("mixed", "https://example.com/live/Alpha", &platform.id);
    mixed.id = "streamer-b".to_string();
    let mut setup_tx = begin_immediate(pool).await.unwrap();
    persist_streamer(&mut setup_tx, &lower).await.unwrap();
    persist_streamer(&mut setup_tx, &mixed).await.unwrap();
    setup_tx.commit().await.unwrap();
    platform
}

#[test]
fn validation_rejects_non_argon2id_password_hashes() {
    let global = GlobalConfigDbModel::default();
    let mut config = import_config(&global);
    let mut user = imported_user("admin", vec!["admin".to_string()], true);
    user.password_hash = "plaintext-password".to_string();
    config.users.push(user);

    assert!(matches!(
        validate_import(&config, ImportMode::Merge),
        Err(ConfigurationImportError::Validation(message))
            if message.contains("Invalid password hash")
    ));
}

#[test]
fn validation_rejects_usernames_longer_than_login_accepts() {
    let global = GlobalConfigDbModel::default();
    let mut config = import_config(&global);
    config.users.push(imported_user(
        &"a".repeat(MAX_USERNAME_LENGTH + 1),
        vec!["admin".to_string()],
        true,
    ));

    assert!(matches!(
        validate_import(&config, ImportMode::Merge),
        Err(ConfigurationImportError::Validation(message))
            if message.contains("exceeds")
    ));
}

#[test]
fn validation_measures_usernames_in_characters() {
    let global = GlobalConfigDbModel::default();
    let mut config = import_config(&global);
    config.users.push(imported_user(
        &"用".repeat(MAX_USERNAME_LENGTH),
        vec!["admin".to_string()],
        true,
    ));

    assert!(validate_import(&config, ImportMode::Merge).is_ok());
}

#[test]
fn replace_validation_requires_an_active_admin() {
    let global = GlobalConfigDbModel::default();
    let mut config = import_config(&global);
    config
        .users
        .push(imported_user("viewer", vec!["user".to_string()], true));

    assert!(matches!(
        validate_import(&config, ImportMode::Replace),
        Err(ConfigurationImportError::Validation(message))
            if message.contains("at least one active user")
    ));
}

#[tokio::test]
async fn replace_import_retains_configuration_until_its_streamer_is_reaped() {
    use crate::database::repositories::streamer::StreamerRepository;

    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let global: GlobalConfigDbModel = sqlx::query_as("SELECT * FROM global_config LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO template_config(id, name) VALUES ('retained-template', 'retained-template')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state) VALUES ('retiring', 'Retiring', 'https://example.com/retiring', 'platform-huya', 'retained-template', 'NOT_LIVE')")
        .execute(&pool).await.unwrap();
    let preset_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_presets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(preset_count > 0);
    let mut config = import_config(&global);
    config.global_config.default_download_engine = "review-engine".to_string();
    config.engines.push(crate::config::backup::EngineExport {
        name: "review-engine".to_string(),
        engine_type: "MESIO".to_string(),
        config: serde_json::json!({}),
    });
    config
        .users
        .push(imported_user("admin", vec!["admin".to_string()], true));
    validate_import(&config, ImportMode::Replace).unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    snapshot
        .validate_references(&config, ImportMode::Replace)
        .unwrap();
    apply_import(&mut tx, &snapshot, &config, ImportMode::Replace)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let marked: bool =
        sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM streamers WHERE id = 'retiring'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(marked);
    let held: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM template_config WHERE id = 'retained-template'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(held, 1);
    let held_presets: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_presets")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(held_presets, preset_count);
    assert!(
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .delete_marked_streamer("retiring")
            .await
            .unwrap()
    );
    let held: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM template_config WHERE id = 'retained-template'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(held, 0);
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM retirement_config_deletions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    let violations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(violations, 0);
}

#[tokio::test]
async fn replace_import_updates_lowest_id_case_duplicate_and_marks_shadow_deleted() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let global: GlobalConfigDbModel =
        sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let platform = seed_case_duplicate_streamers(&pool).await;

    let mut config = import_config(&global);
    let engines: Vec<EngineConfigurationDbModel> =
        sqlx::query_as("SELECT * FROM engine_configuration")
            .fetch_all(&pool)
            .await
            .unwrap();
    config.engines = engines
        .iter()
        .map(|engine| crate::config::backup::EngineExport {
            name: engine.name.clone(),
            engine_type: engine.engine_type.clone(),
            config: serde_json::from_str(&engine.config).unwrap(),
        })
        .collect();
    config.streamers.push(imported_streamer(
        "https://example.com/live/ALPHA",
        &platform.platform_name,
    ));
    validate_import(&config, ImportMode::Replace).unwrap();

    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    snapshot
        .validate_references(&config, ImportMode::Replace)
        .unwrap();
    let AppliedImport { stats, .. } =
        apply_import(&mut tx, &snapshot, &config, ImportMode::Replace)
            .await
            .unwrap();
    tx.commit().await.unwrap();

    let rows: Vec<StreamerDbModel> = sqlx::query_as("SELECT * FROM streamers ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "streamer-a");
    assert_eq!(rows[0].url, "https://example.com/live/ALPHA");
    assert!(rows[0].deleted_at.is_none());
    assert_eq!(rows[1].id, "streamer-b");
    assert!(rows[1].deleted_at.is_some());
    assert_eq!(stats.streamers_updated, 1);
    assert_eq!(stats.streamers_created, 0);
    assert_eq!(stats.streamers_deleted, 1);
}

#[tokio::test]
async fn merge_import_updates_one_case_duplicate_and_keeps_the_shadow() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let global: GlobalConfigDbModel =
        sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let platform = seed_case_duplicate_streamers(&pool).await;

    let mut config = import_config(&global);
    config.streamers.push(imported_streamer(
        "https://example.com/live/ALPHA",
        &platform.platform_name,
    ));
    validate_import(&config, ImportMode::Merge).unwrap();

    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    snapshot
        .validate_references(&config, ImportMode::Merge)
        .unwrap();
    let AppliedImport { stats, .. } = apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let rows: Vec<StreamerDbModel> = sqlx::query_as("SELECT * FROM streamers ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "streamer-a");
    assert_eq!(rows[0].url, "https://example.com/live/ALPHA");
    assert_eq!(rows[1].id, "streamer-b");
    assert_eq!(rows[1].url, "https://example.com/live/Alpha");
    assert_eq!(rows[1].name, "mixed");
    assert!(rows.iter().all(|row| row.deleted_at.is_none()));
    assert_eq!(stats.streamers_updated, 1);
    assert_eq!(stats.streamers_created, 0);
    assert_eq!(stats.streamers_deleted, 0);
}

#[test]
fn imported_state_carries_disable_intent_but_not_runtime_state() {
    let mut live = StreamerDbModel::new("live", "https://example.com/live", "platform-huya");
    live.state = StreamerState::Live.as_str().to_string();
    assert_eq!(
        imported_state(Some(&live), StreamerState::NotLive.as_str()),
        StreamerState::Live.as_str()
    );
    assert_eq!(
        imported_state(Some(&live), StreamerState::Disabled.as_str()),
        StreamerState::Disabled.as_str()
    );
    assert_eq!(
        imported_state(Some(&live), "CANCELLED"),
        StreamerState::Disabled.as_str()
    );

    let mut disabled =
        StreamerDbModel::new("disabled", "https://example.com/other", "platform-huya");
    disabled.state = StreamerState::Disabled.as_str().to_string();
    assert_eq!(
        imported_state(Some(&disabled), StreamerState::NotLive.as_str()),
        StreamerState::NotLive.as_str()
    );

    assert_eq!(
        imported_state(None, StreamerState::Live.as_str()),
        StreamerState::NotLive.as_str()
    );
    assert_eq!(
        imported_state(None, StreamerState::Disabled.as_str()),
        StreamerState::Disabled.as_str()
    );
}

#[test]
fn clears_error_tracking_follows_the_stop_and_re_enable_writes() {
    let mut not_live =
        StreamerDbModel::new("not live", "https://example.com/live", "platform-huya");
    not_live.state = StreamerState::NotLive.as_str().to_string();
    let mut disabled =
        StreamerDbModel::new("disabled", "https://example.com/other", "platform-huya");
    disabled.state = StreamerState::Disabled.as_str().to_string();

    assert!(clears_error_tracking(
        Some(&not_live),
        StreamerState::Disabled.as_str()
    ));
    assert!(clears_error_tracking(
        Some(&disabled),
        StreamerState::NotLive.as_str()
    ));
    assert!(!clears_error_tracking(
        Some(&not_live),
        StreamerState::NotLive.as_str()
    ));

    let mut errored = StreamerDbModel::new("errored", "https://example.com/third", "platform-huya");
    errored.state = StreamerState::Error.as_str().to_string();
    assert_eq!(
        imported_state(Some(&errored), StreamerState::NotLive.as_str()),
        StreamerState::Error.as_str()
    );
    assert!(!clears_error_tracking(
        Some(&errored),
        StreamerState::Error.as_str()
    ));
}
