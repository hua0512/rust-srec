use crate::error::is_broken_pipe_error;
use crate::output::pipe_flv_strategy::PipeFlvStrategy;
use crate::output::provider::OutputFormat;
use crate::utils::{expand_name_url, format_bytes};
use crate::{config::ProgramConfig, error::AppError};
use crate::{processor::generic::process_stream, utils::create_dirs, utils::spans};
use flv::data::FlvData;
use flv::parser_async::FlvDecoderStream;
use flv_fix::FlvPipeline;
use flv_fix::FlvPipelineConfig;
use flv_fix::writer::FlvWriter;
use futures::{Stream, StreamExt};
use mesio_engine::DownloaderInstance;
use pipeline_common::{
    CancellationToken, PipelineError, PipelineProvider, ProtocolWriter, StreamerContext,
    config::PipelineConfig,
};
use pipeline_common::{WriterConfig, WriterTask};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use std::time::Instant;
use tokio::fs::File;
use tokio::io::BufReader;
use tracing::{Level, Span, info, span, warn};

async fn process_raw_stream(
    stream: Pin<Box<dyn Stream<Item = Result<FlvData, PipelineError>> + Send>>,
    output_dir: &Path,
    base_name: &str,
    pipeline_common_config: &PipelineConfig,
) -> Result<(usize, u32), AppError> {
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

/// Statistics from pipe stream processing
struct PipeStreamStats {
    items_written: usize,
    segment_count: u32,
    bytes_written: u64,
}

/// Process FLV stream to pipe output (stdout)
/// Uses PipeFlvStrategy for segment boundary detection
async fn process_pipe_stream(
    stream: Pin<Box<dyn Stream<Item = Result<FlvData, PipelineError>> + Send>>,
    pipeline_common_config: &PipelineConfig,
) -> Result<PipeStreamStats, AppError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(pipeline_common_config.channel_size);

    // Create the pipe strategy and writer task config
    let strategy = PipeFlvStrategy::new();
    let config = WriterConfig::new(PathBuf::from("."), "stdout".to_string(), "flv".to_string());

    let mut writer_task_instance = WriterTask::new(config, strategy);

    // Capture the current span to propagate to the blocking task
    let current_span = Span::current();

    // Use a Result<_, (String, bool)> where bool indicates if it's a broken pipe error
    let writer_task =
        tokio::task::spawn_blocking(move || -> Result<PipeStreamStats, (String, bool)> {
            let _enter = current_span.enter();

            // Process items from the receiver using blocking_recv
            while let Some(item_result) = rx.blocking_recv() {
                match item_result {
                    Ok(item) => {
                        if let Err(e) = writer_task_instance.process_item(item) {
                            // Check if it's a broken pipe error
                            let err_str = e.to_string();
                            if is_broken_pipe_error(&err_str) {
                                warn!("Pipe closed by consumer (broken pipe)");
                                // Broken pipe is not an error - consumer just closed the connection
                                break;
                            }
                            return Err((format!("Writer error: {}", err_str), false));
                        }
                    }
                    Err(e) => {
                        return Err((format!("Pipeline error: {}", e), false));
                    }
                }
            }

            // Close the writer task - handle broken pipe gracefully
            if let Err(e) = writer_task_instance.close() {
                let err_str = e.to_string();
                if is_broken_pipe_error(&err_str) {
                    warn!("Broken pipe during close: consumer already disconnected");
                    // Not an error - just return current state
                } else {
                    return Err((format!("Close error: {}", err_str), false));
                }
            }

            let state = writer_task_instance.get_state();
            Ok(PipeStreamStats {
                items_written: state.items_written_total,
                segment_count: state.file_sequence_number,
                bytes_written: state.bytes_written_total,
            })
        });

    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        if tx.send(item_result).await.is_err() {
            // Receiver dropped - likely due to broken pipe
            break;
        }
    }
    drop(tx);

    match writer_task.await {
        Ok(Ok(stats)) => Ok(stats),
        Ok(Err((msg, is_broken_pipe))) => {
            if is_broken_pipe {
                // Broken pipe is expected behavior when consumer closes
                Err(AppError::BrokenPipe)
            } else {
                Err(AppError::Writer(msg))
            }
        }
        Err(e) => Err(AppError::Writer(e.to_string())),
    }
}

