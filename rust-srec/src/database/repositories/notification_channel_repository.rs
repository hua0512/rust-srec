use crate::database::models;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::notification_channel::NotificationChannel;
use async_trait::async_trait;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait]
pub trait NotificationChannelRepository {
    async fn create(&self, notification_channel: &NotificationChannel) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<NotificationChannel>>;
    async fn find_all(&self) -> RepositoryResult<Vec<NotificationChannel>>;
    async fn update(&self, notification_channel: &NotificationChannel) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteNotificationChannelRepository {
    db: SqlitePool,
}

impl SqliteNotificationChannelRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl NotificationChannelRepository for SqliteNotificationChannelRepository {
    async fn create(&self, notification_channel: &NotificationChannel) -> RepositoryResult<()> {
        let model = models::NotificationChannel::from(notification_channel);
        sqlx::query!(
            r#"
            INSERT INTO notification_channel (id, name, channel_type, settings)
            VALUES (?, ?, ?, ?)
            "#,
            model.id,
            model.name,
            model.channel_type,
            model.settings
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<NotificationChannel>> {
        let channel = sqlx::query_as!(
            models::NotificationChannel,
            r#"
            SELECT id as "id!", name as "name!", channel_type as "channel_type!", settings as "settings!"
            FROM notification_channel
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(NotificationChannel::from);

        Ok(channel)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<NotificationChannel>> {
        let channels = sqlx::query_as!(
            models::NotificationChannel,
            r#"
            SELECT id as "id!", name as "name!", channel_type as "channel_type!", settings as "settings!"
            FROM notification_channel
            "#,
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(NotificationChannel::from)
        .collect();

        Ok(channels)
    }

    async fn update(&self, notification_channel: &NotificationChannel) -> RepositoryResult<()> {
        let model = models::NotificationChannel::from(notification_channel);
        sqlx::query!(
            r#"
            UPDATE notification_channel
            SET name = ?, channel_type = ?, settings = ?
            WHERE id = ?
            "#,
            model.name,
            model.channel_type,
            model.settings,
            model.id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            DELETE FROM notification_channel
            WHERE id = ?
            "#,
            id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }
}
