//! Job Preset repository.

mod writes;
pub(crate) use writes::{import_job_preset, import_pipeline_preset};

use std::sync::Arc;

use crate::Result;
use crate::database::models::{JobPreset, Pagination};
pub(crate) use crate::domain::step_routes::summarize_users;
use crate::domain::step_routes::{Layout, pipeline_steps_mut};
pub use crate::domain::step_routes::{PresetChange, StepRef, StepUser, StepUserKind};
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

fn step_ref_table(step: StepRef) -> &'static str {
    super::config_retirement::RetiredConfigKind::from(step).table()
}

/// Which stored pipelines count as users of a step.
#[derive(Clone, Copy)]
pub(crate) enum Owners {
    /// Every stored pipeline. Streamers and definitions being removed still
    /// run the pipelines owed to their last session; their labels say so.
    All,
    /// Pipelines that stay once removals settle. Owners being removed are
    /// left out: retirement keeps definitions for them until they finish
    /// (`config_retirement::delete_or_defer`).
    Kept,
}

/// The stored pipeline owners that name the `step` definition `id` called
/// `name`. A workflow does not count as its own user.
pub(crate) async fn step_users(
    conn: &mut sqlx::SqliteConnection,
    step: StepRef,
    id: &str,
    name: &str,
    owners: Owners,
) -> std::result::Result<Vec<StepUser>, sqlx::Error> {
    let names_step = |step_value: &serde_json::Value| {
        step_value.get("type").and_then(serde_json::Value::as_str) == Some(step.tag())
            && step_value.get("name").and_then(serde_json::Value::as_str) == Some(name)
    };
    Ok(pipeline_owners(conn, owners)
        .await?
        .into_iter()
        .filter(|owner| {
            !(matches!(step, StepRef::Workflow)
                && owner.user.kind == StepUserKind::Workflow
                && owner.user.id == id)
        })
        .filter_map(|mut owner| {
            owner
                .documents
                .iter_mut()
                .any(|(layout, document)| {
                    pipeline_steps_mut(document, *layout)
                        .into_iter()
                        .any(|step_value| names_step(step_value))
                })
                .then_some(owner.user)
        })
        .collect())
}

/// A stored configuration that keeps pipelines, with its documents that
/// hold them.
pub(crate) struct PipelineOwner {
    pub user: StepUser,
    pub documents: Vec<(Layout, serde_json::Value)>,
}

/// Every stored pipeline owner `owners` covers, ordered by kind, then name.
/// A document that is not JSON holds no steps.
pub(crate) async fn pipeline_owners(
    conn: &mut sqlx::SqliteConnection,
    owners: Owners,
) -> std::result::Result<Vec<PipelineOwner>, sqlx::Error> {
    const TEMPLATE_REMOVING: &str = "EXISTS(SELECT 1 FROM retirement_config_deletions r WHERE r.kind = 'template' AND r.config_id = template_config.id)";
    const WORKFLOW_REMOVING: &str = "EXISTS(SELECT 1 FROM retirement_config_deletions r WHERE r.kind = 'pipeline_preset' AND r.config_id = pipeline_presets.id)";
    const STREAMER_REMOVING: &str = "deleted_at IS NOT NULL";
    let kept = |removing: &str| match owners {
        Owners::All => "1".to_owned(),
        Owners::Kept => format!("NOT {removing}"),
    };
    let query = format!(
        "SELECT 'global', '', '', 0, pipeline, session_complete_pipeline, paired_segment_pipeline, NULL FROM global_config \
         UNION ALL SELECT 'platform', id, platform_name, 0, pipeline, session_complete_pipeline, paired_segment_pipeline, NULL FROM platform_config \
         UNION ALL SELECT 'template', id, name, {TEMPLATE_REMOVING}, pipeline, session_complete_pipeline, paired_segment_pipeline, platform_overrides FROM template_config WHERE {kept_template} \
         UNION ALL SELECT 'streamer', id, name, {STREAMER_REMOVING}, streamer_specific_config, NULL, NULL, NULL FROM streamers WHERE {kept_streamer} \
         UNION ALL SELECT 'workflow', id, name, {WORKFLOW_REMOVING}, dag_definition, NULL, NULL, NULL FROM pipeline_presets WHERE {kept_workflow} \
         ORDER BY 1, 3, 2",
        kept_template = kept(TEMPLATE_REMOVING),
        kept_streamer = kept(STREAMER_REMOVING),
        kept_workflow = kept(WORKFLOW_REMOVING),
    );
    type Row = (
        String,
        String,
        String,
        bool,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(query))
        .fetch_all(conn)
        .await?;
    rows.into_iter()
        .map(|(raw_kind, id, name, removing, a, b, c, d)| {
            let kind = StepUserKind::parse(&raw_kind).ok_or_else(|| {
                sqlx::Error::Decode(format!("unknown step user kind: {raw_kind:?}").into())
            })?;
            // The first column is a streamer's config or a pipeline; a
            // template's fourth holds its platform overrides.
            let first = match kind {
                StepUserKind::Streamer => Layout::Config,
                _ => Layout::Pipeline,
            };
            let layouts = [first, Layout::Pipeline, Layout::Pipeline, Layout::Overrides];
            Ok(PipelineOwner {
                user: StepUser {
                    kind,
                    id,
                    name,
                    removing,
                },
                documents: layouts
                    .into_iter()
                    .zip([a, b, c, d])
                    .filter_map(|(layout, text)| Some((layout, serde_json::from_str(&text?).ok()?)))
                    .collect(),
            })
        })
        .collect()
}

