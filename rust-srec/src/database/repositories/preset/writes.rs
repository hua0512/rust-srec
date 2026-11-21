use super::super::proxies::{StepDocument, require_step_routes};
use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::{JobPreset, PipelinePreset};

pub(crate) async fn write_job_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &JobPreset,
    mode: WriteMode,
    updated_at: i64,
) -> crate::Result<()> {
    require_step_routes(
        connection,
        &[StepDocument::PresetConfig {
            processor: &model.processor,
            config: &model.config,
        }],
    )
    .await?;
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("job_presets", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field("description", &model.description, true)?;
    row.field("category", &model.category, true)?;
    row.field("processor", &model.processor, true)?;
    row.field("config", &model.config, true)?;
    row.field("created_at", model.created_at.as_millisecond(), false)?;
    row.field("updated_at", updated_at, true)?;
    Ok(row.execute(connection).await?)
}

pub(crate) async fn import_job_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &JobPreset,
) -> crate::Result<()> {
    write_job_preset(
        connection,
        model,
        WriteMode::Import,
        model.updated_at.as_millisecond(),
    )
    .await
}

pub(crate) async fn write_pipeline_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &PipelinePreset,
    mode: WriteMode,
    updated_at: i64,
) -> crate::Result<()> {
    require_step_routes(
        connection,
        &[StepDocument::Pipelines(
            crate::pipeline::step_routes::Layout::Pipeline,
            model.dag_definition.as_deref(),
        )],
    )
    .await?;
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("pipeline_presets", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field("description", &model.description, true)?;
    row.field("dag_definition", &model.dag_definition, true)?;
    row.field("pipeline_type", &model.pipeline_type, true)?;
    row.field("created_at", model.created_at.as_millisecond(), false)?;
    row.field("updated_at", updated_at, true)?;
    Ok(row.execute(connection).await?)
}

pub(crate) async fn import_pipeline_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &PipelinePreset,
) -> crate::Result<()> {
    write_pipeline_preset(
        connection,
        model,
        WriteMode::Import,
        model.updated_at.as_millisecond(),
    )
    .await
}
