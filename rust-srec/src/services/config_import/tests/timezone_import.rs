use super::*;
use crate::config::backup::{FilterExport, import_filter_config};
use serde_json::json;

#[tokio::test]
async fn validation_and_persistence_share_versioned_filter_timezone_normalization() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let mut config = import_config(&snapshot.global);
    config
        .streamers
        .push(imported_streamer("https://example.test/timezone", "Huya"));
    // Resolve the platform name from migrated data rather than depending on its label.
    config.streamers[0].platform = snapshot
        .platforms
        .values()
        .next()
        .unwrap()
        .platform_name
        .clone();
    let body = json!({"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","extension":{"kept":[1,true]}});
    for version in ["0.1.7", "0.1.8"] {
        config.version = version.into();
        config.streamers[0].filters = vec![FilterExport {
            filter_type: "TIME_BASED".into(),
            config: json!(body.to_string()),
        }];
        validate_import(&config, ImportMode::Merge).unwrap();
        apply_import(&mut tx, &snapshot, &config, ImportMode::Merge)
            .await
            .unwrap();
        let stored: String = sqlx::query_scalar("SELECT config FROM filters")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
            import_filter_config(version, "TIME_BASED", body.clone())
        );
        // Roll back each version fixture so the original snapshot remains current.
        tx.rollback().await.unwrap();
        tx = begin_immediate(&pool).await.unwrap();
    }
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn imported_invalid_explicit_timezone_is_rejected_without_relaxing_runtime_filter_parsing() {
    let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
    run_migrations(&pool).await.unwrap();
    let mut tx = begin_immediate(&pool).await.unwrap();
    let snapshot = ImportSnapshot::load(&mut tx).await.unwrap();
    let mut config = import_config(&snapshot.global);
    let mut streamer = imported_streamer(
        "https://example.test/timezone",
        &snapshot.platforms.values().next().unwrap().platform_name,
    );
    for kind in ["TIME_BASED", "CRON"] {
        let body = if kind == "TIME_BASED" {
            json!({"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","timezone":"invalid/zone"})
        } else {
            json!({"expression":"0 * * * * *","timezone":"invalid/zone"})
        };
        streamer.filters = vec![FilterExport {
            filter_type: kind.into(),
            config: body,
        }];
        config.streamers = vec![streamer.clone()];
        assert!(
            validate_import(&config, ImportMode::Merge)
                .unwrap_err()
                .to_string()
                .contains("timezone")
        );
    }
    tx.rollback().await.unwrap();
}
