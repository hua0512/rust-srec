//! Notification repository.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::{
    NotificationChannelDbModel, NotificationDeadLetterDbModel,
};
use crate::{Error, Result};

/// Notification repository trait.
#[async_trait]
pub trait NotificationRepository: Send + Sync {
    // Channels
    async fn get_channel(&self, id: &str) -> Result<NotificationChannelDbModel>;
    async fn list_channels(&self) -> Result<Vec<NotificationChannelDbModel>>;
    async fn create_channel(&self, channel: &NotificationChannelDbModel) -> Result<()>;
    async fn update_channel(&self, channel: &NotificationChannelDbModel) -> Result<()>;
    async fn delete_channel(&self, id: &str) -> Result<()>;

    // Subscriptions
    async fn get_subscriptions_for_channel(&self, channel_id: &str) -> Result<Vec<String>>;
    async fn get_channels_for_event(&self, event_name: &str) -> Result<Vec<NotificationChannelDbModel>>;
    async fn subscribe(&self, channel_id: &str, event_name: &str) -> Result<()>;
    async fn unsubscribe(&self, channel_id: &str, event_name: &str) -> Result<()>;
    async fn unsubscribe_all(&self, channel_id: &str) -> Result<()>;

    // Dead Letter Queue
    async fn add_to_dead_letter(&self, entry: &NotificationDeadLetterDbModel) -> Result<()>;
    async fn list_dead_letters(&self, channel_id: Option<&str>, limit: i32) -> Result<Vec<NotificationDeadLetterDbModel>>;
    async fn get_dead_letter(&self, id: &str) -> Result<NotificationDeadLetterDbModel>;
    async fn delete_dead_letter(&self, id: &str) -> Result<()>;
    async fn cleanup_old_dead_letters(&self, retention_days: i32) -> Result<i32>;
}

/// SQLx implementation of NotificationRepository.
pub struct SqlxNotificationRepository {
    pool: SqlitePool,
}

impl SqlxNotificationRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl NotificationRepository for SqlxNotificationRepository {
    async fn get_channel(&self, id: &str) -> Result<NotificationChannelDbModel> {
        sqlx::query_as::<_, NotificationChannelDbModel>(
            "SELECT * FROM notification_channel WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("NotificationChannel", id))
    }

    async fn list_channels(&self) -> Result<Vec<NotificationChannelDbModel>> {
        let channels = sqlx::query_as::<_, NotificationChannelDbModel>(
            "SELECT * FROM notification_channel ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(channels)
    }

    async fn create_channel(&self, channel: &NotificationChannelDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO notification_channel (id, name, channel_type, settings)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(&channel.id)
        .bind(&channel.name)
        .bind(&channel.channel_type)
        .bind(&channel.settings)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_channel(&self, channel: &NotificationChannelDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE notification_channel SET
                name = ?,
                channel_type = ?,
                settings = ?
            WHERE id = ?
            "#,
        )
        .bind(&channel.name)
        .bind(&channel.channel_type)
        .bind(&channel.settings)
        .bind(&channel.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_channel(&self, id: &str) -> Result<()> {
        // Delete subscriptions first (no CASCADE in schema)
        sqlx::query("DELETE FROM notification_subscription WHERE channel_id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;

        sqlx::query("DELETE FROM notification_channel WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_subscriptions_for_channel(&self, channel_id: &str) -> Result<Vec<String>> {
        let subs: Vec<(String,)> = sqlx::query_as(
            "SELECT event_name FROM notification_subscription WHERE channel_id = ? ORDER BY event_name",
        )
        .bind(channel_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(subs.into_iter().map(|(e,)| e).collect())
    }

    async fn get_channels_for_event(&self, event_name: &str) -> Result<Vec<NotificationChannelDbModel>> {
        let channels = sqlx::query_as::<_, NotificationChannelDbModel>(
            r#"
            SELECT nc.* FROM notification_channel nc
            INNER JOIN notification_subscription ns ON nc.id = ns.channel_id
            WHERE ns.event_name = ?
            ORDER BY nc.name
            "#,
        )
        .bind(event_name)
        .fetch_all(&self.pool)
        .await?;
        Ok(channels)
    }

    async fn subscribe(&self, channel_id: &str, event_name: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO notification_subscription (channel_id, event_name)
            VALUES (?, ?)
            "#,
        )
        .bind(channel_id)
        .bind(event_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn unsubscribe(&self, channel_id: &str, event_name: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM notification_subscription WHERE channel_id = ? AND event_name = ?",
        )
        .bind(channel_id)
        .bind(event_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn unsubscribe_all(&self, channel_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM notification_subscription WHERE channel_id = ?")
            .bind(channel_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn add_to_dead_letter(&self, entry: &NotificationDeadLetterDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO notification_dead_letter (
                id, channel_id, event_name, event_payload, error_message,
                retry_count, first_attempt_at, last_attempt_at, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.channel_id)
        .bind(&entry.event_name)
        .bind(&entry.event_payload)
        .bind(&entry.error_message)
        .bind(entry.retry_count)
        .bind(&entry.first_attempt_at)
        .bind(&entry.last_attempt_at)
        .bind(&entry.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_dead_letters(
        &self,
        channel_id: Option<&str>,
        limit: i32,
    ) -> Result<Vec<NotificationDeadLetterDbModel>> {
        let entries = if let Some(cid) = channel_id {
            sqlx::query_as::<_, NotificationDeadLetterDbModel>(
                "SELECT * FROM notification_dead_letter WHERE channel_id = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(cid)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, NotificationDeadLetterDbModel>(
                "SELECT * FROM notification_dead_letter ORDER BY created_at DESC LIMIT ?",
            )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };
        Ok(entries)
    }

    async fn get_dead_letter(&self, id: &str) -> Result<NotificationDeadLetterDbModel> {
        sqlx::query_as::<_, NotificationDeadLetterDbModel>(
            "SELECT * FROM notification_dead_letter WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("NotificationDeadLetter", id))
    }

    async fn delete_dead_letter(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM notification_dead_letter WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn cleanup_old_dead_letters(&self, retention_days: i32) -> Result<i32> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let result = sqlx::query("DELETE FROM notification_dead_letter WHERE created_at < ?")
            .bind(&cutoff_str)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() as i32)
    }
}
