//! Job Preset repository.

mod writes;
pub(crate) use writes::{import_job_preset, import_pipeline_preset};

use std::sync::Arc;

use crate::Result;
use crate::database::models::{JobPreset, Pagination};
use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

/// Append each optional job preset predicate with its bound value.
fn push_job_preset_filters(query: &mut QueryBuilder<Sqlite>, filters: &JobPresetFilters) {
    let mut separator = " WHERE ";
    let mut next_separator = || std::mem::replace(&mut separator, " AND ");

    if let Some(category) = &filters.category {
        query
            .push(next_separator())
            .push("category = ")
            .push_bind(category);
    }
    if let Some(processor) = &filters.processor {
        query
            .push(next_separator())
            .push("processor = ")
            .push_bind(processor);
    }
    if let Some(name) = &filters.name {
        query.push(next_separator()).push("name = ").push_bind(name);
    }
    push_search_filter(query, next_separator(), filters.search.as_deref());
}

/// Append a literal substring match on name or description when `search` is set.
fn push_search_filter(query: &mut QueryBuilder<Sqlite>, separator: &str, search: Option<&str>) {
    let Some(search) = search else {
        return;
    };
    let pattern = super::literal_substring_pattern(search);
    query
        .push(separator)
        .push("(name LIKE ")
        .push_bind(pattern.clone())
        .push(r" ESCAPE '\' OR description LIKE ")
        .push_bind(pattern)
        .push(r" ESCAPE '\')");
}

/// Filter parameters for job presets.
#[derive(Debug, Clone, Default)]
pub struct JobPresetFilters {
    /// Filter by category.
    pub category: Option<String>,
    /// Filter by processor type.
    pub processor: Option<String>,
    /// Exact preset name. `job_presets.name` is UNIQUE, so this matches at most one row —
    /// the same identity `get_preset_by_name` resolves a `PipelineStep::Preset` by.
    pub name: Option<String>,
    /// Search query (substring of name or description).
    pub search: Option<String>,
}

/// Repository for managing job presets.
#[async_trait]
pub trait JobPresetRepository: Send + Sync {
    /// Get a preset by ID.
    async fn get_preset(&self, id: &str) -> Result<Option<JobPreset>>;

    /// Get a preset by name.
    async fn get_preset_by_name(&self, name: &str) -> Result<Option<JobPreset>>;

    /// Check if a preset name exists (optionally excluding a specific ID).
    async fn name_exists(&self, name: &str, exclude_id: Option<&str>) -> Result<bool>;

    /// List all presets.
    async fn list_presets(&self) -> Result<Vec<JobPreset>>;

    /// List presets filtered by category.
    async fn list_presets_by_category(&self, category: Option<&str>) -> Result<Vec<JobPreset>>;

    /// List presets with filtering, searching, and pagination.
    async fn list_presets_filtered(
        &self,
        filters: &JobPresetFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobPreset>, u64)>;

    /// List all unique categories.
    async fn list_categories(&self) -> Result<Vec<String>>;

    /// Create a new preset.
    async fn create_preset(&self, preset: &JobPreset) -> Result<()>;

    /// Update an existing preset.
    async fn update_preset(&self, preset: &JobPreset) -> Result<()>;

    /// Delete a preset.
    async fn delete_preset(&self, id: &str) -> Result<()>;
}

/// SQLite implementation of JobPresetRepository.
pub struct SqliteJobPresetRepository {
    pool: Arc<SqlitePool>,
    write_pool: Arc<SqlitePool>,
}

