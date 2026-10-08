//! Upgrade a database written before saved proxies: each scope's
//! `proxy_config` JSON becomes a route, distinct exits become named entries,
//! and the old settings are cleared.

use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;

/// The last migration released before saved proxies and account profiles.
const LAST_RELEASED: &str = "20260926000000";

/// A database migrated through the migration versioned `last`.
async fn database_through(last: &str) -> SqlitePool {
    let directory = tempfile::tempdir().unwrap();
    let legacy_migrations = directory.path().join("legacy-migrations");
    std::fs::create_dir(&legacy_migrations).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".sql")
            && name
                .split('_')
                .next()
                .is_some_and(|version| version <= last)
        {
            std::fs::copy(entry.path(), legacy_migrations.join(name)).unwrap();
        }
    }
    let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(legacy_migrations.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    pool
}

async fn exec(pool: &SqlitePool, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(pool)
        .await
        .unwrap();
}

/// A scope's stored route as `kind` or `proxy:<entry name>`.
async fn route(pool: &SqlitePool, table: &str, id: &str) -> String {
    let (kind, name): (String, Option<String>) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT s.proxy_route, p.name FROM {table} s LEFT JOIN proxies p ON p.id = s.proxy_id WHERE s.id = ?"
    )))
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    match name {
        Some(name) => format!("{kind}:{name}"),
        None => kind,
    }
}

async fn global_route(pool: &SqlitePool) -> String {
    route(pool, "global_config", "global-configuration").await
}

