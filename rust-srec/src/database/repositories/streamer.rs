//! Streamer repository.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::StreamerDbModel;
use crate::{Error, Result};

use chrono::{DateTime, Utc};

/// Streamer repository trait.
#[async_trait]
pub trait StreamerRepository: Send + Sync {
    async fn get_streamer(&self, id: &str) -> Result<StreamerDbModel>;
    async fn get_streamer_by_url(&self, url: &str) -> Result<StreamerDbModel>;
    async fn list_streamers(&self) -> Result<Vec<StreamerDbModel>>;
    async fn list_all_streamers(&self) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_state(&self, state: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_priority(&self, priority: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_platform(&self, platform_id: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_template(&self, template_id: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_active_streamers(&self) -> Result<Vec<StreamerDbModel>>;
    async fn create_streamer(&self, streamer: &StreamerDbModel) -> Result<()>;
    async fn update_streamer(&self, streamer: &StreamerDbModel) -> Result<()>;
    async fn update_streamer_state(&self, id: &str, state: &str) -> Result<()>;
    async fn update_streamer_priority(&self, id: &str, priority: &str) -> Result<()>;
    async fn increment_error_count(&self, id: &str) -> Result<i32>;
    async fn reset_error_count(&self, id: &str) -> Result<()>;
    async fn set_disabled_until(&self, id: &str, until: Option<&str>) -> Result<()>;
    async fn update_last_live_time(&self, id: &str, time: &str) -> Result<()>;
    async fn update_avatar(&self, id: &str, avatar_url: Option<&str>) -> Result<()>;
    async fn delete_streamer(&self, id: &str) -> Result<()>;

    // Methods for StreamerManager
    async fn clear_streamer_error_state(&self, id: &str) -> Result<()>;
    async fn record_streamer_error(
        &self,
        id: &str,
        error_count: i32,
        disabled_until: Option<DateTime<Utc>>,
        error: Option<&str>,
    ) -> Result<()>;
    async fn record_streamer_success(
        &self,
        id: &str,
        last_live_time: Option<DateTime<Utc>>,
    ) -> Result<()>;
}

/// SQLx implementation of StreamerRepository.
pub struct SqlxStreamerRepository {
    pool: SqlitePool,
}

impl SqlxStreamerRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl StreamerRepository for SqlxStreamerRepository {
    async fn get_streamer(&self, id: &str) -> Result<StreamerDbModel> {
        sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("Streamer", id))
    }

    async fn get_streamer_by_url(&self, url: &str) -> Result<StreamerDbModel> {
        sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE url = ?")
            .bind(url)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("Streamer", url))
    }

    async fn list_streamers(&self) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers ORDER BY priority DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_state(&self, state: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE state = ? ORDER BY priority DESC, name",
        )
        .bind(state)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_priority(&self, priority: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE priority = ? ORDER BY name",
        )
        .bind(priority)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_platform(&self, platform_id: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE platform_config_id = ? ORDER BY priority DESC, name",
        )
        .bind(platform_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_template(&self, template_id: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE template_config_id = ? ORDER BY priority DESC, name",
        )
        .bind(template_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_active_streamers(&self) -> Result<Vec<StreamerDbModel>> {
        // Active streamers are those not in CANCELLED, FATAL_ERROR, or NOT_FOUND states
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            r#"
            SELECT * FROM streamers 
            WHERE state NOT IN ('CANCELLED', 'FATAL_ERROR', 'NOT_FOUND')
            AND (disabled_until IS NULL OR disabled_until < datetime('now'))
            ORDER BY priority DESC, name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn create_streamer(&self, streamer: &StreamerDbModel) -> Result<()> {
        let result = sqlx::query(
            r#"
            INSERT INTO streamers (
                id, name, url, platform_config_id, template_config_id,
                state, priority, avatar, last_live_time, streamer_specific_config,
                consecutive_error_count, disabled_until, last_error,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&streamer.id)
        .bind(&streamer.name)
        .bind(&streamer.url)
        .bind(&streamer.platform_config_id)
        .bind(&streamer.template_config_id)
        .bind(&streamer.state)
        .bind(&streamer.priority)
        .bind(&streamer.avatar)
        .bind(&streamer.last_live_time)
        .bind(&streamer.streamer_specific_config)
        .bind(streamer.consecutive_error_count)
        .bind(&streamer.disabled_until)
        .bind(&streamer.last_error)
        .bind(&streamer.created_at)
        .bind(&streamer.updated_at)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(Error::duplicate_url(&streamer.url))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn update_streamer(&self, streamer: &StreamerDbModel) -> Result<()> {
        let result = sqlx::query(
            r#"
            UPDATE streamers SET
                name = ?,
                url = ?,
                platform_config_id = ?,
                template_config_id = ?,
                state = ?,
                priority = ?,
                avatar = ?,
                last_live_time = ?,
                streamer_specific_config = ?,
                consecutive_error_count = ?,
                disabled_until = ?,
                last_error = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&streamer.name)
        .bind(&streamer.url)
        .bind(&streamer.platform_config_id)
        .bind(&streamer.template_config_id)
        .bind(&streamer.state)
        .bind(&streamer.priority)
        .bind(&streamer.avatar)
        .bind(&streamer.last_live_time)
        .bind(&streamer.streamer_specific_config)
        .bind(streamer.consecutive_error_count)
        .bind(&streamer.disabled_until)
        .bind(&streamer.last_error)
        .bind(&streamer.updated_at)
        .bind(&streamer.id)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(Error::duplicate_url(&streamer.url))
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn update_streamer_state(&self, id: &str, state: &str) -> Result<()> {
        sqlx::query("UPDATE streamers SET state = ? WHERE id = ?")
            .bind(state)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_streamer_priority(&self, id: &str, priority: &str) -> Result<()> {
        sqlx::query("UPDATE streamers SET priority = ? WHERE id = ?")
            .bind(priority)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn increment_error_count(&self, id: &str) -> Result<i32> {
        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = COALESCE(consecutive_error_count, 0) + 1 WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        let result: (i32,) = sqlx::query_as(
            "SELECT COALESCE(consecutive_error_count, 0) FROM streamers WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Ok(result.0)
    }

    async fn reset_error_count(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_disabled_until(&self, id: &str, until: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE streamers SET disabled_until = ? WHERE id = ?")
            .bind(until)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_last_live_time(&self, id: &str, time: &str) -> Result<()> {
        sqlx::query("UPDATE streamers SET last_live_time = ? WHERE id = ?")
            .bind(time)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_avatar(&self, id: &str, avatar_url: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE streamers SET avatar = ? WHERE id = ?")
            .bind(avatar_url)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_streamer(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM streamers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_all_streamers(&self) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers ORDER BY priority DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn clear_streamer_error_state(&self, id: &str) -> Result<()> {
        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, state = 'NOT_LIVE' WHERE id = ?",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_streamer_error(
        &self,
        id: &str,
        error_count: i32,
        disabled_until: Option<DateTime<Utc>>,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE streamers SET 
                consecutive_error_count = ?,
                disabled_until = ?,
                last_error = ?
            WHERE id = ?
            "#,
        )
        .bind(error_count)
        .bind(disabled_until)
        .bind(error)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_streamer_success(
        &self,
        id: &str,
        last_live_time: Option<DateTime<Utc>>,
    ) -> Result<()> {
        if let Some(time) = last_live_time {
            sqlx::query(
                "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, last_live_time = ? WHERE id = ?",
            )
            .bind(time)
            .bind(id)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL, last_error = NULL WHERE id = ?",
            )
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}
