//! Full-row and transaction contracts exercised through the session repository entrypoints.

use serde::Serialize;
use sqlx::SqlitePool;
use srec_core::CoreError;

use super::*;
use crate::models::{
    LiveSessionDbModel, MediaOutputDbModel, SessionSegmentDbModel, StreamerDbModel,
};

async fn fixture() -> SqlitePool {
    let pool = crate::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::run_migrations(&pool).await.unwrap();
    let mut streamer =
        StreamerDbModel::new("Writer", "https://www.twitch.tv/writer", "platform-twitch");
    streamer.id = "streamer".into();
    SqlxStreamerRepository::new(pool.clone(), pool.clone())
        .create_streamer(&streamer)
        .await
        .unwrap();
    let mut session = LiveSessionDbModel::new("streamer").with_streamer_name("Writer snapshot");
    session.id = "session".into();
    sessions(&pool).create_session(&session).await.unwrap();
    pool
}

fn sessions(pool: &SqlitePool) -> SqlxSessionRepository {
    SqlxSessionRepository::new(pool.clone(), pool.clone())
}

fn assert_row<T: Serialize>(actual: &T, expected: &T) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

fn media(id: &str, path: &str, bytes: i64) -> MediaOutputDbModel {
    MediaOutputDbModel {
        id: id.into(),
        session_id: "session".into(),
        parent_media_output_id: None,
        file_path: path.into(),
        file_type: "VIDEO".into(),
        size_bytes: bytes,
        created_at: 1_710_000_004_123,
    }
}

