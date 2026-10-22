use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

const VERSION: i64 = 20261022120000;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

async fn subscriptions(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT event_name FROM notification_subscription ORDER BY event_name")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn upgrade_drops_only_subscriptions_to_config_updated() {
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
        "INSERT INTO notification_channel (id, name, channel_type, settings) \
         VALUES ('channel', 'Channel', 'Webhook', '{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    for event in [
        "config_updated",
        "config.updated",
        "ConfigUpdated",
        "stream_online",
        "future_event",
        "config_updated_later",
    ] {
        sqlx::query(
            "INSERT INTO notification_subscription (channel_id, event_name) VALUES ('channel', ?)",
        )
        .bind(event)
        .execute(&pool)
        .await
        .unwrap();
    }

    MIGRATOR.run(&pool).await.unwrap();

    assert_eq!(
        subscriptions(&pool).await,
        ["config_updated_later", "future_event", "stream_online"]
    );
}
