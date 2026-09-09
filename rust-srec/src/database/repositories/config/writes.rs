use super::super::row_write::{Mutation, RowWrite, WriteMode};
use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, TemplateConfigDbModel,
};

pub(crate) async fn write_global(
    connection: &mut sqlx::SqliteConnection,
    model: &GlobalConfigDbModel,
    mode: WriteMode,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Update,
    };
    let mut row = RowWrite::new("global_config", mutation, &model.id)?;
    row.field("output_folder", &model.output_folder, true)?;
    row.field(
        "output_filename_template",
        &model.output_filename_template,
        true,
    )?;
    row.field("output_file_format", &model.output_file_format, true)?;
    row.field("min_segment_size_bytes", model.min_segment_size_bytes, true)?;
    row.field(
        "max_download_duration_secs",
        model.max_download_duration_secs,
        true,
    )?;
    row.field("max_part_size_bytes", model.max_part_size_bytes, true)?;
    row.field("record_danmu", model.record_danmu, true)?;
    row.field("danmu_statistics", &model.danmu_statistics, true)?;
    row.field(
        "max_concurrent_downloads",
        model.max_concurrent_downloads,
        true,
    )?;
    row.field("max_concurrent_uploads", model.max_concurrent_uploads, true)?;
    row.field(
        "streamer_check_delay_ms",
        model.streamer_check_delay_ms,
        true,
    )?;
    row.field("proxy_config", &model.proxy_config, true)?;
    row.field("offline_check_delay_ms", model.offline_check_delay_ms, true)?;
    row.field("offline_check_count", model.offline_check_count, true)?;
    row.field(
        "default_download_engine",
        &model.default_download_engine,
        true,
    )?;
    if mode != WriteMode::Import {
        row.field("default_extractor", &model.default_extractor, true)?;
    }
    row.field(
        "max_concurrent_cpu_jobs",
        model.max_concurrent_cpu_jobs,
        true,
    )?;
    row.field("max_concurrent_io_jobs", model.max_concurrent_io_jobs, true)?;
    row.field(
        "job_history_retention_days",
        model.job_history_retention_days,
        true,
    )?;
    row.field(
        "notification_event_log_retention_days",
        model.notification_event_log_retention_days,
        true,
    )?;
    row.field("pipeline", &model.pipeline, true)?;
    row.field(
        "session_complete_pipeline",
        &model.session_complete_pipeline,
        true,
    )?;
    row.field(
        "paired_segment_pipeline",
        &model.paired_segment_pipeline,
        true,
    )?;
    row.field("log_filter_directive", &model.log_filter_directive, true)?;
    row.field("auto_thumbnail", model.auto_thumbnail, true)?;
    row.field(
        "pipeline_cpu_job_timeout_secs",
        model.pipeline_cpu_job_timeout_secs,
        true,
    )?;
    row.field(
        "pipeline_io_job_timeout_secs",
        model.pipeline_io_job_timeout_secs,
        true,
    )?;
    row.field(
        "pipeline_execute_timeout_secs",
        model.pipeline_execute_timeout_secs,
        true,
    )?;
    row.field(
        "queue_freshness_threshold_ms",
        model.queue_freshness_threshold_ms,
        true,
    )?;
    row.field(
        "gpu_health_probe_interval_secs",
        model.gpu_health_probe_interval_secs,
        true,
    )?;
    row.field(
        "stream_proxy_allow_private_targets",
        model.stream_proxy_allow_private_targets,
        true,
    )?;
    row.execute(connection).await
}

pub(crate) async fn import_global(
    connection: &mut sqlx::SqliteConnection,
    model: &GlobalConfigDbModel,
) -> Result<(), sqlx::Error> {
    write_global(connection, model, WriteMode::Import).await
}

