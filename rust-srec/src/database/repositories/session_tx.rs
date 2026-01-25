//! Transactional operations for live sessions.
//!
//! This module provides transaction-aware operations for session management.
//! Use these when you need to manage sessions as part of a larger transaction
//! (e.g., combined with streamer state updates and outbox events).

use chrono::{DateTime, Utc};
use sqlx::{Row, SqliteConnection};
use tracing::warn;

use crate::Result;
use crate::database::models::{LiveSessionDbModel, TitleEntry};

/// Transactional operations for live sessions.
///
/// These methods operate within an existing transaction and do NOT commit.
/// The caller is responsible for committing or rolling back the transaction.
pub struct SessionTxOps;

impl SessionTxOps {
    /// Get the most recent session for a streamer.
    pub async fn get_last_session(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<LiveSessionDbModel>> {
        let session = sqlx::query_as::<_, LiveSessionDbModel>(
            "SELECT * FROM live_sessions WHERE streamer_id = ? ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?;

        Ok(session)
    }

    /// Get the active (no end_time) session ID for a streamer.
    pub async fn get_active_session_id(
        tx: &mut SqliteConnection,
        streamer_id: &str,
    ) -> Result<Option<String>> {
        let session_id: Option<String> = sqlx::query(
            "SELECT id FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(tx)
        .await?
        .map(|row| row.get::<String, _>("id"));

        Ok(session_id)
    }

    /// Create a new session.
    pub async fn create_session(
        tx: &mut SqliteConnection,
        session_id: &str,
        streamer_id: &str,
        start_time: DateTime<Utc>,
        initial_title: &str,
    ) -> Result<()> {
        let initial_titles = vec![TitleEntry {
            ts: start_time.timestamp_millis(),
            title: initial_title.to_string(),
        }];
        let titles_json = serde_json::to_string(&initial_titles)?;

        sqlx::query(
            r#"
            INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, danmu_statistics_id, total_size_bytes)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(session_id)
        .bind(streamer_id)
        .bind(start_time.timestamp_millis())
        .bind(Option::<i64>::None)
        .bind(Some(titles_json))
        .bind(Option::<String>::None)
        .bind(0_i64)
        .execute(tx)
        .await?;

        Ok(())
    }

    /// Resume a session by clearing its end_time.
    pub async fn resume_session(tx: &mut SqliteConnection, session_id: &str) -> Result<u64> {
        let result = sqlx::query("UPDATE live_sessions SET end_time = NULL WHERE id = ?")
            .bind(session_id)
            .execute(tx)
            .await?;

        Ok(result.rows_affected())
    }

    /// End a session by setting end_time and calculating total_size_bytes.
    pub async fn end_session(
        tx: &mut SqliteConnection,
        session_id: &str,
        end_time: DateTime<Utc>,
    ) -> Result<u64> {
        let result = sqlx::query(
            r#"
            UPDATE live_sessions
            SET end_time = ?,
                total_size_bytes = (SELECT COALESCE(SUM(size_bytes), 0) FROM media_outputs WHERE session_id = ?)
            WHERE id = ?
            "#,
        )
        .bind(end_time.timestamp_millis())
        .bind(session_id)
        .bind(session_id)
        .execute(tx)
        .await?;

        Ok(result.rows_affected())
    }

    /// End the active session for a streamer (if any).
    ///
    /// Returns the session ID if one was ended.
    pub async fn end_active_session(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        end_time: DateTime<Utc>,
    ) -> Result<Option<String>> {
        let session_id = Self::get_active_session_id(tx, streamer_id).await?;

        if let Some(ref id) = session_id {
            Self::end_session(tx, id, end_time).await?;
        }

        Ok(session_id)
    }

    /// Update session titles by adding a new title entry.
    ///
    /// Only adds if the new title differs from the last one.
    pub async fn update_titles(
        tx: &mut SqliteConnection,
        session_id: &str,
        current_titles_json: Option<&str>,
        new_title: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<bool> {
        let mut titles: Vec<TitleEntry> = match current_titles_json {
            Some(json) => match serde_json::from_str(json) {
                Ok(parsed) => parsed,
                Err(error) => {
                    warn!(
                        session_id = %session_id,
                        raw_len = json.len(),
                        error = %error,
                        "Invalid session titles JSON; resetting to empty list"
                    );
                    Vec::new()
                }
            },
            None => Vec::new(),
        };

        let needs_update = titles.last().map(|t| t.title != new_title).unwrap_or(true);

        if needs_update {
            titles.push(TitleEntry {
                ts: timestamp.timestamp_millis(),
                title: new_title.to_string(),
            });
            let titles_json = serde_json::to_string(&titles)?;

            sqlx::query("UPDATE live_sessions SET titles = ? WHERE id = ?")
                .bind(titles_json)
                .bind(session_id)
                .execute(tx)
                .await?;
        }

        Ok(needs_update)
    }

    /// Helper: Determine if a session should be resumed based on gap time.
    ///
    /// Returns true if the session ended recently enough to resume.
    pub fn should_resume_by_gap(
        session_end_time: DateTime<Utc>,
        now: DateTime<Utc>,
        gap_threshold_secs: i64,
    ) -> bool {
        let offline_duration_secs = (now - session_end_time).num_seconds();
        offline_duration_secs < gap_threshold_secs
    }

    /// Helper: Determine if a session should be resumed because stream is a continuation.
    ///
    /// Returns true if the stream started before or when the session ended,
    /// indicating the stream was actually continuous (monitoring gap).
    pub fn should_resume_by_continuation(
        session_end_time: DateTime<Utc>,
        stream_started_at: Option<DateTime<Utc>>,
    ) -> bool {
        stream_started_at
            .map(|start| start <= session_end_time)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE live_sessions (
                id TEXT PRIMARY KEY,
                streamer_id TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                titles TEXT,
                danmu_statistics_id TEXT,
                total_size_bytes INTEGER DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE media_outputs (
                id TEXT PRIMARY KEY,
                session_id TEXT,
                size_bytes INTEGER DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_create_session() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let now = Utc::now();
        SessionTxOps::create_session(&mut tx, "sess-1", "streamer-1", now, "Test Stream")
            .await
            .unwrap();

        tx.commit().await.unwrap();

        // Verify
        let session: LiveSessionDbModel =
            sqlx::query_as("SELECT * FROM live_sessions WHERE id = 'sess-1'")
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(session.streamer_id, "streamer-1");
        assert!(session.end_time.is_none());
        assert!(session.titles.is_some());
    }

    #[tokio::test]
    async fn test_resume_and_end_session() {
        let pool = setup_test_db().await;

        // Create a session with end_time
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time, end_time) VALUES (?, ?, ?, ?)",
        )
        .bind("sess-1")
        .bind("streamer-1")
        .bind(now.timestamp_millis())
        .bind(now.timestamp_millis())
        .execute(&pool)
        .await
        .unwrap();

        // Resume
        let mut tx = pool.begin().await.unwrap();
        SessionTxOps::resume_session(&mut tx, "sess-1")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // Verify resumed
        let session: LiveSessionDbModel =
            sqlx::query_as("SELECT * FROM live_sessions WHERE id = 'sess-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(session.end_time.is_none());

        // End again
        let mut tx = pool.begin().await.unwrap();
        SessionTxOps::end_session(&mut tx, "sess-1", Utc::now())
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // Verify ended
        let session: LiveSessionDbModel =
            sqlx::query_as("SELECT * FROM live_sessions WHERE id = 'sess-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(session.end_time.is_some());
    }

    #[tokio::test]
    async fn test_update_titles() {
        let pool = setup_test_db().await;

        // Create session
        let now = Utc::now();
        let mut tx = pool.begin().await.unwrap();
        SessionTxOps::create_session(&mut tx, "sess-1", "streamer-1", now, "Initial Title")
            .await
            .unwrap();
        tx.commit().await.unwrap();

        // Update with same title - should not update
        let session: LiveSessionDbModel =
            sqlx::query_as("SELECT * FROM live_sessions WHERE id = 'sess-1'")
                .fetch_one(&pool)
                .await
                .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let updated = SessionTxOps::update_titles(
            &mut tx,
            "sess-1",
            session.titles.as_deref(),
            "Initial Title",
            Utc::now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(!updated);

        // Update with different title - should update
        let session: LiveSessionDbModel =
            sqlx::query_as("SELECT * FROM live_sessions WHERE id = 'sess-1'")
                .fetch_one(&pool)
                .await
                .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let updated = SessionTxOps::update_titles(
            &mut tx,
            "sess-1",
            session.titles.as_deref(),
            "New Title",
            Utc::now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(updated);
    }

    #[test]
    fn test_should_resume_by_gap() {
        let now = Utc::now();
        let end_time = now - chrono::Duration::seconds(30);

        // 30 seconds offline, 60 second threshold -> should resume
        assert!(SessionTxOps::should_resume_by_gap(end_time, now, 60));

        // 30 seconds offline, 20 second threshold -> should not resume
        assert!(!SessionTxOps::should_resume_by_gap(end_time, now, 20));
    }

    #[test]
    fn test_should_resume_by_continuation() {
        let end_time = Utc::now();
        let before_end = end_time - chrono::Duration::minutes(5);
        let after_end = end_time + chrono::Duration::minutes(5);

        // Stream started before session ended -> continuation
        assert!(SessionTxOps::should_resume_by_continuation(
            end_time,
            Some(before_end)
        ));

        // Stream started after session ended -> not continuation
        assert!(!SessionTxOps::should_resume_by_continuation(
            end_time,
            Some(after_end)
        ));

        // No started_at -> not continuation
        assert!(!SessionTxOps::should_resume_by_continuation(end_time, None));
    }
}
