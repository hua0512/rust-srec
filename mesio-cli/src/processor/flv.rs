use crate::config::ProgramConfig;
use crate::error::AppError;
use crate::processor::generic::process_stream;
use crate::utils::{create_dirs, format_bytes};
use flv::data::FlvData;
use flv::parser_async::FlvDecoderStream;
use flv_fix::writer::FlvWriter;
use flv_fix::{FlvPipeline, FlvPipelineConfig, flv_error_to_pipeline_error};
use futures::{Stream, StreamExt, TryStreamExt};
use mesio_engine::DownloaderInstance;
use pipeline_common::{OnProgress, PipelineError, ProtocolWriter, config::PipelineConfig};
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::info;

async fn process_raw_stream<S, E>(
    stream: S,
    output_dir: &Path,
    base_name: &str,
    pipeline_common_config: &PipelineConfig,
    on_progress: Option<OnProgress>,
) -> Result<(usize, u32), AppError>
where
    S: Stream<Item = Result<FlvData, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let (tx, rx) = mpsc::sync_channel(pipeline_common_config.channel_size);
    let mut writer = FlvWriter::new(
        output_dir.to_path_buf(),
        base_name.to_string(),
        "flv".to_string(),
        on_progress,
    );
    let writer_task = tokio::task::spawn_blocking(move || writer.run(rx));

    let mut stream = Box::pin(stream);
    while let Some(item_result) = stream.next().await {
        let item = item_result.map_err(|e| PipelineError::Processing(e.to_string()));
        if tx.send(item).is_err() {
            break;
        }
    }
    drop(tx);

    writer_task
        .await
        .map_err(|e| AppError::Writer(e.to_string()))?
        .map_err(|e| AppError::Writer(e.to_string()))
}

/// Process a single FLV file
pub async fn process_file(
    input_path: &Path,
    output_dir: &Path,
    config: &ProgramConfig,
    on_progress: Option<OnProgress>,
) -> Result<(), AppError> {
    create_dirs(output_dir).await?;

    let base_name = input_path
        .file_stem()
        .ok_or_else(|| AppError::InvalidInput("Invalid filename".to_string()))?
        .to_string_lossy()
        .to_string();

    let start_time = std::time::Instant::now();
    info!(
        path = %input_path.display(),
        processing_enabled = config.enable_processing,
        "Starting to process file"
    );

    let file = File::open(input_path).await?;
    let file_reader = BufReader::new(file);
    let file_size = file_reader.get_ref().metadata().await?.len();
    let decoder_stream = FlvDecoderStream::with_capacity(file_reader, 1024 * 1024)
        .map_err(flv_error_to_pipeline_error);

    let (tags_written, files_created) = if config.enable_processing {
        process_stream::<FlvPipelineConfig, FlvData, FlvPipeline, FlvWriter, _, _>(
            &config.pipeline_config,
            config.flv_pipeline_config.clone(),
            decoder_stream,
            output_dir,
            &base_name,
            "flv",
            on_progress,
        )
        .await?
    } else {
        process_raw_stream(
            decoder_stream,
            output_dir,
            &base_name,
            &config.pipeline_config,
            on_progress,
        )
        .await?
    };

    let elapsed = start_time.elapsed();
    info!(
        path = %input_path.display(),
        input_size = %format_bytes(file_size),
        duration = ?elapsed,
        tags_written,
        files_created,
        processing_enabled = config.enable_processing,
        "Processing complete"
    );

    Ok(())
}

/// Process an FLV stream
pub async fn process_flv_stream(
    url_str: &str,
    output_dir: &Path,
    config: &ProgramConfig,
    name_template: &str,
    on_progress: Option<OnProgress>,
    downloader: &mut DownloaderInstance,
) -> Result<u64, AppError> {
    let start_time = Instant::now();
    tokio::fs::create_dir_all(output_dir).await?;

    let url_name = {
        let url = url_str
            .parse::<reqwest::Url>()
            .map_err(|e| AppError::InvalidInput(e.to_string()))?;
        let file_name = url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("stream")
            .to_string();
        match file_name.rfind('.') {
            Some(pos) => file_name[..pos].to_string(),
            None => file_name,
        }
    };

    let base_name = name_template.replace("%u", &url_name);
    downloader.add_source(url_str, 0);

    let stream = match downloader {
        DownloaderInstance::Flv(flv) => flv.download_with_sources(url_str).await?,
        _ => {
            return Err(AppError::InvalidInput(
                "Expected FLV downloader".to_string(),
            ));
        }
    };

    let (tags_written, files_created) = if config.enable_processing {
        process_stream::<FlvPipelineConfig, FlvData, FlvPipeline, FlvWriter, _, _>(
            &config.pipeline_config,
            config.flv_pipeline_config.clone(),
            stream,
            output_dir,
            &base_name,
            "flv",
            on_progress,
        )
        .await?
    } else {
        process_raw_stream(
            stream,
            output_dir,
            &base_name,
            &config.pipeline_config,
            on_progress,
        )
        .await?
    };

    let elapsed = start_time.elapsed();
    info!(
        url = %url_str,
        duration = ?elapsed,
        tags_written,
        files_created,
        "FLV processing complete"
    );

    Ok(tags_written as u64)
}
