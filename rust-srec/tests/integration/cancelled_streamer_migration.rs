use sqlx::{migrate::Migrator, sqlite::SqlitePoolOptions};

const VERSION: i64 = 20261108120000;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[tokio::test]
async fn upgrade_disables_cancelled_streamers_with_a_clean_error_history() {
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

    sqlx::query(
        "INSERT INTO streamers (id, name, url, platform_config_id, state, \
             consecutive_error_count, last_error, disabled_until) VALUES \
         ('cancelled', 'Cancelled', 'https://www.huya.com/1', 'platform-huya', 'CANCELLED', \
             3, 'timeout', 4102444800000), \
         ('errored', 'Errored', 'https://www.huya.com/2', 'platform-huya', 'ERROR', \
             2, 'banned', 4102444800000)",
    )
    .execute(&pool)
    .await
    .unwrap();

    MIGRATOR.run(&pool).await.unwrap();

    type Row = (String, String, i64, Option<String>, Option<i64>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, state, consecutive_error_count, last_error, disabled_until \
         FROM streamers ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            ("cancelled".into(), "DISABLED".into(), 0, None, None),
            (
                "errored".into(),
                "ERROR".into(),
                2,
                Some("banned".into()),
                Some(4102444800000)
            ),
        ]
    );
}
