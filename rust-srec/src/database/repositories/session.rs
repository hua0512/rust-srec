//! Session repository.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::{DanmuStatisticsDbModel, LiveSessionDbModel, MediaOutputDbModel};
use crate::{Error, Result};

/// Session repository trait.
#[async_trait]
pub trait SessionRepository: Send + Sync {
    // Live Sessions
    async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel>;
    async fn get_active_session_for_streamer(&self, streamer_id: &str) -> Result<Option<LiveSessionDbModel>>;
    async fn list_sessions_for_streamer(&self, streamer_id: &str, limit: i32) -> Result<Vec<LiveSessionDbModel>>;
    async fn create_session(&self, session: &LiveSessionDbModel) -> Result<()>;
    async fn end_session(&self, id: &str, end_time: &str) -> Result<()>;
    async fn update_session_titles(&self, id: &str, titles: &str) -> Result<()>;
    async fn delete_session(&self, id: &str) -> Result<()>;

    // Media Outputs
    async fn get_media_output(&self, id: &str) -> Result<MediaOutputDbModel>;
    async fn get_media_outputs_for_session(&self, session_id: &str) -> Result<Vec<MediaOutputDbModel>>;
    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()>;
    async fn delete_media_output(&self, id: &str) -> Result<()>;

    // Danmu Statistics
    async fn get_danmu_statistics(&self, session_id: &str) -> Result<Option<DanmuStatisticsDbModel>>;
    async fn create_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()>;
    async fn update_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()>;
}

/// SQLx implementation of SessionRepository.
pub struct SqlxSessionRepository {
    pool: SqlitePool,
}

impl SqlxSessionRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SessionRepository for SqlxSessionRepository {
    async fn get_session(&self, id: &str) -> Result<LiveSessionDbModel> {
        sqlx::query_as::<_, LiveSessionDbModel>("SELECT * FROM live_sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("LiveSession", id))
    }

    async fn get_active_session_for_streamer(&self, streamer_id: &str) -> Result<Option<LiveSessionDbModel>> {
        let session = sqlx::query_as::<_, LiveSessionDbModel>(
            "SELECT * FROM live_sessions WHERE streamer_id = ? AND end_time IS NULL ORDER BY start_time DESC LIMIT 1",
        )
        .bind(streamer_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(session)
    }

    async fn list_sessions_for_streamer(&self, streamer_id: &str, limit: i32) -> Result<Vec<LiveSessionDbModel>> {
        let sessions = sqlx::query_as::<_, LiveSessionDbModel>(
            "SELECT * FROM live_sessions WHERE streamer_id = ? ORDER BY start_time DESC LIMIT ?",
        )
        .bind(streamer_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
    }

    async fn create_session(&self, session: &LiveSessionDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, danmu_statistics_id)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&session.id)
        .bind(&session.streamer_id)
        .bind(&session.start_time)
        .bind(&session.end_time)
        .bind(&session.titles)
        .bind(&session.danmu_statistics_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn end_session(&self, id: &str, end_time: &str) -> Result<()> {
        sqlx::query("UPDATE live_sessions SET end_time = ? WHERE id = ?")
            .bind(end_time)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_session_titles(&self, id: &str, titles: &str) -> Result<()> {
        sqlx::query("UPDATE live_sessions SET titles = ? WHERE id = ?")
            .bind(titles)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_session(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM live_sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_media_output(&self, id: &str) -> Result<MediaOutputDbModel> {
        sqlx::query_as::<_, MediaOutputDbModel>("SELECT * FROM media_outputs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("MediaOutput", id))
    }

    async fn get_media_outputs_for_session(&self, session_id: &str) -> Result<Vec<MediaOutputDbModel>> {
        let outputs = sqlx::query_as::<_, MediaOutputDbModel>(
            "SELECT * FROM media_outputs WHERE session_id = ? ORDER BY created_at",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(outputs)
    }

    async fn create_media_output(&self, output: &MediaOutputDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO media_outputs (id, session_id, parent_media_output_id, file_path, file_type, size_bytes, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&output.id)
        .bind(&output.session_id)
        .bind(&output.parent_media_output_id)
        .bind(&output.file_path)
        .bind(&output.file_type)
        .bind(output.size_bytes)
        .bind(&output.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_media_output(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM media_outputs WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_danmu_statistics(&self, session_id: &str) -> Result<Option<DanmuStatisticsDbModel>> {
        let stats = sqlx::query_as::<_, DanmuStatisticsDbModel>(
            "SELECT * FROM danmu_statistics WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(stats)
    }

    async fn create_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO danmu_statistics (id, session_id, total_danmus, danmu_rate_timeseries, top_talkers, word_frequency)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&stats.id)
        .bind(&stats.session_id)
        .bind(stats.total_danmus)
        .bind(&stats.danmu_rate_timeseries)
        .bind(&stats.top_talkers)
        .bind(&stats.word_frequency)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_danmu_statistics(&self, stats: &DanmuStatisticsDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE danmu_statistics SET
                total_danmus = ?,
                danmu_rate_timeseries = ?,
                top_talkers = ?,
                word_frequency = ?
            WHERE id = ?
            "#,
        )
        .bind(stats.total_danmus)
        .bind(&stats.danmu_rate_timeseries)
        .bind(&stats.top_talkers)
        .bind(&stats.word_frequency)
        .bind(&stats.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
