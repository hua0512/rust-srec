use crate::database::models;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::notification_subscription::NotificationSubscription;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait NotificationSubscriptionRepository {
    async fn create(
        &self,
        notification_subscription: &NotificationSubscription,
    ) -> RepositoryResult<()>;
    async fn find_by_channel_id(
        &self,
        channel_id: &str,
    ) -> RepositoryResult<Vec<NotificationSubscription>>;
    async fn delete(
        &self,
        notification_subscription: &NotificationSubscription,
    ) -> RepositoryResult<()>;
}

pub struct SqliteNotificationSubscriptionRepository {
    db: SqlitePool,
}

impl SqliteNotificationSubscriptionRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl NotificationSubscriptionRepository for SqliteNotificationSubscriptionRepository {
    async fn create(
        &self,
        notification_subscription: &NotificationSubscription,
    ) -> RepositoryResult<()> {
        let model = models::NotificationSubscription::from(notification_subscription);
        sqlx::query!(
            r#"
            INSERT INTO notification_subscription (channel_id, event_name)
            VALUES (?, ?)
            "#,
            model.channel_id,
            model.event_name
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_channel_id(
        &self,
        channel_id: &str,
    ) -> RepositoryResult<Vec<NotificationSubscription>> {
        let subscriptions = sqlx::query_as!(
            models::NotificationSubscription,
            r#"
            SELECT channel_id, event_name
            FROM notification_subscription
            WHERE channel_id = ?
            "#,
            channel_id
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(NotificationSubscription::from)
        .collect();

        Ok(subscriptions)
    }

    async fn delete(
        &self,
        notification_subscription: &NotificationSubscription,
    ) -> RepositoryResult<()> {
        let model = models::NotificationSubscription::from(notification_subscription);
        sqlx::query!(
            r#"
            DELETE FROM notification_subscription
            WHERE channel_id = ? AND event_name = ?
            "#,
            model.channel_id,
            model.event_name
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }
}
