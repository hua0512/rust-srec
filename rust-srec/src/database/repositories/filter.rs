//! Filter repository.

mod writes;
pub(crate) use writes::{delete_for_streamer, import_filter};

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use sqlx::{SqliteConnection, SqlitePool};

use crate::database::filter_store::FilterSnapshotCache;
use crate::database::models::FilterDbModel;
use crate::{Error, Result};

/// Synchronous publication for the distinct owners changed by a committed
/// mutation. The writer owns this hook through commit and cache invalidation,
/// even if the caller drops its request. Empty mutations never invoke it.
pub type FilterCommitHook = Box<dyn FnOnce(&[String]) + Send + 'static>;

/// Filter repository trait.
#[async_trait]
pub trait FilterRepository: Send + Sync {
    /// Associate snapshots with successful repository mutations. Custom readers
    /// may omit this and rely on store invalidation and its bounded TTL.
    fn filter_snapshot_cache(&self) -> Option<FilterSnapshotCache> {
        None
    }
    async fn get_filter(&self, id: &str) -> Result<FilterDbModel>;
    async fn get_filters_for_streamer(&self, streamer_id: &str) -> Result<Vec<FilterDbModel>>;
    /// Group filters by owner, retaining each owner's filter-type order.
    /// An absent map entry represents an owner with no filters.
    async fn get_filters_for_streamers(
        &self,
        streamer_ids: &[String],
    ) -> Result<HashMap<String, Vec<FilterDbModel>>> {
        let mut filters = HashMap::new();
        for id in streamer_ids {
            filters.insert(id.clone(), self.get_filters_for_streamer(id).await?);
        }
        Ok(filters)
    }
    async fn create_filter(&self, filter: &FilterDbModel) -> Result<()>;
    async fn update_filter(&self, filter: &FilterDbModel) -> Result<()>;
    /// Replace a row only if its owner, type and raw config still match the
    /// expected row. A missing or changed row returns false without invalidation
    /// or publication. Comparison, write and publication share the commit owner;
    /// custom repositories fail closed until they provide that guarantee.
    /// The replacement must retain the ID.
    async fn update_filter_if_current(
        &self,
        _expected: &FilterDbModel,
        _replacement: &FilterDbModel,
        _on_commit: FilterCommitHook,
    ) -> Result<bool> {
        Err(Error::config(
            "Filter repository does not support conditional updates",
        ))
    }
    async fn delete_filter(&self, id: &str) -> Result<()>;
    async fn delete_filters_for_streamer(&self, streamer_id: &str) -> Result<()>;

    /// These entrypoints require publication to share the mutation's commit
    /// owner. Custom repositories must not implement them as await-then-publish.
    async fn create_filter_with_commit_hook(
        &self,
        _filter: &FilterDbModel,
        _on_commit: FilterCommitHook,
    ) -> Result<()> {
        Err(Error::config(
            "Filter repository does not support owned commit hooks",
        ))
    }

    async fn update_filter_with_commit_hook(
        &self,
        _filter: &FilterDbModel,
        _on_commit: FilterCommitHook,
    ) -> Result<()> {
        Err(Error::config(
            "Filter repository does not support owned commit hooks",
        ))
    }

    async fn delete_filter_with_commit_hook(
        &self,
        _id: &str,
        _on_commit: FilterCommitHook,
    ) -> Result<()> {
        Err(Error::config(
            "Filter repository does not support owned commit hooks",
        ))
    }

    /// Alias for get_filters_for_streamer.
    async fn get_by_streamer(&self, streamer_id: &str) -> Result<Vec<FilterDbModel>> {
        self.get_filters_for_streamer(streamer_id).await
    }
}

/// SQLx implementation of FilterRepository.
pub struct SqlxFilterRepository {
    pool: SqlitePool,
    snapshots: FilterSnapshotCache,
    committed_writer: Arc<crate::database::CommittedWriter>,
}

impl SqlxFilterRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        let owner = Arc::new(crate::utils::task_supervisor::TaskSupervisor::new());
        let committed_writer = Arc::new(crate::database::CommittedWriter::for_invalidation(
            write_pool, owner,
        ));
        Self {
            pool,
            snapshots: FilterSnapshotCache::default(),
            committed_writer,
        }
    }

    pub(crate) fn with_committed_writer(
        mut self,
        writer: Arc<crate::database::CommittedWriter>,
    ) -> Self {
        self.committed_writer = writer;
        self
    }

    async fn mutate<F>(
        &self,
        label: &'static str,
        operation: F,
        on_commit: FilterCommitHook,
    ) -> Result<bool>
    where
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<Vec<String>>>
            + Send
            + 'static,
    {
        let cache = self.snapshots.clone();
        let publish = move |owners: &Vec<String>| {
            for id in owners {
                cache.invalidate(id);
            }
            if !owners.is_empty() {
                on_commit(owners);
            }
        };
        let owners = self
            .committed_writer
            .transaction(label, operation, publish)
            .await?;
        Ok(!owners.is_empty())
    }
}

