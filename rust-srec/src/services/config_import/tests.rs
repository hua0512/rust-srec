mod credential_profiles;
mod email_validation;
mod import_projection;
mod proxies;

use std::sync::Arc;

use sqlx::SqlitePool;

use super::snapshot::{ImportSnapshot, validate_import};
use super::streamers::{clears_error_tracking, imported_state};
use super::*;
use crate::api::auth_store::{UserDbModel, import_user as persist_user};
use crate::config::ConfigService;
use crate::config::backup::{ConfigExport, GlobalConfigExport, ImportMode, UserExport};
use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, StreamerDbModel,
};
use crate::database::repositories::config::SqlxConfigRepository;
use crate::database::repositories::credential_selections;
use crate::database::repositories::streamer::{
    SqlxStreamerRepository, import_streamer_row as persist_streamer,
};
use crate::database::{begin_immediate, init_pool_with_size, run_migrations};
use crate::domain::StreamerState;

fn test_auth_import() -> Arc<dyn AuthImportTxPort> {
    crate::backend::auth_import_tx_port()
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
    let auth = test_auth_import();
    auth.validate_users_tx(
        &mut tx,
        &config.users,
        includes_imported_users(&config),
        ImportMode::Replace,
    )
    .await
    .unwrap();
    apply_import(
        &mut tx,
        &snapshot,
        &config,
        ImportMode::Replace,
        auth.as_ref(),
    )
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

/// Case-only duplicate URLs survive only in databases from before the
/// case-insensitive index. An import updates the lowest id; a replace then
/// marks the other row while a merge keeps it unchanged.
#[tokio::test]
async fn imports_update_the_lowest_id_case_duplicate_and_only_replace_marks_the_shadow() {
    for mode in [ImportMode::Replace, ImportMode::Merge] {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let global: GlobalConfigDbModel =
            sqlx::query_as("SELECT * FROM global_config ORDER BY rowid LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        let platform = seed_case_duplicate_streamers(&pool).await;

        let mut config = import_config(&global);
        if mode == ImportMode::Replace {
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
        }
        config.streamers.push(imported_streamer(
            "https://example.com/live/ALPHA",
            &platform.platform_name,
        ));
        validate_import(&config, mode).unwrap();

        let mut tx = begin_immediate(&pool).await.unwrap();
        let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
        snapshot.validate_references(&config, mode).unwrap();
        let auth = test_auth_import();
        let AppliedImport { stats, .. } =
            apply_import(&mut tx, &snapshot, &config, mode, auth.as_ref())
                .await
                .unwrap();
        tx.commit().await.unwrap();

        let rows: Vec<StreamerDbModel> = sqlx::query_as("SELECT * FROM streamers ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "{mode:?}");
        assert_eq!(rows[0].id, "streamer-a", "{mode:?}");
        assert_eq!(rows[0].url, "https://example.com/live/ALPHA", "{mode:?}");
        assert!(rows[0].deleted_at.is_none(), "{mode:?}");
        assert_eq!(rows[1].id, "streamer-b", "{mode:?}");
        assert_eq!(stats.streamers_updated, 1, "{mode:?}");
        assert_eq!(stats.streamers_created, 0, "{mode:?}");
        if mode == ImportMode::Replace {
            assert!(rows[1].deleted_at.is_some());
            assert_eq!(stats.streamers_deleted, 1);
        } else {
            assert_eq!(rows[1].url, "https://example.com/live/Alpha");
            assert_eq!(rows[1].name, "mixed");
            assert!(rows[1].deleted_at.is_none());
            assert_eq!(stats.streamers_deleted, 0);
        }
    }
}

/// An import carries a stop or a re-enable but never a runtime state, and
/// only a stop or a re-enable resets the backoff.
#[test]
fn imported_state_carries_disable_intent_and_clears_error_tracking_on_stop_and_re_enable() {
    let streamer = |state: StreamerState| {
        let mut row = StreamerDbModel::new("existing", "https://example.com/live", "platform-huya");
        row.state = state.as_str().to_string();
        row
    };
    let live = streamer(StreamerState::Live);
    let not_live = streamer(StreamerState::NotLive);
    let disabled = streamer(StreamerState::Disabled);
    let errored = streamer(StreamerState::Error);
    // (stored row, bundle state, imported state, clears error tracking)
    let cases: [(Option<&StreamerDbModel>, &str, StreamerState, Option<bool>); 10] = [
        (Some(&live), "NOT_LIVE", StreamerState::Live, None),
        (Some(&live), "DISABLED", StreamerState::Disabled, None),
        (Some(&live), "CANCELLED", StreamerState::Disabled, None),
        (
            Some(&disabled),
            "NOT_LIVE",
            StreamerState::NotLive,
            Some(true),
        ),
        (None, "LIVE", StreamerState::NotLive, None),
        (None, "DISABLED", StreamerState::Disabled, None),
        (
            Some(&not_live),
            "DISABLED",
            StreamerState::Disabled,
            Some(true),
        ),
        (
            Some(&not_live),
            "NOT_LIVE",
            StreamerState::NotLive,
            Some(false),
        ),
        (Some(&errored), "NOT_LIVE", StreamerState::Error, None),
        (Some(&errored), "ERROR", StreamerState::Error, Some(false)),
    ];
    for (stored, bundle, imported, clears) in cases {
        let case = format!("{:?} <- {bundle}", stored.map(|row| &row.state));
        assert_eq!(imported_state(stored, bundle), imported.as_str(), "{case}");
        if let Some(clears) = clears {
            assert_eq!(clears_error_tracking(stored, bundle), clears, "{case}");
        }
    }
}
