//! Streamer repository.

mod committed;
mod writes;
use std::any::Any;
use std::sync::Arc;
pub use writes::import_streamer_row;

use async_trait::async_trait;
use futures::future::BoxFuture;
use jiff::Timestamp;
use sqlx::{SqliteConnection, SqlitePool};

use crate::models::StreamerDbModel;
use crate::repositories::MonitorOutboxTxOps;
use crate::{DbError as Error, DbResult as Result};
use srec_core::domain::streamer::StreamerUpdateParams;

/// How a committed streamer write publishes changes to runtime caches and listeners.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum StatePublication {
    Silent,
    StateOnly,
    Metadata,
    Manager,
    Deleted,
}

/// Rows and invalidations produced inside a committed streamer transaction.
#[doc(hidden)]
pub struct StateChange<T> {
    pub value: T,
    pub rows: Vec<StreamerDbModel>,
    pub removed: Vec<String>,
    /// Streamers whose account selection changed. The selection is stored
    /// outside the row, so an identical row does not mean an unchanged
    /// configuration.
    pub reconfigured: Vec<String>,
}

impl<T> StateChange<T> {
    pub fn row(value: T, row: Option<StreamerDbModel>) -> Self {
        Self {
            value,
            rows: row.into_iter().collect(),
            removed: Vec::new(),
            reconfigured: Vec::new(),
        }
    }
}

#[doc(hidden)]
pub type StreamerTxOp = Box<
    dyn for<'c> FnOnce(
            &'c mut SqliteConnection,
        ) -> BoxFuture<'c, Result<StateChange<Option<StreamerDbModel>>>>
        + Send,
>;

/// Narrow port implemented by the runtime streamer state store so
/// [`SqlxStreamerRepository`] can serialize and publish writes without
/// depending on outer runtime modules.
#[doc(hidden)]
#[async_trait]
pub trait StreamerCommitOwner: Send + Sync {
    fn require_same_pool(&self, pool: &SqlitePool) -> Result<()>;

    async fn commit_streamer_write(
        &self,
        label: &'static str,
        publication: StatePublication,
        operation: StreamerTxOp,
    ) -> Result<Option<StreamerDbModel>>;

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

/// Streamer repository trait.
#[async_trait]
pub trait StreamerRepository: Send + Sync {
    async fn create_streamer_for_manager(&self, row: &StreamerDbModel) -> Result<()> {
        self.create_streamer(row).await
    }
    async fn patch_streamer_for_manager(
        &self,
        patch: StreamerUpdateParams,
    ) -> Result<StreamerDbModel> {
        self.patch_streamer(patch).await
    }

