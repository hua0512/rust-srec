use crate::output::pipe_hls_strategy::PipeHlsStrategy;
use crate::output::provider::OutputFormat;
use crate::processor::generic::process_pipe_stream;
use crate::utils::spans;
use crate::{config::ProgramConfig, error::AppError, utils::create_dirs, utils::expand_name_url};
use futures::{StreamExt, stream};
use hls::HlsData;
use hls_fix::{HlsPipeline, HlsWriter, HlsWriterConfig};
use mesio_engine::{DownloadError, DownloaderInstance};
use pipeline_common::CancellationToken;
use pipeline_common::PipelineError;
use std::path::Path;
use std::time::Instant;
use tracing::{Level, debug, info, span};
use tracing_indicatif::span_ext::IndicatifSpanExt;

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
            return Err(AppError::Download(DownloadError::source_exhausted(
                "HLS stream is empty",
            )));
        }
    };

    let extension = match first_segment {
        HlsData::TsData(_) => "ts",
        HlsData::M4sData(_) => "m4s",
        // should never happen
        HlsData::EndMarker => {
            return Err(AppError::InvalidInput(
                "First segment is EndMarker".to_string(),
            ));
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

    let stream = stream.map(|r| r.map_err(|e| PipelineError::Strategy(Box::new(e))));

    // Use pipe output strategy when stdout mode is active
    let stats = if is_pipe_mode {
        // Pipe mode: write directly to stdout using PipeHlsStrategy
        let pipe_stats = process_pipe_stream(
            Box::pin(stream),
            &config.pipeline_config,
            PipeHlsStrategy::new(),
            extension,
        )
        .await?;

        // Log completion statistics for pipe mode
        let elapsed = start_time.elapsed();
        info!(
            url = %url_str,
            duration = ?elapsed,
            items_written = pipe_stats.items_written,
            bytes_written = pipe_stats.bytes_written,
            segment_count = pipe_stats.segment_count,
            output_mode = %config.output_format,
            "HLS pipe output complete"
        );

        return Ok(pipe_stats.items_written as u64);
    } else {
        let max_file_size = if config.pipeline_config.max_file_size > 0 {
            Some(config.pipeline_config.max_file_size)
        } else {
            None
        };

        crate::processor::generic::process_stream_with_span::<HlsPipeline, HlsWriter>(
            &config.pipeline_config,
            hls_pipe_config,
            Box::pin(stream),
            writer_span.clone(),
            |_writer_span| {
                HlsWriter::new(HlsWriterConfig {
                    output_dir: output_dir.to_path_buf(),
                    base_name: base_name.to_string(),
                    extension: extension.to_string(),
                    max_file_size,
                })
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

    // Log completion (goes to stderr in pipe mode)
    info!(
        url = %url_str,
        items = stats.items_written,
        files = stats.files_created,
        duration = ?elapsed,
        output_mode = %config.output_format,
        "HLS download complete"
    );

    Ok(stats.items_written as u64)
}
