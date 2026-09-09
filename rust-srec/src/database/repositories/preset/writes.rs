use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::{JobPreset, PipelinePreset};

pub(crate) async fn write_job_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &JobPreset,
    mode: WriteMode,
    updated_at: i64,
) -> Result<(), sqlx::Error> {
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
    row.field("created_at", model.created_at.timestamp_millis(), false)?;
    row.field("updated_at", updated_at, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_job_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &JobPreset,
) -> Result<(), sqlx::Error> {
    write_job_preset(
        connection,
        model,
        WriteMode::Import,
        model.updated_at.timestamp_millis(),
    )
    .await
}

pub(crate) async fn write_pipeline_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &PipelinePreset,
    mode: WriteMode,
    updated_at: i64,
) -> Result<(), sqlx::Error> {
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
    row.field("created_at", model.created_at.timestamp_millis(), false)?;
    row.field("updated_at", updated_at, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_pipeline_preset(
    connection: &mut sqlx::SqliteConnection,
    model: &PipelinePreset,
) -> Result<(), sqlx::Error> {
    write_pipeline_preset(
        connection,
        model,
        WriteMode::Import,
        model.updated_at.timestamp_millis(),
    )
    .await
}