impl SqliteJobPresetRepository {
    /// Create a new SqliteJobPresetRepository.
    pub fn new(pool: Arc<SqlitePool>, write_pool: Arc<SqlitePool>) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl JobPresetRepository for SqliteJobPresetRepository {
    async fn get_preset(&self, id: &str) -> Result<Option<JobPreset>> {
        let preset = sqlx::query_as::<_, JobPreset>(
            r#"
            SELECT * FROM job_presets WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&*self.pool)
        .await?;

        Ok(preset)
    }

    async fn get_preset_by_name(&self, name: &str) -> Result<Option<JobPreset>> {
        let preset = sqlx::query_as::<_, JobPreset>(
            r#"
            SELECT * FROM job_presets WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(&*self.pool)
        .await?;

        Ok(preset)
    }

    async fn name_exists(&self, name: &str, exclude_id: Option<&str>) -> Result<bool> {
        let count: (i64,) = if let Some(id) = exclude_id {
            sqlx::query_as(
                r#"
                SELECT COUNT(*) FROM job_presets WHERE name = $1 AND id != $2
                "#,
            )
            .bind(name)
            .bind(id)
            .fetch_one(&*self.pool)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT COUNT(*) FROM job_presets WHERE name = $1
                "#,
            )
            .bind(name)
            .fetch_one(&*self.pool)
            .await?
        };

        Ok(count.0 > 0)
    }

    async fn list_presets(&self) -> Result<Vec<JobPreset>> {
        let presets = sqlx::query_as::<_, JobPreset>(
            r#"
            SELECT * FROM job_presets ORDER BY name
            "#,
        )
        .fetch_all(&*self.pool)
        .await?;

        Ok(presets)
    }

    async fn list_presets_by_category(&self, category: Option<&str>) -> Result<Vec<JobPreset>> {
        let presets = if let Some(cat) = category {
            sqlx::query_as::<_, JobPreset>(
                r#"
                SELECT * FROM job_presets WHERE category = $1 ORDER BY name
                "#,
            )
            .bind(cat)
            .fetch_all(&*self.pool)
            .await?
        } else {
            sqlx::query_as::<_, JobPreset>(
                r#"
                SELECT * FROM job_presets ORDER BY name
                "#,
            )
            .fetch_all(&*self.pool)
            .await?
        };

        Ok(presets)
    }

    async fn list_presets_filtered(
        &self,
        filters: &JobPresetFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobPreset>, u64)> {
        let mut count_query = QueryBuilder::new("SELECT COUNT(*) FROM job_presets");
        push_job_preset_filters(&mut count_query, filters);
        let total = count_query
            .build_query_scalar::<i64>()
            .fetch_one(&*self.pool)
            .await? as u64;

        let mut data_query = QueryBuilder::new("SELECT * FROM job_presets");
        push_job_preset_filters(&mut data_query, filters);
        data_query
            .push(" ORDER BY name LIMIT ")
            .push_bind(pagination.limit as i64)
            .push(" OFFSET ")
            .push_bind(pagination.offset as i64);
        let presets = data_query
            .build_query_as::<JobPreset>()
            .fetch_all(&*self.pool)
            .await?;

        Ok((presets, total))
    }

    async fn list_categories(&self) -> Result<Vec<String>> {
        let categories: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT DISTINCT category FROM job_presets WHERE category IS NOT NULL ORDER BY category
            "#,
        )
        .fetch_all(&*self.pool)
        .await?;

        Ok(categories.into_iter().map(|(c,)| c).collect())
    }

    async fn create_preset(&self, preset: &JobPreset) -> Result<()> {
        writes::write_job_preset(
            &mut *self.write_pool.acquire().await?,
            preset,
            super::row_write::WriteMode::Insert,
            preset.updated_at.timestamp_millis(),
        )
        .await?;

        Ok(())
    }

    async fn update_preset(&self, preset: &JobPreset) -> Result<()> {
        let updated_at = crate::database::time::now_ms();
        writes::write_job_preset(
            &mut *self.write_pool.acquire().await?,
            preset,
            super::row_write::WriteMode::Update,
            updated_at,
        )
        .await?;

        Ok(())
    }

    async fn delete_preset(&self, id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM job_presets WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&*self.write_pool)
        .await?;

        Ok(())
    }
}

// ============================================================================
// Pipeline Preset Repository
// ============================================================================

use crate::database::models::PipelinePreset;

/// Filter parameters for pipeline presets.
#[derive(Debug, Clone, Default)]
pub struct PipelinePresetFilters {
    /// Search query (matches name or description).
    pub search: Option<String>,
}