fn segment(id: &str, path: &str, index: i64) -> SessionSegmentDbModel {
    SessionSegmentDbModel {
        id: id.into(),
        session_id: "session".into(),
        segment_index: index,
        file_path: path.into(),
        duration_secs: 3.25,
        size_bytes: 99,
        split_reason_code: Some("duration".into()),
        split_reason_details_json: Some(r#"{"limit":3,"extension":true}"#.into()),
        created_at: Some(1_710_000_005_123),
        completed_at: Some(1_710_000_005_456),
        persisted_at: 1_710_000_005_789,
    }
}

async fn read_segment(pool: &SqlitePool, id: &str) -> SessionSegmentDbModel {
    sqlx::query_as("SELECT * FROM session_segments WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn session_creation_and_end_modes_preserve_raw_values_defaults_and_idempotence() {
    let pool = fixture().await;
    let repo = sessions(&pool);
    let raw = LiveSessionDbModel {
        id: "raw".into(),
        streamer_id: None,
        streamer_name: None,
        start_time: -1234,
        end_time: Some(9876),
        titles: Some(r#"[{"title":"raw","ts":42,"extra":true}]"#.into()),
        total_size_bytes: 77,
    };
    repo.create_session(&raw).await.unwrap();
    assert_row(&repo.get_session(&raw.id).await.unwrap(), &raw);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT session_complete_dispatched FROM live_sessions WHERE id = 'raw'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    repo.end_session("session", 1000).await.unwrap();
    let started = jiff::Timestamp::from_millisecond(1_710_000_007_123).unwrap();
    let mut tx = crate::begin_immediate(&pool).await.unwrap();
    SessionTxOps::create_session(
        &mut tx,
        "live",
        "streamer",
        "Snapshot 名称",
        started,
        "Title 名称",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let live = LiveSessionDbModel {
        id: "live".into(),
        streamer_id: Some("streamer".into()),
        streamer_name: Some("Snapshot 名称".into()),
        start_time: started.as_millisecond(),
        end_time: None,
        titles: Some(
            serde_json::to_string(&[crate::models::TitleEntry {
                ts: started.as_millisecond(),
                title: "Title 名称".into(),
            }])
            .unwrap(),
        ),
        total_size_bytes: 0,
    };
    assert_row(&repo.get_session("live").await.unwrap(), &live);
    let ended = started + jiff::SignedDuration::from_millis(321);
    let mut tx = crate::begin_immediate(&pool).await.unwrap();
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "live", ended)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "raw", ended)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "missing", ended)
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert_row(&repo.get_session("raw").await.unwrap(), &raw);
    sqlx::query("UPDATE live_sessions SET total_size_bytes = 999, session_complete_dispatched = 1 WHERE id = 'live'").execute(&pool).await.unwrap();
    let mut tx = crate::begin_immediate(&pool).await.unwrap();
    assert_eq!(
        SessionTxOps::end_session(&mut tx, "live", ended + jiff::SignedDuration::from_secs(1))
            .await
            .unwrap(),
        0
    );
    tx.commit().await.unwrap();
    assert_eq!(
        repo.get_session("live").await.unwrap().total_size_bytes,
        999
    );
    repo.end_session("live", i64::MAX).await.unwrap();
    let result = repo.get_session("live").await.unwrap();
    assert_eq!(result.end_time, Some(i64::MAX));
    assert_eq!(result.total_size_bytes, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT session_complete_dispatched FROM live_sessions WHERE id = 'live'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    repo.end_session("missing", i64::MAX).await.unwrap();
}

#[tokio::test]
async fn media_and_segment_rows_preserve_all_fields_and_account_size_once() {
    let pool = fixture().await;
    let repo = sessions(&pool);
    let parent = media("parent", "parent.flv", 3);
    repo.create_media_output(&parent).await.unwrap();
    for combined in [false, true] {
        let mut output = media(
            if combined { "combined" } else { "single" },
            if combined {
                "combined.flv"
            } else {
                "single.flv"
            },
            101,
        );
        output.parent_media_output_id = Some(parent.id.clone());
        output.file_type = "THUMBNAIL".into();
        let mut row = segment(&output.id, &output.file_path, if combined { 2 } else { 1 });
        if !combined {
            row.created_at = None;
            row.completed_at = None;
        }
        if combined {
            repo.create_segment_output(&output, &row).await.unwrap();
        } else {
            repo.create_media_output(&output).await.unwrap();
            let before = repo.get_session("session").await.unwrap().total_size_bytes;
            repo.create_session_segment(&row).await.unwrap();
            assert_eq!(
                repo.get_session("session").await.unwrap().total_size_bytes,
                before
            );
        }
        assert_row(&repo.get_media_output(&output.id).await.unwrap(), &output);
        assert_row(&read_segment(&pool, &row.id).await, &row);
    }
    assert_eq!(
        repo.get_session("session").await.unwrap().total_size_bytes,
        205
    );
    let output = media("bad-pair", "bad.flv", 11);
    let mut mismatched = segment("bad-pair", "other.flv", 3);
    assert!(
        repo.create_segment_output(&output, &mismatched)
            .await
            .is_err()
    );
    mismatched.file_path = output.file_path.clone();
    mismatched.session_id = "other-session".into();
    assert!(
        repo.create_segment_output(&output, &mismatched)
            .await
            .is_err()
    );
    assert!(matches!(
        repo.get_media_output(&output.id).await,
        Err(crate::DbError::Core(CoreError::NotFound { .. }))
    ));
    assert_eq!(
        repo.get_session("session").await.unwrap().total_size_bytes,
        205
    );
}

#[tokio::test]
async fn media_size_and_segment_failures_roll_back_every_related_write() {
    let pool = fixture().await;
    let repo = sessions(&pool);
    sqlx::query("CREATE TRIGGER reject_total BEFORE UPDATE OF total_size_bytes ON live_sessions BEGIN SELECT RAISE(ABORT, 'reject total'); END").execute(&pool).await.unwrap();
    let output = media("rejected-media", "reject.flv", 41);
    assert!(repo.create_media_output(&output).await.is_err());
    assert!(matches!(
        repo.get_media_output(&output.id).await,
        Err(crate::DbError::Core(CoreError::NotFound { .. }))
    ));
    sqlx::query("DROP TRIGGER reject_total")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER reject_segment BEFORE INSERT ON session_segments BEGIN SELECT RAISE(ABORT, 'reject segment'); END").execute(&pool).await.unwrap();
    assert!(
        repo.create_segment_output(&output, &segment("rejected-segment", &output.file_path, 1))
            .await
            .is_err()
    );
    assert!(matches!(
        repo.get_media_output(&output.id).await,
        Err(crate::DbError::Core(CoreError::NotFound { .. }))
    ));
    assert_eq!(
        repo.get_session("session").await.unwrap().total_size_bytes,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM session_segments")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn connection_helpers_do_not_commit_the_callers_transaction() {
    let pool = fixture().await;
    let mut tx = crate::begin_immediate(&pool).await.unwrap();
    assert_eq!(
        session_tx::writes::end_session(
            &mut tx,
            "session",
            i64::MAX,
            session_tx::writes::EndCondition::Any
        )
        .await
        .unwrap(),
        1
    );
    let row = LiveSessionDbModel {
        id: "rolled-session".into(),
        streamer_id: None,
        streamer_name: Some("history".into()),
        start_time: 123,
        end_time: None,
        titles: None,
        total_size_bytes: 0,
    };
    session_tx::writes::insert_session(&mut tx, &row)
        .await
        .unwrap();
    let mut output = media("rolled-media", "rolled.flv", 51);
    output.session_id = row.id.clone();
    session_tx::writes::insert_media_output(&mut tx, &output)
        .await
        .unwrap();
    let mut part = segment("rolled-segment", &output.file_path, 1);
    part.session_id = row.id.clone();
    session_tx::writes::insert_segment(&mut tx, &part)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(
        sessions(&pool)
            .get_session("session")
            .await
            .unwrap()
            .end_time
            .is_none()
    );
    assert!(matches!(
        sessions(&pool).get_session(&row.id).await,
        Err(crate::DbError::Core(CoreError::NotFound { .. }))
    ));
    assert!(matches!(
        sessions(&pool).get_media_output(&output.id).await,
        Err(crate::DbError::Core(CoreError::NotFound { .. }))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM session_segments")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
