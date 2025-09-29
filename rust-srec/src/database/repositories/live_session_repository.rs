use crate::database::models::LiveSession as DbLiveSession;
use crate::domain::live_session::LiveSession;
use async_trait::async_trait;
use sqlx::SqlitePool;
use std::sync::Arc;

use super::MediaOutputRepository;
use super::errors::RepositoryResult;

#[async_trait]
pub trait LiveSessionRepository {
    async fn create(&self, live_session: &LiveSession) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<LiveSession>>;
    async fn find_all(&self) -> RepositoryResult<Vec<LiveSession>>;
    async fn update(&self, live_session: &LiveSession) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteLiveSessionRepository {
    pool: SqlitePool,
    media_output_repository: Arc<dyn MediaOutputRepository>,
}

impl SqliteLiveSessionRepository {
    pub fn new(pool: SqlitePool, media_output_repository: Arc<dyn MediaOutputRepository>) -> Self {
        Self {
            pool,
            media_output_repository,
        }
    }
}

#[async_trait]
impl LiveSessionRepository for SqliteLiveSessionRepository {
    async fn create(&self, live_session: &LiveSession) -> RepositoryResult<()> {
        let titles = serde_json::to_string(&live_session.titles)?;
        let end_time = live_session.end_time.map(|t| t.to_rfc3339());
        let start_time = live_session.start_time.to_rfc3339();

        sqlx::query!(
            r#"
            INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles)
            VALUES (?, ?, ?, ?, ?)
            "#,
            live_session.id,
            live_session.streamer_id,
            start_time,
            end_time,
            titles,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<LiveSession>> {
        let db_session = sqlx::query_as!(
            DbLiveSession,
            r#"
            SELECT 
                id as "id!", 
                streamer_id as "streamer_id!", 
                start_time as "start_time!", 
                end_time, 
                titles 
            FROM live_sessions
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(db_session) = db_session {
            let mut live_session = LiveSession::from(db_session);

            live_session.media_outputs = self
                .media_output_repository
                .find_by_live_session_id(&live_session.id)
                .await?;

            Ok(Some(live_session))
        } else {
            Ok(None)
        }
    }

    async fn find_all(&self) -> RepositoryResult<Vec<LiveSession>> {
        let db_sessions = sqlx::query_as!(
            DbLiveSession,
            r#"
            SELECT 
                id as "id!", 
                streamer_id as "streamer_id!", 
                start_time as "start_time!", 
                end_time, 
                titles 
            FROM live_sessions
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut sessions = Vec::new();
        for db_session in db_sessions {
            let mut live_session = LiveSession::from(db_session);

            live_session.media_outputs = self
                .media_output_repository
                .find_by_live_session_id(&live_session.id)
                .await?;

            sessions.push(live_session);
        }
        Ok(sessions)
    }

    async fn update(&self, live_session: &LiveSession) -> RepositoryResult<()> {
        let titles = serde_json::to_string(&live_session.titles)?;
        let end_time = live_session.end_time.map(|t| t.to_rfc3339());
        let start_time = live_session.start_time.to_rfc3339();

        sqlx::query!(
            r#"
            UPDATE live_sessions
            SET start_time = ?, end_time = ?, titles = ?
            WHERE id = ?
            "#,
            start_time,
            end_time,
            titles,
            live_session.id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!("DELETE FROM live_sessions WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
