use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::{ChannelType, NotificationChannelDbModel};
use crate::domain::proxy::{ProxyError, ProxyRoute};

/// Whether a channel write stores the model's route or leaves the stored
/// one, so an edit of other fields cannot revert a concurrent route change.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RouteWrite {
    Write,
    Keep,
}

/// An edit of a stored channel. `None` keeps the stored route.
pub struct ChannelPatch {
    pub name: String,
    pub settings: String,
    pub proxy_route: Option<ProxyRoute>,
}

/// Writes a channel, with its route unless `route` keeps the stored one. The
/// type is stored in its canonical spelling. Only HTTP channels can take a
/// route other than `inherit`, and a route must name an existing saved
/// proxy. Inserts and imports always write the route.
pub(crate) async fn write_channel(
    connection: &mut sqlx::SqliteConnection,
    model: &NotificationChannelDbModel,
    mode: WriteMode,
    route: RouteWrite,
) -> crate::Result<()> {
    channel_row(model, mode, route)?
        .execute(connection)
        .await
        .map_err(|error| channel_write_error(model, error))
}

/// The statement [`write_channel`] runs, once the rules are checked.
fn channel_row(
    model: &NotificationChannelDbModel,
    mode: WriteMode,
    route: RouteWrite,
) -> crate::Result<RowWrite> {
    let channel_type = ChannelType::parse(&model.channel_type);
    let write_route = route == RouteWrite::Write || mode != WriteMode::Update;
    if write_route
        && !model.proxy_route.is_inherit()
        && !channel_type.is_some_and(|kind| kind.supports_proxy())
    {
        return Err(crate::Error::validation(
            "only Webhook, Telegram, Gotify and Discord channels can use a proxy route; email always connects directly",
        ));
    }
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("notification_channel", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field(
        "channel_type",
        channel_type.map_or(model.channel_type.as_str(), |kind| kind.as_str()),
        true,
    )?;
    row.field("settings", &model.settings, true)?;
    if write_route {
        row.field("proxy_route", model.proxy_route.kind(), true)?;
        row.field("proxy_id", model.proxy_route.proxy_id(), true)?;
    }
    Ok(row)
}

fn channel_write_error(model: &NotificationChannelDbModel, error: sqlx::Error) -> crate::Error {
    match error {
        // The saved proxy is the channel's only foreign key.
        sqlx::Error::Database(error) if error.is_foreign_key_violation() => {
            match model.proxy_route.proxy_id() {
                Some(id) => ProxyError::Missing(id.to_owned()).into(),
                None => sqlx::Error::Database(error).into(),
            }
        }
        error => error.into(),
    }
}

/// Applies `patch` to the stored channel `id` and returns it as written.
/// The caller's transaction covers the read and the write, so the rules
/// are checked against the row being updated, and a failed commit cannot
/// leave the returned row unwritten.
pub(crate) async fn patch_channel(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
    patch: ChannelPatch,
) -> crate::Result<NotificationChannelDbModel> {
    let not_found = || crate::Error::not_found("NotificationChannel", id);
    let mut model = sqlx::query_as::<_, NotificationChannelDbModel>(
        "SELECT * FROM notification_channel WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(not_found)?;
    model.name = patch.name;
    model.settings = patch.settings;
    let route = match patch.proxy_route {
        Some(route) => {
            model.proxy_route = route;
            RouteWrite::Write
        }
        None => RouteWrite::Keep,
    };
    channel_row(&model, WriteMode::Update, route)?
        .fetch_optional(connection)
        .await
        .map_err(|error| channel_write_error(&model, error))?
        .ok_or_else(not_found)
}

pub(crate) async fn import_channel(
    connection: &mut sqlx::SqliteConnection,
    model: &NotificationChannelDbModel,
) -> crate::Result<()> {
    write_channel(connection, model, WriteMode::Import, RouteWrite::Write).await
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