    fn commit_owner(&self) -> Option<Arc<dyn StreamerCommitOwner>> {
        None
    }
    fn get_or_init_commit_owner(
        &self,
        _init: Box<dyn FnOnce(SqlitePool) -> Arc<dyn StreamerCommitOwner> + Send>,
    ) -> Option<Arc<dyn StreamerCommitOwner>> {
        self.commit_owner()
    }
    async fn patch_streamer(&self, patch: StreamerUpdateParams) -> Result<StreamerDbModel> {
        let mut model = self.get_streamer(&patch.id).await?;
        committed::apply_patch(&mut model, patch);
        self.update_streamer(&model).await?;
        Ok(model)
    }
    async fn get_streamer(&self, id: &str) -> Result<StreamerDbModel>;
    async fn get_streamers_by_ids(&self, ids: &[String]) -> Result<Vec<StreamerDbModel>> {
        let mut streamers = Vec::new();
        for id in ids {
            streamers.push(self.get_streamer(id).await?);
        }
        Ok(streamers)
    }
    async fn get_streamer_by_url(&self, url: &str) -> Result<StreamerDbModel>;
    async fn list_streamers(&self) -> Result<Vec<StreamerDbModel>>;
    /// Every row, including ones carrying a `deleted_at` marker.
    ///
    /// `StreamerManager::hydrate` needs the marked rows: their cached metadata
    /// is what makes `StreamerMetadata::is_active` false after a restart, so no
    /// actor is spawned for a streamer the reaper is still retiring.
    async fn list_all_streamers(&self) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_platform(&self, platform_id: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_template(&self, template_id: &str) -> Result<Vec<StreamerDbModel>>;
    async fn create_streamer(&self, streamer: &StreamerDbModel) -> Result<()>;
    async fn update_streamer(&self, streamer: &StreamerDbModel) -> Result<()>;
    async fn update_streamer_state(&self, id: &str, state: &str) -> Result<()>;
    async fn update_streamer_priority(&self, id: &str, priority: &str) -> Result<()>;
    async fn set_disabled_until(&self, id: &str, until: Option<i64>) -> Result<()>;
    async fn update_avatar(&self, id: &str, avatar_url: Option<&str>) -> Result<()>;

    /// Stamp `streamers.deleted_at` so every runtime owner stands down.
    ///
    /// `Ok(false)` when the row is missing or already marked, which callers read
    /// as "somebody else already started retiring this streamer".
    async fn mark_streamer_deleted(&self, id: &str) -> Result<bool>;

    /// Physically remove a marked row.
    ///
    /// Refuses unmarked rows so the reaper can only ever finish a deletion some
    /// caller committed through [`Self::mark_streamer_deleted`]; `Ok(false)`
    /// means the row was already gone or was never marked.
    async fn delete_marked_streamer(&self, id: &str) -> Result<bool>;

    // Methods for StreamerManager
    async fn clear_streamer_error_state(&self, id: &str) -> Result<()>;
    async fn clear_streamer_last_error(&self, id: &str) -> Result<()>;
    /// Clears the error state after a success. With a threshold, a cleared
    /// run that reached it also enqueues a `ChecksRestored` outbox row in the
    /// same write.
    async fn record_streamer_success(
        &self,
        id: &str,
        last_live_time: Option<Timestamp>,
        checks_restored_threshold: Option<i32>,
    ) -> Result<()>;
}

/// Stamp `streamers.deleted_at` for `id`, leaving an already-marked row alone.
///
/// Takes an executor rather than a pool so a caller that already owns a
/// transaction marks the row inside it: `ConfigurationImportService::import`
/// does exactly that, which is what makes a rejected import stop nothing.
///
/// `Ok(false)` means no row moved from unmarked to marked, so the caller is not
/// the one that owns this retirement.
pub async fn mark_streamer_deleted<'e, E>(
    executor: E,
    id: &str,
    now_ms: i64,
) -> std::result::Result<bool, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    Ok(mark_streamer_deleted_row(executor, id, now_ms)
        .await?
        .is_some())
}

pub async fn mark_streamer_deleted_row<'e, E>(
    executor: E,
    id: &str,
    now_ms: i64,
) -> std::result::Result<Option<StreamerDbModel>, sqlx::Error>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    sqlx::query_as(
        "UPDATE streamers SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL RETURNING *",
    )
    .bind(now_ms)
    .bind(id)
    .fetch_optional(executor)
    .await
}

fn duplicate_url(error: Error, url: &str) -> Error {
    match error {
        Error::DatabaseSqlx(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            Error::duplicate_url(url)
        }
        error => error,
    }
}

/// SQLx implementation of StreamerRepository.
pub struct SqlxStreamerRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
    commit_owner: std::sync::OnceLock<Arc<dyn StreamerCommitOwner>>,
}

impl SqlxStreamerRepository {
    fn mutation_owner(&self) -> Result<Option<Arc<dyn StreamerCommitOwner>>> {
        let state = self.commit_owner.get().cloned();
        if let Some(state) = &state {
            state.require_same_pool(&self.write_pool)?;
        }
        Ok(state)
    }