#[async_trait]
impl FilterRepository for SqlxFilterRepository {
    fn filter_snapshot_cache(&self) -> Option<FilterSnapshotCache> {
        Some(self.snapshots.clone())
    }
    async fn get_filters_for_streamers(
        &self,
        streamer_ids: &[String],
    ) -> Result<HashMap<String, Vec<FilterDbModel>>> {
        let mut filters: HashMap<String, Vec<FilterDbModel>> = HashMap::new();
        let ids = super::unique_lookup_ids(streamer_ids);
        for ids in ids.chunks(super::LOOKUP_BATCH_SIZE) {
            let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                "SELECT * FROM filters WHERE streamer_id IN (",
            );
            let mut separated = query.separated(", ");
            for id in ids {
                separated.push_bind(*id);
            }
            separated.push_unseparated(") ORDER BY streamer_id, filter_type");
            for filter in query
                .build_query_as::<FilterDbModel>()
                .fetch_all(&self.pool)
                .await?
            {
                filters
                    .entry(filter.streamer_id.clone())
                    .or_default()
                    .push(filter);
            }
        }
        Ok(filters)
    }

    async fn get_filter(&self, id: &str) -> Result<FilterDbModel> {
        sqlx::query_as::<_, FilterDbModel>("SELECT * FROM filters WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("Filter", id))
    }

    async fn get_filters_for_streamer(&self, streamer_id: &str) -> Result<Vec<FilterDbModel>> {
        let filters = sqlx::query_as::<_, FilterDbModel>(
            "SELECT * FROM filters WHERE streamer_id = ? ORDER BY filter_type",
        )
        .bind(streamer_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(filters)
    }

    async fn create_filter(&self, filter: &FilterDbModel) -> Result<()> {
        self.create_filter_with_commit_hook(filter, Box::new(|_| {}))
            .await
    }

    async fn create_filter_with_commit_hook(
        &self,
        filter: &FilterDbModel,
        on_commit: FilterCommitHook,
    ) -> Result<()> {
        let filter = filter.clone();
        self.mutate(
            "create filter",
            move |connection| {
                Box::pin(async move {
                    writes::write_filter(connection, &filter, super::row_write::WriteMode::Insert)
                        .await?;
                    Ok(vec![filter.streamer_id])
                })
            },
            on_commit,
        )
        .await?;
        Ok(())
    }

    async fn update_filter(&self, filter: &FilterDbModel) -> Result<()> {
        self.update_filter_with_commit_hook(filter, Box::new(|_| {}))
            .await
    }

    async fn update_filter_with_commit_hook(
        &self,
        filter: &FilterDbModel,
        on_commit: FilterCommitHook,
    ) -> Result<()> {
        let filter = filter.clone();
        self.mutate(
            "update filter",
            move |connection| {
                Box::pin(async move {
                    let previous: Option<String> =
                        sqlx::query_scalar("SELECT streamer_id FROM filters WHERE id = ?")
                            .bind(&filter.id)
                            .fetch_optional(&mut *connection)
                            .await?;
                    let Some(previous) = previous else {
                        return Ok(Vec::new());
                    };
                    writes::write_filter(connection, &filter, super::row_write::WriteMode::Update)
                        .await?;
                    let mut owners = vec![previous];
                    if owners[0] != filter.streamer_id {
                        owners.push(filter.streamer_id);
                    }
                    Ok(owners)
                })
            },
            on_commit,
        )
        .await?;
        Ok(())
    }

    async fn update_filter_if_current(
        &self,
        expected: &FilterDbModel,
        replacement: &FilterDbModel,
        on_commit: FilterCommitHook,
    ) -> Result<bool> {
        if expected.id != replacement.id {
            return Err(Error::validation(
                "Conditional filter updates must retain the filter ID",
            ));
        }
        let expected = expected.clone();
        let replacement = replacement.clone();
        self.mutate(
            "conditionally update filter",
            move |connection| {
                Box::pin(async move {
                    // CommittedWriter holds BEGIN IMMEDIATE through both
                    // operations, excluding a writer between compare and save.
                    let matches: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM filters WHERE id = ? \
                         AND streamer_id = ? AND filter_type = ? AND config = ?)",
                    )
                    .bind(&expected.id)
                    .bind(&expected.streamer_id)
                    .bind(&expected.filter_type)
                    .bind(&expected.config)
                    .fetch_one(&mut *connection)
                    .await?;
                    if !matches {
                        return Ok(Vec::new());
                    }
                    writes::write_filter(
                        connection,
                        &replacement,
                        super::row_write::WriteMode::Update,
                    )
                    .await?;
                    let mut owners = vec![expected.streamer_id];
                    if owners[0] != replacement.streamer_id {
                        owners.push(replacement.streamer_id);
                    }
                    Ok(owners)
                })
            },
            on_commit,
        )
        .await
    }

    async fn delete_filter(&self, id: &str) -> Result<()> {
        self.delete_filter_with_commit_hook(id, Box::new(|_| {}))
            .await
    }

    async fn delete_filter_with_commit_hook(
        &self,
        id: &str,
        on_commit: FilterCommitHook,
    ) -> Result<()> {
        let id = id.to_owned();
        self.mutate(
            "delete filter",
            move |connection| {
                Box::pin(async move {
                    let owner: Option<String> = sqlx::query_scalar(
                        "DELETE FROM filters WHERE id = ? RETURNING streamer_id",
                    )
                    .bind(id)
                    .fetch_optional(connection)
                    .await?;
                    Ok(owner.into_iter().collect())
                })
            },
            on_commit,
        )
        .await?;
        Ok(())
    }

    async fn delete_filters_for_streamer(&self, streamer_id: &str) -> Result<()> {
        let id = streamer_id.to_owned();
        self.mutate(
            "delete streamer filters",
            move |connection| {
                Box::pin(async move {
                    writes::delete_for_streamer(connection, &id).await?;
                    Ok(vec![id])
                })
            },
            Box::new(|_| {}),
        )
        .await?;
        Ok(())
    }
}
