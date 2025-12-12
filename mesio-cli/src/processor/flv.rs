use crate::output::pipe_flv_strategy::PipeFlvStrategy;
use crate::output::provider::OutputFormat;
use crate::processor::generic::{
    process_pipe_stream, process_pipe_stream_with_processing, process_stream,
};
use crate::utils::{create_dirs, expand_name_url, format_bytes, spans};
use crate::{config::ProgramConfig, error::AppError};
use flv::data::FlvData;
use flv::parser_async::FlvDecoderStream;
use flv_fix::FlvPipeline;
use flv_fix::writer::FlvWriter;
use futures::{Stream, StreamExt};
use mesio_engine::DownloaderInstance;
use pipeline_common::{CancellationToken, PipelineError, ProtocolWriter, config::PipelineConfig};
use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::time::Instant;
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::{Level, Span, info, span};

async fn process_raw_stream(
    stream: Pin<Box<dyn Stream<Item = Result<FlvData, PipelineError>> + Send>>,
    output_dir: &Path,
    base_name: &str,
    pipeline_common_config: &PipelineConfig,
) -> Result<(usize, u32, u64, f64), AppError> {
    let (tx, rx) = tokio::sync::mpsc::channel(pipeline_common_config.channel_size);
    let mut writer = FlvWriter::new(
        output_dir.to_path_buf(),
        base_name.to_string(),
        "flv".to_string(),
        Some(HashMap::from([(
            "enable_low_latency".to_string(),
            "false".to_string(),
        )])),
    );

    // Capture the current span to propagate to the blocking task
    let current_span = Span::current();
    let writer_task = tokio::task::spawn_blocking(move || {
        let _enter = current_span.enter();
        writer.run(rx)
    });

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        if tx.send(item_result).await.is_err() {
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
    token: &CancellationToken,
) -> Result<(), AppError> {
    // Check if we're in pipe output mode
    let is_pipe_mode = matches!(
        config.output_format,
        OutputFormat::Stdout | OutputFormat::Stderr
    );

    // Only create output directory for file mode
    if !is_pipe_mode {
        create_dirs(output_dir).await?;
    }

    let base_name = input_path
        .file_stem()
        .ok_or_else(|| AppError::InvalidInput("Invalid filename".to_string()))?
        .to_string_lossy()
        .to_string();

    let start_time = std::time::Instant::now();

    // Create span for file processing
    let file_span = span!(Level::INFO, "process_flv_file", path = %input_path.display());
    let _file_enter = file_span.enter();

    info!(
        path = %input_path.display(),
        processing_enabled = config.enable_processing,
        "Starting to process file"
    );

    let file = File::open(input_path).await?;
    let file_reader = BufReader::new(file);
    let file_size = file_reader.get_ref().metadata().await?.len();
    let decoder_stream = FlvDecoderStream::with_capacity(file_reader, 4 * 1024 * 1024) // 4MB buffer for better I/O throughput
        .map(|r| r.map_err(|e| PipelineError::Processing(e.to_string())));

    // Use pipe output strategy when stdout mode is active
    let (tags_written, files_created, _bytes_written, _duration) = if is_pipe_mode {
        // Pipe mode: write to stdout using PipeFlvStrategy
        let stats = if config.enable_processing {
            // Processing enabled: run through FlvPipeline before writing to stdout
            info!(
                path = %input_path.display(),
                processing_enabled = true,
                low_latency = config.flv_pipeline_config.enable_low_latency,
                output_mode = %config.output_format,
                "Starting pipe output with FLV processing"
            );
            process_pipe_stream_with_processing::<FlvPipeline, _>(
                Box::pin(decoder_stream),
                &config.pipeline_config,
                config.flv_pipeline_config.clone(),
                PipeFlvStrategy::new(),
                "flv",
            )
            .await?
        } else {
            // Raw output: bypass processing pipeline
            process_pipe_stream(
                Box::pin(decoder_stream),
                &config.pipeline_config,
                PipeFlvStrategy::new(),
                "flv",
            )
            .await?
        };

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            path = %input_path.display(),
            input_size = %format_bytes(file_size),
            duration = ?elapsed,
            tags_written = stats.items_written,
            bytes_written = stats.bytes_written,
            segment_count = stats.segment_count,
            output_mode = %config.output_format,
            processing_enabled = config.enable_processing,
            "FLV pipe output complete"
        );

        return Ok(());
    } else if config.enable_processing {
        // we need to expand base_name with %i for output file numbering
        let base_name = format!("{base_name}_p%i");
        // Create a span for pipeline processing
        let pipeline_span = span!(Level::INFO, "flv_pipeline");
        let _pipeline_enter = pipeline_span.enter();
        spans::init_processing_span(&pipeline_span, "Processing FLV tags");

        process_stream::<FlvPipeline, FlvWriter>(
            &config.pipeline_config,
            config.flv_pipeline_config.clone(),
            Box::pin(decoder_stream),
            "Writing FLV output",
            |_writer_span| {
                FlvWriter::new(
                    output_dir.to_path_buf(),
                    base_name.to_string(),
                    "flv".to_string(),
                    Some(HashMap::from([(
                        "enable_low_latency".to_string(),
                        config.flv_pipeline_config.enable_low_latency.to_string(),
                    )])),
                )
            },
            token.clone(),
        )
        .await?
    } else {
        // Create a span for raw stream writing
        let write_span = span!(Level::INFO, "flv_write_raw");
        let _write_enter = write_span.enter();
        spans::init_writing_span(&write_span, "Writing raw FLV");

        process_raw_stream(
            Box::pin(decoder_stream),
            output_dir,
            &base_name,
            &config.pipeline_config,
        )
        .await?
    };

    let elapsed = start_time.elapsed();
    // file_sequence_number starts at 0, so add 1 to get actual file count
    let actual_files_created = if tags_written > 0 {
        files_created + 1
    } else {
        0
    };
    info!(
        path = %input_path.display(),
        input_size = %format_bytes(file_size),
        duration = ?elapsed,
        tags_written,
        files_created = actual_files_created,
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
    downloader: &mut DownloaderInstance,
    token: &CancellationToken,
) -> Result<u64, AppError> {
    // Check if we're in pipe output mode
    let is_pipe_mode = matches!(
        config.output_format,
        OutputFormat::Stdout | OutputFormat::Stderr
    );

    // Only create output directory for file mode
    if !is_pipe_mode {
        create_dirs(output_dir).await?;
    }

    let start_time = Instant::now();

    // Create span for FLV stream download
    // Note: Progress bars are disabled in pipe mode via main.rs configuration
    let download_span = span!(Level::INFO, "download_flv", url = %url_str);
    let _download_enter = download_span.enter();

    // Only initialize download span visuals if not in pipe mode
    if !is_pipe_mode {
        spans::init_download_span(&download_span, format!("Downloading {}", url_str));
    }

    // Expand the name template with the URL filename
    let base_name = expand_name_url(name_template, url_str)?;
    downloader.add_source(url_str, 0);

    let stream = match downloader {
        DownloaderInstance::Flv(flv) => flv.download_with_sources(url_str).await?,
        _ => {
            return Err(AppError::InvalidInput(
                "Expected FLV downloader".to_string(),
            ));
        }
    };

    let stream = stream.map(|r| r.map_err(|e| PipelineError::Processing(e.to_string())));

    // Use pipe output strategy when stdout mode is active
    let (tags_written, files_created, _bytes_written) = if is_pipe_mode {
        // Pipe mode: write to stdout using PipeFlvStrategy
        // Check if processing is enabled to determine whether to use FlvPipeline
        let stats = if config.enable_processing {
            // Processing enabled: run through FlvPipeline before writing to stdout
            info!(
                url = %url_str,
                processing_enabled = true,
                low_latency = config.flv_pipeline_config.enable_low_latency,
                output_mode = %config.output_format,
                "Starting pipe output with FLV processing"
            );
            process_pipe_stream_with_processing::<FlvPipeline, _>(
                Box::pin(stream),
                &config.pipeline_config,
                config.flv_pipeline_config.clone(),
                PipeFlvStrategy::new(),
                "flv",
            )
            .await?
        } else {
            // Raw output: bypass processing pipeline
            process_pipe_stream(
                Box::pin(stream),
                &config.pipeline_config,
                PipeFlvStrategy::new(),
                "flv",
            )
            .await?
        };

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            url = %url_str,
            duration = ?elapsed,
            tags_written = stats.items_written,
            bytes_written = stats.bytes_written,
            segment_count = stats.segment_count,
            output_mode = %config.output_format,
            processing_enabled = config.enable_processing,
            "FLV pipe output complete"
        );

        return Ok(stats.items_written as u64);
    } else if config.enable_processing {
        let result = process_stream::<FlvPipeline, FlvWriter>(
            &config.pipeline_config,
            config.flv_pipeline_config.clone(),
            Box::pin(stream),
            "Writing FLV output",
            |_writer_span| {
                FlvWriter::new(
                    output_dir.to_path_buf(),
                    base_name.clone(),
                    "flv".to_string(),
                    Some(HashMap::from([(
                        "enable_low_latency".to_string(),
                        config.flv_pipeline_config.enable_low_latency.to_string(),
                    )])),
                )
            },
            token.clone(),
        )
        .await?;
        (result.0, result.1, result.2)
    } else {
        let result = process_raw_stream(
            Box::pin(stream),
            output_dir,
            &base_name,
            &config.pipeline_config,
        )
        .await?;
        (result.0, result.1, result.2)
    };

    let elapsed = start_time.elapsed();
    // file_sequence_number starts at 0, so add 1 to get actual file count
    let actual_files_created = if tags_written > 0 {
        files_created + 1
    } else {
        0
    };

    // Log completion (goes to stderr in pipe mode)
    info!(
        url = %url_str,
        duration = ?elapsed,
        tags_written,
        files_created = actual_files_created,
        output_mode = %config.output_format,
        "FLV processing complete"
    );

    Ok(tags_written as u64)
}