/// Process FLV stream to pipe output (stdout) with FlvPipeline processing
/// This function chains the FlvPipeline with PipeFlvStrategy for processed pipe output
async fn process_pipe_stream_with_processing(
    stream: Pin<Box<dyn Stream<Item = Result<FlvData, PipelineError>> + Send>>,
    pipeline_config: &PipelineConfig,
    flv_pipeline_config: FlvPipelineConfig,
) -> Result<PipeStreamStats, AppError> {
    // Create the context and pipeline
    let context = StreamerContext::new(CancellationToken::new());
    let pipeline_provider = FlvPipeline::with_config(context, pipeline_config, flv_pipeline_config);
    let pipeline = pipeline_provider.build_pipeline();

    // Spawn the pipeline tasks - this gives us input_tx, output_rx, and task handles
    let pipeline_common::channel_pipeline::SpawnedPipeline {
        input_tx,
        output_rx,
        tasks: processing_tasks,
    } = pipeline.spawn();

    // Create the pipe strategy and writer task config
    let strategy = PipeFlvStrategy::new();
    let config = WriterConfig::new(PathBuf::from("."), "stdout".to_string(), "flv".to_string());
    let mut writer_task_instance = WriterTask::new(config, strategy);

    // Capture the current span to propagate to the blocking task
    let current_span = Span::current();

    // Spawn the writer task that reads from pipeline output
    let writer_task =
        tokio::task::spawn_blocking(move || -> Result<PipeStreamStats, (String, bool)> {
            let _enter = current_span.enter();
            let mut output_rx = output_rx;

            // Process items from the pipeline output using blocking_recv
            while let Some(item_result) = output_rx.blocking_recv() {
                match item_result {
                    Ok(item) => {
                        if let Err(e) = writer_task_instance.process_item(item) {
                            // Check if it's a broken pipe error
                            let err_str = e.to_string();
                            if is_broken_pipe_error(&err_str) {
                                warn!("Pipe closed by consumer (broken pipe)");
                                // Broken pipe is not an error - consumer just closed the connection
                                break;
                            }
                            return Err((format!("Writer error: {}", err_str), false));
                        }
                    }
                    Err(e) => {
                        return Err((format!("Pipeline error: {}", e), false));
                    }
                }
            }

            // Close the writer task - handle broken pipe gracefully
            if let Err(e) = writer_task_instance.close() {
                let err_str = e.to_string();
                if is_broken_pipe_error(&err_str) {
                    warn!("Broken pipe during close: consumer already disconnected");
                    // Not an error - just return current state
                } else {
                    return Err((format!("Close error: {}", err_str), false));
                }
            }

            let state = writer_task_instance.get_state();
            Ok(PipeStreamStats {
                items_written: state.items_written_total,
                segment_count: state.file_sequence_number,
                bytes_written: state.bytes_written_total,
            })
        });

    // Feed the input stream to the pipeline
    let mut stream = stream;
    while let Some(item_result) = stream.next().await {
        if input_tx.send(item_result).await.is_err() {
            // Pipeline input channel closed - likely due to downstream error
            break;
        }
    }
    drop(input_tx); // Close the input channel to signal completion to the pipeline

    // Wait for the writer task to complete
    let writer_result = match writer_task.await {
        Ok(Ok(stats)) => Ok(stats),
        Ok(Err((msg, is_broken_pipe))) => {
            if is_broken_pipe {
                Err(AppError::BrokenPipe)
            } else {
                Err(AppError::Writer(msg))
            }
        }
        Err(e) => Err(AppError::Writer(e.to_string())),
    };

    // Wait for processing tasks to ensure clean shutdown
    for task in processing_tasks {
        let task_result = task
            .await
            .map_err(|e| AppError::Pipeline(PipelineError::Processing(e.to_string())))?;

        // If writer succeeded, we care about task errors
        // If writer failed, we might ignore task errors (which are likely "channel closed")
        if writer_result.is_ok() {
            task_result?;
        }
    }

    writer_result
}

/// Process a single FLV file
pub async fn process_file(
    input_path: &Path,
    output_dir: &Path,
    config: &ProgramConfig,
    token: &CancellationToken,
) -> Result<(), AppError> {
    // Create output directory if it doesn't exist
    create_dirs(output_dir).await?;

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

    let (tags_written, files_created) = if config.enable_processing {
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
    let (tags_written, files_created, bytes_written) = if is_pipe_mode {
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
            process_pipe_stream_with_processing(
                Box::pin(stream),
                &config.pipeline_config,
                config.flv_pipeline_config.clone(),
            )
            .await?
        } else {
            // Raw output: bypass processing pipeline
            process_pipe_stream(Box::pin(stream), &config.pipeline_config).await?
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
        (result.0, result.1, 0u64)
    } else {
        let result = process_raw_stream(
            Box::pin(stream),
            output_dir,
            &base_name,
            &config.pipeline_config,
        )
        .await?;
        (result.0, result.1, 0u64)
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

    let _ = bytes_written; // Suppress unused warning for file mode
    Ok(tags_written as u64)
}
