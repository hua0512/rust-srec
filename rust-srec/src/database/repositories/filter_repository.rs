use crate::database::models;
use crate::database::repositories::errors::{RepositoryError, RepositoryResult};
use crate::domain::types::Filter;
use sqlx::SqlitePool;

#[async_trait::async_trait]
pub trait FilterRepository: Send + Sync {
    async fn find_by_streamer_id(&self, streamer_id: &str) -> RepositoryResult<Vec<Filter>>;
}

pub struct SqliteFilterRepository {
    db: SqlitePool,
}

impl SqliteFilterRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl FilterRepository for SqliteFilterRepository {
    async fn find_by_streamer_id(&self, streamer_id: &str) -> RepositoryResult<Vec<Filter>> {
        let filters = sqlx::query_as!(
            models::Filter,
            r#"
            SELECT
                id as "id!",
                streamer_id as "streamer_id!",
                filter_type as "filter_type!",
                config as "config!"
            FROM filters
            WHERE streamer_id = ?
            "#,
            streamer_id
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(Filter::from)
        .collect();

        Ok(filters)
    }
}
