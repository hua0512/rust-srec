//! Job Preset repository.

use std::sync::Arc;

use crate::Result;
use crate::database::models::{JobPreset, Pagination};
use async_trait::async_trait;
use sqlx::SqlitePool;

/// Filter parameters for job presets.
#[derive(Debug, Clone, Default)]
pub struct JobPresetFilters {
    /// Filter by category.
    pub category: Option<String>,
    /// Filter by processor type.
    pub processor: Option<String>,
    /// Search query (matches name or description).
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
}

impl SqliteJobPresetRepository {
    /// Create a new SqliteJobPresetRepository.
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
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
        // Build dynamic WHERE clause
        let mut conditions = Vec::new();
        let mut bind_index = 1;

        if filters.category.is_some() {
            conditions.push(format!("category = ${}", bind_index));
            bind_index += 1;
        }

        if filters.processor.is_some() {
            conditions.push(format!("processor = ${}", bind_index));
            bind_index += 1;
        }

        if filters.search.is_some() {
            conditions.push(format!(
                "(name LIKE ${} OR description LIKE ${})",
                bind_index, bind_index
            ));
            bind_index += 1;
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Count query
        let count_sql = format!("SELECT COUNT(*) FROM job_presets {}", where_clause);

        // Data query with pagination
        let data_sql = format!(
            "SELECT * FROM job_presets {} ORDER BY name LIMIT ${} OFFSET ${}",
            where_clause,
            bind_index,
            bind_index + 1
        );

        // Execute count query
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
        if let Some(ref cat) = filters.category {
            count_query = count_query.bind(cat);
        }
        if let Some(ref proc) = filters.processor {
            count_query = count_query.bind(proc);
        }
        if let Some(ref search) = filters.search {
            let search_pattern = format!("%{}%", search);
            count_query = count_query.bind(search_pattern);
        }
        let total = count_query.fetch_one(&*self.pool).await? as u64;

        // Execute data query
        let mut data_query = sqlx::query_as::<_, JobPreset>(&data_sql);
        if let Some(ref cat) = filters.category {
            data_query = data_query.bind(cat);
        }
        if let Some(ref proc) = filters.processor {
            data_query = data_query.bind(proc);
        }
        if let Some(ref search) = filters.search {
            let search_pattern = format!("%{}%", search);
            data_query = data_query.bind(search_pattern);
        }
        data_query = data_query.bind(pagination.limit as i64);
        data_query = data_query.bind(pagination.offset as i64);

        let presets = data_query.fetch_all(&*self.pool).await?;

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
        sqlx::query(
            r#"
            INSERT INTO job_presets (id, name, description, category, processor, config, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(&preset.id)
        .bind(&preset.name)
        .bind(&preset.description)
        .bind(&preset.category)
        .bind(&preset.processor)
        .bind(&preset.config)
        .bind(preset.created_at)
        .bind(preset.updated_at)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn update_preset(&self, preset: &JobPreset) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE job_presets
            SET name = $1, description = $2, category = $3, processor = $4, config = $5, updated_at = $6
            WHERE id = $7
            "#,
        )
        .bind(&preset.name)
        .bind(&preset.description)
        .bind(&preset.category)
        .bind(&preset.processor)
        .bind(&preset.config)
        .bind(chrono::Utc::now())
        .bind(&preset.id)
        .execute(&*self.pool)
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
        .execute(&*self.pool)
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
}

impl SqlitePipelinePresetRepository {
    /// Create a new SqlitePipelinePresetRepository.
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
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
        // Build dynamic WHERE clause
        let mut bind_index = 1;

        let where_clause = if filters.search.is_some() {
            let clause = format!(
                "WHERE (name LIKE ${} OR description LIKE ${})",
                bind_index, bind_index
            );
            bind_index += 1;
            clause
        } else {
            String::new()
        };

        // Count query
        let count_sql = format!("SELECT COUNT(*) FROM pipeline_presets {}", where_clause);

        // Data query with pagination
        let data_sql = format!(
            "SELECT * FROM pipeline_presets {} ORDER BY name LIMIT ${} OFFSET ${}",
            where_clause,
            bind_index,
            bind_index + 1
        );

        // Execute count query
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
        if let Some(ref search) = filters.search {
            let search_pattern = format!("%{}%", search);
            count_query = count_query.bind(search_pattern);
        }
        let total = count_query.fetch_one(&*self.pool).await? as u64;

        // Execute data query
        let mut data_query = sqlx::query_as::<_, PipelinePreset>(&data_sql);
        if let Some(ref search) = filters.search {
            let search_pattern = format!("%{}%", search);
            data_query = data_query.bind(search_pattern);
        }
        data_query = data_query.bind(pagination.limit as i64);
        data_query = data_query.bind(pagination.offset as i64);

        let presets = data_query.fetch_all(&*self.pool).await?;

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

    async fn create_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO pipeline_presets (id, name, description, steps, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&preset.id)
        .bind(&preset.name)
        .bind(&preset.description)
        .bind(&preset.steps)
        .bind(preset.created_at)
        .bind(preset.updated_at)
        .execute(&*self.pool)
        .await?;

        Ok(())
    }

    async fn update_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE pipeline_presets
            SET name = $1, description = $2, steps = $3, updated_at = $4
            WHERE id = $5
            "#,
        )
        .bind(&preset.name)
        .bind(&preset.description)
        .bind(&preset.steps)
        .bind(chrono::Utc::now())
        .bind(&preset.id)
        .execute(&*self.pool)
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
        .execute(&*self.pool)
        .await?;

        Ok(())
    }
}
