//! Streamer repository.

mod committed;
mod writes;
use crate::streamer::CommittedStreamerState;
use std::sync::Arc;
pub(crate) use writes::import_streamer_row;

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::StreamerDbModel;
use crate::{Error, Result};

use chrono::{DateTime, Utc};

/// Streamer repository trait.
#[async_trait]
pub trait StreamerRepository: Send + Sync {
    async fn create_streamer_for_manager(&self, row: &StreamerDbModel) -> Result<()> {
        self.create_streamer(row).await
    }
    async fn update_streamer_for_manager(&self, row: &StreamerDbModel) -> Result<()> {
        self.update_streamer(row).await
    }
    async fn patch_streamer_for_manager(
        &self,
        patch: crate::streamer::manager::StreamerUpdateParams,
    ) -> Result<StreamerDbModel> {
        self.patch_streamer(patch).await
    }

    fn committed_state(&self) -> Option<Arc<CommittedStreamerState>> {
        None
    }
    fn bind_committed_state(
        &self,
        _broadcaster: crate::config::ConfigEventBroadcaster,
    ) -> Option<Arc<CommittedStreamerState>> {
        self.committed_state()
    }
    async fn patch_streamer(
        &self,
        patch: crate::streamer::manager::StreamerUpdateParams,
    ) -> Result<StreamerDbModel> {
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
    async fn list_streamers_by_state(&self, state: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_priority(&self, priority: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_platform(&self, platform_id: &str) -> Result<Vec<StreamerDbModel>>;
    async fn list_streamers_by_template(&self, template_id: &str) -> Result<Vec<StreamerDbModel>>;
    async fn create_streamer(&self, streamer: &StreamerDbModel) -> Result<()>;
    async fn update_streamer(&self, streamer: &StreamerDbModel) -> Result<()>;
    async fn update_streamer_state(&self, id: &str, state: &str) -> Result<()>;
    async fn update_streamer_priority(&self, id: &str, priority: &str) -> Result<()>;
    async fn increment_error_count(&self, id: &str) -> Result<i32>;
    async fn reset_error_count(&self, id: &str) -> Result<()>;
    async fn set_disabled_until(&self, id: &str, until: Option<i64>) -> Result<()>;
    async fn update_last_live_time(&self, id: &str, time: i64) -> Result<()>;
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
    async fn record_streamer_success(
        &self,
        id: &str,
        last_live_time: Option<DateTime<Utc>>,
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
pub(crate) async fn mark_streamer_deleted<'e, E>(
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

pub(crate) async fn mark_streamer_deleted_row<'e, E>(
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

/// SQLx implementation of StreamerRepository.
pub struct SqlxStreamerRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
    committed_state: std::sync::OnceLock<Arc<CommittedStreamerState>>,
}

impl SqlxStreamerRepository {
    fn mutation_owner(&self) -> Result<Option<Arc<CommittedStreamerState>>> {
        let state = self.committed_state.get().cloned();
        if let Some(state) = &state {
            state.writer.require_same_pool(&self.write_pool)?;
        }
        Ok(state)
    }

    pub(crate) fn with_committed_state(self, state: Arc<CommittedStreamerState>) -> Self {
        self.committed_state.get_or_init(|| state);
        self
    }
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self {
            pool,
            write_pool,
            committed_state: std::sync::OnceLock::new(),
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
    async fn update_streamer_for_manager(&self, row: &StreamerDbModel) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write_for_manager(
                store,
                row.id.clone(),
                committed::Mutation::Update(row.clone()),
            )
            .await
            .map(|_| ());
        }
        self.update_streamer(row).await
    }
    async fn patch_streamer_for_manager(
        &self,
        patch: crate::streamer::manager::StreamerUpdateParams,
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

    fn committed_state(&self) -> Option<Arc<CommittedStreamerState>> {
        self.committed_state.get().cloned()
    }
    fn bind_committed_state(
        &self,
        broadcaster: crate::config::ConfigEventBroadcaster,
    ) -> Option<Arc<CommittedStreamerState>> {
        Some(
            self.committed_state
                .get_or_init(|| {
                    Arc::new(CommittedStreamerState::new(
                        Arc::new(crate::database::CommittedWriter::for_invalidation(
                            self.write_pool.clone(),
                            Arc::new(crate::utils::task_supervisor::TaskSupervisor::new()),
                        )),
                        broadcaster,
                    ))
                })
                .clone(),
        )
    }
    async fn patch_streamer(
        &self,
        patch: crate::streamer::manager::StreamerUpdateParams,
    ) -> Result<StreamerDbModel> {
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

        let mut streamers = Vec::new();
        let ids = super::unique_lookup_ids(ids);
        for ids in ids.chunks(super::LOOKUP_BATCH_SIZE) {
            let mut builder =
                sqlx::QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM streamers WHERE id IN (");
            let mut separated = builder.separated(", ");
            for id in ids {
                separated.push_bind(*id);
            }
            separated.push_unseparated(")");

            streamers.extend(
                builder
                    .build_query_as::<StreamerDbModel>()
                    .fetch_all(&self.pool)
                    .await?,
            );
        }
        Ok(streamers)
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

    async fn list_streamers_by_state(&self, state: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE state = ? AND deleted_at IS NULL ORDER BY priority DESC, name",
        )
        .bind(state)
        .fetch_all(&self.pool)
        .await?;
        Ok(streamers)
    }

    async fn list_streamers_by_priority(&self, priority: &str) -> Result<Vec<StreamerDbModel>> {
        let streamers = sqlx::query_as::<_, StreamerDbModel>(
            "SELECT * FROM streamers WHERE priority = ? AND deleted_at IS NULL ORDER BY name",
        )
        .bind(priority)
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

        let result = writes::write_streamer(
            &mut *self.write_pool.acquire().await?,
            streamer,
            super::row_write::WriteMode::Insert,
            streamer.updated_at,
        )
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(Error::duplicate_url(&streamer.url))
            }
            Err(e) => Err(e.into()),
        }
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

        let result = writes::write_streamer(
            &mut *self.write_pool.acquire().await?,
            streamer,
            super::row_write::WriteMode::Update,
            streamer.updated_at,
        )
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                Err(Error::duplicate_url(&streamer.url))
            }
            Err(e) => Err(e.into()),
        }
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

    async fn increment_error_count(&self, id: &str) -> Result<i32> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::IncrementErrors,
            )
            .await
            .map(|row| row.and_then(|row| row.consecutive_error_count).unwrap_or(0));
        }

