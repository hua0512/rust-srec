use std::time::Duration;

use rust_srec::database::{self, models::*, repositories::*};
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
