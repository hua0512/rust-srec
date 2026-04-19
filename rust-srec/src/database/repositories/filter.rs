//! Filter repository.

use sqlx::SqlitePool;

use crate::database::models::FilterDbModel;
use crate::{Error, Result};

/// Filter repository trait.
#[dynosaur::dynosaur(pub DynFilterRepository = dyn(box) FilterRepository)]
pub trait FilterRepository: Send + Sync {
    fn get_filter(
        &self,
        id: &str,
    ) -> impl std::future::Future<Output = Result<FilterDbModel>> + Send;
    fn get_filters_for_streamer(
        &self,
        streamer_id: &str,
    ) -> impl std::future::Future<Output = Result<Vec<FilterDbModel>>> + Send;
    fn create_filter(
        &self,
        filter: &FilterDbModel,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
    fn update_filter(
        &self,
        filter: &FilterDbModel,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
    fn delete_filter(&self, id: &str) -> impl std::future::Future<Output = Result<()>> + Send;
    fn delete_filters_for_streamer(
        &self,
        streamer_id: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Alias for get_filters_for_streamer.
    fn get_by_streamer(
        &self,
        streamer_id: &str,
    ) -> impl std::future::Future<Output = Result<Vec<FilterDbModel>>> + Send {
        async move { self.get_filters_for_streamer(streamer_id).await }
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

impl FilterRepository for SqlxFilterRepository {
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
        sqlx::query(
            r#"
            INSERT INTO filters (id, streamer_id, filter_type, config)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(&filter.id)
        .bind(&filter.streamer_id)
        .bind(&filter.filter_type)
        .bind(&filter.config)
        .execute(&self.write_pool)
        .await?;
        Ok(())
    }

    async fn update_filter(&self, filter: &FilterDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE filters SET
                streamer_id = ?,
                filter_type = ?,
                config = ?
            WHERE id = ?
            "#,
        )
        .bind(&filter.streamer_id)
        .bind(&filter.filter_type)
        .bind(&filter.config)
        .bind(&filter.id)
        .execute(&self.write_pool)
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
        sqlx::query("DELETE FROM filters WHERE streamer_id = ?")
            .bind(streamer_id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }
}
