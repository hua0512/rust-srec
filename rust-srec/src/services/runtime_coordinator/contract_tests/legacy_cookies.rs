use super::*;

#[tokio::test]
async fn download_and_danmu_keep_legacy_merged_cookie_inputs_including_blanks() {
    tokio::time::timeout(Duration::from_secs(45), async {
        // Absent and explicit JSON null are distinct streamer documents; both
        // become SQL NULL at the platform/template scalar boundary.
        let cases = [None, Some(serde_json::Value::Null), Some(serde_json::json!("")), Some(serde_json::json!(" \t ")), Some(serde_json::json!("selected=1"))];
        for layer in ["platform", "template", "streamer"] {
            for value in &cases {
                let fixture = Fixture::new().await;
                let scalar = value.as_ref().and_then(serde_json::Value::as_str);
                let platform_cookie = if layer == "platform" { scalar } else { Some("platform=1") };
                sqlx::query("UPDATE platform_config SET cookies = ? WHERE id = 'platform-twitch'")
                    .bind(platform_cookie).execute(&fixture.pool).await.unwrap();
                if layer != "platform" {
                    let template_cookie = if layer == "template" { scalar } else { Some("template=1") };
                    sqlx::query("INSERT INTO template_config(id, name, cookies) VALUES ('legacy-startup-template', 'Legacy startup', ?)")
                        .bind(template_cookie).execute(&fixture.pool).await.unwrap();
                    sqlx::query("UPDATE streamers SET template_config_id = 'legacy-startup-template' WHERE id = ?")
                        .bind(STREAMER).execute(&fixture.pool).await.unwrap();
                }
                if layer == "streamer" {
                    let document = value.as_ref().map(|cookie| serde_json::json!({"cookies":cookie}).to_string());
                    sqlx::query("UPDATE streamers SET streamer_specific_config = ? WHERE id = ?")
                        .bind(document).bind(STREAMER).execute(&fixture.pool).await.unwrap();
                }
                let session = fixture.live().await;
                let pipeline = spawned_pipeline(&fixture, &session, false);
                pipeline.await.unwrap();
                fixture.engine.started.notified().await;
                let expected = scalar.or(match layer { "platform" => None, "template" => Some("platform=1"), _ => Some("template=1") });
                assert_eq!(fixture.engine.handles.lock()[0].config.read().cookies.as_deref(), expected, "download {layer}/{value:?}");
                assert_eq!(fixture.danmu_configs.lock()[0].cookies.as_deref(), expected, "danmu {layer}/{value:?}");
                if scalar.is_some_and(|cookies| cookies.trim().is_empty()) && layer != "platform" {
                    let context = fixture.coordinator.config_service.get_context_for_streamer(STREAMER).await.unwrap();
                    assert!(!context.credential_source.as_ref().unwrap().cookies.trim().is_empty(), "legacy refresh source intentionally differs from startup cookies");
                }
                fixture.close().await;
            }
        }
    }).await.expect("legacy startup consumer fixtures must finish without network");
}
