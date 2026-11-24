//! Notification repository.

mod writes;
pub use writes::ChannelPatch;
pub(crate) use writes::{
    SubscriptionInsert, delete_channel_dead_letters, delete_channel_row, delete_subscriptions,
    import_channel, insert_subscription,
};

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::{QueryBuilder, SqlitePool};

use crate::database::models::{
    NotificationChannelDbModel, NotificationDeadLetterDbModel, NotificationEventLogDbModel,
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
    /// Applies `patch` to the channel `id` in one write transaction and
    /// returns it as stored.
    async fn patch_channel(
        &self,
        id: &str,
        patch: ChannelPatch,
    ) -> Result<NotificationChannelDbModel>;
    async fn delete_channel(&self, id: &str) -> Result<()>;

    // Subscriptions
    async fn get_subscriptions_for_channel(&self, channel_id: &str) -> Result<Vec<String>>;
    /// Group subscriptions by channel, retaining event-name order within each channel.
    /// An absent map entry represents a channel with no subscriptions.
    async fn get_subscriptions_for_channels(
        &self,
        channel_ids: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut subscriptions = HashMap::new();
        for id in channel_ids {
            subscriptions.insert(id.clone(), self.get_subscriptions_for_channel(id).await?);
        }
        Ok(subscriptions)
    }
    async fn subscribe(&self, channel_id: &str, event_name: &str) -> Result<()>;
    async fn unsubscribe(&self, channel_id: &str, event_name: &str) -> Result<()>;
    /// Atomically replace all event subscriptions of `channel_id`.
    async fn replace_subscriptions(&self, channel_id: &str, events: &[String]) -> Result<()> {
        self.get_channel(channel_id).await?;
        for existing in self.get_subscriptions_for_channel(channel_id).await? {
            self.unsubscribe(channel_id, &existing).await?;
        }
        for event in events {
            self.subscribe(channel_id, event).await?;
        }
        Ok(())
    }

    /// Resolve a persisted `users.id` row to associate with Web Push
    /// subscriptions when API authentication is disabled (`AUTH_DISABLED=true`).
    async fn default_web_push_user_id(&self) -> Result<Option<String>> {
        Ok(Some(
            "default-admin-00000000-0000-0000-0000-000000000001".to_string(),
        ))
    }

    // Dead Letter Queue
    async fn add_to_dead_letter(&self, entry: &NotificationDeadLetterDbModel) -> Result<()>;
    async fn list_dead_letters(
        &self,
        channel_id: Option<&str>,
        limit: i32,
    ) -> Result<Vec<NotificationDeadLetterDbModel>>;

    // Event log
    async fn add_event_log(&self, entry: &NotificationEventLogDbModel) -> Result<()>;
    async fn list_event_logs(
        &self,
        event_type: Option<&str>,
        streamer_id: Option<&str>,
        search: Option<&str>,
        priority: Option<&str>,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<NotificationEventLogDbModel>>;
}

/// SQLx implementation of NotificationRepository.
pub struct SqlxNotificationRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxNotificationRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl NotificationRepository for SqlxNotificationRepository {
    async fn get_subscriptions_for_channels(
        &self,
        channel_ids: &[String],
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut subscriptions: HashMap<String, Vec<String>> = HashMap::new();
        let ids = super::unique_lookup_ids(channel_ids);
        for (id, event) in super::fetch_in_chunks::<(String, String)>(
            &self.pool,
            "SELECT channel_id, event_name FROM notification_subscription WHERE channel_id IN (",
            &ids,
            ") ORDER BY channel_id, event_name",
        )
        .await?
        {
            subscriptions.entry(id).or_default().push(event);
        }
        Ok(subscriptions)
    }

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
        writes::write_channel(
            &mut *self.write_pool.acquire().await?,
            channel,
            super::row_write::WriteMode::Insert,
            writes::RouteWrite::Write,
        )
        .await
    }

    async fn update_channel(&self, channel: &NotificationChannelDbModel) -> Result<()> {
        writes::write_channel(
            &mut *self.write_pool.acquire().await?,
            channel,
            super::row_write::WriteMode::Update,
            writes::RouteWrite::Write,
        )
        .await
    }

    async fn patch_channel(
        &self,
        id: &str,
        patch: ChannelPatch,
    ) -> Result<NotificationChannelDbModel> {
        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
        let channel = writes::patch_channel(&mut tx, id, patch).await?;
        tx.commit().await?;
        Ok(channel)
    }

    async fn delete_channel(&self, id: &str) -> Result<()> {
        crate::database::retry::retry_on_sqlite_busy("delete_notification_channel", || async {
            let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM notification_channel WHERE id = ?)",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if !exists {
                return Err(Error::not_found("NotificationChannel", id));
            }
            writes::delete_subscriptions(&mut tx, id).await?;
            writes::delete_channel_dead_letters(&mut tx, id).await?;
            writes::delete_channel_row(&mut tx, id).await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn replace_subscriptions(&self, channel_id: &str, events: &[String]) -> Result<()> {
        crate::database::retry::retry_on_sqlite_busy(
            "replace_notification_subscriptions",
            || async {
                let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM notification_channel WHERE id = ?)",
                )
                .bind(channel_id)
                .fetch_one(&mut *tx)
                .await?;
                if !exists {
                    return Err(Error::not_found("NotificationChannel", channel_id));
                }
                writes::delete_subscriptions(&mut tx, channel_id).await?;
                for event in events {
                    writes::insert_subscription(
                        &mut tx,
                        channel_id,
                        event,
                        writes::SubscriptionInsert::IgnoreDuplicate,
                    )
                    .await?;
                }
                tx.commit().await?;
                Ok(())
            },
        )
        .await
    }

    async fn default_web_push_user_id(&self) -> Result<Option<String>> {
        let user_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM users \
             ORDER BY \
                 CASE \
                     WHEN id = 'default-admin-00000000-0000-0000-0000-000000000001' THEN 0 \
                     WHEN is_active = 1 AND roles LIKE '%\"admin\"%' THEN 1 \
                     ELSE 2 \
                 END, \
                 created_at ASC \
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(user_id)
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

    async fn subscribe(&self, channel_id: &str, event_name: &str) -> Result<()> {
        writes::insert_subscription(
            &mut *self.write_pool.acquire().await?,
            channel_id,
            event_name,
            writes::SubscriptionInsert::IgnoreDuplicate,
        )
        .await?;
        Ok(())
    }

    async fn unsubscribe(&self, channel_id: &str, event_name: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM notification_subscription WHERE channel_id = ? AND event_name = ?",
        )
        .bind(channel_id)
        .bind(event_name)
        .execute(&self.write_pool)
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
        .bind(entry.first_attempt_at)
        .bind(entry.last_attempt_at)
        .bind(entry.created_at)
        .execute(&self.write_pool)
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

    async fn add_event_log(&self, entry: &NotificationEventLogDbModel) -> Result<()> {
        // Resolve the foreign-key column against `streamers` atomically so
        // replaying an event after a streamer row is physically deleted stores
        // NULL in the FK column while preserving the original IDs in `payload`.
        sqlx::query(
            r#"
            INSERT INTO notification_event_log (
                id, event_type, priority, payload, streamer_id, created_at
            ) VALUES (
                ?, ?, ?, ?,
                (SELECT id FROM streamers WHERE id = ?),
                ?
            )
            "#,
        )
        .bind(&entry.id)
        .bind(&entry.event_type)
        .bind(entry.priority)
        .bind(&entry.payload)
        .bind(&entry.streamer_id)
        .bind(entry.created_at)
        .execute(&self.write_pool)
        .await?;
        Ok(())
    }

    async fn list_event_logs(
        &self,
        event_type: Option<&str>,
        streamer_id: Option<&str>,
        search: Option<&str>,
        priority: Option<&str>,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<NotificationEventLogDbModel>> {
        let limit = limit.clamp(1, 1000);
        let offset = offset.max(0);

        let mut query = QueryBuilder::new("SELECT nel.* FROM notification_event_log nel");
        if search.is_some() {
            query.push(" LEFT JOIN streamers s ON nel.streamer_id = s.id");
        }
        let mut separator = " WHERE ";
        let mut next_separator = || std::mem::replace(&mut separator, " AND ");

        if let Some(search) = search {
            let pattern = super::literal_substring_pattern(search);
            query
                .push(next_separator())
                .push("(LOWER(s.name) LIKE LOWER(")
                .push_bind(pattern.clone())
                .push(r") ESCAPE '\' OR LOWER(nel.payload) LIKE LOWER(")
                .push_bind(pattern)
                .push(r") ESCAPE '\')");
        }
        if let Some(event_type) = event_type {
            query
                .push(next_separator())
                .push("nel.event_type = ")
                .push_bind(event_type);
        }
        if let Some(streamer_id) = streamer_id {
            query
                .push(next_separator())
                .push("nel.streamer_id = ")
                .push_bind(streamer_id);
        }
        if let Some(p) = priority {
            // Accept both integer (new format) and string (legacy format).
            let priority_value: i32 = if let Ok(int_val) = p.parse::<i32>() {
                int_val
            } else {
                match p.trim() {
                    s if s.eq_ignore_ascii_case("low") => 2,
                    s if s.eq_ignore_ascii_case("normal") => 5,
                    s if s.eq_ignore_ascii_case("high") => 8,
                    s if s.eq_ignore_ascii_case("critical") => 10,
                    _ => 2,
                }
            };
            query
                .push(next_separator())
                .push("nel.priority >= ")
                .push_bind(priority_value);
        }
        query
            .push(" ORDER BY nel.created_at DESC LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);

        let entries = query
            .build_query_as::<NotificationEventLogDbModel>()
            .fetch_all(&self.pool)
            .await?;
        Ok(entries)
    }
}
