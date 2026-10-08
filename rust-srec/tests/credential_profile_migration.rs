//! Upgrade a populated pre-profile database: configuration cookies become
//! platform-owned profiles that each scope selects, and nothing else changes.

use std::path::Path;
use std::time::Duration;

use rust_srec::credentials::CredentialOwner;
use rust_srec::database::repositories::{ConfigRepository, SqlxConfigRepository};
use sqlx::SqlitePool;

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

async fn json(pool: &SqlitePool, sql: &str) -> serde_json::Value {
    let raw: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap();
    raw.map_or(serde_json::Value::Null, |raw| {
        serde_json::from_str(&raw).unwrap()
    })
}

/// The selection `owner` stores on `platform_id`, as its wire JSON; null when
/// the scope inherits.
async fn selection(
    pool: &SqlitePool,
    owner: CredentialOwner,
    platform_id: &str,
) -> serde_json::Value {
    SqlxConfigRepository::new(pool.clone(), pool.clone())
        .list_credential_selections()
        .await
        .unwrap()
        .into_iter()
        .find(|stored| stored.owner == owner && stored.platform_id == platform_id)
        .map_or(serde_json::Value::Null, |stored| {
            serde_json::to_value(stored.selection).unwrap()
        })
}

fn platform_owner(id: &str) -> CredentialOwner {
    CredentialOwner::Platform {
        platform_id: id.into(),
    }
}

fn streamer(id: &str) -> CredentialOwner {
    CredentialOwner::Streamer {
        streamer_id: id.into(),
    }
}

