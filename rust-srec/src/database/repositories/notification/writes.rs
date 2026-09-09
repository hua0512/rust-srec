use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::NotificationChannelDbModel;

pub(crate) async fn write_channel(
    connection: &mut sqlx::SqliteConnection,
    model: &NotificationChannelDbModel,
    mode: WriteMode,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("notification_channel", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field("channel_type", &model.channel_type, true)?;
    row.field("settings", &model.settings, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_channel(
    connection: &mut sqlx::SqliteConnection,
    model: &NotificationChannelDbModel,
) -> Result<(), sqlx::Error> {
    write_channel(connection, model, WriteMode::Import).await
}

#[derive(Clone, Copy)]
pub(crate) enum SubscriptionInsert {
    Strict,
    IgnoreDuplicate,
}

pub(crate) async fn delete_subscriptions(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM notification_subscription WHERE channel_id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn insert_subscription(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
    event: &str,
    policy: SubscriptionInsert,
) -> Result<(), sqlx::Error> {
    let sql = match policy {
        SubscriptionInsert::Strict => {
            "INSERT INTO notification_subscription (channel_id, event_name) VALUES (?, ?)"
        }
        SubscriptionInsert::IgnoreDuplicate => {
            "INSERT OR IGNORE INTO notification_subscription (channel_id, event_name) VALUES (?, ?)"
        }
    };
    sqlx::query(sql)
        .bind(id)
        .bind(event)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn delete_channel_row(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM notification_channel WHERE id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}

pub(crate) async fn delete_channel_dead_letters(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM notification_dead_letter WHERE channel_id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}