pub(crate) async fn write_engine(
    connection: &mut sqlx::SqliteConnection,
    model: &EngineConfigurationDbModel,
    mode: WriteMode,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("engine_configuration", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field("engine_type", &model.engine_type, true)?;
    row.field("config", &model.config, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_engine(
    connection: &mut sqlx::SqliteConnection,
    model: &EngineConfigurationDbModel,
) -> Result<(), sqlx::Error> {
    write_engine(connection, model, WriteMode::Import).await
}

pub(crate) async fn write_template(
    connection: &mut sqlx::SqliteConnection,
    model: &TemplateConfigDbModel,
    mode: WriteMode,
    updated_at: i64,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("template_config", mutation, &model.id)?;
    row.field("name", &model.name, true)?;
    row.field("output_folder", &model.output_folder, true)?;
    row.field(
        "output_filename_template",
        &model.output_filename_template,
        true,
    )?;
    row.field("cookies", &model.cookies, true)?;
    row.field("output_file_format", &model.output_file_format, true)?;
    row.field("min_segment_size_bytes", model.min_segment_size_bytes, true)?;
    row.field(
        "max_download_duration_secs",
        model.max_download_duration_secs,
        true,
    )?;
    row.field("max_part_size_bytes", model.max_part_size_bytes, true)?;
    row.field("record_danmu", model.record_danmu, true)?;
    row.field("danmu_statistics", &model.danmu_statistics, true)?;
    row.field("platform_overrides", &model.platform_overrides, true)?;
    row.field("download_retry_policy", &model.download_retry_policy, true)?;
    row.field("download_engine", &model.download_engine, true)?;
    if mode != WriteMode::Import {
        row.field("extractor", &model.extractor, true)?;
    }
    row.field("engines_override", &model.engines_override, true)?;
    row.field("proxy_config", &model.proxy_config, true)?;
    row.field(
        "stream_selection_config",
        &model.stream_selection_config,
        true,
    )?;
    row.field("pipeline", &model.pipeline, true)?;
    row.field(
        "session_complete_pipeline",
        &model.session_complete_pipeline,
        true,
    )?;
    row.field(
        "paired_segment_pipeline",
        &model.paired_segment_pipeline,
        true,
    )?;
    row.field("offline_check_count", model.offline_check_count, true)?;
    row.field("offline_check_delay_ms", model.offline_check_delay_ms, true)?;
    row.field("created_at", model.created_at.timestamp_millis(), false)?;
    row.field("updated_at", updated_at, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_template(
    connection: &mut sqlx::SqliteConnection,
    model: &TemplateConfigDbModel,
) -> Result<(), sqlx::Error> {
    write_template(
        connection,
        model,
        WriteMode::Import,
        model.updated_at.timestamp_millis(),
    )
    .await
}

pub(crate) async fn write_platform(
    connection: &mut sqlx::SqliteConnection,
    model: &PlatformConfigDbModel,
    mode: WriteMode,
) -> Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Update,
    };
    let mut row = RowWrite::new("platform_config", mutation, &model.id)?;
    row.field("platform_name", &model.platform_name, true)?;
    row.field("fetch_delay_ms", model.fetch_delay_ms, true)?;
    row.field("download_delay_ms", model.download_delay_ms, true)?;
    row.field("cookies", &model.cookies, true)?;
    row.field(
        "platform_specific_config",
        &model.platform_specific_config,
        true,
    )?;
    row.field("proxy_config", &model.proxy_config, true)?;
    row.field("record_danmu", model.record_danmu, true)?;
    row.field("danmu_statistics", &model.danmu_statistics, true)?;
    row.field("output_folder", &model.output_folder, true)?;
    row.field(
        "output_filename_template",
        &model.output_filename_template,
        true,
    )?;
    row.field("download_engine", &model.download_engine, true)?;
    if mode != WriteMode::Import {
        row.field("extractor", &model.extractor, true)?;
    }
    row.field(
        "stream_selection_config",
        &model.stream_selection_config,
        true,
    )?;
    row.field("output_file_format", &model.output_file_format, true)?;
    row.field("min_segment_size_bytes", model.min_segment_size_bytes, true)?;
    row.field(
        "max_download_duration_secs",
        model.max_download_duration_secs,
        true,
    )?;
    row.field("max_part_size_bytes", model.max_part_size_bytes, true)?;
    row.field("download_retry_policy", &model.download_retry_policy, true)?;
    row.field("pipeline", &model.pipeline, true)?;
    row.field(
        "session_complete_pipeline",
        &model.session_complete_pipeline,
        true,
    )?;
    row.field(
        "paired_segment_pipeline",
        &model.paired_segment_pipeline,
        true,
    )?;
    row.field("offline_check_count", model.offline_check_count, true)?;
    row.field("offline_check_delay_ms", model.offline_check_delay_ms, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_platform(
    connection: &mut sqlx::SqliteConnection,
    model: &PlatformConfigDbModel,
) -> Result<(), sqlx::Error> {
    write_platform(connection, model, WriteMode::Import).await
}

pub(crate) async fn delete_engine(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM engine_configuration WHERE id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}
