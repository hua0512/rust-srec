use std::time::Duration;

use rust_srec::credentials::{
    CredentialScope, CredentialSource, CredentialStore, RefreshedCredentials,
};
use rust_srec::database::{self, models::*, repositories::*};
use serde_json::json;
use sqlx::SqlitePool;
use tempfile::TempDir;

async fn pools() -> (TempDir, SqlitePool, SqlitePool) {
    let dir = TempDir::new().unwrap();
    let url = format!(
        "sqlite:{}?mode=rwc",
        dir.path()
            .join("atomicity.db")
            .to_string_lossy()
            .replace('\\', "/")
    );
    let read = database::init_pool_with_size(&url, 1).await.unwrap();
    database::run_migrations(&read).await.unwrap();
    let write = database::init_write_pool(&url).await.unwrap();
    (dir, read, write)
}

async fn streamer(read: &SqlitePool, write: &SqlitePool) -> StreamerDbModel {
    let row = StreamerDbModel::new("test", "https://example.com/atomic", "platform-twitch");
    SqlxStreamerRepository::new(read.clone(), write.clone())
        .create_streamer(&row)
        .await
        .unwrap();
    row
}

#[tokio::test]
async fn competing_output_deletes_adjust_size_only_for_the_deleted_row() {
    let (_dir, read, write) = pools().await;
    let streamer = streamer(&read, &write).await;
    let repo = SqlxSessionRepository::new(read.clone(), write.clone());
    let session = LiveSessionDbModel::new(&streamer.id);
    repo.create_session(&session).await.unwrap();
    let output = MediaOutputDbModel::new(&session.id, "video.flv", MediaFileType::Video, 2048);
    repo.create_media_output(&output).await.unwrap();

    // A mutation must not need a second pool to identify the row it removed.
    let held_read = read.acquire().await.unwrap();
    let (first, second) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(
            repo.delete_media_output(&output.id),
            repo.delete_media_output(&output.id)
        )
    })
    .await
    .unwrap();
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let error = first.err().or_else(|| second.err()).unwrap();
    assert!(matches!(error, rust_srec::Error::NotFound { .. }));
    drop(held_read);
    assert_eq!(
        repo.get_session(&session.id)
            .await
            .unwrap()
            .total_size_bytes,
        0
    );
}

#[tokio::test]
async fn output_delete_rolls_back_when_session_size_update_fails() {
    let (_dir, read, write) = pools().await;
    let streamer = streamer(&read, &write).await;
    let repo = SqlxSessionRepository::new(read, write.clone());
    let session = LiveSessionDbModel::new(&streamer.id);
    repo.create_session(&session).await.unwrap();
    let output = MediaOutputDbModel::new(&session.id, "video.flv", MediaFileType::Video, 2048);
    repo.create_media_output(&output).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_size_change BEFORE UPDATE OF total_size_bytes ON live_sessions BEGIN SELECT RAISE(ABORT, 'reject size update'); END")
        .execute(&write).await.unwrap();
    assert!(repo.delete_media_output(&output.id).await.is_err());
    assert_eq!(
        repo.get_media_output(&output.id).await.unwrap().size_bytes,
        2048
    );
    assert_eq!(
        repo.get_session(&session.id)
            .await
            .unwrap()
            .total_size_bytes,
        2048
    );
}

#[tokio::test]
async fn concurrent_error_increments_return_their_own_committed_value() {
    let (_dir, read, write) = pools().await;
    let streamer = streamer(&read, &write).await;
    let repo = SqlxStreamerRepository::new(read.clone(), write);
    let held_read = read.acquire().await.unwrap();
    let (first, second) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(
            repo.increment_error_count(&streamer.id),
            repo.increment_error_count(&streamer.id)
        )
    })
    .await
    .unwrap();
    let mut counts = [first.unwrap(), second.unwrap()];
    counts.sort();
    assert_eq!(counts, [1, 2]);
    assert!(repo.increment_error_count("missing").await.is_err());
    drop(held_read);
    assert_eq!(
        repo.get_streamer(&streamer.id)
            .await
            .unwrap()
            .consecutive_error_count,
        Some(2)
    );
}

#[tokio::test]
async fn template_credential_refresh_reads_the_reserved_write_snapshot() {
    let (_dir, read, write) = pools().await;
    let configs = SqlxConfigRepository::new(read.clone(), write.clone());
    let mut template = TemplateConfigDbModel::new("atomic-template");
    template.cookies = Some("old-cookie".to_owned());
    template.platform_overrides = Some(json!({"bilibili": {"quality": "old"}}).to_string());
    configs.create_template_config(&template).await.unwrap();
    let store = SqlxCredentialStore::new(read.clone(), write.clone());
    let source = CredentialSource::new(
        CredentialScope::Template {
            template_id: template.id.clone(),
            template_name: template.name.clone(),
        },
        "old-cookie".to_owned(),
        None,
        "bilibili".to_owned(),
    );
    let credentials = RefreshedCredentials {
        cookies: "new-cookie".to_owned(),
        refresh_token: Some("new-refresh".to_owned()),
        access_token: Some("new-access".to_owned()),
        expires_at: None,
    };
    let mut competing_write = database::begin_immediate(&write).await.unwrap();
    sqlx::query("UPDATE template_config SET platform_overrides = ? WHERE id = ?")
        .bind(json!({"bilibili": {"quality": "updated"}, "twitch": {"custom": true}}).to_string())
        .bind(&template.id)
        .execute(&mut *competing_write)
        .await
        .unwrap();
    let held_read = read.acquire().await.unwrap();
    let update = store.update_credentials(&source, &credentials);
    tokio::pin!(update);
    assert!(futures::poll!(&mut update).is_pending());
    competing_write.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), &mut update)
        .await
        .unwrap()
        .unwrap();
    drop(held_read);
    let saved = configs.get_template_config(&template.id).await.unwrap();
    let value: serde_json::Value =
        serde_json::from_str(saved.platform_overrides.as_ref().unwrap()).unwrap();
    assert_eq!(
        value,
        json!({"bilibili": {"quality": "updated", "refresh_token": "new-refresh", "access_token": "new-access"}, "twitch": {"custom": true}})
    );
    assert_eq!(saved.cookies.as_deref(), Some("new-cookie"));

    sqlx::query("UPDATE template_config SET platform_overrides = '[]' WHERE id = ?")
        .bind(&template.id)
        .execute(&write)
        .await
        .unwrap();
    let invalid = RefreshedCredentials {
        cookies: "must-not-persist".to_owned(),
        ..credentials.clone()
    };
    assert!(store.update_credentials(&source, &invalid).await.is_err());
    assert_eq!(
        configs
            .get_template_config(&template.id)
            .await
            .unwrap()
            .cookies
            .as_deref(),
        Some("new-cookie")
    );
}
