use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

const VERSION: i64 = 20260909120000;
const MIGRATION: &str =
    include_str!("../migrations/20260909120000_preserve_local_filter_timezones.sql");
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

async fn rows(pool: &SqlitePool) -> Vec<(String, String, String, String)> {
    sqlx::query_as("SELECT id, streamer_id, filter_type, config FROM filters ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn upgrade_materializes_only_legacy_time_zones_and_is_idempotent() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let previous = Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < VERSION)
            .cloned()
            .collect::<Vec<_>>(),
    );
    previous.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO streamers(id,name,url,platform_config_id,state) VALUES('timezone-owner','Timezone','https://example.test/timezone','platform-huya','NOT_LIVE')").execute(&pool).await.unwrap();
    let fixtures = [
        (
            "missing",
            "TIME_BASED",
            r#"{"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","extension":{"nested":[1,true]}}"#,
        ),
        ("null", "TIME_BASED", r#"{"timezone":null,"extra":42}"#),
        (
            "iana",
            "TIME_BASED",
            r#"{ "timezone": "Europe/Madrid", "extra": 42 }"#,
        ),
        ("utc", "TIME_BASED", r#"{"timezone":"UTC"}"#),
        ("local", "TIME_BASED", r#"{"timezone":"local"}"#),
        (
            "invalid-zone",
            "TIME_BASED",
            r#"{"timezone":"invalid/zone"}"#,
        ),
        ("blank-zone", "TIME_BASED", r#"{"timezone":""}"#),
        ("malformed", "TIME_BASED", "{malformed"),
        ("array", "TIME_BASED", "[1,2]"),
        ("scalar", "TIME_BASED", "42"),
        ("json-null", "TIME_BASED", "null"),
        ("double-encoded", "TIME_BASED", r#""{\"timezone\":null}""#),
        ("cron", "CRON", r#"{"expression":"0 * * * * *"}"#),
        (
            "keyword",
            "KEYWORD",
            r#"{"timezone":null,"include":[],"exclude":[]}"#,
        ),
    ];
    for (id, kind, config) in fixtures {
        sqlx::query(
            "INSERT INTO filters(id,streamer_id,filter_type,config) VALUES(?,'timezone-owner',?,?)",
        )
        .bind(id)
        .bind(kind)
        .bind(config)
        .execute(&pool)
        .await
        .unwrap();
    }
    let before = rows(&pool).await;
    MIGRATOR.run(&pool).await.unwrap();
    let after = rows(&pool).await;
    assert_eq!(before.len(), after.len());
    for (old, new) in before.iter().zip(&after) {
        assert_eq!((&old.0, &old.1, &old.2), (&new.0, &new.1, &new.2));
        if matches!(old.0.as_str(), "missing" | "null") {
            let mut expected: serde_json::Value = serde_json::from_str(&old.3).unwrap();
            expected["timezone"] = serde_json::json!("local");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&new.3).unwrap(),
                expected
            );
        } else {
            assert_eq!(old.3, new.3, "{}", old.0);
        }
    }
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();
    assert_eq!(rows(&pool).await, after);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}
