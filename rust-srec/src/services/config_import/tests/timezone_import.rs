use super::*;
use crate::config::backup::FilterExport;
use serde_json::json;

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
