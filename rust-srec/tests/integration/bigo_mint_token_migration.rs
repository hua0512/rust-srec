use serde_json::{Value, json};
use sqlx::{SqlitePool, migrate::Migrator, sqlite::SqlitePoolOptions};

const VERSION: i64 = 20261116120000;
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

async fn text(pool: &SqlitePool, sql: &'static str) -> String {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn upgrade_removes_mint_token_wherever_bigo_options_are_stored() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    Migrator::with_migrations(
        MIGRATOR
            .iter()
            .filter(|migration| migration.version < VERSION)
            .cloned()
            .collect::<Vec<_>>(),
    )
    .run(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(r#"
        UPDATE platform_config SET platform_specific_config = '{"stream_password":"pw","mint_token":false}' WHERE id = 'platform-bigo';
        INSERT INTO template_config(id, name, platform_overrides) VALUES
            ('both', 'Both', '{"bigo":{"mint_token":false,"platform_specific_config":{"mint_token":true,"stream_password":"pw"}},"huya":{"mint_token":1}}'),
            ('malformed', 'Malformed', '{"bigo":');
        INSERT INTO retirement_config_deletions(kind, config_id) VALUES ('template', 'both');
        INSERT INTO streamers(id, name, url, platform_config_id, state, streamer_specific_config) VALUES
            ('bigo', 'Bigo', 'https://www.bigo.tv/1', 'platform-bigo', 'NOT_LIVE', '{"platform_extras":{"mint_token":false,"stream_password":"pw"},"record_danmu":true}'),
            ('broken', 'Broken', 'https://www.bigo.tv/2', 'platform-bigo', 'NOT_LIVE', '{"platform_extras":');
    "#)
    .execute(&pool)
    .await
    .unwrap();

    MIGRATOR.run(&pool).await.unwrap();

    let json = |text: String| serde_json::from_str::<Value>(&text).unwrap();
    assert_eq!(
        json(
            text(
                &pool,
                "SELECT platform_specific_config FROM platform_config WHERE id = 'platform-bigo'"
            )
            .await
        ),
        json!({"stream_password": "pw"})
    );
    assert_eq!(
        json(
            text(
                &pool,
                "SELECT platform_overrides FROM template_config WHERE id = 'both'"
            )
            .await
        ),
        json!({"bigo": {"platform_specific_config": {"stream_password": "pw"}}, "huya": {"mint_token": 1}})
    );
    assert_eq!(
        json(
            text(
                &pool,
                "SELECT streamer_specific_config FROM streamers WHERE id = 'bigo'"
            )
            .await
        ),
        json!({"platform_extras": {"stream_password": "pw"}, "record_danmu": true})
    );
    assert_eq!(
        text(
            &pool,
            "SELECT platform_overrides FROM template_config WHERE id = 'malformed'"
        )
        .await,
        r#"{"bigo":"#
    );
    assert_eq!(
        text(
            &pool,
            "SELECT streamer_specific_config FROM streamers WHERE id = 'broken'"
        )
        .await,
        r#"{"platform_extras":"#
    );
    // Rewriting the template must not cancel its pending deletion.
    assert_eq!(
        text(
            &pool,
            "SELECT config_id FROM retirement_config_deletions WHERE kind = 'template'"
        )
        .await,
        "both"
    );
}