/// Repository for managing pipeline presets (workflow sequences).
#[async_trait]
pub trait PipelinePresetRepository: Send + Sync {
    /// List all pipeline presets.
    async fn list_pipeline_presets(&self) -> Result<Vec<PipelinePreset>>;

    /// List pipeline presets with filtering, searching, and pagination.
    async fn list_pipeline_presets_filtered(
        &self,
        filters: &PipelinePresetFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<PipelinePreset>, u64)>;

    /// Get a pipeline preset by ID.
    async fn get_pipeline_preset(&self, id: &str) -> Result<Option<PipelinePreset>>;

    /// Get a pipeline preset by name.
    async fn get_pipeline_preset_by_name(&self, name: &str) -> Result<Option<PipelinePreset>>;

    /// Create a new pipeline preset.
    async fn create_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()>;

    /// Update an existing pipeline preset.
    async fn update_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()>;

    /// Delete a pipeline preset.
    async fn delete_pipeline_preset(&self, id: &str) -> Result<()>;
}

/// SQLite implementation of PipelinePresetRepository.
pub struct SqlitePipelinePresetRepository {
    pool: Arc<SqlitePool>,
    write_pool: Arc<SqlitePool>,
}

impl SqlitePipelinePresetRepository {
    /// Create a new SqlitePipelinePresetRepository.
    pub fn new(pool: Arc<SqlitePool>, write_pool: Arc<SqlitePool>) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl PipelinePresetRepository for SqlitePipelinePresetRepository {
    async fn list_pipeline_presets(&self) -> Result<Vec<PipelinePreset>> {
        let presets = sqlx::query_as::<_, PipelinePreset>(
            r#"
            SELECT * FROM pipeline_presets ORDER BY name
            "#,
        )
        .fetch_all(&*self.pool)
        .await?;

        Ok(presets)
    }

    async fn list_pipeline_presets_filtered(
        &self,
        filters: &PipelinePresetFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<PipelinePreset>, u64)> {
        let mut count_query = QueryBuilder::new("SELECT COUNT(*) FROM pipeline_presets");
        push_search_filter(&mut count_query, " WHERE ", filters.search.as_deref());
        let total = count_query
            .build_query_scalar::<i64>()
            .fetch_one(&*self.pool)
            .await? as u64;

        let mut data_query = QueryBuilder::new("SELECT * FROM pipeline_presets");
        push_search_filter(&mut data_query, " WHERE ", filters.search.as_deref());
        data_query
            .push(" ORDER BY name LIMIT ")
            .push_bind(pagination.limit as i64)
            .push(" OFFSET ")
            .push_bind(pagination.offset as i64);
        let presets = data_query
            .build_query_as::<PipelinePreset>()
            .fetch_all(&*self.pool)
            .await?;

        Ok((presets, total))
    }

    async fn get_pipeline_preset(&self, id: &str) -> Result<Option<PipelinePreset>> {
        let preset = sqlx::query_as::<_, PipelinePreset>(
            r#"
            SELECT * FROM pipeline_presets WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&*self.pool)
        .await?;

        Ok(preset)
    }

    async fn get_pipeline_preset_by_name(&self, name: &str) -> Result<Option<PipelinePreset>> {
        let preset = sqlx::query_as::<_, PipelinePreset>(
            r#"
            SELECT * FROM pipeline_presets WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(&*self.pool)
        .await?;

        Ok(preset)
    }

    async fn create_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()> {
        writes::write_pipeline_preset(
            &mut *self.write_pool.acquire().await?,
            preset,
            super::row_write::WriteMode::Insert,
            preset.updated_at.timestamp_millis(),
        )
        .await?;

        Ok(())
    }

    async fn update_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()> {
        let updated_at = crate::database::time::now_ms();
        writes::write_pipeline_preset(
            &mut *self.write_pool.acquire().await?,
            preset,
            super::row_write::WriteMode::Update,
            updated_at,
        )
        .await?;

        Ok(())
    }

    async fn delete_pipeline_preset(&self, id: &str) -> Result<()> {
        sqlx::query(
            r#"
            DELETE FROM pipeline_presets WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&*self.write_pool)
        .await?;

        Ok(())
    }
}
