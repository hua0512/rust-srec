//! Upgrade a populated legacy database without touching a developer database.

use std::path::Path;
use std::time::Duration;

type LegacyRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

#[tokio::test]
async fn additive_profile_upgrade_preserves_legacy_rows_indexes_triggers_and_replays() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let directory = tempfile::tempdir().unwrap();
        let legacy_migrations = directory.path().join("legacy-migrations");
        std::fs::create_dir(&legacy_migrations).unwrap();
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".sql") && name.as_str() < "20260930120000" {
                std::fs::copy(entry.path(), legacy_migrations.join(name)).unwrap();
            }
        }
        let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        sqlx::migrate::Migrator::new(legacy_migrations.as_path()).await.unwrap().run(&pool).await.unwrap();
        sqlx::query("CREATE TABLE legacy_trigger_probe(session_id TEXT NOT NULL)").execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER legacy_session_probe AFTER UPDATE OF end_time ON live_sessions BEGIN INSERT INTO legacy_trigger_probe(session_id) VALUES (NEW.id); END").execute(&pool).await.unwrap();
        sqlx::query("UPDATE platform_config SET cookies = '   ', platform_specific_config = '{\"username\":\"legacy-user\",\"password\":\"legacy-password\",\"session_cookies\":\"session=old\",\"last_cookie_check_result\":\"valid\",\"last_cookie_check_date\":\"2026-09-01\"}' WHERE id = 'platform-soop'").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO template_config(id, name, cookies, platform_overrides) VALUES ('legacy-template', 'Legacy', '', '{\"Bilibili\":{\"refresh_token\":\"legacy-refresh\",\"custom\":[1,true]}}')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state, streamer_specific_config) VALUES ('legacy-streamer', 'Legacy', 'https://example.test/legacy-upgrade', 'platform-soop', 'legacy-template', 'NOT_LIVE', '{\"cookies\":null,\"refresh_token\":\"local-token\"}')").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO live_sessions(id, streamer_id, streamer_name, start_time, total_size_bytes) VALUES ('legacy-live', 'legacy-streamer', 'Legacy', 1234567890000, 1234)").execute(&pool).await.unwrap();
        let before: LegacyRow = sqlx::query_as("SELECT p.cookies, p.platform_specific_config, t.cookies, t.platform_overrides, s.streamer_specific_config, l.start_time, l.total_size_bytes FROM platform_config p, template_config t, streamers s, live_sessions l WHERE p.id = 'platform-soop' AND t.id = 'legacy-template' AND s.id = 'legacy-streamer' AND l.id = 'legacy-live'").fetch_one(&pool).await.unwrap();
        let schema_before: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT type, name, sql FROM sqlite_schema WHERE type IN ('index', 'trigger') AND tbl_name IN ('platform_config','template_config','streamers','live_sessions') ORDER BY type, name").fetch_all(&pool).await.unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let after: LegacyRow = sqlx::query_as("SELECT p.cookies, p.platform_specific_config, t.cookies, t.platform_overrides, s.streamer_specific_config, l.start_time, l.total_size_bytes FROM platform_config p, template_config t, streamers s, live_sessions l WHERE p.id = 'platform-soop' AND t.id = 'legacy-template' AND s.id = 'legacy-streamer' AND l.id = 'legacy-live'").fetch_one(&pool).await.unwrap();
        assert_eq!(before, after);
        let schema_after: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT type, name, sql FROM sqlite_schema WHERE type IN ('index', 'trigger') AND tbl_name IN ('platform_config','template_config','streamers','live_sessions') ORDER BY type, name").fetch_all(&pool).await.unwrap();
        for entry in schema_before { assert!(schema_after.contains(&entry), "lost legacy schema object {}", entry.1); }
        let policy: Option<String> = sqlx::query_scalar("SELECT credential_selection FROM platform_config WHERE id = 'platform-soop'").fetch_one(&pool).await.unwrap();
        assert!(policy.is_none());
        let binding: Option<String> = sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = 'legacy-live'").fetch_one(&pool).await.unwrap();
        assert!(binding.is_none());
        let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles").fetch_one(&pool).await.unwrap();
        assert_eq!(profiles, 0, "upgrade must not convert legacy accounts implicitly");
        sqlx::query("UPDATE live_sessions SET end_time = 1234567891000 WHERE id = 'legacy-live'").execute(&pool).await.unwrap();
        let fired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM legacy_trigger_probe WHERE session_id = 'legacy-live'").fetch_one(&pool).await.unwrap();
        assert_eq!(fired, 1);
        assert!(sqlx::query("PRAGMA foreign_key_check").fetch_all(&pool).await.unwrap().is_empty());
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check").fetch_one(&pool).await.unwrap();
        assert_eq!(integrity, "ok");
    }).await.expect("legacy profile migration fixture must finish");
}
