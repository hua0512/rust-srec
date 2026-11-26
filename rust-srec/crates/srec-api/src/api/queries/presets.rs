//! Filtered job preset and pipeline preset listing queries.

use sqlx::{QueryBuilder, Sqlite};

use crate::Result;
use crate::database::models::{JobPreset, Pagination, PipelinePreset};
use crate::database::repositories::literal_substring_pattern;

use super::ApiQueries;

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

/// Filter parameters for pipeline presets.
#[derive(Debug, Clone, Default)]
pub struct PipelinePresetFilters {
    /// Search query (matches name or description).
    pub search: Option<String>,
}

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
    let pattern = literal_substring_pattern(search);
    query
        .push(separator)
        .push("(name LIKE ")
        .push_bind(pattern.clone())
        .push(r" ESCAPE '\' OR description LIKE ")
        .push_bind(pattern)
        .push(r" ESCAPE '\')");
}

impl ApiQueries {
    /// List job presets with filtering, searching, and pagination.
    pub async fn list_presets_filtered(
        &self,
        filters: &JobPresetFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobPreset>, u64)> {
        let mut count_query = QueryBuilder::new("SELECT COUNT(*) FROM job_presets");
        push_job_preset_filters(&mut count_query, filters);
        let total = count_query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
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
            .fetch_all(&self.pool)
            .await?;

        Ok((presets, total))
    }

    /// List pipeline presets with filtering, searching, and pagination.
    pub async fn list_pipeline_presets_filtered(
        &self,
        filters: &PipelinePresetFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<PipelinePreset>, u64)> {
        let mut count_query = QueryBuilder::new("SELECT COUNT(*) FROM pipeline_presets");
        push_search_filter(&mut count_query, " WHERE ", filters.search.as_deref());
        let total = count_query
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
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
            .fetch_all(&self.pool)
            .await?;

        Ok((presets, total))
    }
}
