use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

const VERSION: i64 = 20261013120000;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

async fn config(pool: &SqlitePool, id: &str) -> String {
    sqlx::query_scalar("SELECT config FROM engine_configuration WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn upgrade_drops_only_the_inert_default_buffer_size() {
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

    let fixtures = [
        (
            "stored-default",
            "MESIO",
            r#"{"buffer_size":8388608,"fix_flv":false,"hls":{"base":{"connect_timeout_ms":7000}}}"#,
        ),
        (
            "chosen",
            "MESIO",
            r#"{"buffer_size":1048576,"fix_flv":true}"#,
        ),
        ("absent", "MESIO", r#"{"fix_hls":false}"#),
        ("text", "MESIO", r#"{"buffer_size":"8388608"}"#),
        ("malformed", "MESIO", "{malformed"),
        ("array", "MESIO", "[8388608]"),
        ("ffmpeg", "FFMPEG", r#"{"buffer_size":8388608}"#),
    ];
    for (id, engine_type, config) in fixtures {
        sqlx::query(
            "INSERT INTO engine_configuration(id, name, engine_type, config) VALUES(?, ?, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(engine_type)
        .bind(config)
        .execute(&pool)
        .await
        .unwrap();
    }

    MIGRATOR.run(&pool).await.unwrap();

    let parsed = |text: String| serde_json::from_str::<serde_json::Value>(&text).unwrap();
    assert_eq!(
        parsed(config(&pool, "stored-default").await),
        serde_json::json!({"fix_flv": false, "hls": {"base": {"connect_timeout_ms": 7000}}})
    );
    // The seeded default engine stored the same inert value.
    assert_eq!(
        parsed(config(&pool, "default-mesio").await),
        serde_json::json!({"fix_flv": true, "fix_hls": true})
    );
    for (id, _, original) in fixtures.iter().skip(1) {
        assert_eq!(&config(&pool, id).await, original, "{id} changed");
    }

    // Replaying the statement changes nothing more.
    let migration =
        include_str!("../../migrations/20261013120000_drop_inert_mesio_buffer_size.sql");
    sqlx::raw_sql(migration).execute(&pool).await.unwrap();
    assert_eq!(
        parsed(config(&pool, "stored-default").await),
        serde_json::json!({"fix_flv": false, "hls": {"base": {"connect_timeout_ms": 7000}}})
    );
}