/// Fail with [`crate::Error::PresetInUse`] if a stored pipeline names the
/// `step` definition `id` called `name`.
async fn ensure_unused(
    conn: &mut sqlx::SqliteConnection,
    step: StepRef,
    id: &str,
    name: &str,
    change: PresetChange,
) -> Result<()> {
    let users = step_users(conn, step, id, name, Owners::All).await?;
    if users.is_empty() {
        return Ok(());
    }
    Err(crate::Error::PresetInUse {
        change,
        kind: step,
        name: name.to_string(),
        users,
    })
}

async fn stored_name(
    conn: &mut sqlx::SqliteConnection,
    step: StepRef,
    id: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT name FROM {} WHERE id = ?",
        step_ref_table(step)
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Delete the `step` definition `id` unless a stored pipeline names it.
/// The check and the delete share one immediate write transaction.
async fn delete_unused(write_pool: &SqlitePool, step: StepRef, id: &str) -> Result<()> {
    let mut tx = crate::database::begin_immediate(write_pool).await?;
    let Some(name) = stored_name(&mut tx, step, id).await? else {
        return Ok(());
    };
    ensure_unused(&mut tx, step, id, &name, PresetChange::Delete).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {} WHERE id = ?",
        step_ref_table(step)
    )))
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Fail with [`crate::Error::PresetInUse`] if renaming the `step` definition
/// `id` to `new_name` would leave a stored pipeline naming nothing.
async fn ensure_rename_allowed(
    conn: &mut sqlx::SqliteConnection,
    step: StepRef,
    id: &str,
    new_name: &str,
) -> Result<()> {
    match stored_name(conn, step, id).await? {
        Some(name) if name != new_name => {
            ensure_unused(conn, step, id, &name, PresetChange::Rename).await
        }
        _ => Ok(()),
    }
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

    /// Update an existing preset. Fails with [`crate::Error::PresetInUse`]
    /// when it renames a preset a stored pipeline references by name.
    async fn update_preset(&self, preset: &JobPreset) -> Result<()>;

    /// Delete a preset. Fails with [`crate::Error::PresetInUse`] while a
    /// stored pipeline references it by name.
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
            preset.updated_at.as_millisecond(),
        )
        .await?;

        Ok(())
    }

    async fn update_preset(&self, preset: &JobPreset) -> Result<()> {
        let updated_at = crate::database::time::now_ms();
        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
        ensure_rename_allowed(&mut tx, StepRef::JobPreset, &preset.id, &preset.name).await?;
        writes::write_job_preset(
            &mut tx,
            preset,
            super::row_write::WriteMode::Update,
            updated_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_preset(&self, id: &str) -> Result<()> {
        delete_unused(&self.write_pool, StepRef::JobPreset, id).await
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

    /// Update an existing pipeline preset. Fails with
    /// [`crate::Error::PresetInUse`] when it renames a workflow a stored
    /// pipeline references by name.
    async fn update_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()>;

    /// Delete a pipeline preset. Fails with [`crate::Error::PresetInUse`]
    /// while a stored pipeline references it by name.
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
            preset.updated_at.as_millisecond(),
        )
        .await?;

        Ok(())
    }

    async fn update_pipeline_preset(&self, preset: &PipelinePreset) -> Result<()> {
        let updated_at = crate::database::time::now_ms();
        let mut tx = crate::database::begin_immediate(&self.write_pool).await?;
        ensure_rename_allowed(&mut tx, StepRef::Workflow, &preset.id, &preset.name).await?;
        writes::write_pipeline_preset(
            &mut tx,
            preset,
            super::row_write::WriteMode::Update,
            updated_at,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_pipeline_preset(&self, id: &str) -> Result<()> {
        delete_unused(&self.write_pool, StepRef::Workflow, id).await
    }
}
