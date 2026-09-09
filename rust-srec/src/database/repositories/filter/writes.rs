use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::FilterDbModel;

pub(crate) async fn write_filter(
    connection: &mut sqlx::SqliteConnection,
    model: &FilterDbModel,
    mode: WriteMode,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Insert,
    };
    let mut row = RowWrite::new("filters", mutation, &model.id)?;
    row.field("streamer_id", &model.streamer_id, true)?;
    row.field("filter_type", &model.filter_type, true)?;
    row.field("config", &model.config, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_filter(
    connection: &mut sqlx::SqliteConnection,
    model: &FilterDbModel,
) -> Result<(), sqlx::Error> {
    write_filter(connection, model, WriteMode::Import).await
}

pub(crate) async fn delete_for_streamer(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM filters WHERE streamer_id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}
