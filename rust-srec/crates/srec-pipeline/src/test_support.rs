//! Test fixtures shared with downstream integration and unit tests.

use std::sync::Arc;

/// A job repository over a fresh, migrated in-memory database with one connection.
pub async fn job_repository() -> Arc<crate::repositories::SqlxJobRepository> {
    let pool = srec_sqlite::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    srec_sqlite::run_migrations(&pool).await.unwrap();
    Arc::new(crate::repositories::SqlxJobRepository::new(
        pool.clone(),
        pool,
    ))
}