/// The profile a stored selection names, as (label, cookies, refresh token, reauth).
async fn selected(
    pool: &SqlitePool,
    selection: &serde_json::Value,
) -> (String, String, Option<String>, Option<String>) {
    assert_eq!(selection["mode"], "fixed", "{selection}");
    sqlx::query_as(
        "SELECT label, cookies, refresh_token, reauth_config FROM credential_profiles WHERE id = ?",
    )
    .bind(selection["credential_id"].as_str().unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn upgrade_converts_configured_cookies_into_selected_platform_profiles() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = database_through("20260926000000").await;
        exec(&pool, "CREATE TABLE legacy_trigger_probe(session_id TEXT NOT NULL)").await;
        exec(&pool, "CREATE TRIGGER legacy_session_probe AFTER UPDATE OF end_time ON live_sessions BEGIN INSERT INTO legacy_trigger_probe(session_id) VALUES (NEW.id); END").await;
        exec(&pool, r#"UPDATE platform_config SET cookies = 'SESSDATA=platform', platform_specific_config = '{"refresh_token":"rt-p","access_token":"at-p","last_cookie_check_result":"valid","quality":"origin"}' WHERE id = 'platform-bilibili'"#).await;
        exec(&pool, r#"UPDATE platform_config SET platform_specific_config = '{"username":"viewer","password":"secret"}' WHERE id = 'platform-soop'"#).await;
        exec(&pool, r#"UPDATE platform_config SET platform_specific_config = '{"oauth_token":"twitch-token"}' WHERE id = 'platform-twitch'"#).await;
        exec(&pool, r#"UPDATE platform_config SET platform_specific_config = '{"device_id":"D1","api_mode":"app"}' WHERE id = 'platform-douyu'"#).await;
        exec(&pool, r#"UPDATE platform_config SET platform_specific_config = '{"password":"room"}' WHERE id = 'platform-bigo'"#).await;
        exec(&pool, r#"UPDATE platform_config SET platform_specific_config = '{"password":"room"}' WHERE id = 'platform-twitcasting'"#).await;
        exec(&pool, "UPDATE platform_config SET cookies = '   ' WHERE id = 'platform-huya'").await;
        exec(&pool, "UPDATE platform_config SET cookies = 'site=platform' WHERE id = 'platform-streamlink'").await;
        exec(&pool, r#"INSERT INTO template_config(id, name, cookies, platform_overrides) VALUES ('shared', 'Shared', 'SESSDATA=template', '{"bilibili":{"refresh_token":"rt-t"},"huya":{"custom":[1,true]},"last_cookie_check_date":"2026-09-01"}')"#).await;
        exec(&pool, "INSERT INTO template_config(id, name, cookies) VALUES ('unused', 'Unused', 'SESSDATA=unused')").await;
        for (id, platform, template, config) in [
            ("s1", "platform-bilibili", "'shared'", r#"{"cookies":"SESSDATA=dup"}"#),
            ("s2", "platform-bilibili", "NULL", r#"{"cookies":"SESSDATA=dup"}"#),
            ("s3", "platform-bilibili", "'shared'", r#"{"cookies":""}"#),
            ("s4", "platform-soop", "NULL", r#"{"cookies":"AuthTicket=x"}"#),
            ("s5", "platform-bilibili", "NULL", r#"{"cookies":"SESSDATA=deleted"}"#),
            ("s6", "platform-bilibili", "NULL", "{\"cookies\":\"bad\\nvalue\"}"),
            ("s7", "platform-bilibili", "NULL", r#"{"cookies":"SESSDATA=kept","credential_selection":{"mode":"none"}}"#),
            ("l1", "platform-streamlink", "NULL", "{}"),
            ("l2", "platform-streamlink", "'shared'", "{}"),
            ("l3", "platform-streamlink", "'shared'", r#"{"cookies":"site=own"}"#),
            ("l4", "platform-streamlink", "'shared'", "{}"),
        ] {
            exec(&pool, &format!("INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state, streamer_specific_config) VALUES ('{id}', '{}', 'https://example.test/{id}', '{platform}', {template}, 'NOT_LIVE', '{config}')", id.to_uppercase())).await;
        }
        exec(&pool, "UPDATE streamers SET deleted_at = 1 WHERE id = 's5'").await;
        exec(&pool, "INSERT INTO live_sessions(id, streamer_id, streamer_name, start_time, total_size_bytes) VALUES ('legacy-live', 's1', 'S1', 1234567890000, 1234)").await;
        let schema_before: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT type, name, sql FROM sqlite_schema WHERE type IN ('index', 'trigger') AND tbl_name IN ('platform_config','template_config','streamers','live_sessions') ORDER BY type, name").fetch_all(&pool).await.unwrap();

        rust_srec::database::run_migrations(&pool).await.unwrap();

        // Platform cookies, tokens and content settings.
        let platform = selection(&pool, platform_owner("platform-bilibili"), "platform-bilibili").await;
        let (label, cookies, refresh, _) = selected(&pool, &platform).await;
        assert_eq!((label.as_str(), cookies.as_str(), refresh.as_deref()), ("bilibili (migrated)", "SESSDATA=platform", Some("rt-p")));
        assert_eq!(json(&pool, "SELECT platform_specific_config FROM platform_config WHERE id = 'platform-bilibili'").await, serde_json::json!({"quality":"origin"}));
        // Login-only SOOP, Twitch OAuth, and Douyu's app-mode device cookie.
        let soop = selection(&pool, platform_owner("platform-soop"), "platform-soop").await;
        let (_, cookies, _, reauth) = selected(&pool, &soop).await;
        assert_eq!(cookies, "");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&reauth.unwrap()).unwrap(), serde_json::json!({"username":"viewer","password":"secret"}));
        let twitch = selection(&pool, platform_owner("platform-twitch"), "platform-twitch").await;
        let token: Option<String> = sqlx::query_scalar("SELECT access_token FROM credential_profiles WHERE id = ?").bind(twitch["credential_id"].as_str().unwrap()).fetch_one(&pool).await.unwrap();
        assert_eq!(token.as_deref(), Some("twitch-token"));
        let douyu = selection(&pool, platform_owner("platform-douyu"), "platform-douyu").await;
        assert_eq!(selected(&pool, &douyu).await.1, "acf_did=D1");
        assert_eq!(json(&pool, "SELECT platform_specific_config FROM platform_config WHERE id = 'platform-douyu'").await, serde_json::json!({"api_mode":"app"}));
        // Room passwords are content settings, not accounts.
        assert_eq!(json(&pool, "SELECT platform_specific_config FROM platform_config WHERE id = 'platform-bigo'").await, serde_json::json!({"stream_password":"room"}));
        assert_eq!(json(&pool, "SELECT platform_specific_config FROM platform_config WHERE id = 'platform-twitcasting'").await, serde_json::json!({"password":"room"}));
        for blank in ["platform-huya", "platform-bigo", "platform-twitcasting"] {
            assert!(selection(&pool, platform_owner(blank), blank).await.is_null(), "{blank}");
        }

        // A template's top-level cookie converts only where streamers use it.
        let overrides = json(&pool, "SELECT platform_overrides FROM template_config WHERE id = 'shared'").await;
        let shared = CredentialOwner::Template { template_id: "shared".into() };
        let (label, cookies, refresh, _) = selected(&pool, &selection(&pool, shared, "platform-bilibili").await).await;
        assert_eq!(overrides["bilibili"], serde_json::json!({}));
        assert_eq!((label.as_str(), cookies.as_str(), refresh.as_deref()), ("Shared (migrated)", "SESSDATA=template", Some("rt-t")));
        assert_eq!(overrides["huya"], serde_json::json!({"custom":[1,true]}));
        assert!(overrides.get("last_cookie_check_date").is_none());
        // The old cookie columns and the values kept for the conversion are gone.
        let leftovers: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM pragma_table_info('platform_config') WHERE name = 'cookies')
                  + (SELECT COUNT(*) FROM pragma_table_info('template_config') WHERE name = 'cookies')
                  + (SELECT COUNT(*) FROM sqlite_schema WHERE name = 'legacy_cookies')",
        ).fetch_one(&pool).await.unwrap();
        assert_eq!(leftovers, 0);

        // Identical bundles share one profile; blank cookies inherit; a SOOP
        // streamer keeps the platform login it used.
        let s1 = json(&pool, "SELECT streamer_specific_config FROM streamers WHERE id = 's1'").await;
        let s2 = json(&pool, "SELECT streamer_specific_config FROM streamers WHERE id = 's2'").await;
        assert_eq!(s1, serde_json::json!({}));
        assert_eq!(s2, serde_json::json!({}));
        let s1 = selection(&pool, streamer("s1"), "platform-bilibili").await;
        assert_eq!(s1, selection(&pool, streamer("s2"), "platform-bilibili").await);
        assert_eq!(selected(&pool, &s1).await.0, "S1 (migrated)");
        assert_eq!(json(&pool, "SELECT streamer_specific_config FROM streamers WHERE id = 's3'").await, serde_json::json!({}));
        let s4 = selection(&pool, streamer("s4"), "platform-soop").await;
        let (_, cookies, _, reauth) = selected(&pool, &s4).await;
        assert_eq!(cookies, "AuthTicket=x");
        assert!(reauth.is_some());
        // Deleted streamers and invalid material are cleared without a profile;
        // an existing selection is kept.
        for cleared in ["s5", "s6"] {
            assert_eq!(json(&pool, &format!("SELECT streamer_specific_config FROM streamers WHERE id = '{cleared}'")).await, serde_json::json!({}), "{cleared}");
        }
        assert_eq!(json(&pool, "SELECT streamer_specific_config FROM streamers WHERE id = 's7'").await, serde_json::json!({}));
        assert_eq!(selection(&pool, streamer("s7"), "platform-bilibili").await, serde_json::json!({"mode":"none"}));
        for cleared in ["s3", "s5", "s6"] {
            assert!(selection(&pool, streamer(cleared), "platform-bilibili").await.is_null(), "{cleared}");
        }

        // Streamlink accounts are chosen per streamer: each streamer without
        // its own cookie selects the platform or template account it used.
        assert!(selection(&pool, platform_owner("platform-streamlink"), "platform-streamlink").await.is_null());
        let shared = CredentialOwner::Template { template_id: "shared".into() };
        assert!(selection(&pool, shared, "platform-streamlink").await.is_null());
        assert!(overrides.get("streamlink").is_none());
        let l1 = selection(&pool, streamer("l1"), "platform-streamlink").await;
        assert_eq!(selected(&pool, &l1).await.0, "streamlink (migrated)");
        assert_eq!(selected(&pool, &l1).await.1, "site=platform");
        let l2 = selection(&pool, streamer("l2"), "platform-streamlink").await;
        assert_eq!(l2, selection(&pool, streamer("l4"), "platform-streamlink").await);
        let (label, cookies, _, _) = selected(&pool, &l2).await;
        assert_eq!((label.as_str(), cookies.as_str()), ("Shared (migrated)", "SESSDATA=template"));
        let l3 = selection(&pool, streamer("l3"), "platform-streamlink").await;
        assert_eq!(selected(&pool, &l3).await.1, "site=own");

        let selections: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_selections").fetch_one(&pool).await.unwrap();
        // bilibili, SOOP, Twitch and Douyu platforms; the template; S1, S2, S4,
        // S7 and the four Streamlink streamers.
        assert_eq!(selections, 13);

        // bilibili platform, SOOP, Twitch, Douyu, template, shared S1/S2, S4,
        // and the Streamlink platform, template and L3 accounts.
        let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles").fetch_one(&pool).await.unwrap();
        assert_eq!(profiles, 10);
        let owners: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT platform_config_id) FROM credential_profiles").fetch_one(&pool).await.unwrap();
        assert_eq!(owners, 5);

        // The conversion runs once; replaying migrations changes nothing.
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let replayed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles").fetch_one(&pool).await.unwrap();
        assert_eq!(replayed, 10);
        let marker: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'legacy_credential_upgrade_pending')").fetch_one(&pool).await.unwrap();
        assert!(!marker);

        let schema_after: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT type, name, sql FROM sqlite_schema WHERE type IN ('index', 'trigger') AND tbl_name IN ('platform_config','template_config','streamers','live_sessions') ORDER BY type, name").fetch_all(&pool).await.unwrap();
        for entry in schema_before {
            assert!(schema_after.contains(&entry), "lost schema object {}", entry.1);
        }
        let binding: Option<String> = sqlx::query_scalar("SELECT credential_binding FROM live_sessions WHERE id = 'legacy-live'").fetch_one(&pool).await.unwrap();
        assert!(binding.is_none());
        exec(&pool, "UPDATE live_sessions SET end_time = 1234567891000 WHERE id = 'legacy-live'").await;
        let fired: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM legacy_trigger_probe WHERE session_id = 'legacy-live'").fetch_one(&pool).await.unwrap();
        assert_eq!(fired, 1);
        assert!(sqlx::query("PRAGMA foreign_key_check").fetch_all(&pool).await.unwrap().is_empty());
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check").fetch_one(&pool).await.unwrap();
        assert_eq!(integrity, "ok");
    })
    .await
    .expect("profile upgrade fixture must finish");
}

#[tokio::test]
async fn a_fresh_database_finishes_the_upgrade_without_profiles() {
    let pool = rust_srec::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    rust_srec::database::run_migrations(&pool).await.unwrap();
    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credential_profiles")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(profiles, 0);
    let marker: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'legacy_credential_upgrade_pending')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!marker);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
}

/// Later migrations must carry selections through: rebuilding a table they
/// reference with foreign keys enabled would delete or refuse them.
#[tokio::test]
async fn later_migrations_keep_account_selections() {
    let pool = database_through("20261003120000").await;
    exec(&pool, "INSERT INTO credential_profiles(id, platform_config_id, label, cookies, created_at, updated_at) VALUES ('account', 'platform-bilibili', 'A', 'SESSDATA=a', 0, 0)").await;
    exec(
        &pool,
        "INSERT INTO template_config(id, name) VALUES ('template', 'Template')",
    )
    .await;
    exec(&pool, "INSERT INTO streamers(id, name, url, platform_config_id, template_config_id, state) VALUES ('streamer', 'S', 'https://example.test/s', 'platform-bilibili', 'template', 'NOT_LIVE')").await;
    exec(&pool, "INSERT INTO credential_selections(id, platform_config_id, template_config_id, streamer_id, mode) VALUES (1, 'platform-bilibili', NULL, NULL, 'fixed'), (2, 'platform-bilibili', 'template', NULL, 'fixed'), (3, 'platform-bilibili', NULL, 'streamer', 'fixed')").await;
    exec(&pool, "INSERT INTO credential_selection_members(selection_id, platform_config_id, position, profile_id) VALUES (1, 'platform-bilibili', 0, 'account'), (2, 'platform-bilibili', 0, 'account'), (3, 'platform-bilibili', 0, 'account')").await;

    rust_srec::database::run_migrations(&pool).await.unwrap();

    for owner in [
        platform_owner("platform-bilibili"),
        CredentialOwner::Template {
            template_id: "template".into(),
        },
        streamer("streamer"),
    ] {
        let selection = selection(&pool, owner, "platform-bilibili").await;
        assert_eq!(selected(&pool, &selection).await.0, "A");
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn upgrade_keeps_templates_awaiting_deletion_pending() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let pool = database_through("20260926000000").await;
        exec(&pool, r#"INSERT INTO template_config(id, name, cookies, platform_overrides) VALUES ('retiring', 'Retiring', 'SESSDATA=old', '{"bilibili":{"refresh_token":"rt"}}'), ('live', 'Live', 'SESSDATA=live', NULL)"#).await;
        exec(&pool, "INSERT INTO retirement_config_deletions(kind, config_id) VALUES ('template', 'retiring')").await;
        rust_srec::database::run_migrations(&pool).await.unwrap();
        let pending: Vec<String> = sqlx::query_scalar(
            "SELECT config_id FROM retirement_config_deletions WHERE kind = 'template' ORDER BY config_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(pending, ["retiring"]);
        // The conversion still cleared the retiring template's material.
        let overrides = json(&pool, "SELECT platform_overrides FROM template_config WHERE id = 'retiring'").await;
        assert!(overrides["bilibili"].get("refresh_token").is_none(), "{overrides}");
    })
    .await
    .unwrap();
}
