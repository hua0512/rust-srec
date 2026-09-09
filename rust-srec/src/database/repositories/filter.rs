//! Filter repository.

mod writes;
pub(crate) use writes::{delete_for_streamer, import_filter};

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::FilterDbModel;
use crate::{Error, Result};

/// Filter repository trait.
#[async_trait]
pub trait FilterRepository: Send + Sync {
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
    async fn delete_filter(&self, id: &str) -> Result<()>;
    async fn delete_filters_for_streamer(&self, streamer_id: &str) -> Result<()>;

    /// Alias for get_filters_for_streamer.
    async fn get_by_streamer(&self, streamer_id: &str) -> Result<Vec<FilterDbModel>> {
        self.get_filters_for_streamer(streamer_id).await
    }
}

/// SQLx implementation of FilterRepository.
pub struct SqlxFilterRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxFilterRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl FilterRepository for SqlxFilterRepository {
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
        writes::write_filter(
            &mut *self.write_pool.acquire().await?,
            filter,
            super::row_write::WriteMode::Insert,
        )
        .await?;
        Ok(())
    }

    async fn update_filter(&self, filter: &FilterDbModel) -> Result<()> {
        writes::write_filter(
            &mut *self.write_pool.acquire().await?,
            filter,
            super::row_write::WriteMode::Update,
        )
        .await?;
        Ok(())
    }

    async fn delete_filter(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM filters WHERE id = ?")
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn delete_filters_for_streamer(&self, streamer_id: &str) -> Result<()> {
        writes::delete_for_streamer(&mut *self.write_pool.acquire().await?, streamer_id).await?;
        Ok(())
    }
}