        let count = sqlx::query_scalar(
            "UPDATE streamers SET consecutive_error_count = COALESCE(consecutive_error_count, 0) + 1 WHERE id = ? RETURNING consecutive_error_count",
        )
        .bind(id)
        .fetch_one(&self.write_pool)
        .await?;

        Ok(count)
    }

    async fn reset_error_count(&self, id: &str) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::ResetErrors,
            )
            .await
            .map(|_| ());
        }

        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL WHERE id = ?",
        )
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

    async fn update_last_live_time(&self, id: &str, time: i64) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::LastLive(time),
            )
            .await
            .map(|_| ());
        }

        sqlx::query("UPDATE streamers SET last_live_time = ? WHERE id = ?")
            .bind(time)
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
                committed::Mutation::MarkDeleted(crate::database::time::now_ms()),
            )
            .await
            .map(|row| row.is_some());
        }

        let mut conn = self.write_pool.acquire().await?;
        Ok(mark_streamer_deleted(&mut *conn, id, crate::database::time::now_ms()).await?)
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

        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
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
            "UPDATE streamers SET consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, state = 'NOT_LIVE' WHERE id = ?",
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
        last_live_time: Option<DateTime<Utc>>,
    ) -> Result<()> {
        if let Some(store) = self.mutation_owner()? {
            return committed::write(
                store.clone(),
                id.to_owned(),
                committed::Mutation::Success(last_live_time.map(|time| time.timestamp_millis())),
            )
            .await
            .map(|_| ());
        }

        if let Some(time) = last_live_time {
            let time_ms = time.timestamp_millis();
            sqlx::query(
                "UPDATE streamers SET state = 'LIVE', consecutive_error_count = 0, disabled_until = NULL, last_error = NULL, last_live_time = ? WHERE id = ?",
            )
            .bind(time_ms)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        } else {
            sqlx::query(
                "UPDATE streamers SET state = 'NOT_LIVE', consecutive_error_count = 0, disabled_until = NULL, last_error = NULL WHERE id = ?",
            )
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        }
        Ok(())
    }
}