    pub fn with_commit_owner(self, owner: Arc<dyn StreamerCommitOwner>) -> Self {
        self.commit_owner.get_or_init(|| owner);
        self
    }
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self {
            pool,
            write_pool,
            commit_owner: std::sync::OnceLock::new(),
        }
    }
}

#[async_trait]
impl StreamerRepository for SqlxStreamerRepository {
    async fn create_streamer_for_manager(&self, row: &StreamerDbModel) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write_for_manager(
                store,
                row.id.clone(),
                committed::Mutation::Insert(row.clone()),
            )
            .await
            .map(|_| ());
        }
        self.create_streamer(row).await
    }
    async fn patch_streamer_for_manager(
        &self,
        patch: StreamerUpdateParams,
    ) -> Result<StreamerDbModel> {
        if let Some(store) = self.mutation_owner()? {
            let id = patch.id.clone();
            return committed::write_for_manager(
                store,
                id.clone(),
                committed::Mutation::Patch(patch),
            )
            .await?
            .ok_or_else(|| Error::not_found("Streamer", id));
        }
        self.patch_streamer(patch).await
    }

    fn commit_owner(&self) -> Option<Arc<dyn StreamerCommitOwner>> {
        self.commit_owner.get().cloned()
    }
    fn get_or_init_commit_owner(
        &self,
        init: Box<dyn FnOnce(SqlitePool) -> Arc<dyn StreamerCommitOwner> + Send>,
    ) -> Option<Arc<dyn StreamerCommitOwner>> {
        Some(
            self.commit_owner
                .get_or_init(|| init(self.write_pool.clone()))
                .clone(),
        )
    }
    async fn patch_streamer(&self, patch: StreamerUpdateParams) -> Result<StreamerDbModel> {
        if let Some(store) = self.mutation_owner()? {
            let id = patch.id.clone();
            return committed::write(store.clone(), id.clone(), committed::Mutation::Patch(patch))
                .await?
                .ok_or_else(|| Error::not_found("Streamer", id));
        }
        let mut model = self.get_streamer(&patch.id).await?;
        committed::apply_patch(&mut model, patch);
        self.update_streamer(&model).await?;
        Ok(model)
    }
    async fn get_streamer(&self, id: &str) -> Result<StreamerDbModel> {
        sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("Streamer", id))
    }

    async fn get_streamers_by_ids(&self, ids: &[String]) -> Result<Vec<StreamerDbModel>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        super::fetch_in_chunks(
            &self.pool,
            "SELECT * FROM streamers WHERE id IN (",
            &super::unique_lookup_ids(ids),
            ")",
        )
        .await
    }

    async fn get_streamer_by_url(&self, url: &str) -> Result<StreamerDbModel> {
        sqlx::query_as::<_, StreamerDbModel>("SELECT * FROM streamers WHERE url = ?")
            .bind(url)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("Streamer", url))
    }

    async fn list_streamers(&self) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE deleted_at IS NULL ORDER BY priority DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_platform(&self, platform_id: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE platform_config_id = ? AND deleted_at IS NULL ORDER BY priority DESC, name",
        )
        .bind(platform_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_template(&self, template_id: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE template_config_id = ? AND deleted_at IS NULL ORDER BY priority DESC, name",
        )
        .bind(template_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn create_streamer(&self, streamer: &StreamerDbModel) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                streamer.id.clone(),
                committed::Mutation::Insert(streamer.clone()),
            )
            .await
            .map(|_| ());
        }

        let mut transaction = crate::begin_immediate(&self.write_pool).await?;
        writes::write_streamer(
            &mut transaction,
            streamer,
            super::row_write::WriteMode::Insert,
            streamer.updated_at,
        )
        .await
        .map_err(|error| duplicate_url(error, &streamer.url))?;
        transaction.commit().await?;
        Ok(())
    }

    async fn update_streamer(&self, streamer: &StreamerDbModel) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                streamer.id.clone(),
                committed::Mutation::Update(streamer.clone()),
            )
            .await
            .map(|_| ());
        }

        let mut transaction = crate::begin_immediate(&self.write_pool).await?;
        // Update never creates: a missing streamer is a zero-row update.
        writes::write_streamer(
            &mut transaction,
            streamer,
            super::row_write::WriteMode::Update,
            streamer.updated_at,
        )
        .await
        .map_err(|error| duplicate_url(error, &streamer.url))?;
        transaction.commit().await?;
        Ok(())
    }

    async fn update_streamer_state(&self, id: &str, state: &str) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::State(state.to_owned()),
            )
            .await
            .map(|_| ());
        }

        sqlx::query("UPDATE streamers SET state = ? WHERE id = ?")
            .bind(state)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn update_streamer_priority(&self, id: &str, priority: &str) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::Priority(priority.to_owned()),
            )
            .await
            .map(|_| ());
        }

        sqlx::query("UPDATE streamers SET priority = ? WHERE id = ?")
            .bind(priority)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn set_disabled_until(&self, id: &str, until: Option<i64>) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::DisabledUntil(until),
            )
            .await
            .map(|_| ());
        }

        sqlx::query("UPDATE streamers SET disabled_until = ? WHERE id = ?")
            .bind(until)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn update_avatar(&self, id: &str, avatar_url: Option<&str>) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::Avatar(avatar_url.map(str::to_owned)),
            )
            .await
            .map(|_| ());
        }

        sqlx::query("UPDATE streamers SET avatar = ? WHERE id = ?")
            .bind(avatar_url)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn mark_streamer_deleted(&self, id: &str) -> Result<bool> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::MarkDeleted(crate::time::now_ms()),
            )
            .await
            .map(|row| row.is_some());
        }

        let mut conn = self.write_pool.acquire().await?;
        Ok(mark_streamer_deleted(&mut *conn, id, crate::time::now_ms()).await?)
    }

    async fn delete_marked_streamer(&self, id: &str) -> Result<bool> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::DeleteMarked,
            )
            .await
            .map(|row| row.is_some());
        }

        let mut tx = crate::begin_immediate(&self.write_pool).await?;
        let result = sqlx::query("DELETE FROM streamers WHERE id = ? AND deleted_at IS NOT NULL")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        super::config_retirement::reap(&mut tx).await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_all_streamers(&self) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers ORDER BY priority DESC, name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn clear_streamer_error_state(&self, id: &str) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::ClearErrors,
            )
            .await
            .map(|_| ());
        }

        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, state = CASE WHEN state IN ('FATAL_ERROR', 'OUT_OF_SPACE', 'NOT_FOUND', 'TEMPORAL_DISABLED', 'ERROR') THEN 'NOT_LIVE' ELSE state END WHERE id = ?",
        )
        .bind(id)
        .execute(&self.write_pool)
        .await?;
        Ok(())
    }

    async fn clear_streamer_last_error(&self, id: &str) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::ClearLastError,
            )
            .await
            .map(|_| ());
        }

        sqlx::query("UPDATE streamers SET last_error = NULL WHERE id = ?")
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn record_streamer_success(
        &self,
        id: &str,
        last_live_time: Option<Timestamp>,
        checks_restored_threshold: Option<i32>,
    ) -> Result<()> {
        let last_live_time = last_live_time.map(|time| time.as_millisecond());
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::Success(last_live_time, checks_restored_threshold),
            )
            .await
            .map(|_| ());
        }

        let mut tx = self.write_pool.begin().await?;
        if let Some(threshold) = checks_restored_threshold {
            MonitorOutboxTxOps::enqueue_checks_restored(&mut tx, id, threshold, Timestamp::now())
                .await?;
        }
        if let Some(time_ms) = last_live_time {
            sqlx::query(
                "UPDATE streamers SET state = 'LIVE', consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, last_live_time = ? WHERE id = ?",
            )
            .bind(time_ms)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE streamers SET state = 'NOT_LIVE', consecutive_error_count = 0, disabled_until = NULL, last_error = NULL WHERE id = ?",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
