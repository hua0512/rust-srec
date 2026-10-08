use super::super::credential_selections;
use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::StreamerDbModel;

/// A streamer row as written, and whether its account selection or proxy
/// route changed.
pub(crate) struct WrittenStreamer {
    pub row: Option<StreamerDbModel>,
    pub selection_changed: bool,
}

/// Writes the row and applies the selection and proxy route its specific
/// configuration carries; a configuration without one keeps the stored
/// value, so internal writes of a stored row never change either. A streamer
/// moved to another platform starts inheriting accounts there. Every streamer
/// write passes here, so configuration carrying authentication or the
/// replaced `proxy_config` is refused for all of them.
pub(crate) async fn write_streamer(
    connection: &mut sqlx::SqliteConnection,
    model: &StreamerDbModel,
    mode: WriteMode,
    updated_at: i64,
) -> crate::Result<WrittenStreamer> {
    super::super::credential_profiles::reject_streamer_authentication(connection, model).await?;
    super::super::proxies::reject_legacy_document(model.streamer_specific_config.as_deref())?;
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut streamer_specific_config = model.streamer_specific_config.clone();
    let selection = credential_selections::take_document(&mut streamer_specific_config)?;
    let route = super::super::proxies::take_document(&mut streamer_specific_config)?;
    let previous = match mode {
        WriteMode::Insert => None,
        WriteMode::Update | WriteMode::Import => {
            credential_selections::streamer_before_write(connection, &model.id).await?
        }
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
    row.field("streamer_specific_config", &streamer_specific_config, true)?;
    row.field(
        "consecutive_error_count",
        model.consecutive_error_count,
        true,
    )?;
    row.field("disabled_until", model.disabled_until, true)?;
    row.field("last_error", &model.last_error, true)?;
    row.field("created_at", model.created_at, false)?;
    row.field("updated_at", updated_at, true)?;
    let row: Option<StreamerDbModel> = row.fetch_optional(&mut *connection).await?;
    let selection_changed = match &row {
        Some(row) => {
            credential_selections::write_streamer(
                connection,
                &row.id,
                &row.platform_config_id,
                selection,
                previous,
            )
            .await?
        }
        None => false,
    };
    let route_changed = match (&row, route) {
        (Some(row), Some(route)) => {
            let owner = super::super::proxies::RouteOwner::Streamer(row.id.clone());
            let changed = super::super::proxies::route_of(connection, &owner).await? != route;
            super::super::proxies::set_route(connection, &owner, &route).await?;
            changed
        }
        _ => false,
    };
    Ok(WrittenStreamer {
        row,
        selection_changed: selection_changed || route_changed,
    })
}

pub(crate) async fn import_streamer_row(
    connection: &mut sqlx::SqliteConnection,
    model: &StreamerDbModel,
) -> crate::Result<Option<StreamerDbModel>> {
    Ok(
        write_streamer(connection, model, WriteMode::Import, model.updated_at)
            .await?
            .row,
    )
}