/// Seeds every shape the conversion meets.
async fn seed(pool: &SqlitePool) {
    exec(
        pool,
        r#"UPDATE global_config SET proxy_config = '{"enabled":false,"url":null}'"#,
    )
    .await;
    // A bare host:port with a login in the fields, which clients read as http.
    exec(pool, r#"UPDATE platform_config SET proxy_config = '{"enabled":true,"url":"Proxy.Example:3128","username":"user","password":"secret"}' WHERE id = 'platform-bilibili'"#).await;
    // The same exit written another way shares the entry.
    exec(pool, r#"UPDATE platform_config SET proxy_config = '{"enabled":true,"url":"http://proxy.example:3128/","username":"user","password":"secret"}' WHERE id = 'platform-huya'"#).await;
    // A login embedded in the URL, percent-encoded.
    exec(pool, r#"UPDATE platform_config SET proxy_config = '{"enabled":true,"url":"socks5h://em%40bed:pa%3Ass@login.example:1080"}' WHERE id = 'platform-douyu'"#).await;
    // No client can use socks4: the platform connects directly.
    exec(pool, r#"UPDATE platform_config SET proxy_config = '{"enabled":true,"url":"socks4://old.example:1080"}' WHERE id = 'platform-twitch'"#).await;
    exec(
        pool,
        r#"UPDATE platform_config SET proxy_config = 'not json' WHERE id = 'platform-soop'"#,
    )
    .await;
    exec(pool, r#"INSERT INTO template_config(id, name, proxy_config, platform_overrides) VALUES ('system', 'System', '{"enabled":true,"url":"","use_system_proxy":true}', '{"bilibili":{"proxy_config":{"enabled":true,"url":"http://never-read.example:1"},"quality":"origin"}}')"#).await;
    exec(pool, r#"INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state, streamer_specific_config) VALUES ('direct', 'Direct', 'https://live.bilibili.com/1', 'platform-bilibili', 'system', 'NOT_LIVE', '{"proxy_config":{"enabled":false},"quality":"origin"}')"#).await;
    exec(pool, r#"INSERT INTO streamers(id, name, url, platform_config_id, state, streamer_specific_config, deleted_at) VALUES ('leaving', 'Leaving', 'https://live.bilibili.com/2', 'platform-bilibili', 'NOT_LIVE', '{"proxy_config":{"enabled":true,"url":"http://leaving.example:1"}}', 1)"#).await;
}

async fn assert_consistent(pool: &SqlitePool) {
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(pool)
            .await
            .unwrap()
            .is_empty()
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    let marker: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'legacy_proxy_upgrade_pending')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(!marker);
}

#[tokio::test]
async fn upgrade_turns_proxy_settings_into_named_proxies_and_routes() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = database_through(LAST_RELEASED).await;
        seed(&pool).await;
        rust_srec::database::run_migrations_with_environment_proxy(&pool, false)
            .await
            .unwrap();
        assert_consistent(&pool).await;

        let entries: Vec<(String, String, Option<String>, Option<String>)> =
            sqlx::query_as("SELECT name, url, username, password FROM proxies ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            entries,
            [
                (
                    "login.example:1080".to_owned(),
                    "socks5h://login.example:1080".to_owned(),
                    Some("em@bed".to_owned()),
                    Some("pa:ss".to_owned())
                ),
                (
                    "proxy.example:3128".to_owned(),
                    "http://proxy.example:3128".to_owned(),
                    Some("user".to_owned()),
                    Some("secret".to_owned())
                ),
            ]
        );
        // A disabled global setting without environment proxies is direct.
        assert_eq!(global_route(&pool).await, "direct");
        for (platform, expected) in [
            ("platform-bilibili", "proxy:proxy.example:3128"),
            ("platform-huya", "proxy:proxy.example:3128"),
            ("platform-douyu", "proxy:login.example:1080"),
            ("platform-twitch", "direct"),
            ("platform-soop", "inherit"),
            ("platform-acfun", "inherit"),
        ] {
            assert_eq!(route(&pool, "platform_config", platform).await, expected, "{platform}");
        }
        assert_eq!(route(&pool, "template_config", "system").await, "system");
        assert_eq!(route(&pool, "streamers", "direct").await, "direct");
        assert_eq!(route(&pool, "streamers", "leaving").await, "inherit");

        // Nothing reads the old settings, so nothing keeps them.
        let leftovers: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM pragma_table_info('global_config') WHERE name = 'proxy_config')
                  + (SELECT COUNT(*) FROM pragma_table_info('platform_config') WHERE name = 'proxy_config')
                  + (SELECT COUNT(*) FROM pragma_table_info('template_config') WHERE name = 'proxy_config')
                  + (SELECT COUNT(*) FROM sqlite_schema WHERE name = 'legacy_proxy_settings')
                  + (SELECT COUNT(*) FROM streamers WHERE streamer_specific_config LIKE '%proxy_config%')
                  + (SELECT COUNT(*) FROM template_config WHERE platform_overrides LIKE '%proxy_config%')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leftovers, 0);
        let document: String =
            sqlx::query_scalar("SELECT streamer_specific_config FROM streamers WHERE id = 'direct'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(document, r#"{"quality":"origin"}"#);
        let overrides: String =
            sqlx::query_scalar("SELECT platform_overrides FROM template_config WHERE id = 'system'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(overrides, r#"{"bilibili":{"quality":"origin"}}"#);

        // Starting again finds nothing left to convert.
        rust_srec::database::run_migrations_with_environment_proxy(&pool, true)
            .await
            .unwrap();
        assert_eq!(global_route(&pool).await, "direct");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_global_setting_without_a_proxy_keeps_the_environment_proxy() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for (global, environment, expected) in [
            (r#"{"enabled":false,"url":null}"#, true, "system"),
            ("", true, "system"),
            ("not json", true, "system"),
            (
                r#"{"enabled":true,"url":"","use_system_proxy":false}"#,
                true,
                "system",
            ),
            ("", false, "direct"),
            (
                r#"{"enabled":true,"url":"http://global.example:8080"}"#,
                true,
                "proxy:global.example:8080",
            ),
            (
                r#"{"enabled":true,"url":"socks4://global.example:1080"}"#,
                true,
                "direct",
            ),
            (
                r#"{"enabled":true,"use_system_proxy":true}"#,
                false,
                "system",
            ),
        ] {
            let pool = database_through(LAST_RELEASED).await;
            sqlx::query("UPDATE global_config SET proxy_config = ?")
                .bind(global)
                .execute(&pool)
                .await
                .unwrap();
            rust_srec::database::run_migrations_with_environment_proxy(&pool, environment)
                .await
                .unwrap();
            assert_eq!(
                global_route(&pool).await,
                expected,
                "{global} {environment}"
            );
            assert_consistent(&pool).await;
        }
    })
    .await
    .unwrap();
}

/// A database converted before the old columns were dropped has no values
/// left to keep, so startup removes the tables that would hold them.
#[tokio::test]
async fn dropping_the_old_columns_after_the_conversions_leaves_no_held_values() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = database_through("20261007120000").await;
        exec(&pool, "UPDATE global_config SET proxy_config = ''").await;
        exec(&pool, "DROP TABLE legacy_credential_upgrade_pending").await;
        exec(&pool, "DROP TABLE legacy_proxy_upgrade_pending").await;
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let held: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('legacy_cookies', 'legacy_proxy_settings')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(held, 0);
        assert_eq!(global_route(&pool).await, "direct");
    })
    .await
    .unwrap();
}
