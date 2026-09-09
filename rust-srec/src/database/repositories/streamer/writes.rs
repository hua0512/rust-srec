use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::StreamerDbModel;

pub(crate) async fn write_streamer(
    connection: &mut sqlx::SqliteConnection,
    model: &StreamerDbModel,
    mode: WriteMode,
    updated_at: i64,
) -> Result<Option<StreamerDbModel>, sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("streamers", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field("url", &model.url, true)?;
    row.field("platform_config_id", &model.platform_config_id, true)?;
    row.field("template_config_id", &model.template_config_id, true)?;
    row.field("state", &model.state, true)?;
    row.field("priority", &model.priority, true)?;
    row.field("avatar", &model.avatar, true)?;
    row.field("last_live_time", model.last_live_time, true)?;
    row.field(
        "streamer_specific_config",
        &model.streamer_specific_config,
        true,
    )?;
    row.field(
        "consecutive_error_count",
        model.consecutive_error_count,
        true,
    )?;
    row.field("disabled_until", model.disabled_until, true)?;
    row.field("last_error", &model.last_error, true)?;
    row.field("created_at", model.created_at, false)?;
    row.field("updated_at", updated_at, true)?;
    row.fetch_optional(connection).await
}

pub(crate) async fn import_streamer_row(
    connection: &mut sqlx::SqliteConnection,
    model: &StreamerDbModel,
) -> Result<Option<StreamerDbModel>, sqlx::Error> {
    write_streamer(connection, model, WriteMode::Import, model.updated_at).await
}
