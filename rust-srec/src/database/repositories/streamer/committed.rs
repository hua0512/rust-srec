use std::sync::Arc;

use crate::database::models::StreamerDbModel;
use crate::database::repositories::row_write::WriteMode;
use crate::streamer::CommittedStreamerState;
use crate::streamer::state_store::{StateChange, StatePublication};
use crate::{Error, Result};

pub(super) enum Mutation {
    Insert(StreamerDbModel),
    Update(StreamerDbModel),
    State(String),
    Priority(String),
    IncrementErrors,
    ResetErrors,
    DisabledUntil(Option<i64>),
    LastLive(i64),
    Avatar(Option<String>),
    MarkDeleted(i64),
    DeleteMarked,
    ClearErrors,
    ClearLastError,
    Success(Option<i64>),
    Patch(crate::streamer::manager::StreamerUpdateParams),
}

pub(super) async fn write(
    store: Arc<CommittedStreamerState>,
    id: String,
    mutation: Mutation,
) -> Result<Option<StreamerDbModel>> {
    let publication = match mutation {
        Mutation::MarkDeleted(_) => StatePublication::StateOnly,
        Mutation::DeleteMarked => StatePublication::Deleted,
        _ => StatePublication::Silent,
    };
    write_with_publication(store, id, mutation, publication).await
}

pub(super) async fn write_for_manager(
    store: Arc<CommittedStreamerState>,
    id: String,
    mutation: Mutation,
) -> Result<Option<StreamerDbModel>> {
    write_with_publication(store, id, mutation, StatePublication::Manager).await
}

async fn write_with_publication(
    store: Arc<CommittedStreamerState>,
    id: String,
    mutation: Mutation,
    publication: StatePublication,
) -> Result<Option<StreamerDbModel>> {
    store.transaction("write committed streamer", publication, move |connection| Box::pin(async move {
        macro_rules! row {
            ($sql:literal $(, $value:expr)*) => {
                sqlx::query_as::<_, StreamerDbModel>($sql)$(.bind($value))*.bind(&id).fetch_optional(&mut *connection).await?
            };
        }
        let deleted = matches!(mutation, Mutation::DeleteMarked);
        let row = match mutation {
            Mutation::Insert(model) => super::writes::write_streamer(connection, &model, WriteMode::Insert, model.updated_at).await.map_err(|error| translate(error, &model.url))?,
            Mutation::Update(model) => super::writes::write_streamer(connection, &model, WriteMode::Update, model.updated_at).await.map_err(|error| translate(error, &model.url))?,
            Mutation::State(state) => row!("UPDATE streamers SET state = ? WHERE id = ? RETURNING *", state),
            Mutation::Priority(priority) => row!("UPDATE streamers SET priority = ? WHERE id = ? RETURNING *", priority),
            Mutation::IncrementErrors => {
                let row = row!("UPDATE streamers SET consecutive_error_count = COALESCE(consecutive_error_count, 0) + 1 WHERE id = ? RETURNING *");
                if row.is_none() { return Err(Error::not_found("Streamer", &id)); }
                row
            }
            Mutation::ResetErrors => row!("UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL WHERE id = ? RETURNING *"),
            Mutation::DisabledUntil(until) => row!("UPDATE streamers SET disabled_until = ? WHERE id = ? RETURNING *", until),
            Mutation::LastLive(time) => row!("UPDATE streamers SET last_live_time = ? WHERE id = ? RETURNING *", time),
            Mutation::Avatar(avatar) => row!("UPDATE streamers SET avatar = ? WHERE id = ? RETURNING *", avatar),
            Mutation::MarkDeleted(time) => row!("UPDATE streamers SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL RETURNING *", time),
            Mutation::DeleteMarked => {
                let row = row!("DELETE FROM streamers WHERE id = ? AND deleted_at IS NOT NULL RETURNING *");
                crate::database::repositories::config_retirement::reap(connection).await?;
                row
            }
            Mutation::ClearErrors => row!("UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, state = 'NOT_LIVE' WHERE id = ? RETURNING *"),
            Mutation::ClearLastError => row!("UPDATE streamers SET last_error = NULL WHERE id = ? RETURNING *"),
            Mutation::Success(Some(time)) => row!("UPDATE streamers SET state = 'LIVE', consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, last_live_time = ? WHERE id = ? RETURNING *", time),
            Mutation::Success(None) => row!("UPDATE streamers SET state = 'NOT_LIVE', consecutive_error_count = 0, disabled_until = NULL, last_error = NULL WHERE id = ? RETURNING *"),
            Mutation::Patch(patch) => {
                let mut model = sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE id = ?").bind(&id).fetch_optional(&mut *connection).await?.ok_or_else(|| Error::not_found("Streamer", &id))?;
                apply_patch(&mut model, patch);
                super::writes::write_streamer(connection, &model, WriteMode::Update, model.updated_at).await.map_err(|error| translate(error, &model.url))?
            }
        };
        let mut change = StateChange::row(row.clone(), if deleted { None } else { row.clone() });
        if deleted && row.is_some() { change.removed.push(id); }
        Ok(change)
    })).await
}

pub(super) fn apply_patch(
    model: &mut StreamerDbModel,
    patch: crate::streamer::manager::StreamerUpdateParams,
) {
    if let Some(name) = patch.name {
        model.name = name;
    }
    if let Some(url) = patch.url {
        model.url = url;
    }
    if let Some(platform) = patch.platform_config_id {
        model.platform_config_id = platform;
    }
    if let Some(template) = patch.template_config_id {
        model.template_config_id = template;
    }
    if let Some(priority) = patch.priority {
        model.priority = priority.to_string();
    }
    if let Some(state) = patch.state {
        model.state = state.to_string();
        if state == crate::domain::StreamerState::Disabled {
            model.consecutive_error_count = Some(0);
            model.disabled_until = None;
            model.last_error = None;
        }
    }
    if let Some(config) = patch.streamer_specific_config {
        model.streamer_specific_config = config;
    }
    model.updated_at = crate::database::time::now_ms();
}

fn translate(error: sqlx::Error, url: &str) -> Error {
    match error {
        sqlx::Error::Database(error) if error.is_unique_violation() => Error::duplicate_url(url),
        error => error.into(),
    }
}
