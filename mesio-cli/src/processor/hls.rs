use crate::error::is_broken_pipe_error;
use crate::output::pipe_hls_strategy::PipeHlsStrategy;
use crate::output::provider::OutputFormat;
use crate::utils::spans;
use crate::{config::ProgramConfig, error::AppError, utils::create_dirs, utils::expand_name_url};
use futures::{Stream, StreamExt, stream};
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsWriter};
use mesio_engine::{DownloadError, DownloaderInstance};
use pipeline_common::CancellationToken;
use pipeline_common::{PipelineError, ProtocolWriter, WriterConfig, WriterTask};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Instant;
use tracing::{Level, Span, debug, info, span, warn};
use tracing_indicatif::span_ext::IndicatifSpanExt;

/// Statistics from pipe stream processing
struct PipeStreamStats {
    items_written: usize,
    segment_count: u32,
    bytes_written: u64,
}

/// Process HLS stream to pipe output (stdout)
/// Uses PipeHlsStrategy for segment boundary detection
async fn process_pipe_stream(
    stream: Pin<Box<dyn Stream<Item = Result<HlsData, PipelineError>> + Send>>,
    pipeline_common_config: &pipeline_common::config::PipelineConfig,
) -> Result<PipeStreamStats, AppError> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(pipeline_common_config.channel_size);

    // Create the pipe strategy and writer task config
    let strategy = PipeHlsStrategy::new();
    let config = WriterConfig::new(
        PathBuf::from("."),
        "stdout".to_string(),
        "ts".to_string(), // Default extension, actual data format is determined by content
    );

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

/// Process an HLS stream
pub async fn process_hls_stream(
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

    let base_name = expand_name_url(name_template, url_str)?;
    downloader.add_source(url_str, 10);

    // Create the writer progress span up-front so downloads inherit it
    // Note: Progress bars are disabled in pipe mode via main.rs configuration
    let writer_span = span!(Level::INFO, "writer_processing");

    // Only initialize span visuals if not in pipe mode
    if !is_pipe_mode {
        spans::init_writing_span(&writer_span, format!("Writing HLS {}", base_name));
    }

    let download_span = span!(parent: &writer_span, Level::INFO, "download_hls", url = %url_str);

    // Only initialize download span visuals if not in pipe mode
    if !is_pipe_mode {
        spans::init_spinner_span(&download_span, format!("Downloading {}", url_str));
    }

    // Start the download while the download span is active so child spans attach correctly
    let mut stream = {
        let _writer_enter = writer_span.enter();
        let _download_enter = download_span.enter();
        match downloader {
            DownloaderInstance::Hls(hls_manager) => {
                hls_manager.download_with_sources(url_str).await?
            }
            _ => {
                return Err(AppError::InvalidInput(
                    "Expected HLS downloader".to_string(),
                ));
            }
        }
    };

    // Peek at the first segment to determine the file extension
    let first_segment = match stream.next().await {
        Some(Ok(segment)) => segment,
        Some(Err(e)) => {
            return Err(AppError::InvalidInput(format!(
                "Failed to get first HLS segment: {e}"
            )));
        }
        None => {
            info!("HLS stream is empty.");
            return Err(AppError::Download(DownloadError::NoSource(
                "HLS stream is empty".to_string(),
            )));
        }
    };

    let extension = match first_segment {
        HlsData::TsData(_) => "ts",
        HlsData::M4sData(_) => "m4s",
        // should never happen
        HlsData::EndMarker => {
            return Err(AppError::Pipeline(PipelineError::InvalidData(
                "First segment is EndMarker".to_string(),
            )));
        }
    };

    info!(
        "Detected HLS stream type: {}. Saving with .{} extension.",
        extension.to_uppercase(),
        extension
    );

    // Prepend the first segment back to the stream
    let stream_with_first_segment = stream::once(async { Ok(first_segment) }).chain(stream);
    let stream = stream_with_first_segment;

    let hls_pipe_config = config.hls_pipeline_config.clone();
    debug!("Pipeline config: {:?}", hls_pipe_config);

    let stream = stream.map(|r| r.map_err(|e| PipelineError::Processing(e.to_string())));

    // Use pipe output strategy when stdout mode is active
    let (total_items_written, files_created) = if is_pipe_mode {
        // Pipe mode: write directly to stdout using PipeHlsStrategy
        let stats = process_pipe_stream(Box::pin(stream), &config.pipeline_config).await?;

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            url = %url_str,
            duration = ?elapsed,
            items_written = stats.items_written,
            bytes_written = stats.bytes_written,
            segment_count = stats.segment_count,
            output_mode = %config.output_format,
            "HLS pipe output complete"
        );

        return Ok(stats.items_written as u64);
    } else {
        crate::processor::generic::process_stream_with_span::<HlsPipeline, HlsWriter>(
            &config.pipeline_config,
            hls_pipe_config,
            Box::pin(stream),
            writer_span.clone(),
            |_writer_span| {
                use std::collections::HashMap;
                let mut extras = HashMap::new();
                // Pass max_file_size to writer for progress bar length
                if config.pipeline_config.max_file_size > 0 {
                    extras.insert(
                        "max_file_size".to_string(),
                        config.pipeline_config.max_file_size.to_string(),
                    );
                }
                HlsWriter::new(
                    output_dir.to_path_buf(),
                    base_name.to_string(),
                    extension.to_string(),
                    if extras.is_empty() {
                        None
                    } else {
                        Some(extras)
                    },
                )
            },
            token.clone(),
        )
        .await?
    };

    // Only update progress bar finish message if not in pipe mode
    if !is_pipe_mode {
        download_span.pb_set_finish_message(&format!("Downloaded {}", url_str));
    }
    drop(download_span);

    let elapsed = start_time.elapsed();

    // Log summary
    // file_sequence_number starts at 0, so add 1 to get actual file count
    let actual_files_created = if total_items_written > 0 {
        files_created + 1
    } else {
        0
    };

    // Log completion (goes to stderr in pipe mode)
    info!(
        url = %url_str,
        items = total_items_written,
        files = actual_files_created,
        duration = ?elapsed,
        output_mode = %config.output_format,
        "HLS download complete"
    );

    Ok(total_items_written as u64)
}
